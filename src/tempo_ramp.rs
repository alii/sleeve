//! Tempo changes in a beat grid: a stretch of beats where the tempo moves smoothly from one value
//! to another, as when a song speeds up from 120 to 140 BPM.
//!
//! A ramp is pinned at both ends: the time it starts, the time it ends, the tempo going in and the
//! tempo coming out. What's left to choose is how the tempo gets from one to the other, which is
//! the curve. Changing the curve never moves the ends.
//!
//! The maths works in beat lengths (ms per beat) rather than BPM, because lengths add up to time.
//! Across the ramp a curve `s` runs from 0 to 1, and the beat length `x` of the way through is
//! `p0 + (p1 - p0) * s(x)`. Over `n` beats the ramp then lasts `n * (p0 + (p1 - p0) * area)`, where
//! `area` is the area under `s`. With both ends and both tempos fixed, that area is fixed, and any
//! curve with that area fits.
//!
//! The curves are S-shapes with a sharpness (how sudden the change is) and a centre (where it
//! happens). The sharpness is chosen, and the centre is solved to give the area. The gentlest
//! sharpness that can still reach the area is the most even change that fits; anything gentler
//! would need the tempo to overshoot and come back.
//!
//! # Why the curve is edited the way it is
//!
//! This is the reasoning behind the controls in [`crate::ramp_edit`], since from the outside they
//! can look like an odd way to shape a tempo change, especially next to a DAW.
//!
//! ## What's pinned, and what that leaves
//!
//! Sleeve fits a grid to audio that already exists. Where a tempo change starts and where it has
//! settled are facts about the recording, so a ramp's start and end are pinned to times in the
//! audio, and the tempos either side are whatever the music plays at. The beats between have to
//! be a whole number, or beat 1 after the ramp stops landing on the downbeat and every bar after
//! it is counted wrong.
//!
//! Those pins leave less freedom than it seems. With both ends, both tempos and the beat count
//! fixed, the time the beats take is fixed, which (from the sum above) fixes the area under the
//! curve. So the curve isn't free: it can only be reshaped in ways that keep its area.
//!
//! ## Why it can't simply be bent one way, as in a DAW
//!
//! In a DAW, bending a tempo automation line one way (up, say, so a speed-up gets there early) is
//! the natural move. Here that move isn't available with the ends pinned. Getting to the new tempo
//! early fits more beats into the same time; taking them back out means getting there late
//! somewhere else, which is an S. Put another way: with the area fixed, *where* the change happens
//! is decided by the numbers, and the only free choice is *how spread out* it is. That's why the
//! one free control, the plain drag on the handle, is a sharpness.
//!
//! A DAW doesn't hit this because its timeline is in beats, not seconds: its breakpoints sit on
//! bars, and bending the line changes how many seconds the change takes, so the end is free to
//! move in time. Sleeve offers exactly that too, as a separate gesture: an Alt-drag on the handle
//! keeps the start and the beat count and moves the end ([`Curve::bent_area`] works out where).
//! The two gestures are the two honest options; something has to give, and the user picks what.
//! The others were ruled out: giving up whole beats breaks the bar count after the ramp, and
//! giving up the exact end tempo puts a jump in the tempo at the end.
//!
//! ## How much of what a DAW makes the S-curves cover
//!
//! A DAW's curved tempo change is (near enough) exponential in the tempo, along the beats `b`:
//! `bpm(b) = a + c * e^(k*b)`. A beat's length is `60000 / bpm`, so it goes as
//! `1 / (a + c * e^(k*b))`. When `a` and `c` are both positive, that's exactly a logistic (an S)
//! in `b` with steepness `k`, shifted and scaled: one of these S-curves, with the sharpness the
//! DAW's bend amount and the centre outside the ramp, leaving one tail of the S inside, which is
//! why it looks one-way.
//!
//! `a` and `c` are both positive when the tempo line *sags* below the straight line between the
//! two tempos: a speed-up that holds back and then rushes in (the usual build-up), or a slow-down
//! that drops early and then levels off. Those come back exactly. In practice a grid made from
//! one is found again by [`find`] as one ramp with its sharpness about the DAW's `k` and no pins;
//! to make one by hand, the ends go where the audio says, the bars go in as the song really has
//! them, and the handle is dragged until the beats line up. The test `a_daw_curve_is_one_of_ours`
//! checks this.
//!
//! When the line *bulges* above the straight line instead (a speed-up that gets most of the way
//! early), `c` is negative and it isn't a logistic. The S-curves only come close: such a grid is
//! found as the nearest one with a few beats pinned where it's off, so the beats are still where
//! the DAW put them, but as a shape plus pins rather than one clean shape. Covering it exactly
//! would take a second family of curves bent in the tempo rather than the beat length (a "DAW
//! curve" feel, with exactly one member fitting, as with [`Feel::Gradual`]). That hasn't been
//! needed yet. Likewise when `a` comes out negative (a bend so gentle it's nearly a straight
//! line), but then [`Feel::Gradual`] is within a ms or two of it.
//!
//! [`Feel::Gradual`] is *not* in general the DAW's curve, only the gentlest S that fits. They
//! differ by tens of ms on a strongly bent change.
//!
//! ## Why the bar count matters so much, and why it's guessed in whole bars
//!
//! Since a fixed beat count fixes the area, the beat count *is* most of the shape: a change bent
//! late spends longer at the old tempo, so fewer beats fit, and an even one fits more. Nothing in
//! a steady grid says which, so when a ramp is first placed the count is a guess
//! ([`Ramp::count_beats`]). It prefers whole bars, as tempo changes made in a DAW start and end on
//! bar lines, and among those the most even fit. On a strongly bent change it can still be a bar
//! out: over a 16-bar DAW change bent late, an even guess says 66 beats, whole bars narrow that to
//! 64 or 68, and the more even of those is 68. Nothing in a steady grid can tell them apart, which
//! is why the bars box is right there to type the real count into.
//!
//! ## Why not a node editor
//!
//! Drawing the tempo as breakpoints with curved segments, as a DAW does, was considered. With the
//! ends pinned it would mostly repeat what's here: the handle reaches every DAW-style bend, and a
//! beat that's still off can be dragged, which pins it ([`warp`]); pins are the nodes, placed
//! where the audio needs them rather than on a tempo line.

use rbl_anlz::Beat;
use rbl_anlz::grid::{MAX_BPM_X100, MIN_BPM_X100};

/// How sudden a tempo change is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Feel {
    /// The most even change that fits.
    Gradual,
    /// Most of the change over about half the ramp.
    Medium,
    /// Most of the change over about a fifth of the ramp.
    Sudden,
    /// A sharpness set by dragging.
    Custom(f64),
}

impl Feel {
    fn sharpness(self) -> f64 {
        match self {
            Feel::Gradual => 0.0,
            Feel::Medium => 10.0,
            Feel::Sudden => 30.0,
            Feel::Custom(s) => s,
        }
    }
}

/// The sharpest a curve gets. Past this the change already happens within a beat or two.
pub const MAX_SHARPNESS: f64 = 80.0;

/// The furthest a curve bends either way. Past this the change already happens within a beat or
/// two of an end.
pub const MAX_BEND: f64 = 40.0;

/// The shape of a tempo change: how far through the change the tempo is, 0 to 1, at each point of
/// the ramp.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Curve {
    /// How sudden the change is. Near 0 it's a straight line.
    pub sharpness: f64,
    /// Where the change happens, as a fraction of the ramp. Can sit outside 0 to 1, which leaves
    /// only one end of the S inside the ramp.
    pub centre: f64,
}

fn logistic(z: f64) -> f64 {
    1.0 / (1.0 + (-z).exp())
}

impl Curve {
    /// How far through the change the tempo is, `x` of the way through the ramp. 0 at the start,
    /// 1 at the end, and never going back on itself between.
    pub fn at(self, x: f64) -> f64 {
        let Curve { sharpness: k, centre: c } = self;
        if k < 1e-3 {
            return x;
        }
        // The S, squeezed so its piece inside the ramp runs exactly from 0 to 1. With the centre
        // early, that piece is the S's top, where its values are all a hair under 1 and taking one
        // from another loses every digit. There the values are taken from 1 first: what's left to
        // go is tiny but exact, and differences of those keep their precision.
        let (num, den) = if c < 0.5 {
            let left = |z: f64| logistic(-z);
            (left(-k * c) - left(k * (x - c)), left(-k * c) - left(k * (1.0 - c)))
        } else {
            (logistic(k * (x - c)) - logistic(-k * c), logistic(k * (1.0 - c)) - logistic(-k * c))
        };
        if den <= 0.0 {
            return x;
        }
        (num / den).clamp(0.0, 1.0)
    }

    /// The area under the curve, by Simpson's rule.
    fn area(self) -> f64 {
        const N: u32 = 64;
        let sum: f64 = (0..=N)
            .map(|i| {
                let weight = if i == 0 || i == N { 1.0 } else if i % 2 == 1 { 4.0 } else { 2.0 };
                weight * self.at(f64::from(i) / f64::from(N))
            })
            .sum();
        sum / (3.0 * f64::from(N))
    }

    /// The curve of this sharpness with this area, or None if no centre gives it.
    fn with_area(sharpness: f64, area: f64) -> Option<Curve> {
        // A later centre leaves less area under the curve, so the centre can be bisected for.
        let area_at = |centre| Curve { sharpness, centre }.area();
        let (mut lo, mut hi) = (-3.0, 4.0);
        if !Self::reaches(sharpness, area) {
            return None;
        }
        for _ in 0..50 {
            let mid = (lo + hi) / 2.0;
            if area_at(mid) > area {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some(Curve { sharpness, centre: (lo + hi) / 2.0 })
    }

    /// Whether a curve this sharp can have this area: whether it's between the areas of the
    /// earliest and latest centres.
    fn reaches(sharpness: f64, area: f64) -> bool {
        let area_at = |centre| Curve { sharpness, centre }.area();
        area_at(-3.0) >= area && area_at(4.0) <= area
    }

    /// The least sharpness that can reach `area`. A sharper S can reach further from a straight
    /// line's area of a half, so this is bisected for too.
    pub fn gentlest(area: f64) -> Option<f64> {
        const LOW: f64 = 0.01;
        if Self::reaches(LOW, area) {
            return Some(LOW);
        }
        let (mut lo, mut hi) = (LOW, 200.0);
        if !Self::reaches(hi, area) {
            return None;
        }
        for _ in 0..40 {
            let mid = (lo * hi).sqrt();
            if Self::reaches(mid, area) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        Some(hi)
    }

    /// The area under a line bent the way a DAW bends automation, `(e^(bx) - 1) / (e^b - 1)`,
    /// worked out exactly: `1/b - 1/(e^b - 1)`. Below 0 the change comes early and there's more
    /// area; above 0 it comes late and there's less. Bending a ramp this way moves its end to
    /// where this area puts it, and [`Feel::Gradual`] then draws the bent line: the gentlest S
    /// that fits an area off the middle has its middle outside the ramp, leaving only a tail of
    /// the S inside, and a tail of an S is this bend.
    pub fn bent_area(b: f64) -> f64 {
        if b.abs() < 1e-6 { 0.5 } else { 1.0 / b - 1.0 / b.exp_m1() }
    }

    /// The bend that gives `area`, the other way from [`Curve::bent_area`].
    pub fn bend_for(area: f64) -> f64 {
        let (mut lo, mut hi) = (-MAX_BEND, MAX_BEND);
        for _ in 0..60 {
            let mid = (lo + hi) / 2.0;
            if Self::bent_area(mid) > area {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (lo + hi) / 2.0
    }

    /// The curve with this feel and this area. A feel gentler than the area allows becomes the
    /// gentlest that does.
    pub fn fit(area: f64, feel: Feel) -> Option<Curve> {
        let min = Self::gentlest(area)?;
        let wanted = feel.sharpness();
        let sharpness = if wanted.is_finite() { wanted.clamp(min, MAX_SHARPNESS.max(min)) } else { min };
        Self::with_area(sharpness, area)
    }
}

/// A tempo change, pinned at both ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ramp {
    /// Where the ramp starts, in ms: the time of the last beat at the old tempo.
    pub start_ms: f64,
    /// Where it ends, in ms: the time of the first beat at the new tempo.
    pub end_ms: f64,
    /// The tempo going in.
    pub from_bpm: f64,
    /// The tempo coming out, which the grid carries on at after the ramp.
    pub to_bpm: f64,
    /// Beats from the start to the end. Whole, so the bars after keep counting 1 2 3 4.
    pub beats: u32,
    /// How sudden the change is.
    pub feel: Feel,
}

impl Ramp {
    fn lengths(&self) -> (f64, f64) {
        (60_000.0 / self.from_bpm, 60_000.0 / self.to_bpm)
    }

    /// The area the curve needs for this many beats to fill the ramp, or None if only a tempo that
    /// overshoots and comes back could: too many or too few beats, or the same tempo at both ends.
    pub fn area(&self) -> Option<f64> {
        let (p0, p1) = self.lengths();
        let span = self.end_ms - self.start_ms;
        let finite = [p0, p1, span].iter().all(|v| v.is_finite());
        if !finite || self.beats == 0 || span <= 0.0 || (p1 - p0).abs() < 1e-6 {
            return None;
        }
        let area = (span / f64::from(self.beats) - p0) / (p1 - p0);
        (0.002..0.998).contains(&area).then_some(area)
    }

    /// The curve that fits this ramp with its feel.
    pub fn curve(&self) -> Option<Curve> {
        Curve::fit(self.area()?, self.feel)
    }

    /// The time of every beat in the ramp, in ms, from the start to the end: `beats + 1` of them.
    /// The first is exactly the start and the last exactly the end.
    pub fn times(&self) -> Option<Vec<f64>> {
        if self.beats == 0 {
            return self.instant().then(|| vec![self.start_ms]);
        }
        Some(self.times_with(self.curve()?))
    }

    /// Whether this is an instant change: no beats between its start and its end, which are the
    /// same beat. The tempo is the old one up to that beat and the new one from it.
    pub fn instant(&self) -> bool {
        self.beats == 0 && self.end_ms == self.start_ms
    }

    /// An instant change from `from_bpm` to `to_bpm` on the beat at `at_ms`.
    pub fn instant_at(at_ms: f64, from_bpm: f64, to_bpm: f64) -> Ramp {
        Ramp { start_ms: at_ms, end_ms: at_ms, from_bpm, to_bpm, beats: 0, feel: Feel::Gradual }
    }

    /// The beat times with this curve, whatever the feel says.
    fn times_with(&self, curve: Curve) -> Vec<f64> {
        let (p0, p1) = self.lengths();
        let n = f64::from(self.beats);
        // Each beat's length is the beat length averaged across it (Simpson's rule again).
        let lengths: Vec<f64> = (0..self.beats)
            .map(|i| {
                let at = |u: f64| p0 + (p1 - p0) * curve.at((f64::from(i) + u) / n);
                (at(0.0) + 4.0 * at(0.25) + 2.0 * at(0.5) + 4.0 * at(0.75) + at(1.0)) / 12.0
            })
            .collect();
        // The sums are a hair off the exact area; scaling puts the last beat right on the end.
        let scale = (self.end_ms - self.start_ms) / lengths.iter().sum::<f64>();
        let mut t = self.start_ms;
        let mut out = Vec::with_capacity(lengths.len() + 1);
        out.push(t);
        for len in lengths {
            t += len * scale;
            out.push(t);
        }
        if let Some(last) = out.last_mut() {
            *last = self.end_ms;
        }
        out
    }

    /// A guess at the whole number of beats between `start_ms` and `end_ms` for a change from
    /// `from_bpm` to `to_bpm`. Whole bars if any fit without bending the curve far, as tempo
    /// changes made in a DAW start and end on bar lines; among those, or among all if none fit, the
    /// one that fits most evenly, whose curve is nearest a straight line. None if no number of
    /// beats fits. See "Why the bar count matters so much" at the top of this module.
    pub fn count_beats(start_ms: f64, end_ms: f64, from_bpm: f64, to_bpm: f64) -> Option<u32> {
        let (p0, p1) = (60_000.0 / from_bpm, 60_000.0 / to_bpm);
        let span = end_ms - start_ms;
        if !(span > 0.0 && p0.is_finite() && p1.is_finite()) {
            return None;
        }
        // Every beat is between the two lengths, so the count is between these.
        let fewest = (span / p0.max(p1)).floor().max(1.0) as u32;
        let most = (span / p0.min(p1)).ceil() as u32;
        let fits: Vec<(u32, f64)> = (fewest..=most)
            .filter_map(|beats| {
                let ramp = Ramp { start_ms, end_ms, from_bpm, to_bpm, beats, feel: Feel::Gradual };
                ramp.area().map(|area| (beats, (area - 0.5).abs()))
            })
            .collect();
        let evenest = |fits: &mut dyn Iterator<Item = (u32, f64)>| fits.min_by(|a, b| a.1.total_cmp(&b.1)).map(|(beats, _)| beats);
        // Within this of the middle, a whole-bar count is a bend a DAW would make, not a stretch.
        evenest(&mut fits.iter().copied().filter(|(beats, off)| beats.is_multiple_of(4) && *off < 0.35)).or_else(|| evenest(&mut fits.into_iter()))
    }
}

fn tempo_x100(length_ms: f64) -> Option<u16> {
    let x100 = (6_000_000.0 / length_ms).round();
    (f64::from(MIN_BPM_X100)..=f64::from(MAX_BPM_X100)).contains(&x100).then_some(x100 as u16)
}

/// The grid with a ramp put in. Beats before the ramp stay as they are, the ramp's beats run from
/// its start to its end, and after it come the `tail` beats (see [`tail_for`]), or if there are
/// none to keep, the grid carries on at the new tempo to the end of the track. `start` is the
/// index of the beat the ramp starts on, which must sit at the ramp's start. Bars keep counting
/// through the ramp.
///
/// None if the ramp doesn't fit, ends past the track, or a beat's tempo would be outside the 40 to
/// 499 BPM rekordbox allows.
pub fn apply(beats: &[Beat], start: usize, ramp: &Ramp, track_ms: f64, tail: &[Beat]) -> Option<Vec<Beat>> {
    let times = grid_times(beats, start, ramp, track_ms, tail)?;
    beats_at(beats, start, &times, tail_for(tail, ramp), ramp.to_bpm, track_ms)
}

/// The beats after a ramp that stay as they are: `tail`, beats from a grid that already followed
/// the change, starting with the one at its end. They're kept while the ramp still changes to the
/// tempo they start at, so what comes after (a later tempo change, say) isn't flattened to a
/// steady tempo; moving the ramp's end moves them with it. Empty once the new tempo is another.
pub fn tail_for<'a>(tail: &'a [Beat], ramp: &Ramp) -> &'a [Beat] {
    match tail.first() {
        Some(b) if (f64::from(b.tempo_x100) - ramp.to_bpm * 100.0).abs() < 0.5 => tail,
        _ => &[],
    }
}

/// The exact time of every beat of the grid with a ramp put in, in ms, unrounded: the beats before
/// the ramp where they are, the ramp's, then those of [`tail_for`], or if none are kept, on at the
/// new tempo to the end of the track. Exact, so a warp can be worked on them before they're
/// rounded. None as for [`apply`].
pub fn grid_times(beats: &[Beat], start: usize, ramp: &Ramp, track_ms: f64, tail: &[Beat]) -> Option<Vec<f64>> {
    let first = *beats.get(start)?;
    if (f64::from(first.time_ms) - ramp.start_ms).abs() > 1.0 || ramp.end_ms >= track_ms || ramp.end_ms < ramp.start_ms {
        return None;
    }
    let mut times: Vec<f64> = beats[..start].iter().map(|b| f64::from(b.time_ms)).collect();
    times.extend(ramp.times()?);
    if let [at_end, rest @ ..] = tail_for(tail, ramp) {
        let by = ramp.end_ms - f64::from(at_end.time_ms);
        times.extend(rest.iter().map(|b| f64::from(b.time_ms) + by));
        return Some(times);
    }
    // After the ramp the grid carries on at the new tempo, each beat counted from the end.
    let after = 60_000.0 / ramp.to_bpm;
    let mut k = 1.0;
    while ramp.end_ms + k * after < track_ms {
        times.push(ramp.end_ms + k * after);
        k += 1.0;
    }
    Some(times)
}

/// The grid with the beats from `start` on carried on steadily at `bpm` to the end of the track:
/// a ramp taken out.
pub fn steady_from(beats: &[Beat], start: usize, bpm: f64, track_ms: f64) -> Option<Vec<Beat>> {
    let times: Vec<f64> = beats.get(..=start)?.iter().map(|b| f64::from(b.time_ms)).collect();
    beats_at(beats, start, &times, &[], bpm, track_ms)
}

/// `times` bent to go through `pins`, each "beat `k` at `t` ms", in order of beat. Between two pins
/// the beats keep their spacing relative to each other and stretch or squeeze to fit; before the
/// first pin and after the last, they move as far as it did. With no pins, nothing moves.
///
/// Every pin's beat must be in `times`. Pins that cross over (a later beat before an earlier one)
/// give beats that go backwards, which the caller has to refuse.
pub fn warp(times: &[f64], pins: &[(usize, f64)]) -> Vec<f64> {
    let Some(&(first_k, first_t)) = pins.first() else { return times.to_vec() };
    let mut j = 0;
    times
        .iter()
        .enumerate()
        .map(|(k, &t)| {
            while j + 1 < pins.len() && pins[j + 1].0 <= k {
                j += 1;
            }
            let (ka, ta) = pins[j];
            if k < first_k {
                t + first_t - times[first_k]
            } else if j + 1 == pins.len() {
                t + ta - times[ka]
            } else {
                let (kb, tb) = pins[j + 1];
                ta + (t - times[ka]) * (tb - ta) / (times[kb] - times[ka])
            }
        })
        .collect()
}

/// The grid with beats from `start` on at `times` (exact, as from [`grid_times`] or [`warp`]),
/// and those before it as they are. Beats past the end of the track go, and room left at the end
/// is filled at `after_bpm`, so the grid still covers the track.
///
/// Each beat's time is rounded to whole ms on its own, from the exact times, so rounding never adds
/// up along the track. Each beat's tempo is the tempo up to the next beat, as rekordbox stores it,
/// worked out from the exact times so a steady stretch keeps a steady tempo. None if a tempo would
/// be outside the 40 to 499 BPM rekordbox allows, or beats go backwards.
///
/// `kept` are beats the last of `times` came from (see [`tail_for`]). Where one is still as far
/// from the next as it was, it keeps its tempo, rather than one worked out from whole ms that
/// would wobble, and the end of the track is filled at the last one's tempo.
pub fn beats_at(beats: &[Beat], start: usize, times: &[f64], kept: &[Beat], after_bpm: f64, track_ms: f64) -> Option<Vec<Beat>> {
    let first = *beats.get(start)?;
    let kept_from = times.len().saturating_sub(kept.len());
    // Kept beats run to the end of the track at their own last tempo, not the ramp's.
    let after = kept.last().map_or(60_000.0 / after_bpm, |b| 6_000_000.0 / f64::from(b.tempo_x100));
    let mut times: Vec<f64> = times.get(start..)?.iter().copied().take_while(|t| *t < track_ms).collect();
    while let Some(&last) = times.last() {
        if last + after >= track_ms {
            break;
        }
        times.push(last + after);
    }
    let mut out = beats[..start].to_vec();
    for (i, t) in times.iter().enumerate() {
        let length = times.get(i + 1).map_or(after, |next| next - t);
        if length <= 0.0 {
            return None;
        }
        let was = (start + i).checked_sub(kept_from).and_then(|j| Some((kept.get(j)?, kept.get(j + 1)?)));
        let tempo = match was {
            Some((b, next)) if (f64::from(next.time_ms) - f64::from(b.time_ms) - length).abs() < 0.5 => b.tempo_x100,
            _ => tempo_x100(length)?,
        };
        out.push(Beat {
            beat_number: ((usize::from(first.beat_number) - 1 + i) % 4 + 1) as u16,
            tempo_x100: tempo,
            time_ms: t.round() as u32,
        });
    }
    Some(out)
}

/// A tempo change already in a grid, worked out from its beats.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The beat it starts on.
    pub start: usize,
    pub ramp: Ramp,
    /// Beats that sit off the curve, pinned where they are, as "beat `k` at `t` ms". The ends
    /// aren't among them.
    pub pins: Vec<(usize, f64)>,
}

/// How far a beat can be from where a found ramp puts it, in ms. Beats are stored to the whole
/// ms, so a ramp Sleeve made sits within half of one; the rest is room for a curve that's close.
const FOUND_WITHIN_MS: f64 = 2.0;

/// The fewest beats a stretch of changing tempo needs to count as a ramp.
const FOUND_FEWEST_BEATS: usize = 4;

/// How many beats in a row at one tempo make a steady stretch: a bar.
const STEADY_BEATS: usize = 4;

/// The tempo changes in a grid. Nothing records where a ramp is (rekordbox has no such thing), so
/// they're worked out again from the beats. Between two steady stretches at different tempos, the
/// beats are tried against the ramp curves, and where one fits, there's a ramp. Beats the curve
/// misses are pinned, a few at most, as a hand-shaped ramp's are.
///
/// Steady means a bar or more within 0.01 BPM, the finest rekordbox stores. So where a change is
/// so gentle at its ends that it moves less than that a beat, it's found starting or ending a beat
/// or so inside, where it shows; the beats it leaves out are as good as steady.
pub fn find(beats: &[Beat]) -> Vec<Found> {
    let tempo = |i: usize| i32::from(beats[i].tempo_x100);
    let mut steady = Vec::new();
    let mut i = 0;
    while i < beats.len() {
        let mut j = i;
        while j + 1 < beats.len() && (tempo(j + 1) - tempo(i)).abs() <= 1 {
            j += 1;
        }
        if j + 1 - i >= STEADY_BEATS {
            steady.push((i, j));
        }
        i = j + 1;
    }
    // A ramp starts on the beat after a steady stretch, so its tempo going in is that stretch's,
    // and ends on the first beat of the next one.
    // The start can be pushed back into the steady stretch before it while a beat of it is left
    // to give the tempo going in, or all the way when it starts the track.
    let room_before = |(first, last): (usize, usize)| last - first + usize::from(first == 0);
    let mut found: Vec<Found> = steady
        .windows(2)
        .flat_map(|w| -> Vec<Found> {
            // Two steady stretches that meet, at different tempos: an instant change on the first
            // beat of the second.
            if w[0].1 + 1 == w[1].0 {
                let at = w[1].0;
                let (from, to) = (f64::from(beats[at - 1].tempo_x100) / 100.0, f64::from(beats[at].tempo_x100) / 100.0);
                return ((to - from).abs() >= 0.5).then(|| Found { start: at, ramp: Ramp::instant_at(f64::from(beats[at].time_ms), from, to), pins: Vec::new() }).into_iter().collect();
            }
            let (start, end, before, after) = (w[0].1 + 1, w[1].0, room_before(w[0]), w[1].1 - w[1].0);
            turned(beats, start, end, before, after).unwrap_or_else(|| recognise(beats, start, end, before, after, None).into_iter().collect())
        })
        .collect();
    // A track can start in the middle of a change, with no steady stretch before it: then it runs
    // from the first beat to the first steady stretch.
    if let Some(&(first, last)) = steady.first()
        && first > 0
    {
        found.splice(0..0, recognise(beats, 0, first, 0, last - first, None));
    }
    found
}

/// A change from beat `start` to `end` that turns back on itself (down, then straight back up,
/// or up then down) as two ramps meeting where it turns, if it does and they fit.
///
/// One ramp can only go one way, and a change that comes back to where it started is no change
/// at all to a single ramp, so without this a dip and recovery with no steady bar in the middle
/// would go unfound: no tint, nothing to reopen, nothing to stop another ramp dragged over it.
/// It turns where a beat's tempo goes past both ends' by half a BPM, the least a ramp is
/// anyway; the two meet at the beat furthest out, or up to two either side of it (or of the
/// plateau there), as the beats' own tempos are each across a whole beat and so a little either
/// side of the turn.
///
/// The tempo they meet at is the one thing the beats don't hold. Where a ramp ends in a steady
/// stretch, the beat it ends on is at its tempo; here that beat is already a beat into the next
/// ramp. So the first ramp's tempo is followed on half a beat past its last beat, as
/// [`tempo_in`] follows one back from the first beat of a track, and also tried rounded to the
/// whole BPM when that's close, as a typed tempo usually is. The second ramp's tempo going in is
/// the beat before it, as always, which is what TEMPO gives a ramp started there. Two ramps the
/// same way, one straight after the other, aren't split: there's no turn to say where, and a
/// single curved ramp would often fit them nearly as well.
fn turned(beats: &[Beat], start: usize, end: usize, before: usize, after: usize) -> Option<Vec<Found>> {
    let tempo = |i: usize| i32::from(beats[i].tempo_x100);
    let (from, to) = (tempo(start.checked_sub(1)?), tempo(end));
    let inside = start + 1..end;
    let top = inside.clone().max_by_key(|&i| tempo(i))?;
    let bottom = inside.clone().min_by_key(|&i| tempo(i))?;
    let turn = if tempo(top) >= from.max(to) + 50 {
        top
    } else if tempo(bottom) <= from.min(to) - 50 {
        bottom
    } else {
        return None;
    };
    // Sudden ends are nearly steady, so the turn can be a short plateau, not one beat: every beat
    // within a third of a BPM of the furthest is tried, and the earliest that fits as well wins.
    let level = inside.clone().filter(|&i| (tempo(i) - tempo(turn)).abs() <= 30);
    let (first, last) = (level.clone().min()?, level.max()?);
    let bpm = |i: usize| f64::from(beats[i].tempo_x100) / 100.0;
    (first.saturating_sub(2)..=last + 2)
        .filter(|k| inside.contains(k) && *k >= 2)
        .flat_map(|k| {
            let ahead = bpm(k - 1) + (bpm(k - 1) - bpm(k - 2)) / 2.0;
            let ahead = (ahead * 100.0).round() / 100.0;
            let whole = ahead.round();
            [Some(ahead), ((whole - ahead).abs() <= 0.3 && whole != ahead).then_some(whole)].into_iter().flatten().map(move |to| (k, to))
        })
        .filter_map(|(k, to)| Some([recognise(beats, start, k, before, 0, Some(to))?, recognise(beats, k, end, 0, after, None)?]))
        .filter(|[a, b]| a.ramp.end_ms <= b.ramp.start_ms)
        .min_by_key(|[a, b]| a.pins.len() + b.pins.len())
        .map(Vec::from)
}

/// Pinned beats, as "beat `k` at `t` ms".
type Pins = Vec<(usize, f64)>;

/// The beats from one beat to another, to be tried against ramp curves.
struct Window<'a> {
    start: usize,
    n: usize,
    actual: &'a [Beat],
    ramp: Ramp,
    area: f64,
    gentlest: f64,
    most_pins: usize,
}

impl<'a> Window<'a> {
    /// The beats from `start` to `end`, changing to `to`, or to the end beat's own tempo.
    fn new(beats: &'a [Beat], start: usize, end: usize, to: Option<f64>) -> Option<Window<'a>> {
        let n = end.checked_sub(start)?;
        let from = tempo_in(beats, start)?;
        let to = match to {
            Some(to) => to,
            None => f64::from(beats.get(end)?.tempo_x100) / 100.0,
        };
        if n < FOUND_FEWEST_BEATS || (to - from).abs() < 0.5 {
            return None;
        }
        let actual = &beats[start..=end];
        let (start_ms, end_ms) = (f64::from(actual[0].time_ms), f64::from(actual[n].time_ms));
        let ramp = Ramp { start_ms, end_ms, from_bpm: from, to_bpm: to, beats: n as u32, feel: Feel::Gradual };
        let area = ramp.area()?;
        // A pin for every two bars at most, so a few beats can't be pinned into any shape at all.
        Some(Window { start, n, actual, ramp, area, gentlest: Curve::gentlest(area)?, most_pins: (n / 8).min(4) })
    }

    fn at(&self, k: usize) -> f64 {
        f64::from(self.actual[k].time_ms)
    }

    /// The sharpness a feel comes to here.
    fn sharpness(&self, feel: Feel) -> f64 {
        feel.sharpness().clamp(self.gentlest, MAX_SHARPNESS.max(self.gentlest))
    }

    /// The beat times a curve of this sharpness gives, before any pins.
    fn times(&self, sharpness: f64) -> Option<Vec<f64>> {
        Some(self.ramp.times_with(Curve::with_area(sharpness, self.area)?))
    }

    /// The beat `times` through these pins miss most, and by how much.
    fn off(&self, times: &[f64], pins: &[(usize, f64)]) -> Option<(usize, f64)> {
        let times = warp(times, pins);
        (1..self.n).map(|k| (k, (times[k] - self.at(k)).abs())).max_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// A curve of this sharpness, with pins added one at a time on the beat it misses most until
    /// it fits or has as many as it can. The pins, the ends among them, and how far off it is.
    fn fit(&self, sharpness: f64) -> Option<(Pins, f64)> {
        self.fit_times(self.times(sharpness)?)
    }

    /// [`Window::fit`] for beat times already worked out.
    fn fit_times(&self, times: Vec<f64>) -> Option<(Pins, f64)> {
        let mut pins = vec![(0, self.at(0)), (self.n, self.at(self.n))];
        loop {
            let (k, worst) = self.off(&times, &pins)?;
            if worst <= FOUND_WITHIN_MS || pins.len() - 2 == self.most_pins {
                return Some((pins, worst));
            }
            pins.push((k, self.at(k)));
            pins.sort_by_key(|p| p.0);
        }
    }

    /// The best sharpness of any, with its pins and how far off it is: tried along the range,
    /// evenly by factor as the handle drags it, then the best of those narrowed down.
    fn best_fit(&self) -> Option<(f64, Pins, f64)> {
        let steps = 48.0;
        let top = MAX_SHARPNESS.max(self.gentlest);
        let at = |i: f64| self.gentlest * (top / self.gentlest).powf(i / steps);
        let (best, pins, _) = (0..=48)
            .filter_map(|i| self.fit(at(f64::from(i))).map(|(pins, worst)| (f64::from(i), pins, worst)))
            .min_by(|a, b| (a.1.len(), a.2).partial_cmp(&(b.1.len(), b.2)).unwrap_or(std::cmp::Ordering::Equal))?;
        let worst_at = |i: f64| self.times(at(i)).and_then(|t| self.off(&t, &pins)).map_or(f64::INFINITY, |o| o.1);
        let (mut lo, mut hi) = ((best - 1.0).max(0.0), (best + 1.0).min(steps));
        for _ in 0..30 {
            let (a, b) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
            if worst_at(a) < worst_at(b) {
                hi = b;
            } else {
                lo = a;
            }
        }
        let i = (lo + hi) / 2.0;
        let worst = worst_at(i);
        Some((at(i), pins, worst))
    }

    fn found(&self, feel: Feel, pins: Pins) -> Found {
        let pins = pins.into_iter().filter(|p| p.0 != 0 && p.0 != self.n).map(|(k, t)| (self.start + k, t)).collect();
        Found { start: self.start, ramp: Ramp { feel, ..self.ramp }, pins }
    }
}

/// The tempo going into a ramp that starts on beat `start`: the tempo of the beat before. On the
/// first beat there isn't one, and the first beat's own tempo is already a little into the change
/// (it's the tempo across the whole beat), so the change is followed back half a beat from the
/// first two. Taking the first beat's tempo as it is would creep the tempo going in up a little
/// each time the ramp was opened and saved.
fn tempo_in(beats: &[Beat], start: usize) -> Option<f64> {
    let bpm = |b: &Beat| f64::from(b.tempo_x100) / 100.0;
    match start.checked_sub(1) {
        Some(before) => beats.get(before).map(bpm),
        None => {
            let (first, second) = (bpm(beats.first()?), bpm(beats.get(1)?));
            Some(((first - (second - first) / 2.0) * 100.0).round() / 100.0)
        }
    }
}

/// The ramp from beat `start` to `end`, if one fits the beats there. It may reach up to
/// `before` beats earlier and `after` later, into the steady stretches either side, as the ends
/// of a sudden change are as good as steady. It changes to `to`, or to the tempo of the beat it
/// ends on, which is right whenever that beat is steady.
fn recognise(beats: &[Beat], start: usize, end: usize, before: usize, after: usize, to: Option<f64>) -> Option<Found> {
    let core = Window::new(beats, start, end, to)?;
    // The named feels first, each end pushed out as far as two bars: a ramp Sleeve made with one
    // of them comes back as it was, ends and all. The fewest pins win, then the closest fit.
    let mut named: Option<(usize, f64, Found)> = None;
    'ends: for e0 in 0..=before.min(8) {
        for e1 in 0..=after.min(8) {
            let Some(w) = Window::new(beats, start - e0, end + e1, to) else { continue };
            for feel in [Feel::Gradual, Feel::Medium, Feel::Sudden] {
                let Some((pins, worst)) = w.fit(w.sharpness(feel)) else { continue };
                let better = named.as_ref().is_none_or(|(p, o, _)| (pins.len(), worst) < (*p, *o - 0.01));
                if worst <= FOUND_WITHIN_MS && better {
                    named = Some((pins.len(), worst, w.found(feel, pins)));
                }
            }
            // Within the rounding to whole ms, unpinned: this is the ramp as it was made.
            if matches!(named, Some((0, worst, _)) if worst <= 0.75) {
                break 'ends;
            }
        }
    }
    // A named feel wins unless a sharpness of its own needs fewer pins, so with none it's no use
    // looking.
    let custom = if matches!(named, Some((0, ..))) { None } else { core.best_fit().filter(|c| c.2 <= FOUND_WITHIN_MS) };
    match (named, custom) {
        (Some((p, _, found)), Some((_, pins, _))) if p <= pins.len() => Some(found),
        (_, Some((sharpness, pins, _))) => Some(core.found(Feel::Custom(sharpness), pins)),
        (named, None) => named.map(|n| n.2),
    }
}

#[cfg(test)]
mod tests {
    use super::{Curve, Feel, Ramp, apply, beats_at, find, grid_times, steady_from, tail_for, warp};
    use rbl_anlz::Beat;

    /// 120 to 140 BPM over 32 beats from 32,480 ms, the way a live speed-up goes: the tempo
    /// rising evenly over time. Every beat's exact time.
    fn speed_up() -> Vec<f64> {
        let (b0, b1, n) = (120.0, 140.0_f64, 32.0);
        let length = n * 120_000.0 / (b0 + b1);
        (0..=32)
            .map(|i| 32_480.0 + (-b0 + (b0 * b0 + 2.0 * (b1 - b0) * 60_000.0 * f64::from(i) / length).sqrt()) * length / (b1 - b0))
            .collect()
    }

    fn ramp(feel: Feel) -> Ramp {
        let truth = speed_up();
        Ramp { start_ms: truth[0], end_ms: truth[32], from_bpm: 120.0, to_bpm: 140.0, beats: 32, feel }
    }

    #[test]
    fn curves_run_from_nothing_to_all_without_going_back() {
        for curve in [Curve { sharpness: 0.0, centre: 0.5 }, Curve { sharpness: 10.0, centre: 0.3 }, Curve { sharpness: 30.0, centre: -2.0 }] {
            assert!(curve.at(0.0).abs() < 1e-9 && (curve.at(1.0) - 1.0).abs() < 1e-9, "{curve:?}");
            let steps: Vec<f64> = (0..=100).map(|i| curve.at(f64::from(i) / 100.0)).collect();
            assert!(steps.windows(2).all(|w| w[1] >= w[0]), "{curve:?} goes back on itself");
        }
    }

    #[test]
    fn the_feel_never_moves_the_ends() {
        for feel in [Feel::Gradual, Feel::Medium, Feel::Sudden, Feel::Custom(55.0)] {
            let r = ramp(feel);
            let times = r.times().unwrap();
            assert_eq!(times.len(), 33);
            assert_eq!(times[0], r.start_ms, "{feel:?}");
            assert_eq!(times[32], r.end_ms, "{feel:?}");
        }
    }

    #[test]
    fn gradual_follows_a_real_speed_up_and_sudden_moves_the_middle() {
        let truth = speed_up();
        let worst = |feel| ramp(feel).times().unwrap().iter().zip(&truth).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
        assert!(worst(Feel::Gradual) < 2.0, "gradual is {} ms off", worst(Feel::Gradual));
        assert!(worst(Feel::Sudden) > 100.0, "sudden should be a different change, not the same one");
    }

    #[test]
    fn a_hand_set_sharpness_gentler_than_fits_becomes_the_gentlest() {
        let gentlest = Curve::gentlest(ramp(Feel::Gradual).area().unwrap()).unwrap();
        assert_eq!(ramp(Feel::Custom(0.0)).curve().unwrap().sharpness, gentlest);
        assert_eq!(ramp(Feel::Custom(f64::NAN)).curve().unwrap().sharpness, gentlest);
    }

    #[test]
    fn counting_finds_the_beats_and_impossible_ramps_are_refused() {
        let r = ramp(Feel::Gradual);
        assert_eq!(Ramp::count_beats(r.start_ms, r.end_ms, 120.0, 140.0), Some(32));
        // 24 beats at 120 to 140 BPM can't fill this long without slowing below 120 somewhere.
        assert_eq!(Ramp { beats: 24, ..r }.area(), None);
        assert_eq!(Ramp { to_bpm: 120.0, ..r }.area(), None, "no change of tempo");
        assert_eq!(Ramp { end_ms: r.start_ms, ..r }.area(), None, "no length");
    }

    #[test]
    fn slowing_down_works_the_same_way() {
        let r = Ramp { start_ms: 0.0, end_ms: 14_769.0, from_bpm: 140.0, to_bpm: 120.0, beats: 32, feel: Feel::Medium };
        let times = r.times().unwrap();
        assert!(times.windows(2).all(|w| w[1] > w[0]));
        assert_eq!(times[32], 14_769.0);
    }

    #[test]
    fn applying_keeps_what_came_before_and_carries_on_at_the_new_tempo() {
        // A steady 120 BPM grid from 480 ms with beat 1 on the first beat, as an analysis gives.
        let grid: Vec<Beat> = (0..140).map(|i| Beat { beat_number: (i % 4 + 1) as u16, tempo_x100: 12000, time_ms: 480 + i * 500 }).collect();
        let r = ramp(Feel::Gradual);
        let out = apply(&grid, 64, &r, 70_000.0, &[]).unwrap();
        assert_eq!(&out[..64], &grid[..64], "the beats before are untouched");
        assert_eq!(out[96].time_ms, r.end_ms.round() as u32, "the end beat sits on the end");
        assert_eq!(out[96].beat_number, 1, "32 beats on, it's still a downbeat");
        assert!((12000..12100).contains(&out[64].tempo_x100), "starts near 120: {}", out[64].tempo_x100);
        assert_eq!(out[96].tempo_x100, 14000);
        // After the ramp: 140 BPM, each beat within half a ms of where it should be, to the end.
        for (j, b) in out[96..].iter().enumerate() {
            assert!((f64::from(b.time_ms) - (r.end_ms + j as f64 * 60_000.0 / 140.0)).abs() <= 0.5);
        }
        assert!(f64::from(out.last().unwrap().time_ms) + 60_000.0 / 140.0 >= 70_000.0, "fills the track");
        // A start that isn't on the beat, or an end past the track, is refused.
        assert_eq!(apply(&grid, 63, &r, 70_000.0, &[]), None);
        assert_eq!(apply(&grid, 64, &r, 40_000.0, &[]), None);
    }

    #[test]
    fn warping_moves_the_pinned_beats_and_spreads_the_ones_between() {
        // Ten beats 500 ms apart, pinned at beats 2 and 8. Beat 5 is dragged 60 ms earlier.
        let times: Vec<f64> = (0..10).map(|k| f64::from(k) * 500.0).collect();
        let out = warp(&times, &[(2, 1000.0), (5, 2440.0), (8, 4000.0)]);
        assert_eq!((out[2], out[5], out[8]), (1000.0, 2440.0, 4000.0), "pins sit where they're put");
        assert_eq!(&out[..2], &times[..2], "before the first pin, nothing moved");
        assert_eq!(&out[9..], &times[9..], "after the last, nothing moved");
        // Between pins the spacing is even again, squeezed before the dragged beat and stretched after.
        assert!((out[3] - 1480.0).abs() < 1e-9 && (out[4] - 1960.0).abs() < 1e-9);
        assert!((out[6] - 2960.0).abs() < 1e-9 && (out[7] - 3480.0).abs() < 1e-9);
        // Dragging the last pin carries everything after it along: the grid follows.
        let moved = warp(&times, &[(2, 1000.0), (8, 4100.0)]);
        assert_eq!(moved[9], 4600.0);
        assert_eq!(warp(&times, &[]), times, "no pins, no change");
    }

    #[test]
    fn beats_from_exact_times_keep_a_steady_tempo_and_fill_the_track() {
        let grid: Vec<Beat> = (0..4).map(|i| Beat { beat_number: (i % 4 + 1) as u16, tempo_x100: 12000, time_ms: i * 500 }).collect();
        // From beat 1 on, 140 BPM exactly: 428.571… ms apart, which rounds unevenly to whole ms.
        let gap = 60_000.0 / 140.0;
        let times: Vec<f64> = (0..4).map(|k| if k == 0 { 0.0 } else { 500.0 + f64::from(k - 1) * gap }).collect();
        let out = beats_at(&grid, 1, &times, &[], 140.0, 3000.0).unwrap();
        assert_eq!(out[0], grid[0], "before the start, untouched");
        assert!(out[1..].iter().all(|b| b.tempo_x100 == 14000), "steady, not jittering with the rounding");
        assert_eq!(out.len(), 7, "filled at 140 to the end of the track");
        // Beats that go backwards are refused.
        assert_eq!(beats_at(&grid, 1, &[0.0, 500.0, 400.0, 900.0], &[], 140.0, 3000.0), None);
    }

    /// A steady 120 BPM grid from 480 ms to 70 s, with beat 1 on the first beat.
    fn steady() -> Vec<Beat> {
        (0..140).map(|i| Beat { beat_number: (i % 4 + 1) as u16, tempo_x100: 12000, time_ms: 480 + i * 500 }).collect()
    }

    /// The steady grid with a ramp up from beat 64 put in, then pinned at any `pins` (moved by
    /// so many ms), as dragging beats does.
    fn ramped(feel: Feel, pins: &[(usize, f64)]) -> Vec<Beat> {
        let r = ramp(feel);
        let times = grid_times(&steady(), 64, &r, 70_000.0, &[]).unwrap();
        let mut held = vec![(64, times[64]), (96, times[96])];
        held.extend(pins.iter().map(|&(k, by)| (k, times[k] + by)));
        held.sort_by_key(|p| p.0);
        beats_at(&steady(), 64, &warp(&times, &held), &[], 140.0, 70_000.0).unwrap()
    }

    #[test]
    fn ramps_made_with_a_named_feel_are_found_as_they_were_made() {
        for feel in [Feel::Gradual, Feel::Medium, Feel::Sudden] {
            let found = find(&ramped(feel, &[]));
            assert_eq!(found.len(), 1, "{feel:?}");
            let f = &found[0];
            assert_eq!((f.start, f.ramp.beats, f.ramp.feel), (64, 32, feel), "{feel:?}");
            assert_eq!((f.ramp.from_bpm, f.ramp.to_bpm), (120.0, 140.0));
            assert!(f.pins.is_empty(), "{feel:?} came back pinned at {:?}", f.pins);
        }
    }

    #[test]
    fn dragged_beats_come_back_as_pins() {
        let found = find(&ramped(Feel::Medium, &[(72, 40.0), (84, -30.0)]));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].ramp.feel, Feel::Medium);
        assert_eq!(found[0].pins.iter().map(|p| p.0).collect::<Vec<_>>(), [72, 84]);
    }

    #[test]
    fn a_hand_bent_ramp_comes_back_close() {
        let grid = ramped(Feel::Custom(17.0), &[]);
        let found = find(&grid);
        assert_eq!(found.len(), 1);
        let f = &found[0];
        // Its ends are as good as steady, so it may be found a beat or two inside them, but
        // every beat it puts back is within 2 ms of where it was.
        assert!((64..=66).contains(&f.start) && f.start + f.ramp.beats as usize <= 96, "{f:?}");
        let mut times = grid_times(&grid, f.start, &f.ramp, 70_000.0, &[]).unwrap();
        let end = f.start + f.ramp.beats as usize;
        let mut held = vec![(f.start, times[f.start]), (end, times[end])];
        held.extend(&f.pins);
        held.sort_by_key(|p| p.0);
        times = warp(&times, &held);
        for (b, t) in grid.iter().zip(&times) {
            assert!((f64::from(b.time_ms) - t).abs() <= 2.0);
        }
    }

    #[test]
    fn steady_grids_and_wobbles_are_not_ramps_and_jumps_are_instant_changes() {
        assert!(find(&steady()).is_empty());
        // A jump from one tempo to another, as an analysis gives a song with two: an instant
        // change on the first beat at the new tempo.
        let mut jump = steady();
        for (i, b) in jump.iter_mut().enumerate().skip(70) {
            b.tempo_x100 = 14000;
            b.time_ms = 480 + 70 * 500 + (i as u32 - 70) * 60_000 / 140;
        }
        let found = find(&jump);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].start, 70);
        assert!(found[0].ramp.instant());
        assert_eq!((found[0].ramp.from_bpm, found[0].ramp.to_bpm, found[0].ramp.start_ms), (120.0, 140.0, 35_480.0));
        // Put back in as found, it's the grid it was found in (filled on to the end of the track,
        // which this one stops short of); with another tempo, the grid after follows that one.
        assert_eq!(&apply(&jump, 70, &found[0].ramp, 70_000.0, &jump[70..]).unwrap()[..jump.len()], &jump[..]);
        let faster = apply(&jump, 70, &Ramp { to_bpm: 150.0, ..found[0].ramp }, 70_000.0, &jump[70..]).unwrap();
        assert_eq!(&faster[..71], &jump[..71].iter().map(|b| if b.time_ms == 35_480 { Beat { tempo_x100: 15000, ..*b } } else { *b }).collect::<Vec<_>>()[..]);
        assert!(faster[70..].iter().all(|b| b.tempo_x100 == 15000));
        assert_eq!(faster[71].time_ms, 35_480 + 400);
        // A grid whose tempo wobbles every beat has no steady stretch for a ramp to go between.
        let wobbly: Vec<Beat> = (0..140u32).map(|i| Beat { beat_number: (i % 4 + 1) as u16, tempo_x100: 12000 + (i * 37 % 11) as u16 * 7, time_ms: 480 + i * 500 }).collect();
        assert!(find(&wobbly).is_empty());
    }

    #[test]
    fn reshaping_one_ramp_leaves_a_later_one_alone() {
        // Up to 140 from beat 64, then back down to 120 over 16 beats from beat 112.
        let first = ramped(Feel::Medium, &[]);
        let (s2, e2) = (f64::from(first[112].time_ms), f64::from(first[112].time_ms) + 7_400.0);
        let beats = Ramp::count_beats(s2, e2, 140.0, 120.0).unwrap();
        let down = Ramp { start_ms: s2, end_ms: e2, from_bpm: 140.0, to_bpm: 120.0, beats, feel: Feel::Gradual };
        let both = apply(&first, 112, &down, 70_000.0, &[]).unwrap();
        let found = find(&both);
        assert_eq!(found.iter().map(|f| f.start).collect::<Vec<_>>(), [64, 112]);
        // Making the first one sudden keeps every beat from its end on as it was.
        let f = &found[0];
        let tail = &both[96..];
        let sudden = apply(&both, 64, &Ramp { feel: Feel::Sudden, ..f.ramp }, 70_000.0, tail).unwrap();
        assert_eq!(&sudden[96..], tail);
        assert_ne!(&sudden[64..96], &both[64..96]);
        // Ending it a beat's worth later carries the later one along with it.
        let later = Ramp { end_ms: f.ramp.end_ms + 60_000.0 / 140.0, beats: 33, ..f.ramp };
        let moved = apply(&both, 64, &later, 70_000.0, tail).unwrap();
        assert_eq!(moved[112 + 1].time_ms, both[112].time_ms + 429, "the later change starts a beat on");
        // Changing the tempo it goes to can't keep them: the grid follows the new tempo.
        assert!(tail_for(tail, &Ramp { to_bpm: 145.0, ..f.ramp }).is_empty());
        // Taking the first one out carries on at 120 from its start.
        let out = steady_from(&both, 64, 120.0, 70_000.0).unwrap();
        assert!(out[64..].iter().all(|b| b.tempo_x100 == 12000) && out[65].time_ms == both[64].time_ms + 500);
    }

    #[test]
    fn a_change_that_turns_straight_back_is_found_as_two() {
        // Up to 140 from beat 64 to 96, then straight back down to 120 from beat 96: no steady
        // bar between, and the same tempo at both ends of the pair.
        let first = ramped(Feel::Medium, &[]);
        for feel in [Feel::Gradual, Feel::Medium, Feel::Sudden] {
            let s2 = f64::from(first[96].time_ms);
            let e2 = s2 + 7_400.0;
            let beats = Ramp::count_beats(s2, e2, 140.0, 120.0).unwrap();
            // Started with TEMPO at beat 96, it goes in at the tempo of the beat before.
            let from = f64::from(first[95].tempo_x100) / 100.0;
            let down = Ramp { start_ms: s2, end_ms: e2, from_bpm: from, to_bpm: 120.0, beats, feel };
            let both = apply(&first, 96, &down, 70_000.0, &[]).unwrap();
            let found = find(&both);
            assert_eq!(found.len(), 2, "{feel:?}");
            assert_eq!(found[0].start, 64, "{feel:?}");
            if feel == Feel::Sudden {
                // A sudden ramp's first beats are as good as steady, so where it starts is only
                // known to a few beats: anywhere there puts every beat back within the tolerance.
                assert!((96..=100).contains(&found[1].start), "{:?}", found[1].start);
                let again = apply(&both, found[1].start, &found[1].ramp, 70_000.0, &[]).unwrap();
                assert!(again.iter().zip(&both).all(|(a, b)| a.time_ms.abs_diff(b.time_ms) <= 2), "Sudden doesn't rebuild the grid");
            } else {
                // Reopened, the later one is the ramp as it was made.
                assert_eq!(found[1].start, 96, "{feel:?}");
                assert_eq!(found[1].ramp.from_bpm, from, "{feel:?}");
            }
            let (up, back) = (&found[0].ramp, &found[1].ramp);
            assert!((up.from_bpm - 120.0).abs() < 0.02 && (up.to_bpm - 140.0).abs() < 0.3, "{feel:?} {up:?}");
            assert!((back.from_bpm - 140.0).abs() < 0.3 && (back.to_bpm - 120.0).abs() < 0.02, "{feel:?} {back:?}");
            assert!(up.end_ms <= back.start_ms, "{feel:?}: they overlap");
        }
    }

    #[test]
    fn bending_moves_the_end_and_gradual_draws_the_bend() {
        // Bent toward early, there's more area under the line; toward late, less.
        assert!(Curve::bent_area(-6.0) > 0.5 && Curve::bent_area(6.0) < 0.5);
        for b in [-14.0, -4.0, 0.0, 9.0] {
            assert!((Curve::bend_for(Curve::bent_area(b)) - b).abs() < 1e-6, "{b}");
        }
        for b in [-14.0, -4.0, 9.0] {
            // A ramp bent this far: its end is wherever 32 beats come to with the bend.
            let (p0, p1) = (500.0, 60_000.0 / 140.0);
            let area = Curve::bent_area(b);
            let end_ms = 32_480.0 + 32.0 * (p0 + (p1 - p0) * area);
            let r = Ramp { start_ms: 32_480.0, end_ms, from_bpm: 120.0, to_bpm: 140.0, beats: 32, feel: Feel::Gradual };
            // Gradual draws the bent line, to within a ms of a beat.
            // The time of each beat on the bent line: every beat before it, each beat's length
            // averaged across it.
            let s = |u: f64| (b * u).exp_m1() / b.exp_m1();
            let length = |j: u32| (0..=8).map(|q| p0 + (p1 - p0) * s((f64::from(j) + f64::from(q) / 8.0) / 32.0)).sum::<f64>() / 9.0;
            let bent: Vec<f64> = (0..=32).map(|i| 32_480.0 + (0..i).map(length).sum::<f64>()).collect();
            let times = r.times().unwrap();
            let scale = (end_ms - 32_480.0) / (bent[32] - 32_480.0);
            for (t, u) in times.iter().zip(&bent) {
                assert!((t - (32_480.0 + (u - 32_480.0) * scale)).abs() < 1.0, "bend {b}: {t} vs {u}");
            }
        }
    }

    /// The beats a DAW makes for a curved tempo change over 16 bars from beat 64 of the steady
    /// grid: tempo bent along the beats from 120 to 160, `bpm(b) = 120 + 40 * (e^(kb/64) - 1) /
    /// (e^k - 1)`, late for `k` above 0. Every beat's exact time from `start`, then on at 160.
    fn daw_curve(k: f64) -> (Vec<Beat>, Vec<f64>) {
        let bpm = |b: f64| 120.0 + 40.0 * (k * b / 64.0).exp_m1() / k.exp_m1();
        let start = 480.0 + 64.0 * 500.0;
        let mut times = vec![start];
        // Each beat lasts as long as the tempo across it takes, added up finely.
        for n in 0..64 {
            let steps = 200.0;
            let length: f64 = (0..200).map(|q| 60_000.0 / bpm(f64::from(n) + (f64::from(q) + 0.5) / steps) / steps).sum();
            times.push(times[n as usize] + length);
        }
        let mut all = times.clone();
        while let Some(&last) = all.last().filter(|t| **t + 375.0 < 120_000.0) {
            all.push(last + 375.0);
        }
        let mut grid: Vec<Beat> = steady()[..64].to_vec();
        for (i, w) in all.windows(2).enumerate() {
            grid.push(Beat { beat_number: ((64 + i) % 4 + 1) as u16, tempo_x100: (6_000_000.0 / (w[1] - w[0])).round() as u16, time_ms: w[0].round() as u32 });
        }
        (grid, times)
    }

    #[test]
    fn a_daw_curve_is_one_of_ours() {
        // Speed-ups that hold back and then rush in: the tempo line sags, so they're S-curves
        // exactly. (One that bulges, getting most of the way early, isn't; see the module docs.)
        for k in [1.5, 2.0, 4.0] {
            let (grid, times) = daw_curve(k);
            // Found as one ramp over the DAW's 16 bars, with the DAW's bend as its sharpness, and
            // nothing to pin.
            let found = find(&grid);
            assert_eq!(found.len(), 1, "k {k}");
            let f = &found[0];
            assert_eq!((f.start, f.ramp.beats), (64, 64), "k {k}");
            assert!(f.pins.is_empty(), "k {k} needed pins at {:?}", f.pins);
            let Feel::Custom(sharpness) = f.ramp.feel else { panic!("k {k} came back {:?}", f.ramp.feel) };
            assert!((sharpness - k).abs() < 0.1, "k {k} came back with sharpness {sharpness}");
            // That sharpness, over the right bars, puts every beat where the DAW did.
            let r = Ramp { feel: Feel::Custom(k), ..f.ramp };
            for (t, daw) in r.times().unwrap().iter().zip(&times) {
                assert!((t - daw).abs() < 1.0, "k {k}: {t} vs {daw}");
            }
            // Gradual over the same bars is the gentlest S, not the DAW's: well off.
            let gradual = Ramp { feel: Feel::Gradual, ..f.ramp }.times().unwrap();
            assert!(gradual.iter().zip(&times).any(|(t, daw)| (t - daw).abs() > 5.0), "k {k}");
        }
    }

    #[test]
    fn the_beat_count_is_guessed_in_whole_bars() {
        // Over the k = 2 curve's span, an even guess would be 66 beats. Whole bars make it 64 or
        // 68, of which 68 is more even; the DAW's was 64, which only typing the bars can say.
        let (_, times) = daw_curve(2.0);
        assert_eq!(Ramp::count_beats(times[0], times[64], 120.0, 160.0), Some(68));
        // A span only a few beats long, where no whole bar fits, still gets a count.
        let r = ramp(Feel::Gradual);
        let short = r.start_ms + 3.0 * 60_000.0 / 130.0;
        assert_eq!(Ramp::count_beats(r.start_ms, short, 120.0, 140.0), Some(3));
    }

    #[test]
    fn a_ramp_can_start_on_the_first_beat() {
        // The track opens speeding up: 120 to 140 over 32 beats from its first beat, then steady.
        for feel in [Feel::Gradual, Feel::Medium, Feel::Sudden] {
            let r = Ramp { start_ms: 480.0, end_ms: 480.0 + ramp(feel).end_ms - ramp(feel).start_ms, ..ramp(feel) };
            let grid = apply(&steady(), 0, &r, 70_000.0, &[]).unwrap();
            let found = find(&grid);
            assert_eq!(found.len(), 1, "{feel:?}");
            let f = &found[0];
            assert_eq!((f.start, f.ramp.beats, f.ramp.to_bpm), (0, 32, 140.0), "{feel:?}");
            assert!((f.ramp.from_bpm - 120.0).abs() <= 0.05, "{feel:?} came in at {}", f.ramp.from_bpm);
            assert!(f.pins.len() <= 1, "{feel:?} pinned at {:?}", f.pins);
        }
    }
}

