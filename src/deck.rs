//! The deck: what's playing, its waveform and its hot cues. It follows the track on the deck,
//! not the selection, so picking other tracks never hides what you're listening to.


use rbl_index::{Cue, Row};
use rune::{Pixels, Tx};

use crate::app::App;

/// The deck waveform's pixels. Wide enough to stay sharp on a full-width panel at 2×.
pub const WAVE_PX: (usize, usize) = (2400, 160);

/// Two memory cues closer than this are the same one: pressing C twice doesn't make a pair.
const MEMORY_CUE_GAP_MS: u32 = 50;

/// rekordbox keeps at most this many memory cues on a track.
pub const MAX_MEMORY_CUES: usize = 8;

/// Hot cue slots A to H, as rekordbox numbers them in `djmdCue.Kind`: 1, 2, 3, then 5 on (4 is
/// the active memory loop).
pub fn kind_of_slot(slot: u8) -> u8 {
    if slot < 3 { slot + 1 } else { slot + 2 }
}

pub struct DeckWave {
    pub id: String,
    pub generation: u64,
    pub pixels: Pixels,
    pub art: Option<rune::Image>,
    /// The track's beat grid from its analysis file, in time order. Empty if it has none.
    pub beats: Vec<rbl_anlz::Beat>,
    /// The zoomed waveform, if the track has a detailed one.
    pub detail: Option<crate::waveform::Detail>,
}

/// What the deck's worker reads for a track: its beat grid and zoomed waveform.
pub struct DeckGrid {
    id: String,
    generation: u64,
    beats: Vec<rbl_anlz::Beat>,
    detail: Option<crate::waveform::Detail>,
}

/// How much of the track the zoomed waveform shows, in seconds: from about two beats to
/// about sixteen bars at 128 BPM.
pub const ZOOMS: [f64; 6] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0];

/// Where a moment falls on the grid: bar (from 1 at the first downbeat), beat in the bar and
/// the tempo there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridPos {
    pub bar: i32,
    pub beat: u16,
    pub bpm: f64,
}

/// The beat nearest `ms`, in ms.
pub fn nearest_beat(beats: &[rbl_anlz::Beat], ms: f64) -> Option<f64> {
    let i = beats.partition_point(|b| f64::from(b.time_ms) < ms);
    let after = beats.get(i).map(|b| f64::from(b.time_ms));
    let before = i.checked_sub(1).and_then(|j| beats.get(j)).map(|b| f64::from(b.time_ms));
    match (before, after) {
        (Some(a), Some(b)) => Some(if ms - a <= b - ms { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Bar and beat at `ms`. Bar 1 starts at the first downbeat; beats before it count as bar 0.
pub fn grid_pos(beats: &[rbl_anlz::Beat], ms: f64) -> Option<GridPos> {
    let i = beats.partition_point(|b| f64::from(b.time_ms) <= ms).checked_sub(1)?;
    let b = beats[i];
    let bar = beats[..=i].iter().filter(|x| x.beat_number == 1).count() as i32;
    Some(GridPos { bar, beat: b.beat_number, bpm: f64::from(b.tempo_x100) / 100.0 })
}

/// Where stepping `n` beats from `ms` lands: on the playhead's beat if it's on one, else from
/// the beat on the side it steps towards, so one step always reaches the next beat and never
/// skips one. Without a grid, a tenth of a second per step.
pub fn nudge_target(beats: &[rbl_anlz::Beat], ms: f64, n: i64) -> f64 {
    if beats.is_empty() || n == 0 {
        return ms + n as f64 * 100.0;
    }
    let times: Vec<f64> = beats.iter().map(|b| f64::from(b.time_ms)).collect();
    // The last beat at or before the playhead, counting one within 2 ms as "on" it.
    let at_or_before = times.partition_point(|t| *t <= ms + 2.0) as i64 - 1;
    let on = at_or_before >= 0 && (ms - times[at_or_before as usize]).abs() <= 2.0;
    // Stepping back from between beats, the beat behind counts as the first step.
    let j = if n > 0 || on { at_or_before + n } else { at_or_before + n + 1 };
    times[j.clamp(0, times.len() as i64 - 1) as usize]
}

fn read_beats(share: &std::path::Path, rel: &str) -> Vec<rbl_anlz::Beat> {
    if rel.is_empty() {
        return Vec::new();
    }
    let mut beats = rbl_anlz::Anlz::read(&rbl_anlz::resolve(share, rel)).ok().and_then(|a| a.beat_grid()).unwrap_or_default();
    beats.sort_by_key(|b| b.time_ms);
    beats
}

impl App {
    /// The deck's track, as a row of the current library.
    pub fn deck_row(&self) -> Option<Row> {
        let l = self.loaded()?;
        l.index.row_of(self.player.track.as_deref()?)
    }

    /// Plays or pauses a track by id. With `at`, starts or moves it there.
    pub fn play_track(&mut self, id: &str, at: Option<f64>, tx: &mut Tx) {
        let Some(l) = self.loaded() else { return };
        let Some(row) = l.index.row_of(id) else { return };
        let path = std::path::PathBuf::from(l.index.folder_path.get(row as usize));
        let title = l.index.title.get(row as usize).to_owned();
        if self.fetch.as_ref().is_some_and(|f| f.1 == id) {
            return;
        }
        // Asking for any other track means a download waiting to play is no longer wanted.
        self.cancel_fetch();
        // From the scan, not the disk: this runs on the main thread.
        if self.evicted.contains(&row) {
            // What's playing keeps playing while this one downloads, and it takes over once
            // it's down. With nothing playing, the deck shows it straight away.
            if !self.player.playing {
                self.player.fetch(id);
                self.sync_deck_wave();
            }
            let job = self.jobs.spawn(crate::jobs::JobKind::Download, format!("Downloading {title}"), move |rep| crate::icloud::run(&path, &title, rep));
            self.fetch = Some((job, id.to_owned(), at));
            return;
        }
        if at.is_none() && self.player.playing && self.player.is_on(id) {
            self.player.pause();
            return;
        }
        if let (Some(at), true, false) = (at, self.player.is_on(id), self.player.playing) {
            // Clicking the waveform of a paused track moves the playhead and leaves it paused.
            self.player.seek(at);
            return;
        }
        self.player.play(id, path, at);
        self.sync_deck_wave();
        tx.wake_after(std::time::Duration::from_millis(33));
    }

    /// Puts a track on the deck, paused at the start, ready to play. What a double click on a
    /// row does.
    pub fn load_track(&mut self, id: &str, tx: &mut Tx) {
        // Already on the deck: leave it playing (or paused) where it is.
        if self.player.is_on(id) {
            return;
        }
        let Some(l) = self.loaded() else { return };
        let Some(row) = l.index.row_of(id) else { return };
        if self.evicted.contains(&row) {
            // In iCloud: it downloads, then plays, as a click on play does.
            self.play_track(id, None, tx);
            return;
        }
        let path = std::path::PathBuf::from(l.index.folder_path.get(row as usize));
        self.cancel_fetch();
        self.player.load(id, path);
        self.sync_deck_wave();
        tx.wake_after(std::time::Duration::from_millis(33));
    }

    /// Stops the download the deck was waiting on, if any.
    pub fn cancel_fetch(&mut self) {
        if let Some((job, _, _)) = self.fetch.take() {
            self.jobs.cancel(job);
        }
    }

    /// Stops playback, stops any download for the deck and clears it.
    pub fn close_deck(&mut self) {
        self.cancel_fetch();
        self.player.stop();
    }

    /// Plays or pauses whatever is on the deck.
    pub fn toggle_deck(&mut self, tx: &mut Tx) {
        match self.player.track.clone() {
            Some(id) => self.play_track(&id, None, tx),
            None => self.play_focused(None, tx),
        }
    }

    pub fn sync_deck_wave(&mut self) {
        let (Some(id), Some(l)) = (self.player.track.clone(), self.loaded()) else {
            self.deck_wave = None;
            return;
        };
        if self.deck_wave.as_ref().is_some_and(|w| w.id == id && w.generation == l.generation) {
            return;
        }
        let Some(row) = l.index.row_of(&id) else { return };
        let rel = l.index.analysis_path.get(row as usize).to_owned();
        let share = l.location.share_root.clone();
        // The same track after a reload (a cue saved, say) keeps showing what it had until the
        // fresh copies land, so nothing blanks for a moment.
        let prev = self.deck_wave.take().filter(|w| w.id == id);
        let pixels = prev.as_ref().map_or_else(Pixels::new, |w| w.pixels.clone());
        crate::waveform::load_into(pixels.clone(), share, rel.clone(), WAVE_PX.0, WAVE_PX.1);
        let art = match prev.as_ref().and_then(|w| w.art.clone()) {
            Some(art) => Some(art),
            None => self.artwork_for(row, 88.0),
        };
        let (beats, detail) = prev.map_or_else(|| (Vec::new(), None), |w| (w.beats, w.detail));
        let Some(l) = self.loaded() else { return };
        // The grid and the zoomed waveform are read on a worker, never here: the disk may be
        // slow, and the window must keep drawing. They land in `poll`.
        let (tx, rx) = std::sync::mpsc::channel();
        let (share, generation, wanted, waker) = (l.location.share_root.clone(), l.generation, id.clone(), self.waker.clone());
        std::thread::spawn(move || {
            let beats = read_beats(&share, &rel);
            let detail = crate::waveform::load_detail(&share, &rel);
            if tx.send(DeckGrid { id: wanted, generation, beats, detail }).is_ok()
                && let Some(w) = waker
            {
                w();
            }
        });
        self.deck_grid_rx = Some(rx);
        self.deck_wave = Some(DeckWave { id, generation, pixels, art, beats, detail });
    }

    /// Takes the grid and zoomed waveform once the worker has read them. Returns true when
    /// they changed what's shown.
    pub(crate) fn land_deck_grid(&mut self) -> bool {
        let Some(grid) = self.deck_grid_rx.as_ref().and_then(|rx| rx.try_recv().ok()) else { return false };
        self.deck_grid_rx = None;
        match self.deck_wave.as_mut() {
            Some(w) if w.id == grid.id && w.generation == grid.generation => {
                w.beats = grid.beats;
                w.detail = grid.detail;
                true
            }
            _ => false,
        }
    }

    /// The deck track's length in ms: the decoder's when it knows, else the library's.
    pub fn deck_duration_ms(&self) -> Option<f64> {
        self.player.duration_ms().or_else(|| {
            let (l, row) = (self.loaded()?, self.deck_row()?);
            Some(f64::from(l.index.length_sec[row as usize]) * 1000.0)
        })
    }

    /// Hot cues A to H of the deck's track, by slot.
    pub fn deck_hot_cues(&self) -> [Option<Cue>; 8] {
        self.deck_row().map_or([None; 8], |row| self.hot_cues_of(row))
    }

    /// A track's hot cues A to H, by slot.
    pub fn hot_cues_of(&self, row: Row) -> [Option<Cue>; 8] {
        let mut out = [None; 8];
        if let Some(l) = self.loaded() {
            for cue in l.index.cues_of(row) {
                if let Some(letter) = cue.hot_letter() {
                    let slot = (letter as u8).wrapping_sub(b'A');
                    if (slot as usize) < out.len() {
                        out[slot as usize] = Some(cue);
                    }
                }
            }
        }
        out
    }

    /// A hot cue pad: jumps to the cue and plays, as a player's pad does. On an empty pad, sets
    /// a cue at the playhead.
    pub fn hot_cue(&mut self, slot: u8, tx: &mut Tx) {
        let Some(id) = self.player.track.clone() else { return };
        let Some(total) = self.deck_duration_ms().filter(|d| *d > 0.0) else { return };
        match self.deck_hot_cues()[slot as usize] {
            Some(cue) => {
                let at = f64::from(cue.position_ms) / total;
                if self.player.playing {
                    self.player.seek(at);
                } else {
                    self.play_track(&id, Some(at), tx);
                }
            }
            None => {
                let ms = self.snap_ms(self.player.now() * total).round() as u32;
                let kind = kind_of_slot(slot);
                let what = format!("hot cue {} at {}", char::from(b'A' + slot), crate::library::duration(ms / 1000));
                self.write_cue(what, move |w| w.add_cue(&id, kind, ms).map(|_| ()));
            }
        }
    }

    /// A track's memory cues (`Kind` 0, the ones C saves), in time order.
    pub fn memory_cues_of(&self, row: Row) -> Vec<Cue> {
        let Some(l) = self.loaded() else { return Vec::new() };
        let mut cues: Vec<Cue> = l.index.cues_of(row).into_iter().filter(|c| c.kind == 0).collect();
        cues.sort_by_key(|c| c.position_ms);
        cues
    }

    pub fn deck_memory_cues(&self) -> Vec<Cue> {
        self.deck_row().map_or_else(Vec::new, |row| self.memory_cues_of(row))
    }

    /// A memory cue picked from a list: on the deck's track it moves the playhead there; any
    /// other track plays from it.
    pub fn go_to_cue(&mut self, id: &str, position_ms: u32, tx: &mut Tx) {
        if self.player.is_on(id) {
            self.jump_to(position_ms);
            return;
        }
        let Some(l) = self.loaded() else { return };
        let Some(row) = l.index.row_of(id) else { return };
        let total = f64::from(l.index.length_sec[row as usize]) * 1000.0;
        if total > 0.0 {
            self.play_track(id, Some((f64::from(position_ms) / total).clamp(0.0, 1.0)), tx);
        }
    }

    /// Saves a memory cue at the playhead, as C does in rekordbox. With quantise on it lands on
    /// the nearest beat. A spot that already has one isn't saved twice.
    pub fn memory_cue(&mut self) {
        let Some(id) = self.player.track.clone() else { return };
        let Some(total) = self.deck_duration_ms().filter(|d| *d > 0.0) else { return };
        let ms = self.snap_ms(self.player.now() * total).round() as u32;
        let memory = self.deck_memory_cues();
        if memory.iter().any(|c| c.position_ms.abs_diff(ms) < MEMORY_CUE_GAP_MS) {
            return;
        }
        if memory.len() >= MAX_MEMORY_CUES {
            // Said in the status bar, as every change's outcome is.
            let why = format!("This track already has {MAX_MEMORY_CUES} memory cues. Delete one to save another.");
            self.jobs.spawn(crate::jobs::JobKind::Edit, "Saving a memory cue", move |_| crate::jobs::Outcome::Failed(why));
            return;
        }
        let what = format!("a memory cue at {}", crate::library::duration(ms / 1000));
        self.write_cue(what, move |w| w.add_cue(&id, 0, ms).map(|_| ()));
    }

    /// Moves the playhead to a memory cue, leaving it playing or paused.
    pub fn jump_to(&mut self, position_ms: u32) {
        let Some(total) = self.deck_duration_ms().filter(|d| *d > 0.0) else { return };
        self.player.seek((f64::from(position_ms) / total).clamp(0.0, 1.0));
    }

    /// Deletes one of `row`'s cues, hot or memory.
    pub fn delete_cue(&mut self, row: Row, cue: Cue) {
        // An id that didn't parse is read as 0: there's no row to delete by it.
        if cue.id == 0 {
            return;
        }
        let Some(l) = self.loaded() else { return };
        let title = l.index.title.get(row as usize).to_owned();
        let what = match cue.hot_letter() {
            Some(letter) => format!("hot cue {letter}"),
            None => format!("the memory cue at {}", crate::library::duration(cue.position_ms / 1000)),
        };
        let id = cue.id.to_string();
        self.change(format!("Deleting {what} on \"{title}\""), format!("Deleted {what}"), move |w| w.delete_cue(&id).map(|_| ()));
    }

    /// The deck track's beat grid, if it has one.
    pub fn deck_beats(&self) -> &[rbl_anlz::Beat] {
        self.deck_wave.as_ref().filter(|w| self.player.is_on(&w.id)).map_or(&[], |w| &w.beats)
    }

    /// With quantise on, the beat nearest `ms`; otherwise `ms` as it is.
    pub fn snap_ms(&self, ms: f64) -> f64 {
        if !self.quantize {
            return ms;
        }
        nearest_beat(self.deck_beats(), ms).unwrap_or(ms)
    }

    /// Moves the playhead `n` beats on from the beat it's nearest, landing exactly on a beat.
    /// Without a grid it moves a tenth of a second per step.
    pub fn nudge_beats(&mut self, n: i64) {
        let Some(total) = self.deck_duration_ms().filter(|t| *t > 0.0) else { return };
        let target = nudge_target(self.deck_beats(), self.player.now() * total, n);
        self.player.seek((target / total).clamp(0.0, 1.0));
    }

    /// Zooms the detailed waveform in (`-1`) or out (`1`) a step.
    pub fn zoom(&mut self, step: i32) {
        let i = ZOOMS.iter().position(|z| *z >= self.zoom_secs).unwrap_or(3) as i32;
        self.zoom_secs = ZOOMS[(i + step).clamp(0, ZOOMS.len() as i32 - 1) as usize];
    }

    /// Adds a scroll or pinch to the zoom, in steps: `steps` above 0 zooms in. A mouse wheel's
    /// notch is a whole step; a trackpad sends many small amounts, which add up to one.
    pub fn zoom_by(&mut self, steps: f32) {
        // Turning back the other way starts over, so it never has to undo the first way first.
        if steps.signum() != self.zoom_gesture.signum() {
            self.zoom_gesture = 0.0;
        }
        self.zoom_gesture += steps;
        while self.zoom_gesture >= 1.0 {
            self.zoom(-1);
            self.zoom_gesture -= 1.0;
        }
        while self.zoom_gesture <= -1.0 {
            self.zoom(1);
            self.zoom_gesture += 1.0;
        }
    }

    /// `at` (0 to 1 along the deck track) snapped to the grid the same way.
    pub fn snap_frac(&self, at: f64) -> f64 {
        match self.deck_duration_ms().filter(|t| *t > 0.0) {
            Some(total) => (self.snap_ms(at * total) / total).clamp(0.0, 1.0),
            None => at,
        }
    }

    /// Selects the deck's track in the list and scrolls to it. Leaves search and playlist alone
    /// if the track is already in view; otherwise goes to the collection and clears the search.
    pub fn locate_deck(&mut self, tx: &mut Tx) {
        let Some(id) = self.player.track.clone() else { return };
        let Some(row) = self.deck_row() else { return };
        if !self.rows.contains(&row) {
            self.search = rune::TextEdit::new("");
            self.browse = crate::library::Browse::Collection;
            self.refresh_rows();
        }
        self.table.clear_selection();
        self.table.reveal(id, tx);
        self.sync_artwork();
    }

    pub fn delete_hot_cue(&mut self, slot: u8) {
        if let Some(cue) = self.deck_hot_cues()[slot as usize] {
            let id = cue.id.to_string();
            let what = format!("deleting hot cue {}", char::from(b'A' + slot));
            self.write_cue(what, move |w| w.delete_cue(&id).map(|_| ()));
        }
    }

    /// Saves a cue change as a job, so it shows its status, can be stopped before it writes,
    /// and logs what happened. The library is read again once it's saved.
    fn write_cue(&mut self, what: String, change: impl FnOnce(&mut rbl_db::write::Writer) -> rbl_db::Result<()> + Send + 'static) {
        let track = self.loaded().zip(self.deck_row()).map(|(l, r)| l.index.title.get(r as usize).to_owned()).unwrap_or_default();
        self.change(format!("Saving {what} on \"{track}\""), format!("Saved {what}"), change);
    }
}

#[cfg(test)]
mod tests {
    use super::{grid_pos, nearest_beat, nudge_target};
    use rbl_anlz::Beat;

    fn grid() -> Vec<Beat> {
        // 120 BPM, a pickup beat, then bars of four from 500 ms.
        (0..12).map(|i| Beat { beat_number: ((i + 3) % 4 + 1) as u16, tempo_x100: 12000, time_ms: i * 500 }).collect()
    }

    #[test]
    fn cues_snap_to_the_nearest_beat() {
        let g = grid();
        assert_eq!(nearest_beat(&g, 740.0), Some(500.0));
        assert_eq!(nearest_beat(&g, 760.0), Some(1000.0));
        assert_eq!(nearest_beat(&g, -50.0), Some(0.0));
        assert_eq!(nearest_beat(&g, 99_999.0), Some(5500.0));
        assert_eq!(nearest_beat(&[], 10.0), None);
    }

    #[test]
    fn stepping_lands_on_the_next_beat_each_way() {
        let g = grid();
        // On a beat: one step is the neighbouring beat.
        assert_eq!(nudge_target(&g, 1000.0, 1), 1500.0);
        assert_eq!(nudge_target(&g, 1000.0, -1), 500.0);
        // Between beats: the first step reaches the beat on that side, never skipping one.
        assert_eq!(nudge_target(&g, 1200.0, 1), 1500.0);
        assert_eq!(nudge_target(&g, 1200.0, -1), 1000.0);
        // A bar is four beats; the ends hold.
        assert_eq!(nudge_target(&g, 1000.0, 4), 3000.0);
        assert_eq!(nudge_target(&g, 100.0, -4), 0.0);
        assert_eq!(nudge_target(&g, 5400.0, 4), 5500.0);
        // Before the first beat, stepping forward lands on it.
        assert_eq!(nudge_target(&g[2..], 10.0, 1), 1000.0);
    }

    #[test]
    fn bars_count_from_the_first_downbeat() {
        let g = grid();
        assert_eq!(grid_pos(&g, 0.0).map(|p| (p.bar, p.beat)), Some((0, 4)));
        assert_eq!(grid_pos(&g, 520.0).map(|p| (p.bar, p.beat)), Some((1, 1)));
        assert_eq!(grid_pos(&g, 3100.0).map(|p| (p.bar, p.beat)), Some((2, 2)));
        assert_eq!(grid_pos(&g, 0.0).unwrap().bpm, 120.0);
    }
}
