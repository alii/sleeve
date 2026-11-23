//! Song structure: where a track's sections start and what each one is, worked out from its
//! beat grid and its three-band waveform, both already in the track's analysis files.
//!
//! The waveform columns are `[bass, mid, high]`, the order rekordbox writes `PWV7` in.

use rbl_anlz::Beat;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    /// Everything before the first drop.
    Intro,
    /// Bass in, and clearly more of it than in the section before.
    Drop,
    /// Bass out, after a drop and before the outro.
    Breakdown,
    /// From the last fall in energy after the final drop to the end.
    Outro,
    /// A change, but none of the above.
    Section,
}

/// `bars` bars from bar `start_bar`, an index into `Structure::bar_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub start_bar: usize,
    pub bars: usize,
    pub label: Label,
}

/// A track's sections, in order and without gaps: the first starts at bar 0 and each starts
/// where the one before ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Structure {
    /// When each bar starts, in milliseconds. Bar 0 is the first downbeat.
    pub bar_ms: Vec<u32>,
    pub sections: Vec<Section>,
}

const MIN_BARS: usize = 16;
/// Bars of waveform compared on each side of a bar line when scoring a change.
const WINDOW: usize = 4;
/// Each band is scaled so these percentiles of it across the track's bars are 0 and 1.
const LOW_PERCENTILE: f64 = 0.10;
const HIGH_PERCENTILE: f64 = 0.90;
/// Below this spread of bass (as a share of its 90th percentile) the track never really changes.
const MIN_BASS_SPREAD: f64 = 0.20;
/// How much bass, mid and high count in the change score and in a section's overall energy.
const WEIGHTS: [f64; 3] = [1.0, 0.5, 0.3];
/// A change-score peak is a boundary when it reaches this share of the strongest peak.
const PEAK_SHARE: f64 = 0.25;
/// The strongest change score must reach this, or nothing in the track changes enough to trust.
const MIN_PEAK: f64 = 0.30;
/// How far a boundary may be from a multiple of 16 bars, or of 8, and still snap onto it.
const SNAP_16_REACH: usize = 2;
const SNAP_8_REACH: usize = 1;
const PHRASE: usize = 4;
const MIN_SECTION_BARS: usize = 4;
/// A drop's scaled bass is at least this, and this much above the section before it.
const DROP_BASS: f64 = 0.60;
const DROP_RISE: f64 = 0.25;
/// Scaled bass at or below this is out, as in a breakdown.
const LOW_BASS: f64 = 0.40;
/// The outro is the first section after the final drop whose overall energy is this much below
/// the drop's, and starts no earlier than this share of the way through: a quiet stretch before
/// that is only a section.
const OUTRO_FALL: f64 = 0.20;
const OUTRO_FROM: f64 = 0.60;

/// When each bar starts: the beats numbered 1, in time order. Beats before the first downbeat
/// are a pickup and aren't a bar.
pub fn bars(beats: &[Beat]) -> Vec<u32> {
    let mut out: Vec<u32> = beats.iter().filter(|b| b.beat_number == 1).map(|b| b.time_ms).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Finds a track's sections from its beat grid and its waveform (`cols`, `cols_per_sec` a
/// second). `None` when it can't tell with confidence: too few bars, a track that never really
/// changes, or no drop. Auto-cue then falls back to cues every so many bars rather than guess.
pub fn detect(beats: &[Beat], cols: &[[u8; 3]], cols_per_sec: f64) -> Option<Structure> {
    let bar_ms = bars(beats);
    if bar_ms.len() < MIN_BARS || cols.is_empty() || !cols_per_sec.is_finite() || cols_per_sec <= 0.0 {
        return None;
    }
    let raw = bar_energy(&bar_ms, cols, cols_per_sec);
    if raw.len() < MIN_BARS {
        return None;
    }
    let energy = scale(&raw)?;
    let mut sections = label(&energy, &boundaries(&energy)?)?;
    // Bars past the end of the waveform have no data, so they join the last section.
    if let Some(last) = sections.last_mut() {
        last.bars = bar_ms.len() - last.start_bar;
    }
    Some(Structure { bar_ms, sections })
}

/// Mean bass, mid and high of each bar the waveform reaches. Each bar runs from its own
/// downbeat to the next, so a tempo change still picks the right columns.
fn bar_energy(bar_ms: &[u32], cols: &[[u8; 3]], cols_per_sec: f64) -> Vec<[f64; 3]> {
    let col_at = |ms: f64| (ms * cols_per_sec / 1000.0).floor().max(0.0) as usize;
    let mut out = Vec::with_capacity(bar_ms.len());
    for (i, &start) in bar_ms.iter().enumerate() {
        let end = match (bar_ms.get(i + 1), i.checked_sub(1).and_then(|j| bar_ms.get(j))) {
            (Some(&next), _) => f64::from(next),
            // The last bar is as long as the one before it.
            (None, Some(&prev)) => f64::from(start) + f64::from(start - prev),
            (None, None) => break,
        };
        let c0 = col_at(f64::from(start));
        let c1 = col_at(end).max(c0 + 1).min(cols.len());
        let Some(slice) = cols.get(c0..c1).filter(|s| !s.is_empty()) else { break };
        out.push(mean(&slice.iter().map(|c| c.map(f64::from)).collect::<Vec<_>>()));
    }
    out
}

/// Each band scaled by the track's own percentiles, so a quiet master reads like a loud one.
/// `None` when the bass barely moves.
fn scale(raw: &[[f64; 3]]) -> Option<Vec<[f64; 3]>> {
    let mut lo = [0.0; 3];
    let mut range = [0.0; 3];
    for k in 0..3 {
        let mut v: Vec<f64> = raw.iter().map(|b| b[k]).collect();
        v.sort_by(f64::total_cmp);
        let at = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
        let (p_lo, p_hi) = (at(LOW_PERCENTILE), at(HIGH_PERCENTILE));
        if k == 0 && (p_hi - p_lo < 1.0 || p_hi - p_lo < MIN_BASS_SPREAD * p_hi) {
            return None;
        }
        lo[k] = p_lo;
        range[k] = p_hi - p_lo;
    }
    Some(raw.iter().map(|b| std::array::from_fn(|k| if range[k] > 0.0 { ((b[k] - lo[k]) / range[k]).clamp(0.0, 1.0) } else { 0.0 })).collect())
}

fn mean(e: &[[f64; 3]]) -> [f64; 3] {
    let sum = e.iter().fold([0.0; 3], |acc: [f64; 3], b| std::array::from_fn(|k| acc[k] + b[k]));
    sum.map(|s| s / e.len().max(1) as f64)
}

fn overall(m: [f64; 3]) -> f64 {
    (0..3).map(|k| WEIGHTS[k] * m[k]).sum::<f64>() / WEIGHTS.iter().sum::<f64>()
}

/// The start bar of every section, 0 first, snapped to the phrase grid and with short sections
/// merged. `None` when fewer than two boundaries are found.
fn boundaries(energy: &[[f64; 3]]) -> Option<Vec<usize>> {
    let n = energy.len();
    if n < 2 * WINDOW + 1 {
        return None;
    }
    // score[b] is the change at the bar line before bar b.
    let score: Vec<f64> = (0..=n - WINDOW)
        .map(|b| {
            if b < WINDOW {
                return 0.0;
            }
            let (before, after) = (mean(&energy[b - WINDOW..b]), mean(&energy[b..b + WINDOW]));
            (0..3).map(|k| WEIGHTS[k] * (after[k] - before[k]).abs()).sum()
        })
        .collect();
    let strongest = score.iter().copied().fold(0.0, f64::max);
    if strongest < MIN_PEAK {
        return None;
    }
    // Phrases needn't count from bar 0 (a seven-bar intro puts every later phrase a bar early),
    // so the grid is laid where the peaks are and each peak snaps to it.
    let peaks: Vec<usize> = (WINDOW..score.len())
        .filter(|&b| score[b] > score[b - 1] && score.get(b + 1).is_none_or(|&next| score[b] >= next) && score[b] >= PEAK_SHARE * strongest)
        .collect();
    let shift = (16 - phrase_anchor(&peaks, &score)) % 16;
    let mut starts = vec![0];
    starts.extend(peaks.into_iter().filter_map(|b| snap(b + shift).checked_sub(shift).filter(|&s| s > 0 && s < n)));
    starts.sort_unstable();
    starts.dedup();
    merge_short(&mut starts, n);
    (starts.len() > 2).then_some(starts)
}

/// The bar, 0 to 15, that 16-bar phrases count from. Found in steps so that a few peaks are
/// enough: which bar in 4 the peaks fall on most (by change score), then which of the two in 8,
/// then which of the two in 16. A tie keeps the grid counting from bar 0.
fn phrase_anchor(peaks: &[usize], score: &[f64]) -> usize {
    let weight = |m: usize, o: usize| peaks.iter().filter(|&&b| b % m == o).map(|&b| score[b]).sum::<f64>();
    let best = |m: usize, options: &[usize]| options.iter().copied().fold(options[0], |a, o| if weight(m, o) > weight(m, a) + 1e-9 { o } else { a });
    let a4 = best(4, &[0, 1, 2, 3]);
    let a8 = best(8, &[a4, a4 + 4]);
    best(16, &[a8, a8 + 8])
}

/// The phrase-grid bar line nearest `bar`: a multiple of 16 or 8 when one is in reach, else the
/// nearest multiple of 4, a tie going to the multiple of 8.
fn snap(bar: usize) -> usize {
    let nearest = |m: usize| {
        let down = bar / m * m;
        if bar - down <= down + m - bar { down } else { down + m }
    };
    for (m, reach) in [(16, SNAP_16_REACH), (8, SNAP_8_REACH)] {
        if nearest(m).abs_diff(bar) <= reach {
            return nearest(m);
        }
    }
    let down = bar / PHRASE * PHRASE;
    match (bar - down).cmp(&(down + PHRASE - bar)) {
        std::cmp::Ordering::Less => down,
        std::cmp::Ordering::Equal if down.is_multiple_of(8) => down,
        _ => down + PHRASE,
    }
}

/// Merges each section shorter than `MIN_SECTION_BARS` into the one before it, or the first
/// section into the one after.
fn merge_short(starts: &mut Vec<usize>, n: usize) {
    while starts.len() > 1 {
        let len = |i: usize| starts.get(i + 1).copied().unwrap_or(n) - starts[i];
        let Some(i) = (0..starts.len()).find(|&i| len(i) < MIN_SECTION_BARS) else { return };
        starts.remove(i.max(1));
    }
}

/// Labels the sections that start at `starts`. `None` when none is a drop.
fn label(energy: &[[f64; 3]], starts: &[usize]) -> Option<Vec<Section>> {
    let n = energy.len();
    let mut secs: Vec<Section> = starts
        .iter()
        .enumerate()
        .map(|(i, &s)| Section { start_bar: s, bars: starts.get(i + 1).copied().unwrap_or(n) - s, label: Label::Section })
        .collect();
    let m = |s: &Section| mean(&energy[s.start_bar..s.start_bar + s.bars]);

    for i in 1..secs.len() {
        let (now, before) = (m(&secs[i]), m(&secs[i - 1]));
        if now[0] >= DROP_BASS && now[0] - before[0] >= DROP_RISE {
            secs[i].label = Label::Drop;
        }
    }
    let first_drop = secs.iter().position(|s| s.label == Label::Drop)?;
    let last_drop = secs.iter().rposition(|s| s.label == Label::Drop)?;
    for s in &mut secs[..first_drop] {
        s.label = Label::Intro;
    }
    let drop_energy = overall(m(&secs[last_drop]));
    let outro = (last_drop + 1..secs.len()).find(|&j| secs[j].start_bar as f64 >= OUTRO_FROM * n as f64 && overall(m(&secs[j])) <= drop_energy - OUTRO_FALL).unwrap_or(secs.len());
    for s in &mut secs[outro..] {
        s.label = Label::Outro;
    }
    for s in secs.iter_mut().take(outro).skip(first_drop + 1) {
        if s.label == Label::Section && m(s)[0] <= LOW_BASS {
            s.label = Label::Breakdown;
        }
    }

    // Neighbours with the same label are one section, except drops and plain changes, which
    // a DJ wants to find one by one.
    let mut out: Vec<Section> = Vec::with_capacity(secs.len());
    for s in secs {
        match out.last_mut() {
            Some(last) if last.label == s.label && !matches!(s.label, Label::Section | Label::Drop) => last.bars += s.bars,
            _ => out.push(s),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{bars, detect, snap, Label, Structure};
    use rbl_anlz::Beat;

    /// A tuning aid, not a check: prints what `detect` makes of one track's analysis files.
    /// Read-only. `SLEEVE_STRUCTURE_SHARE` is a library's share folder and
    /// `SLEEVE_STRUCTURE_ANLZ` the track's analysis path in it.
    /// `cargo test real_track -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_track() {
        let (share, rel) = (std::env::var("SLEEVE_STRUCTURE_SHARE").unwrap(), std::env::var("SLEEVE_STRUCTURE_ANLZ").unwrap());
        let share = std::path::Path::new(&share);
        let beats = crate::deck::read_beats(share, &rel);
        let cols = crate::waveform::read_detail(share, &rel).unwrap_or_default();
        let energy = super::scale(&super::bar_energy(&bars(&beats), &cols, crate::waveform::DETAIL_PER_SEC)).unwrap_or_default();
        for (i, b) in energy.iter().enumerate() {
            println!("bar {:3}  bass {:.2}  mid {:.2}  high {:.2}  {}", i + 1, b[0], b[1], b[2], "#".repeat((b[0] * 30.0) as usize));
        }
        println!("{:?}", detect(&beats, &cols, crate::waveform::DETAIL_PER_SEC).map(|s| s.sections));
    }

    const COLS_PER_SEC: f64 = 150.0;

    // Band levels for each kind of passage: bass, mid, high.
    const INTRO: [u8; 3] = [40, 35, 55];
    const DROP: [u8; 3] = [120, 100, 80];
    const BREAKDOWN: [u8; 3] = [20, 70, 40];
    const BUILD: [u8; 3] = [20, 70, 110];
    const OUTRO: [u8; 3] = [40, 35, 55];

    /// A grid of `pickup` beats and then `runs` of (bars, BPM), four beats a bar.
    fn grid(pickup: usize, runs: &[(usize, f64)]) -> Vec<Beat> {
        let mut out = Vec::new();
        let mut t = 0.0;
        let first_bpm = runs.first().map_or(128.0, |r| r.1);
        for p in 0..pickup {
            out.push(Beat { beat_number: (4 - pickup + p + 1) as u16, tempo_x100: (first_bpm * 100.0) as u16, time_ms: t as u32 });
            t += 60_000.0 / first_bpm;
        }
        for &(n, bpm) in runs {
            for i in 0..n * 4 {
                out.push(Beat { beat_number: (i % 4 + 1) as u16, tempo_x100: (bpm * 100.0) as u16, time_ms: t.round() as u32 });
                t += 60_000.0 / bpm;
            }
        }
        out
    }

    /// Columns for `parts` of (bars, levels) over the bars of `beats`, with a little jitter so no
    /// two columns are the same. The waveform ends one bar after the last downbeat.
    fn wave(beats: &[Beat], parts: &[(usize, [u8; 3])]) -> Vec<[u8; 3]> {
        let bar_ms = bars(beats);
        let level: Vec<[u8; 3]> = parts.iter().flat_map(|&(n, l)| std::iter::repeat_n(l, n)).collect();
        let last = bar_ms.last().copied().unwrap_or(0);
        let bar_len = bar_ms.len().checked_sub(2).map_or(2000, |i| last - bar_ms[i]);
        let total = ((f64::from(last + bar_len)) * COLS_PER_SEC / 1000.0) as usize;
        let mut seed = 12345u32;
        (0..total)
            .map(|c| {
                let ms = c as f64 * 1000.0 / COLS_PER_SEC;
                let bar = bar_ms.partition_point(|&b| f64::from(b) <= ms).saturating_sub(1);
                let l = level.get(bar).or(level.last()).copied().unwrap_or([0; 3]);
                l.map(|v| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    v.saturating_add(((seed >> 16) % 7) as u8).saturating_sub(3)
                })
            })
            .collect()
    }

    /// A 128 BPM track built from `parts`, with no pickup.
    fn track(parts: &[(usize, [u8; 3])]) -> (Vec<Beat>, Vec<[u8; 3]>) {
        let beats = grid(0, &[(parts.iter().map(|p| p.0).sum(), 128.0)]);
        let cols = wave(&beats, parts);
        (beats, cols)
    }

    /// Sections are in order and cover every bar with no gaps.
    fn assert_covers(s: &Structure) {
        assert_eq!(s.sections.first().map(|x| x.start_bar), Some(0));
        for w in s.sections.windows(2) {
            assert_eq!(w[0].start_bar + w[0].bars, w[1].start_bar, "gap or overlap in {:?}", s.sections);
        }
        assert_eq!(s.sections.last().map(|x| x.start_bar + x.bars), Some(s.bar_ms.len()));
        assert!(s.sections.iter().all(|x| x.bars > 0));
    }

    fn starts(s: &Structure, label: Label) -> Vec<usize> {
        s.sections.iter().filter(|x| x.label == label).map(|x| x.start_bar).collect()
    }

    const SHORT: [(usize, [u8; 3]); 5] = [(16, INTRO), (24, DROP), (16, BREAKDOWN), (24, DROP), (16, OUTRO)];

    #[test]
    fn three_minute_track_finds_its_drops_and_breakdown() {
        let (beats, cols) = track(&SHORT);
        let s = detect(&beats, &cols, COLS_PER_SEC).expect("structure");
        assert_covers(&s);
        assert_eq!(s.bar_ms.len(), 96);
        assert_eq!(starts(&s, Label::Intro), vec![0]);
        assert_eq!(starts(&s, Label::Drop), vec![16, 56]);
        assert_eq!(starts(&s, Label::Breakdown), vec![40]);
        assert_eq!(starts(&s, Label::Outro), vec![80]);
    }

    #[test]
    fn six_minute_track_finds_its_drops_and_breakdown() {
        let (beats, cols) = track(&[(32, INTRO), (48, DROP), (32, BREAKDOWN), (48, DROP), (32, OUTRO)]);
        let s = detect(&beats, &cols, COLS_PER_SEC).expect("structure");
        assert_covers(&s);
        assert_eq!(starts(&s, Label::Drop), vec![32, 112]);
        assert_eq!(starts(&s, Label::Breakdown), vec![80]);
        assert_eq!(starts(&s, Label::Outro), vec![160]);
    }

    #[test]
    fn a_build_before_a_drop_stays_part_of_the_breakdown() {
        let (beats, cols) = track(&[(48, INTRO), (64, DROP), (32, BREAKDOWN), (16, BUILD), (64, DROP), (64, OUTRO)]);
        let s = detect(&beats, &cols, COLS_PER_SEC).expect("structure");
        assert_covers(&s);
        assert_eq!(starts(&s, Label::Drop), vec![48, 160]);
        assert_eq!(starts(&s, Label::Breakdown), vec![112]);
        assert_eq!(starts(&s, Label::Outro), vec![224]);
    }

    #[test]
    fn a_flat_track_falls_back() {
        let (beats, cols) = track(&[(96, DROP)]);
        assert_eq!(detect(&beats, &cols, COLS_PER_SEC), None);
    }

    #[test]
    fn no_grid_or_no_waveform_falls_back() {
        let (beats, cols) = track(&SHORT);
        assert_eq!(detect(&[], &cols, COLS_PER_SEC), None);
        assert_eq!(detect(&beats, &[], COLS_PER_SEC), None);
        assert_eq!(detect(&beats, &cols, 0.0), None);
        assert!(bars(&[]).is_empty());
    }

    #[test]
    fn under_sixteen_bars_falls_back() {
        let (beats, cols) = track(&[(4, INTRO), (4, DROP), (4, BREAKDOWN), (3, DROP)]);
        assert_eq!(bars(&beats).len(), 15);
        assert_eq!(detect(&beats, &cols, COLS_PER_SEC), None);
    }

    #[test]
    fn a_track_with_no_drop_falls_back() {
        // Changes enough to find boundaries, but the bass only ever falls.
        let (beats, cols) = track(&[(32, DROP), (32, BREAKDOWN), (32, INTRO)]);
        assert_eq!(detect(&beats, &cols, COLS_PER_SEC), None);
    }

    #[test]
    fn a_quiet_stretch_early_on_is_not_the_outro() {
        // The energy falls straight after the only drop, at bar 48 of 112, and the bass stays
        // out under a vocal until the real outro.
        let (beats, cols) = track(&[(16, INTRO), (32, DROP), (16, [20, 30, 30]), (32, [30, 110, 110]), (16, OUTRO)]);
        let s = detect(&beats, &cols, COLS_PER_SEC).expect("structure");
        assert_covers(&s);
        assert_eq!(starts(&s, Label::Breakdown), vec![48]);
        assert_eq!(starts(&s, Label::Outro), vec![96]);
    }

    #[test]
    fn a_boundary_a_bar_off_the_phrase_grid_snaps_onto_it() {
        // The drop really comes in at bar 17 and the breakdown at bar 47.
        let (beats, cols) = track(&[(17, INTRO), (30, DROP), (17, BREAKDOWN), (32, DROP), (16, OUTRO)]);
        let s = detect(&beats, &cols, COLS_PER_SEC).expect("structure");
        assert_covers(&s);
        assert_eq!(starts(&s, Label::Drop), vec![16, 64]);
        assert_eq!(starts(&s, Label::Breakdown), vec![48]);
        for (bar, snapped) in [(17, 16), (15, 16), (9, 8), (13, 12), (10, 8), (22, 24), (0, 0)] {
            assert_eq!(snap(bar), snapped, "{bar}");
        }
    }

    #[test]
    fn phrases_that_dont_count_from_the_first_bar_are_followed() {
        // A seven-bar intro: every later phrase starts a bar before a multiple of 8.
        let (beats, cols) = track(&[(7, INTRO), (32, DROP), (16, BREAKDOWN), (32, DROP), (16, OUTRO)]);
        let s = detect(&beats, &cols, COLS_PER_SEC).expect("structure");
        assert_covers(&s);
        assert_eq!(starts(&s, Label::Drop), vec![7, 55]);
        assert_eq!(starts(&s, Label::Breakdown), vec![39]);
        assert_eq!(starts(&s, Label::Outro), vec![87]);
    }

    #[test]
    fn a_pickup_beat_is_not_a_bar() {
        let beats = grid(1, &[(20, 128.0)]);
        let b = bars(&beats);
        assert_eq!(b.len(), 20);
        assert_eq!(b[0], beats[1].time_ms);

        let beats = grid(3, &[(96, 128.0)]);
        let s = detect(&beats, &wave(&beats, &SHORT), COLS_PER_SEC).expect("structure");
        assert_covers(&s);
        assert_eq!(s.bar_ms.len(), 96);
        assert_eq!(starts(&s, Label::Drop), vec![16, 56]);
    }

    #[test]
    fn beats_out_of_order_still_give_bars_in_order() {
        let mut beats = grid(0, &[(8, 128.0)]);
        beats.reverse();
        let b = bars(&beats);
        assert_eq!(b.len(), 8);
        assert!(b.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn a_tempo_change_uses_each_bars_own_length() {
        let beats = grid(0, &[(48, 124.0), (48, 140.0)]);
        let s = detect(&beats, &wave(&beats, &SHORT), COLS_PER_SEC).expect("structure");
        assert_covers(&s);
        assert_eq!(starts(&s, Label::Drop), vec![16, 56]);
        assert_eq!(starts(&s, Label::Breakdown), vec![40]);
    }

    #[test]
    fn a_grid_longer_than_the_waveform_still_covers_every_bar() {
        let cols = wave(&grid(0, &[(96, 128.0)]), &SHORT);
        // The grid runs 20 bars past the audio.
        let s = detect(&grid(0, &[(116, 128.0)]), &cols, COLS_PER_SEC).expect("structure");
        assert_covers(&s);
        assert_eq!(s.bar_ms.len(), 116);
        assert_eq!(starts(&s, Label::Drop), vec![16, 56]);
        assert_eq!(s.sections.last().map(|x| x.label), Some(Label::Outro));
    }

    #[test]
    fn a_waveform_longer_than_the_grid_is_cut_to_it() {
        let (beats, mut cols) = track(&SHORT);
        cols.extend(std::iter::repeat_n([127, 127, 127], 150 * 60));
        let s = detect(&beats, &cols, COLS_PER_SEC).expect("structure");
        assert_covers(&s);
        assert_eq!(starts(&s, Label::Drop), vec![16, 56]);
    }
}
