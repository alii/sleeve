//! The beat grid as stretches of tempo, and the one way an edit may change it.
//!
//! The grid is only ever a list of beats. Where the tempo holds and where it changes is worked
//! out from them each time, never kept beside them, so the two can't disagree. Every edit that
//! touches part of a grid goes through `splice`, which rewrites a range of beats and moves the
//! rest along as they are: a change in one place never quietly alters the tempos after it.

use std::ops::Range;

use rbl_anlz::Beat;
use rbl_anlz::grid::{MAX_BPM_X100, MIN_BPM_X100};

/// Beats within this much of each other's tempo, in hundredths of a BPM, count as one tempo.
const STEADY_X100: u16 = 5;

/// A run needs this many beats to count as steady. Fewer are part of a change.
const MIN_STEADY: usize = 4;

/// A stretch of beats in the grid: steady at one tempo, or changing between two.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// The beats in it, as indices into the grid.
    pub beats: Range<usize>,
    /// Whether it holds one tempo. If not, the tempo moves across it from the run before to
    /// the run after.
    pub steady: bool,
}

impl Run {
    /// The tempo going in and coming out, in BPM. For a change, these are the tempos either
    /// side of it. For a steady run, both are its tempo.
    pub fn tempos(&self, beats: &[Beat]) -> (f64, f64) {
        let bpm = |i: usize| f64::from(beats[i].tempo_x100) / 100.0;
        let (start, end) = (self.beats.start, self.beats.end);
        if self.steady {
            return (bpm(start), bpm(start));
        }
        let from = if start > 0 { bpm(start - 1) } else { bpm(start) };
        let to = if end < beats.len() { bpm(end) } else { bpm(end - 1) };
        (from, to)
    }
}

/// The grid split into runs, in order, covering every beat. A steady run is at least
/// `MIN_STEADY` beats whose tempos all sit within `STEADY_X100` of its first. Whatever lies
/// between two steady runs is one change run. Two steady runs that touch are an instant change.
pub fn runs(beats: &[Beat]) -> Vec<Run> {
    let mut out = Vec::new();
    // The first beat not yet in a run. Beats from here to the next steady run are a change.
    let mut loose = 0;
    let mut i = 0;
    while i < beats.len() {
        let tempo = beats[i].tempo_x100;
        let mut j = i + 1;
        while j < beats.len() && beats[j].tempo_x100.abs_diff(tempo) <= STEADY_X100 {
            j += 1;
        }
        if j - i >= MIN_STEADY {
            if loose < i {
                out.push(Run { beats: loose..i, steady: false });
            }
            out.push(Run { beats: i..j, steady: true });
            loose = j;
        }
        i = j;
    }
    if loose < beats.len() {
        out.push(Run { beats: loose..beats.len(), steady: false });
    }
    out
}

/// The run holding beat `i`.
pub fn run_at(runs: &[Run], i: usize) -> Option<&Run> {
    runs.iter().find(|r| r.beats.contains(&i))
}

/// The grid with `beats[range]` replaced by beats at `new_times`, in ms.
///
/// The beats before the range are copied as they are. Each new beat's tempo comes from its
/// exact gap to the next. The last new beat keeps the gap and tempo the range's last beat had
/// to the beat after it, so everything after the range moves by that one amount, with every
/// gap and tempo copied. Bars count on across the new beats. Beats past `track_ms` are dropped,
/// and if the grid now stops short of it, it carries on at its last gap, as rekordbox does.
/// At the start of the grid, new beats before 0 ms are left out, but still count in the bars.
///
/// None if the range is empty or out of bounds, if a beat would land before the one in front
/// of it, or if a gap would be outside the 40 to 499 BPM rekordbox allows.
pub fn splice(beats: &[Beat], range: Range<usize>, new_times: &[f64], track_ms: f64) -> Option<Vec<Beat>> {
    if range.is_empty() || range.end > beats.len() || new_times.is_empty() || !track_ms.is_finite() {
        return None;
    }
    let tempo_of = |gap: f64| -> Option<u16> {
        let x100 = (6_000_000.0 / gap).round_ties_even();
        (f64::from(MIN_BPM_X100)..=f64::from(MAX_BPM_X100)).contains(&x100).then_some(x100 as u16)
    };
    let prefix = &beats[..range.start];
    let skip = if prefix.is_empty() { new_times.iter().take_while(|t| t.round() < 0.0).count() } else { 0 };
    let mut times = Vec::with_capacity(new_times.len());
    // Each beat is checked against the one in front of it, starting from the last kept beat.
    let mut before = prefix.last().map(|b| f64::from(b.time_ms));
    for &t in &new_times[skip..] {
        if !t.is_finite() || t < 0.0 || t > f64::from(u32::MAX) {
            return None;
        }
        let ms = t.round();
        if let Some(b) = before {
            if ms <= b {
                return None;
            }
            tempo_of(t - b)?;
        }
        times.push(t);
        before = Some(ms);
    }
    let mut out = prefix.to_vec();
    let first_number = match prefix.last() {
        Some(b) => next_number(b.beat_number),
        None => number_after(beats[range.start].beat_number, skip as i64),
    };
    let old_last = beats[range.end - 1];
    let has_tail = range.end < beats.len();
    for (k, &t) in times.iter().enumerate() {
        let tempo_x100 = match times.get(k + 1) {
            Some(&next) => tempo_of(next - t)?,
            // Its gap to the beat after the range is the one the old last beat had.
            None if has_tail => old_last.tempo_x100,
            // The grid ends here, so it keeps the tempo of the gap before it.
            None if k > 0 => out[out.len() - 1].tempo_x100,
            None => old_last.tempo_x100,
        };
        let beat_number = if k == 0 { first_number } else { next_number(out[out.len() - 1].beat_number) };
        out.push(Beat { beat_number, tempo_x100, time_ms: t.round() as u32 });
    }
    // Everything after moves by one amount. Bars shift by however many beats were added.
    let shift = i64::from(out[out.len() - 1].time_ms) - i64::from(old_last.time_ms);
    let added = new_times.len() as i64 - range.len() as i64;
    for b in &beats[range.end..] {
        let time_ms = u32::try_from(i64::from(b.time_ms) + shift).ok()?;
        out.push(Beat { time_ms, beat_number: number_after(b.beat_number, added), ..*b });
    }
    fill_to_end(&mut out, track_ms);
    (!out.is_empty()).then_some(out)
}

/// The steady run holding beat `pivot`, spaced `period` ms apart and with `pivot` held where
/// it is. The run's last gap takes the new spacing too, and everything after moves along
/// keeping its own. None if `pivot` is in a change, which has no one tempo to set.
pub fn respace_run(beats: &[Beat], pivot: usize, period: f64, track_ms: f64) -> Option<Vec<Beat>> {
    let runs = runs(beats);
    let run = run_at(&runs, pivot).filter(|r| r.steady)?;
    // The first beat after the run is moved too, as it ends the run's last gap. It keeps its
    // own tempo and gap, so the run after is left as it was.
    let end = (run.beats.end + 1).min(beats.len());
    let at = f64::from(beats[pivot].time_ms);
    let times: Vec<f64> = (run.beats.start..end).map(|i| at + (i as f64 - pivot as f64) * period).collect();
    splice(beats, run.beats.start..end, &times, track_ms)
}

fn next_number(n: u16) -> u16 {
    number_after(n, 1)
}

/// The bar position `by` beats on from `n`, counting 1 to 4.
fn number_after(n: u16, by: i64) -> u16 {
    ((i64::from(n) - 1 + by).rem_euclid(4) + 1) as u16
}

/// Drops beats past `track_ms`, then carries the grid on at its last gap to the end, the way
/// rbl_anlz's grid edits finish.
fn fill_to_end(beats: &mut Vec<Beat>, track_ms: f64) {
    let end = track_ms.max(0.0);
    beats.retain(|b| f64::from(b.time_ms) <= end);
    while beats.len() >= 2 {
        let (last, before) = (beats[beats.len() - 1], beats[beats.len() - 2]);
        let gap = last.time_ms - before.time_ms;
        let next = last.time_ms.saturating_add(gap);
        if gap == 0 || f64::from(next) > end || next == u32::MAX {
            break;
        }
        beats.push(Beat { time_ms: next, beat_number: next_number(last.beat_number), ..last });
    }
}

/// The beat nearest `ms`, as an index.
pub fn nearest(beats: &[Beat], ms: f64) -> Option<usize> {
    rbl_anlz::grid::nearest_index(beats, ms.round().max(0.0) as u32)
}

/// A steady 4/4 grid refitted to where beats were dropped. `a` says beat `a.0` of `beats`
/// belongs at `a.1` ms; `b`, if given, pins a second beat the same way. With one anchor the
/// grid slides there and keeps its tempo. With two, the tempo is whatever puts both beats
/// where they were dropped, so fixing one end never moves the other. Bars carry over from
/// `beats`. The grid runs from the start of the track up to `track_ms`.
///
/// None if there's nothing to fit, or the anchors make no sense: the same beat twice, beats
/// dropped out of order, or a tempo outside the 40 to 499 BPM rekordbox allows.
///
/// Indices are into `beats`, the grid before the fit. A slide can add or drop beats at the
/// start, so the same beat may have a different index in what comes back.
pub fn fit_grid(beats: &[Beat], a: (usize, f64), b: Option<(usize, f64)>, track_ms: f64) -> Option<Vec<Beat>> {
    let (ia, ta) = a;
    let anchor = *beats.get(ia)?;
    let period = match b {
        Some((ib, tb)) => {
            beats.get(ib)?;
            if ib == ia {
                return None;
            }
            (tb - ta) / (ib as f64 - ia as f64)
        }
        // One anchor keeps the tempo: the average gap over the whole grid, which is closer
        // than the BPM rounded to hundredths on each beat.
        None => {
            if beats.len() < 2 {
                return None;
            }
            let (first, last) = (beats[0].time_ms, beats[beats.len() - 1].time_ms);
            (f64::from(last) - f64::from(first)) / (beats.len() - 1) as f64
        }
    };
    let bpm = 60_000.0 / period;
    let range = f64::from(MIN_BPM_X100) / 100.0..=f64::from(MAX_BPM_X100) / 100.0;
    // Also catches NaN and beats dropped out of order, which make the tempo negative.
    if !range.contains(&bpm) || !ta.is_finite() || !track_ms.is_finite() {
        return None;
    }
    let tempo_x100 = (bpm * 100.0).round() as u16;
    // Each beat's time is worked out from the anchor and rounded on its own, so rounding to
    // whole ms never adds up along the track.
    let mut k = (-ta / period).ceil() as i64;
    let mut out = Vec::new();
    loop {
        let t = ta + k as f64 * period;
        if t >= track_ms {
            break;
        }
        let beat_number = ((i64::from(anchor.beat_number) - 1 + k).rem_euclid(4) + 1) as u16;
        out.push(Beat { beat_number, tempo_x100, time_ms: t.round().max(0.0) as u32 });
        k += 1;
    }
    (!out.is_empty()).then_some(out)
}

/// The grid at `bpm`, with beat `pivot` of `beats` held where it is: a tempo typed in, with
/// the beats spaced to match and nothing analysed again. Bars carry over. None outside
/// rekordbox's tempo range, or for a grid of one beat, which has no spacing to change.
pub fn retempo(beats: &[Beat], pivot: usize, bpm: f64, track_ms: f64) -> Option<Vec<Beat>> {
    let t = f64::from(beats.get(pivot)?.time_ms);
    // Any second beat will do: it's put one gap away (or one back) at the new tempo.
    let other = if pivot + 1 < beats.len() { pivot + 1 } else { pivot.checked_sub(1)? };
    let gap = 60_000.0 / bpm;
    fit_grid(beats, (pivot, t), Some((other, t + (other as f64 - pivot as f64) * gap)), track_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid with these tempos, beat by beat, from 0 ms. Each gap follows the beat's tempo.
    fn grid_of(tempos: &[f64]) -> Vec<Beat> {
        let mut at = 0.0_f64;
        tempos
            .iter()
            .enumerate()
            .map(|(i, bpm)| {
                let b = Beat { beat_number: (i % 4 + 1) as u16, tempo_x100: (bpm * 100.0).round() as u16, time_ms: at.round() as u32 };
                at += 60_000.0 / bpm;
                b
            })
            .collect()
    }

    /// Eight beats at 120 BPM, eight speeding up a beat at a time, then eight at 128.
    fn ramp() -> Vec<Beat> {
        let tempos: Vec<f64> = (0..24).map(|i| if i < 8 { 120.0 } else if i < 16 { 120.0 + (i - 7) as f64 * 8.0 / 9.0 } else { 128.0 }).collect();
        grid_of(&tempos)
    }

    fn steady(bpm: f64, n: usize) -> Vec<Beat> {
        grid_of(&vec![bpm; n])
    }

    /// Sixteen beats at 120 BPM, then sixteen at 128.
    fn two_tempos() -> Vec<Beat> {
        grid_of(&(0..32).map(|i| if i < 16 { 120.0 } else { 128.0 }).collect::<Vec<_>>())
    }

    fn times(beats: &[Beat]) -> Vec<f64> {
        beats.iter().map(|b| f64::from(b.time_ms)).collect()
    }

    #[test]
    fn a_steady_grid_is_one_run() {
        assert_eq!(runs(&steady(124.0, 40)), [Run { beats: 0..40, steady: true }]);
        assert_eq!(runs(&[]), []);
    }

    #[test]
    fn a_ramp_lies_between_two_steady_runs() {
        let g = ramp();
        let r = runs(&g);
        assert_eq!(r, [Run { beats: 0..8, steady: true }, Run { beats: 8..16, steady: false }, Run { beats: 16..24, steady: true }]);
        assert_eq!(r[1].tempos(&g), (120.0, 128.0));
    }

    #[test]
    fn an_instant_change_is_two_steady_runs_touching() {
        assert_eq!(runs(&two_tempos()), [Run { beats: 0..16, steady: true }, Run { beats: 16..32, steady: true }]);
    }

    #[test]
    fn a_little_jitter_is_still_steady() {
        let mut g = steady(128.0, 32);
        for (i, b) in g.iter_mut().enumerate() {
            b.tempo_x100 = [12800, 12798, 12802, 12801][i % 4];
        }
        assert_eq!(runs(&g), [Run { beats: 0..32, steady: true }]);
        // Too few beats to be steady on their own count as a change.
        assert_eq!(runs(&g[..3]), [Run { beats: 0..3, steady: false }]);
    }

    /// Checks what `splice` promises, for one splice of `beats`.
    fn check_splice(beats: &[Beat], range: Range<usize>, new: &[f64], out: &[Beat]) {
        let (start, end) = (range.start, range.end);
        assert_eq!(&out[..start], &beats[..start], "the beats before the range are untouched");
        for (k, b) in out[start..start + new.len()].iter().enumerate() {
            assert_eq!(f64::from(b.time_ms), new[k].round(), "new beat {k} is where it was asked to be");
        }
        let tail_at = start + new.len();
        let kept = (beats.len() - end).min(out.len() - tail_at);
        for j in 0..kept {
            let (was, now) = (beats[end + j], out[tail_at + j]);
            assert_eq!(was.tempo_x100, now.tempo_x100, "tempo after the range, beat {j}");
            if j + 1 < kept {
                assert_eq!(beats[end + j + 1].time_ms - was.time_ms, out[tail_at + j + 1].time_ms - now.time_ms, "gap after the range, beat {j}");
            }
        }
        if kept > 0 {
            // The gap into what follows is the one the range's last beat had.
            assert_eq!(beats[end].time_ms - beats[end - 1].time_ms, out[tail_at].time_ms - out[tail_at - 1].time_ms);
        }
        for w in out.windows(2) {
            assert_eq!(w[1].beat_number, next_number(w[0].beat_number), "bars count on");
            assert!(w[1].time_ms > w[0].time_ms, "beats stay in order");
        }
    }

    #[test]
    fn splicing_never_changes_what_comes_after() {
        // Many ranges of a grid with a ramp in it, each moved and stretched a little.
        let g = ramp();
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let mut spliced = 0;
        for _ in 0..500 {
            let start = (next() % 23) as usize;
            let end = start + 1 + (next() % (24 - start as u64 - 1)) as usize;
            // At the start of the grid, a move back would drop beats, which is checked below.
            let shift = if start == 0 { (next() % 51) as f64 } else { (next() % 101) as f64 - 50.0 };
            let scale = 0.9 + (next() % 21) as f64 / 100.0;
            let t0 = f64::from(g[start].time_ms);
            let new: Vec<f64> = times(&g[start..end]).iter().map(|t| t0 + shift + (t - t0) * scale).collect();
            if let Some(out) = splice(&g, start..end, &new, 60_000.0) {
                check_splice(&g, start..end, &new, &out);
                spliced += 1;
            }
        }
        // Some are refused, for pushing a beat too close to the one before, but most go in.
        assert!(spliced > 400, "only {spliced} of 500 went in");
    }

    #[test]
    fn splicing_in_more_beats_carries_the_bars_on() {
        // Four beats at 120 become eight at 240 in the same time.
        let g = two_tempos();
        let new: Vec<f64> = (0..8).map(|i| 2000.0 + f64::from(i) * 250.0).collect();
        let out = splice(&g, 4..8, &new, 60_000.0).unwrap();
        check_splice(&g, 4..8, &new, &out);
        assert_eq!(out[4].tempo_x100, 24000);
        // The last of the new beats keeps the old gap into the beat after, at 120.
        assert_eq!(out[11].tempo_x100, 12000);
    }

    #[test]
    fn splicing_fills_to_the_end_of_the_track() {
        // Slow the second half down: it runs past the end, so the beats past it go.
        let g = two_tempos();
        let end = f64::from(g[31].time_ms) + 400.0;
        let from = f64::from(g[16].time_ms);
        let slower: Vec<f64> = (0..16).map(|i| from + f64::from(i) * 500.0).collect();
        let out = splice(&g, 16..32, &slower, end).unwrap();
        assert!(out.len() < g.len());
        assert!(f64::from(out.last().unwrap().time_ms) <= end);
        // Speed it up: the grid stops short, so it carries on at its last gap.
        let faster: Vec<f64> = (0..16).map(|i| from + f64::from(i) * 400.0).collect();
        let out = splice(&g, 16..32, &faster, end + 2000.0).unwrap();
        assert!(out.len() > g.len());
        let last = out.len() - 1;
        assert_eq!(out[last].time_ms - out[last - 1].time_ms, 400);
        assert_eq!(out[last].tempo_x100, 15000);
    }

    #[test]
    fn beats_before_the_track_starts_are_left_out() {
        // The first run slows to 100 BPM around beat 2, so beat 0 would be at -200 ms.
        let g = two_tempos();
        let new: Vec<f64> = (0..17).map(|i| 1000.0 + (f64::from(i) - 2.0) * 600.0).collect();
        let out = splice(&g, 0..17, &new, 60_000.0).unwrap();
        assert_eq!((out[0].time_ms, out[0].beat_number), (400, 2));
        // Old beat 16 is now beat 15, and still 128 BPM into the beat after.
        assert_eq!(out[16].time_ms - out[15].time_ms, g[17].time_ms - g[16].time_ms);
        assert_eq!(out[15].tempo_x100, 12800);
        assert_eq!(out[15].beat_number, g[16].beat_number);
    }

    #[test]
    fn splices_that_make_no_sense_are_refused() {
        let g = two_tempos();
        assert_eq!(splice(&g, 4..6, &[2000.0, 1900.0], 60_000.0), None, "backwards");
        assert_eq!(splice(&g, 4..6, &[1400.0, 2500.0], 60_000.0), None, "behind the beat before");
        assert_eq!(splice(&g, 4..6, &[2000.0, 2100.0], 60_000.0), None, "600 BPM");
        assert_eq!(splice(&g, 4..6, &[2000.0, 3600.0], 60_000.0), None, "37.5 BPM");
        assert_eq!(splice(&g, 4..4, &[2000.0], 60_000.0), None, "nothing to replace");
        assert_eq!(splice(&g, 4..99, &[2000.0], 60_000.0), None, "past the end");
        assert_eq!(splice(&g, 4..6, &[f64::NAN, 2500.0], 60_000.0), None, "not a time");
    }

    #[test]
    fn respacing_a_run_leaves_the_next_one_alone() {
        // The first half goes from 120 to 125 around beat 8. The second half keeps 128.
        let g = two_tempos();
        let out = respace_run(&g, 8, 60_000.0 / 125.0, 60_000.0).unwrap();
        assert_eq!(out[8].time_ms, g[8].time_ms);
        assert!(out[..16].iter().all(|b| b.tempo_x100 == 12500));
        assert!(out[16..].iter().all(|b| b.tempo_x100 == 12800));
        assert_eq!(runs(&out)[..2], [Run { beats: 0..16, steady: true }, Run { beats: 16..runs(&out)[1].beats.end, steady: true }]);
        // A beat in the middle of a ramp has no one tempo to set.
        assert_eq!(respace_run(&ramp(), 11, 500.0, 60_000.0), None);
    }

    #[test]
    fn the_nearest_beat_is_found_by_index() {
        let g = steady(120.0, 4);
        assert_eq!(nearest(&g, 740.0), Some(1));
        assert_eq!(nearest(&g, -50.0), Some(0));
        assert_eq!(nearest(&[], 10.0), None);
    }

    fn grid() -> Vec<Beat> {
        // 120 BPM, a pickup beat, then bars of four from 500 ms.
        (0..12).map(|i| Beat { beat_number: ((i + 3) % 4 + 1) as u16, tempo_x100: 12000, time_ms: i * 500 }).collect()
    }

    #[test]
    fn one_anchor_slides_the_grid_and_keeps_the_tempo() {
        let g = grid();
        let fit = fit_grid(&g, (2, 1100.0), None, 6000.0).unwrap();
        let moved: Vec<Beat> = g.iter().map(|b| Beat { time_ms: b.time_ms + 100, ..*b }).collect();
        assert_eq!(fit, moved);
    }

    #[test]
    fn sliding_fills_the_grid_to_both_ends_of_the_track() {
        // Back 200 ms: the pickup at 0 would land before the start, so it goes, and there's
        // room for one more beat before 6000.
        let fit = fit_grid(&grid(), (1, 300.0), None, 6000.0).unwrap();
        assert_eq!((fit[0].time_ms, fit[0].beat_number), (300, 1));
        assert_eq!(fit.last().map(|b| b.time_ms), Some(5800));
        // Forward 600 ms: a beat now fits before the old first one, and it's the beat before.
        let fit = fit_grid(&grid(), (0, 600.0), None, 6000.0).unwrap();
        assert_eq!((fit[0].time_ms, fit[0].beat_number), (100, 3));
    }

    #[test]
    fn two_anchors_set_the_tempo_without_moving_either() {
        // The grid says 120 BPM but the track is 121: beat 9 really falls at 500 + 8 gaps.
        let gap = 60_000.0 / 121.0;
        let fit = fit_grid(&grid(), (1, 500.0), Some((9, 500.0 + 8.0 * gap)), 6000.0).unwrap();
        let at = |ms: u32| fit.iter().find(|b| b.time_ms == ms).copied();
        assert_eq!(at(500).map(|b| (b.beat_number, b.tempo_x100)), Some((1, 12100)));
        assert_eq!(at((500.0 + 8.0 * gap).round() as u32).map(|b| b.beat_number), Some(1));
        // Which anchor was dragged last doesn't matter.
        assert_eq!(Some(fit), fit_grid(&grid(), (9, 500.0 + 8.0 * gap), Some((1, 500.0)), 6000.0));
    }

    #[test]
    fn rounding_to_whole_ms_never_drifts() {
        // Over 1000 beats at 127.5 BPM, every beat is within half a ms of where it should be.
        let g: Vec<Beat> = (0..2).map(|i| Beat { beat_number: i + 1, tempo_x100: 12000, time_ms: u32::from(i) * 500 }).collect();
        let gap = 60_000.0 / 127.5;
        let fit = fit_grid(&g, (0, 0.0), Some((1, gap)), 1000.0 * gap - 1.0).unwrap();
        assert_eq!(fit.len(), 1000);
        for (k, b) in fit.iter().enumerate() {
            assert!((f64::from(b.time_ms) - k as f64 * gap).abs() <= 0.5, "beat {k} at {}", b.time_ms);
        }
    }

    #[test]
    fn a_typed_tempo_respaces_the_grid_around_the_pinned_beat() {
        // Beat 5 (2500 ms, a beat 1) holds still; the rest move to 125 BPM, 480 ms apart.
        let fit = retempo(&grid(), 5, 125.0, 6000.0).unwrap();
        let at = |ms: u32| fit.iter().find(|b| b.time_ms == ms).map(|b| (b.beat_number, b.tempo_x100));
        assert_eq!(at(2500), Some((1, 12500)));
        assert_eq!(at(2980), Some((2, 12500)));
        assert_eq!(at(2020), Some((4, 12500)));
        // The last beat holds too, measuring back from the one before.
        assert!(retempo(&grid(), 11, 125.0, 6000.0).unwrap().iter().any(|b| b.time_ms == 5500));
        assert_eq!(retempo(&grid(), 5, 39.0, 6000.0), None, "slower than rekordbox allows");
        assert_eq!(retempo(&grid()[..1], 0, 125.0, 6000.0), None, "one beat has no spacing");
    }

    #[test]
    fn anchors_that_make_no_sense_are_refused() {
        let g = grid();
        assert_eq!(fit_grid(&g, (3, 1500.0), Some((3, 1600.0)), 6000.0), None, "same beat twice");
        assert_eq!(fit_grid(&g, (1, 500.0), Some((5, 400.0)), 6000.0), None, "out of order");
        assert_eq!(fit_grid(&g, (1, 500.0), Some((2, 501.0)), 6000.0), None, "60000 BPM");
        assert_eq!(fit_grid(&g, (99, 500.0), None, 6000.0), None, "no such beat");
        assert_eq!(fit_grid(&g[..1], (0, 500.0), None, 6000.0), None, "no tempo to keep");
        assert_eq!(fit_grid(&[], (0, 0.0), None, 6000.0), None, "no grid");
    }
}
