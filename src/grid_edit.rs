//! Fixing the deck track's beat grid by hand, on the zoomed waveform.
//!
//! With Grid on, dragging anywhere on the zoomed waveform slides the whole grid, carrying the
//! beat nearest the pointer. That beat is then pinned. Dragging with Option (Alt elsewhere)
//! stretches the grid instead: the pinned beat stays where it is and the tempo changes so the
//! grabbed beat follows the pointer. Neither end moves when the other is fixed, so a grid
//! that drifts is fixed by sliding a beat near the start onto its kick, then stretching from
//! one near the end.
//!
//! Edits stay in memory, with undo and revert, until Grid is turned off, which saves once.

use rbl_anlz::Beat;
use rune::Phase;

use crate::app::App;
use crate::deck::{fit_grid, reshape_grid};
use crate::jobs::{JobKind, Outcome};

/// A grid being edited.
pub struct GridEdit {
    /// The track whose grid this is.
    pub id: String,
    /// The grid as it's saved, for revert and to tell whether there's anything to save.
    saved: Vec<Beat>,
    /// The grid as it's been edited so far.
    pub beats: Vec<Beat>,
    /// The grid before each change, latest last, with the tempo change that was being shaped
    /// then, if one was, so undoing a change to a ramp steps back through it rather than out.
    undo: Vec<(Vec<Beat>, Option<crate::ramp_edit::RampEdit>)>,
    /// The tempo change as it stood when the grid was last settled (after a change, outside a
    /// drag): what the next step of undo records beside the grid it replaces.
    settled_ramp: Option<crate::ramp_edit::RampEdit>,
    /// The pinned beat, in ms: where the last slide put the beat it carried. Stretching holds
    /// it still. Before any slide, stretching holds the beat nearest the playhead.
    pub pivot_ms: Option<f64>,
    grab: Option<Grab>,
    /// A tempo change being placed or shaped, which takes over the grid tools while it lasts.
    pub ramp: Option<crate::ramp_edit::RampEdit>,
}

/// The grid buttons shown while editing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridTool {
    /// Moves the beat nearest the playhead onto it, and the whole grid with it.
    SnapToPlayhead,
    /// Twice the beats and twice the tempo: a beat between every two.
    Double,
    /// Half the beats and half the tempo: every other beat.
    Halve,
}

/// One drag, from the press.
struct Grab {
    /// The beat that follows the pointer, as an index into `from`.
    index: usize,
    /// The grid at the press. Every step of the drag is fitted from this, not from the step
    /// before, so rounding never builds up over a long drag.
    from: Vec<Beat>,
    /// The time under the pointer at the press.
    start_ms: f64,
    /// The beat held still, when stretching.
    pivot: Option<usize>,
}

impl GridEdit {
    /// Whether the grid differs from the saved one.
    pub fn changed(&self) -> bool {
        self.beats != self.saved
    }

    /// Puts in a new grid as one step of undo. The same grid again changes nothing.
    /// Whether the grid, or a tempo change in it, is being dragged.
    pub fn holding(&self) -> bool {
        self.grab.is_some() || self.ramp.as_ref().is_some_and(crate::ramp_edit::RampEdit::dragging)
    }

    pub(crate) fn replace(&mut self, beats: Vec<Beat>) {
        if beats != self.beats {
            let before = std::mem::replace(&mut self.beats, beats);
            self.push_undo(before);
        }
    }

    /// Records `before`, the grid ahead of a change, as one step of undo, with the ramp as it
    /// stood then.
    fn push_undo(&mut self, before: Vec<Beat>) {
        self.undo.push((before, self.settled_ramp.clone()));
    }

    /// Records `before` as the grid ahead of a change already made live (by a drag), so undo
    /// takes the whole change back in one step.
    pub(crate) fn undo_from(&mut self, before: Vec<Beat>) {
        self.push_undo(before);
    }
}

/// The beat nearest `ms`, as an index.
pub(crate) fn nearest(beats: &[Beat], ms: f64) -> Option<usize> {
    rbl_anlz::grid::nearest_index(beats, ms.round().max(0.0) as u32)
}

impl App {
    /// Whether Grid is on for the track on the deck.
    pub fn grid_editing(&self) -> bool {
        self.grid_edit.as_ref().is_some_and(|g| self.player.is_on(&g.id))
    }

    /// Turns Grid on for the deck's track, or off, saving the grid if it changed.
    pub fn toggle_grid_edit(&mut self) {
        if self.grid_edit.is_some() {
            self.finish_grid_edit();
            return;
        }
        let Some(id) = self.player.track.clone() else { return };
        let beats = self.deck_beats().to_vec();
        if beats.is_empty() {
            return;
        }
        self.grid_edit = Some(GridEdit { id, saved: beats.clone(), beats, undo: Vec::new(), settled_ramp: None, pivot_ms: None, grab: None, ramp: None });
        self.show_grid_edit();
    }

    /// Sets the grid to the tempo typed in the BPM box, without analysing again: the beats are
    /// spaced for it, holding the pinned beat still (before any, the beat nearest the playhead),
    /// as stretching does. Undo takes it back. A tempo that can't be read, or that rekordbox
    /// wouldn't take, puts the box back and the status bar says why.
    pub fn set_grid_tempo(&mut self) {
        let Some(total) = self.deck_duration_ms().filter(|t| *t > 0.0) else { return };
        let playhead = self.player.now() * total;
        let typed = self.grid_bpm.text.trim().replace(',', ".").parse::<f64>().ok();
        let Some(g) = self.grid_edit.as_mut() else { return };
        let pivot = nearest(&g.beats, g.pivot_ms.unwrap_or(playhead));
        // With tempo changes, the whole grid scales so the pinned beat gets the typed tempo and
        // the rest keep their shape around it.
        let retempo = |bpm: f64, p: usize| {
            if rbl_anlz::grid::is_dynamic_from(&g.beats, None) {
                let held = g.beats[p];
                reshape_grid(&g.beats, f64::from(held.time_ms), f64::from(held.tempo_x100) / (bpm * 100.0), 0.0, total)
            } else {
                crate::deck::retempo(&g.beats, p, bpm, total)
            }
        };
        match typed.zip(pivot).and_then(|(bpm, p)| retempo(bpm, p)) {
            Some(beats) => {
                if beats != g.beats {
                    let before = std::mem::replace(&mut g.beats, beats);
                    g.push_undo(before);
                }
            }
            None => {
                let why = "Type a tempo from 40 to 499 BPM.".to_owned();
                self.jobs.spawn(JobKind::Edit, "Setting the tempo", move |_| Outcome::Failed(why));
            }
        }
        self.show_grid_edit();
    }

    /// Ends editing, saving the grid if it changed. While rekordbox is open nothing can be
    /// saved, so editing carries on and the status bar says why.
    pub fn finish_grid_edit(&mut self) {
        let Some(g) = self.grid_edit.as_ref() else { return };
        if !g.changed() {
            self.grid_edit = None;
            return;
        }
        let Some(l) = self.loaded() else { return };
        let Some(row) = l.index.row_of(&g.id) else {
            // The track has gone from the library: there's nowhere to save it.
            self.grid_edit = None;
            return;
        };
        let store = l.store.clone();
        let title = l.index.title.get(row as usize).to_owned();
        let rel = l.index.analysis_path.get(row as usize).to_owned();
        let doing = format!("Saving the grid of \"{title}\"");
        if let Some(why) = store.refusal() {
            self.jobs.spawn(JobKind::Edit, doing, move |_| Outcome::Failed(why.into()));
            return;
        }
        let Some(GridEdit { id, beats, .. }) = self.grid_edit.take() else { return };
        self.jobs.spawn(JobKind::Edit, doing.clone(), move |rep| {
            rep.info(format!("{doing}. Library: {}", store.location.share_root.display()));
            match crate::analysis::save_grid(&store, &id, &rel, &beats) {
                Ok(()) => {
                    rep.info("Saved the grid.".to_owned());
                    Outcome::Completed("Saved the grid".into())
                }
                Err(e) => {
                    rep.warn(format!("Couldn't save the grid: {e}"));
                    Outcome::Failed(format!("Couldn't save the grid: {e}"))
                }
            }
        });
    }

    /// One step of a drag on the zoomed waveform with Grid on. `at_ms` is the time under the
    /// pointer. `stretch`, read at the press, holds the pinned beat still and changes the tempo
    /// instead of sliding everything.
    pub fn grid_drag(&mut self, phase: Phase, at_ms: f64, reach_ms: f64, stretch: bool, cancelled: bool) {
        // Shaping a tempo change, a drag moves one of its ends or warps a beat instead of sliding
        // the grid, which would move the grid out from under the ramp.
        if self.ramp_edit().is_some() {
            match phase {
                Phase::Down => self.ramp_press(at_ms, reach_ms),
                Phase::Move => self.ramp_drag(at_ms),
                Phase::Up => {
                    if !cancelled {
                        self.ramp_drag(at_ms);
                    }
                    self.ramp_release(cancelled);
                }
            }
            return;
        }
        let Some(total) = self.deck_duration_ms().filter(|t| *t > 0.0) else { return };
        let playhead = self.player.now() * total;
        let Some(g) = self.grid_edit.as_mut() else { return };
        if let Phase::Down = phase {
            let Some(index) = nearest(&g.beats, at_ms) else { return };
            // Stretching from the pinned beat itself has nothing to hold, so it slides.
            let pivot = if stretch { nearest(&g.beats, g.pivot_ms.unwrap_or(playhead)).filter(|p| *p != index) } else { None };
            g.grab = Some(Grab { index, from: g.beats.clone(), start_ms: at_ms, pivot });
            return;
        }
        let Some(grab) = g.grab.as_ref() else { return };
        if cancelled {
            g.beats = grab.from.clone();
            g.grab = None;
        } else {
            // The grabbed beat moves as far as the pointer has, so it doesn't jump to it.
            let at = f64::from(grab.from[grab.index].time_ms);
            let target = at + at_ms - grab.start_ms;
            // A grid whose tempo changes moves and scales as a whole, so its changes keep their
            // shape. A steady one is refitted, which also clears rounding left by earlier edits.
            let fitted = match (grab.pivot, rbl_anlz::grid::is_dynamic_from(&grab.from, None)) {
                (Some(p), true) => {
                    let held = f64::from(grab.from[p].time_ms);
                    reshape_grid(&grab.from, held, (target - held) / (at - held), 0.0, total)
                }
                (None, true) => reshape_grid(&grab.from, at, 1.0, target - at, total),
                (Some(p), false) => fit_grid(&grab.from, (p, f64::from(grab.from[p].time_ms)), Some((grab.index, target)), total),
                (None, false) => fit_grid(&grab.from, (grab.index, target), None, total),
            };
            // A drag that asks for an impossible grid (the beats crossing over, say) holds at
            // the last one that worked.
            if let Some(beats) = fitted {
                g.beats = beats;
            }
            if let Phase::Up = phase
                && let Some(grab) = g.grab.take()
                && g.beats != grab.from
            {
                if grab.pivot.is_none() {
                    g.pivot_ms = Some(target);
                }
                g.push_undo(grab.from);
            }
        }
        self.show_grid_edit();
    }

    /// One of the grid buttons shown while editing, done with rbxport's own grid edits (the
    /// ones rekordbox's grid panel makes). Undo takes it back. A grid it would push outside
    /// rekordbox's tempo range is left as it is, and the status bar says why.
    pub fn grid_tool(&mut self, tool: GridTool) {
        use rbl_anlz::grid::Edit;
        let Some(total) = self.deck_duration_ms().filter(|t| *t > 0.0) else { return };
        let playhead = self.player.now() * total;
        let Some(g) = self.grid_edit.as_mut() else { return };
        let (edit, doing) = match tool {
            GridTool::SnapToPlayhead => (Edit::Align { time_ms: playhead.round().max(0.0) as u32 }, "Snapping the grid"),
            GridTool::Double => (Edit::Double, "Doubling the grid"),
            GridTool::Halve => (Edit::Halve, "Halving the grid"),
        };
        if let Err(why) = rbl_anlz::grid::validate(&g.beats, None, edit) {
            self.jobs.spawn(JobKind::Edit, doing, move |_| Outcome::Failed(why.into()));
            return;
        }
        let next = rbl_anlz::grid::apply_with_duration(&g.beats, None, edit, total as u32);
        if next.is_empty() || next == g.beats {
            return;
        }
        let before = std::mem::replace(&mut g.beats, next);
        g.push_undo(before);
        // The beat put on the playhead is pinned, ready to stretch from.
        if let GridTool::SnapToPlayhead = tool {
            g.pivot_ms = Some(playhead);
        }
        self.show_grid_edit();
    }

    /// Puts the grid back as it was before the last change. False if there's nothing to undo.
    pub fn undo_grid(&mut self) -> bool {
        let Some(g) = self.grid_edit.as_mut() else { return false };
        let Some((before, ramp)) = g.undo.pop() else { return false };
        g.beats = before;
        // The ramp as it was being shaped then, or none: undoing the first change made after
        // opening one goes back to it as opened, and the step before that leaves it.
        g.ramp = ramp;
        let shaping = g.ramp.is_some();
        if shaping {
            self.show_ramp_fields();
        }
        self.show_grid_edit();
        true
    }

    /// Puts the grid back as it's saved. Undo brings the edits back.
    pub fn revert_grid(&mut self) {
        let Some(g) = self.grid_edit.as_mut() else { return };
        if !g.changed() {
            return;
        }
        let before = std::mem::replace(&mut g.beats, g.saved.clone());
        g.push_undo(before);
        g.pivot_ms = None;
        g.ramp = None;
        self.show_grid_edit();
    }

    /// Shows the edited grid on the deck, in place of the one read from disk, and in the BPM box
    /// the tempo of the beat a typed tempo would hold still: the pinned beat, or before any, the
    /// beat nearest the playhead. On a steady grid that's every beat's tempo. On one whose tempo
    /// changes it's the tempo there, which is the one typing changes.
    pub(crate) fn show_grid_edit(&mut self) {
        // Outside a drag, what's shown is settled: the next step of undo comes back to it.
        if let Some(g) = self.grid_edit.as_mut()
            && !g.holding()
        {
            g.settled_ramp = g.ramp.as_ref().map(crate::ramp_edit::RampEdit::settled);
        }
        let playhead = self.player.now() * self.deck_duration_ms().unwrap_or(0.0);
        let Some(g) = self.grid_edit.as_ref() else { return };
        let held = nearest(&g.beats, g.pivot_ms.unwrap_or(playhead)).and_then(|i| g.beats.get(i));
        let tempo = held.map_or_else(|| rbl_anlz::grid::tempo_x100(&g.beats), |b| b.tempo_x100);
        self.grid_bpm = rune::TextEdit::new(format!("{:.2}", f64::from(tempo) / 100.0));
        // Mid-drag the tempo changes aren't looked for again on every step, only once it's let go.
        let dragging = g.holding();
        let mut look = false;
        if let Some(w) = self.deck_wave.as_mut()
            && g.id == w.id
        {
            if w.beats != g.beats {
                w.beats.clone_from(&g.beats);
                w.ramps_stale = true;
            }
            look = w.ramps_stale && !dragging;
        }
        if look {
            self.find_ramps_later();
        }
    }
}
