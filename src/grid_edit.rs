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
use crate::deck::fit_grid;
use crate::jobs::{JobKind, Outcome};

/// A grid being edited.
pub struct GridEdit {
    /// The track whose grid this is.
    pub id: String,
    /// The grid as it's saved, for revert and to tell whether there's anything to save.
    saved: Vec<Beat>,
    /// The grid as it's been edited so far.
    pub beats: Vec<Beat>,
    /// The grid before each change, latest last.
    undo: Vec<Vec<Beat>>,
    /// The pinned beat, in ms: where the last slide put the beat it carried. Stretching holds
    /// it still. Before any slide, stretching holds the beat nearest the playhead.
    pub pivot_ms: Option<f64>,
    grab: Option<Grab>,
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

    /// Makes `beats` the grid, keeping the one before for undo. Returns false, and changes
    /// nothing, if they're the same.
    pub fn commit(&mut self, beats: Vec<Beat>) -> bool {
        if beats == self.beats {
            return false;
        }
        self.undo.push(std::mem::replace(&mut self.beats, beats));
        true
    }
}

/// The beat nearest `ms`, as an index.
fn nearest(beats: &[Beat], ms: f64) -> Option<usize> {
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
        self.grid_edit = Some(GridEdit { id, saved: beats.clone(), beats, undo: Vec::new(), pivot_ms: None, grab: None });
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
        match typed.zip(pivot).and_then(|(bpm, p)| crate::deck::retempo(&g.beats, p, bpm, total)) {
            Some(beats) => {
                g.commit(beats);
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
    pub fn grid_drag(&mut self, phase: Phase, at_ms: f64, stretch: bool, cancelled: bool) {
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
            let target = f64::from(grab.from[grab.index].time_ms) + at_ms - grab.start_ms;
            let fitted = match grab.pivot {
                Some(p) => fit_grid(&grab.from, (p, f64::from(grab.from[p].time_ms)), Some((grab.index, target)), total),
                None => fit_grid(&grab.from, (grab.index, target), None, total),
            };
            // A drag that asks for an impossible grid (the beats crossing over, say) holds at
            // the last one that worked.
            if let Some(beats) = fitted {
                g.beats = beats;
            }
            // The steps of a drag show as they go, but only its end goes on the undo list.
            if let Phase::Up = phase
                && let Some(grab) = g.grab.take()
            {
                let dropped = std::mem::replace(&mut g.beats, grab.from);
                if g.commit(dropped) && grab.pivot.is_none() {
                    g.pivot_ms = Some(target);
                }
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
        if next.is_empty() || !g.commit(next) {
            return;
        }
        // The beat put on the playhead is pinned, ready to stretch from.
        if let GridTool::SnapToPlayhead = tool {
            g.pivot_ms = Some(playhead);
        }
        self.show_grid_edit();
    }

    /// Puts the grid back as it was before the last change. False if there's nothing to undo.
    pub fn undo_grid(&mut self) -> bool {
        let Some(g) = self.grid_edit.as_mut() else { return false };
        let Some(before) = g.undo.pop() else { return false };
        g.beats = before;
        self.show_grid_edit();
        true
    }

    /// Puts the grid back as it's saved. Undo brings the edits back.
    pub fn revert_grid(&mut self) {
        let Some(g) = self.grid_edit.as_mut() else { return };
        if !g.commit(g.saved.clone()) {
            return;
        }
        g.pivot_ms = None;
        self.show_grid_edit();
    }

    /// Shows the edited grid on the deck, in place of the one read from disk, and its tempo in
    /// the BPM box.
    pub(crate) fn show_grid_edit(&mut self) {
        let Some(g) = self.grid_edit.as_ref() else { return };
        self.grid_bpm = rune::TextEdit::new(format!("{:.2}", f64::from(rbl_anlz::grid::tempo_x100(&g.beats)) / 100.0));
        if let Some(w) = self.deck_wave.as_mut()
            && g.id == w.id
        {
            w.beats.clone_from(&g.beats);
        }
    }
}
