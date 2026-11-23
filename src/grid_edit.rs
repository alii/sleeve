//! Fixing the deck track's beat grid by hand, on the zoomed waveform.
//!
//! With Grid on, dragging anywhere on the zoomed waveform slides the whole grid, carrying the
//! beat nearest the pointer. That beat is then pinned. Dragging with Option (Alt elsewhere)
//! stretches the grid instead: the pinned beat stays where it is and the tempo changes so the
//! grabbed beat follows the pointer. Neither end moves when the other is fixed, so a grid
//! that drifts is fixed by sliding a beat near the start onto its kick, then stretching from
//! one near the end.
//!
//! A grid whose tempo changes keeps its changes. Sliding moves every beat by the same amount,
//! and stretching or typing a tempo respaces only the steady stretch the pinned beat is in.
//!
//! Edits stay in memory, with undo and revert, until Grid is turned off, which saves once.

use rbl_anlz::Beat;
use rune::Phase;

use crate::app::App;
use crate::grid_map::{fit_grid, nearest, respace_run, retempo, run_at, runs};
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
        match typed.zip(pivot).ok_or(TempoRefused::OutOfRange).and_then(|(bpm, p)| typed_tempo(&g.beats, p, bpm, total)) {
            Ok(beats) => {
                g.commit(beats);
            }
            Err(refused) => {
                let why = match refused {
                    TempoRefused::OutOfRange => "Type a tempo from 40 to 499 BPM.",
                    TempoRefused::Changing => "The tempo is changing at the pinned beat. Pin a beat where it holds steady to set its tempo.",
                };
                self.jobs.spawn(JobKind::Edit, "Setting the tempo", move |_| Outcome::Failed(why.to_owned()));
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
            let fitted = dragged(&grab.from, grab.index, target, grab.pivot, total);
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

    /// Shows the edited grid on the deck, in place of the one read from disk, and in the BPM
    /// box the tempo where the pinned beat is (before any, the beat nearest the playhead).
    pub(crate) fn show_grid_edit(&mut self) {
        let playhead = self.player.now() * self.deck_duration_ms().unwrap_or(0.0);
        let Some(g) = self.grid_edit.as_ref() else { return };
        let bpm = nearest(&g.beats, g.pivot_ms.unwrap_or(playhead)).map_or(0.0, |p| tempo_at(&g.beats, p));
        self.grid_bpm = rune::TextEdit::new(format!("{bpm:.2}"));
        if let Some(w) = self.deck_wave.as_mut()
            && g.id == w.id
        {
            w.beats.clone_from(&g.beats);
        }
    }
}

/// Why a typed tempo wasn't set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TempoRefused {
    /// It couldn't be read, or it's outside the 40 to 499 BPM rekordbox allows.
    OutOfRange,
    /// The pinned beat is where the tempo changes, so there's no one tempo there to set.
    Changing,
}

/// Whether the grid has more than one tempo in it.
fn dynamic(beats: &[Beat]) -> bool {
    rbl_anlz::grid::is_dynamic_from(beats, None)
}

/// The tempo at beat `i`, in BPM: its steady run's, or its own where the tempo is changing.
fn tempo_at(beats: &[Beat], i: usize) -> f64 {
    let runs = runs(beats);
    let at = run_at(&runs, i).filter(|r| r.steady).map_or(i, |r| r.beats.start);
    f64::from(beats[at].tempo_x100) / 100.0
}

/// The grid `from` with beat `index` dragged to `target` ms. Without a pivot the whole grid
/// slides. With one, the pivot holds still and the tempo changes so the grabbed beat follows.
///
/// A grid with one tempo is refitted as one steady grid. A grid whose tempo changes keeps
/// every beat's own tempo when it slides, and when stretched only the steady run under the
/// pivot is respaced, with everything after it moved along as it was.
fn dragged(from: &[Beat], index: usize, target: f64, pivot: Option<usize>, total: f64) -> Option<Vec<Beat>> {
    if !dynamic(from) {
        return match pivot {
            Some(p) => fit_grid(from, (p, f64::from(from[p].time_ms)), Some((index, target)), total),
            None => fit_grid(from, (index, target), None, total),
        };
    }
    let Some(p) = pivot else {
        let by = (target - f64::from(from[index].time_ms)).round();
        let by = i32::try_from(by as i64).ok()?;
        let out = rbl_anlz::grid::apply_with_duration(from, None, rbl_anlz::grid::Edit::Nudge(by), total as u32);
        return (!out.is_empty()).then_some(out);
    };
    let runs = runs(from);
    let run = run_at(&runs, p).filter(|r| r.steady)?;
    let at = f64::from(from[p].time_ms);
    let period = if run.beats.contains(&index) {
        (target - at) / (index as f64 - p as f64)
    } else if index >= run.beats.end {
        // The grabbed beat is past the run, so it moves with the run's end and keeps its
        // distance from it.
        let end = run.beats.end;
        (target - at - f64::from(from[index].time_ms - from[end].time_ms)) / (end as f64 - p as f64)
    } else {
        // Before the run, the grid stays put, so there's nothing to stretch.
        return None;
    };
    respace_run(from, p, period, total)
}

/// The grid at `bpm`, with beat `pivot` held still. A grid whose tempo changes has only the
/// steady run under the pivot respaced.
fn typed_tempo(beats: &[Beat], pivot: usize, bpm: f64, total: f64) -> Result<Vec<Beat>, TempoRefused> {
    let range = f64::from(rbl_anlz::grid::MIN_BPM_X100) / 100.0..=f64::from(rbl_anlz::grid::MAX_BPM_X100) / 100.0;
    if !range.contains(&bpm) {
        return Err(TempoRefused::OutOfRange);
    }
    if !dynamic(beats) {
        return retempo(beats, pivot, bpm, total).ok_or(TempoRefused::OutOfRange);
    }
    respace_run(beats, pivot, 60_000.0 / bpm, total).ok_or(TempoRefused::Changing)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOTAL: f64 = 60_000.0;

    /// Sixteen beats at 120 BPM from 0 ms, then sixteen at 128.
    fn two_tempos() -> Vec<Beat> {
        let mut at = 0.0;
        (0..32)
            .map(|i| {
                let bpm = if i < 16 { 120.0 } else { 128.0 };
                let b = Beat { beat_number: (i % 4 + 1) as u16, tempo_x100: (bpm * 100.0) as u16, time_ms: f64::round(at) as u32 };
                at += 60_000.0 / bpm;
                b
            })
            .collect()
    }

    /// The tempos in a grid, in order, each once.
    fn tempos(beats: &[Beat]) -> Vec<u16> {
        let mut out: Vec<u16> = beats.iter().map(|b| b.tempo_x100).collect();
        out.dedup();
        out
    }

    #[test]
    fn sliding_a_grid_with_two_tempos_keeps_both() {
        let g = two_tempos();
        let out = dragged(&g, 4, f64::from(g[4].time_ms) + 30.0, None, TOTAL).unwrap();
        assert_eq!(tempos(&out), [12000, 12800]);
        // Every beat moved the same way, so every gap is as it was.
        let at = out.iter().position(|b| b.time_ms == g[4].time_ms + 30).unwrap();
        for i in 0..g.len() - 4 {
            assert_eq!(out[at - 4 + i].time_ms, g[i].time_ms + 30);
        }
    }

    #[test]
    fn stretching_the_first_tempo_keeps_the_second() {
        // Beat 4 is pinned and beat 10 dragged 40 ms earlier: the first run speeds up.
        let g = two_tempos();
        let out = dragged(&g, 10, f64::from(g[10].time_ms) - 40.0, Some(4), TOTAL).unwrap();
        assert_eq!(out[4].time_ms, g[4].time_ms);
        assert_eq!(out[10].time_ms, g[10].time_ms - 40);
        // 2960 ms over six beats is 493.33 ms a beat, 121.62 BPM.
        assert_eq!(tempos(&out), [12162, 12800]);
        assert!(out[16..].iter().all(|b| b.tempo_x100 == 12800));
        // Dragging a beat in the second run moves it, and it stays at 128.
        let out = dragged(&g, 20, f64::from(g[20].time_ms) - 40.0, Some(4), TOTAL).unwrap();
        assert_eq!(out[20].time_ms, g[20].time_ms - 40);
        assert_eq!(tempos(&out).len(), 2);
        assert!(out[..16].iter().all(|b| b.tempo_x100 > 12000));
        assert!(out[16..].iter().all(|b| b.tempo_x100 == 12800));
    }

    #[test]
    fn typing_the_first_tempo_keeps_the_second() {
        let g = two_tempos();
        let out = typed_tempo(&g, 4, 124.0, TOTAL).unwrap();
        assert_eq!(out[4].time_ms, g[4].time_ms);
        assert_eq!(tempos(&out), [12400, 12800]);
        assert_eq!(tempo_at(&out, 4), 124.0);
        assert_eq!(tempo_at(&out, 20), 128.0);
        assert_eq!(typed_tempo(&g, 4, 39.0, TOTAL), Err(TempoRefused::OutOfRange));
    }

    #[test]
    fn a_tempo_cant_be_typed_where_it_changes() {
        // Two beats of a ramp between the two tempos, too few to be steady.
        let mut g = two_tempos();
        g[16].tempo_x100 = 12300;
        g[17].tempo_x100 = 12600;
        assert_eq!(typed_tempo(&g, 16, 124.0, TOTAL), Err(TempoRefused::Changing));
    }
}
