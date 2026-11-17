//! Tempo changes in beat grid edit mode: placing a ramp and shaping it.
//!
//! With Grid on, TEMPO opens one at the playhead: the tempo change the playhead is in, if the
//! grid has one there, or else a new one starting on the beat nearest it and running four bars.
//! Its two ends are dragged to where the tempo starts to change and where it has settled, or a
//! click outside it brings the nearer end there. The start always sits on a beat; the end doesn't
//! snap to anything in the audio, because there isn't always a kick there to snap to. The ramp's
//! settings take the grid tools' place: the tempo it changes to, how many bars it takes, and how
//! sudden it is.
//!
//! Every change is worked out afresh from the grid as it was before the ramp, so nothing builds
//! up. Shaping it never moves its ends or either tempo; only the curve between them changes.
//! Moving an end counts the bars again. The maths is in [`crate::tempo_ramp`]. Each change is one
//! step of the grid's undo, which keeps the ramp as it was beside the grid: undoing steps back
//! through the ramp's changes, then to it as it was opened, and only the step before that leaves
//! ramp editing.
//!
//! Where the curve doesn't follow the music exactly, beats can be dragged. A dragged beat is
//! pinned where it's dropped, and the beats between it and the pins either side spread out to
//! fit, keeping their spacing relative to each other. The ramp's start and end count as pins, so
//! a drag inside the ramp never moves anything outside it; past the last pin, everything moves
//! with it, as the grid follows. A double-click on a pinned beat unpins it. Moving an end changes
//! which beat is which, so it unpins them all.
//!
//! The handle on the tempo line shapes the curve two ways. Dragged, it makes the change more
//! sudden or more gradual between ends that stay put. Alt-dragged, it bends the line as a DAW
//! bends automation, keeping the start and the beats and moving the end to fit, as in Ableton,
//! whose tempo changes are laid out in beats rather than pinned to the audio. Why it's these two
//! and not a free bend or a node editor is set out at length in [`crate::tempo_ramp`], under "Why
//! the curve is edited the way it is".
//!
//! Nothing records a ramp once it's done, so the ones in the grid are found again from its beats
//! ([`tempo_ramp::find`]). Opened again, a found ramp comes back with any beats that sit off its
//! curve pinned where they are. Reshaping a ramp keeps the grid after it as it is (a later change,
//! say), moving with its end, unless its new tempo changes, when the grid follows that.

use rbl_anlz::Beat;
use rune::TextEdit;

use crate::app::App;
use crate::jobs::{JobKind, Outcome};
use crate::tempo_ramp::{self, Curve, Feel, Found, MAX_BEND, Ramp};

/// How long a new ramp starts out, in beats: four bars.
const NEW_RAMP_BEATS: f64 = 16.0;

/// Beats can't be closer than this: 499 BPM, the most rekordbox takes.
const MIN_GAP_MS: f64 = 60_000.0 / 499.0;

/// A tempo change being shaped.
#[derive(Clone)]
pub struct RampEdit {
    /// The beat the ramp starts on, as an index into `head` and into the edited grid alike: the
    /// beats before it never change.
    pub start: usize,
    /// The grid before the ramp, carrying on at the tempo going in. Every change is worked out
    /// from this one, and Remove puts it back.
    head: Vec<Beat>,
    /// Beats from the ramp's end on that stay as they are while it changes to the tempo they
    /// start at (see [`tempo_ramp::tail_for`]). Empty for a ramp placed just now.
    tail: Vec<Beat>,
    pub ramp: Ramp,
    /// Whether the grid shows the ramp yet. It doesn't while the tempo it changes to still needs
    /// typing: when the grid at the end is still at the old tempo, there's nothing to guess from.
    pub shown: bool,
    /// The curve being dragged.
    bend: Option<Bending>,
    /// Beats dragged into place, as "beat `k` at `t` ms", in order of beat. All are after the
    /// ramp's start.
    pub pins: Vec<(usize, f64)>,
    /// The exact time of every beat as shown, warped through the pins: what the grid was rounded
    /// from, and what the tempo line is drawn from, so it doesn't wobble with the rounding.
    pub times: Vec<f64>,
    /// An end or a beat being dragged.
    held: Option<Held>,
    /// The beat nearest the last press, which a double-click unpins.
    last_press: Option<usize>,
    /// How far the ramp's ends can go, in ms: the end of the tempo change before it and the start
    /// of the one after, or the track's ends. Dragging an end or bending stops there rather than
    /// running into a neighbour.
    room: (f64, f64),
    /// Where the end was before Alt-dragging the handle first moved it: where it was put to match
    /// the audio, which a double-click on the handle puts it back to. Moving an end, or typing the
    /// bars or the tempo, makes wherever it is then the place to go back to.
    unbent_end: Option<f64>,
}

/// A drag on the curve's handle, from the press.
#[derive(Clone)]
struct Bending {
    /// Alt-dragged: bent as a DAW bends automation, moving the end, rather than made sharper or
    /// gentler between ends that stay.
    alt: bool,
    /// The sharpness and the bend the curve came to at the press, which the drag goes on from.
    sharpness: f64,
    bend: f64,
    /// The ramp, its pins and the grid at the press, to put back if the drag is cancelled, and so
    /// the whole drag is one step of undo.
    ramp: Ramp,
    pins: Vec<(usize, f64)>,
    before: Vec<Beat>,
}

/// How far an Alt-drag bends the curve for each point dragged.
const BEND_PER_POINT: f64 = 0.06;

/// What a drag on the zoomed waveform has hold of.
#[derive(Clone, Copy, PartialEq)]
enum Grab {
    Start,
    End,
    Beat(usize),
}

/// A drag, from the press.
#[derive(Clone)]
struct Held {
    grab: Grab,
    /// Where what's held was at the press, and the time under the pointer then, so it moves as
    /// far as the pointer does without jumping to it.
    from_ms: f64,
    press_ms: f64,
    /// The ramp and grid at the press, to put back if the drag is cancelled, and so the drag is
    /// one step of undo.
    start: usize,
    ramp: Ramp,
    pins: Vec<(usize, f64)>,
    before: Vec<Beat>,
}

impl RampEdit {
    /// The ramp as it stands, without a drag or a press in hand: what undo puts back.
    pub(crate) fn settled(&self) -> RampEdit {
        RampEdit { held: None, bend: None, last_press: None, ..self.clone() }
    }

    /// The ramp's curve, if it fits.
    pub fn curve(&self) -> Option<Curve> {
        self.ramp.curve()
    }

    /// Whether an end, a beat or the curve is being dragged.
    pub fn dragging(&self) -> bool {
        self.held.is_some() || self.bend.is_some()
    }

    /// Whether the start is being dragged, to draw it as held.
    pub fn holding_start(&self) -> bool {
        self.held.as_ref().is_some_and(|h| h.grab == Grab::Start)
    }

    /// Whether the end is being dragged, to draw it as held.
    pub fn holding_end(&self) -> bool {
        self.held.as_ref().is_some_and(|h| h.grab == Grab::End)
    }

    /// The ramp's end, as a beat.
    fn end(&self) -> usize {
        self.start + self.ramp.beats as usize
    }

    /// The beats nothing moves except a drag: the ramp's start and end, and every pin.
    fn anchors(&self) -> Vec<usize> {
        let mut ks: Vec<usize> = self.pins.iter().map(|p| p.0).chain([self.start, self.end()]).collect();
        ks.sort_unstable();
        ks.dedup();
        ks
    }

    /// The grid with `ramp` from beat `start`, warped through `pins`, with the ramp's own ends
    /// held as pins too: the exact times and the rounded beats. None if the ramp doesn't fit, or
    /// the pins would make beats cross or leave rekordbox's tempo range.
    fn build(&self, start: usize, ramp: &Ramp, pins: &[(usize, f64)], total_ms: f64) -> Option<(Vec<f64>, Vec<Beat>)> {
        let times = tempo_ramp::grid_times(&self.head, start, ramp, total_ms, &self.tail)?;
        let mut held: Vec<(usize, f64)> = pins.iter().copied().filter(|p| p.0 < times.len()).collect();
        for k in [start, start + ramp.beats as usize] {
            if k < times.len() && !held.iter().any(|p| p.0 == k) {
                held.push((k, times[k]));
            }
        }
        held.sort_by_key(|p| p.0);
        let warped = tempo_ramp::warp(&times, &held);
        let beats = tempo_ramp::beats_at(&self.head, start, &warped, tempo_ramp::tail_for(&self.tail, ramp), ramp.to_bpm, total_ms)?;
        Some((warped, beats))
    }
}

/// Why `ramp` can't go into the grid, in words.
fn why_not(ramp: &Ramp, total_ms: f64) -> String {
    if ramp.end_ms >= total_ms {
        return "The tempo change has to end before the track does.".to_owned();
    }
    if ramp.area().is_none() {
        return format!(
            "{} bars can't get from {:.2} to {:.2} BPM between these points without the tempo overshooting. Try other bars.",
            bars_text(ramp.beats),
            ramp.from_bpm,
            ramp.to_bpm
        );
    }
    "That would put a beat outside the 40 to 499 BPM rekordbox allows.".to_owned()
}

/// Bars as typed: 8, 7.5, 7,5.
fn bars_text(beats: u32) -> String {
    if beats.is_multiple_of(4) { (beats / 4).to_string() } else { format!("{}", f64::from(beats) / 4.0) }
}

impl App {
    /// The ramp being shaped, while Grid is on.
    pub fn ramp_edit(&self) -> Option<&RampEdit> {
        self.grid_edit.as_ref().filter(|_| self.grid_editing())?.ramp.as_ref()
    }

    /// The tempo changes found in the deck track's grid.
    pub fn deck_ramps(&self) -> &[Found] {
        self.deck_wave.as_ref().filter(|w| self.player.is_on(&w.id)).map_or(&[], |w| &w.ramps)
    }

    /// TEMPO: opens the tempo change the playhead is in, or starts a new one there, on the beat
    /// nearest it, four bars long.
    pub fn start_ramp(&mut self) {
        let Some(total) = self.deck_duration_ms().filter(|t| *t > 0.0) else { return };
        let playhead = self.player.now() * total;
        // Straight after an edit, the search for tempo changes may not have finished.
        self.find_ramps_now();
        // An instant change has no length to be in, so the playhead within half a beat of it will do.
        let near = |f: &Found| {
            let slack = if f.ramp.instant() { 30_000.0 / f.ramp.to_bpm } else { 0.0 };
            (f.ramp.start_ms - slack..=f.ramp.end_ms + slack).contains(&playhead)
        };
        if let Some(f) = self.deck_ramps().iter().find(|f| near(f)).cloned() {
            self.open_ramp(f, total);
            return;
        }
        let room = self.room_around(playhead, playhead, total);
        let Some(g) = self.grid_edit.as_mut() else { return };
        let start = crate::grid_edit::nearest(&g.beats, playhead).filter(|i| i + 2 < g.beats.len());
        let Some(start) = start else {
            self.ramp_refused("There's no room for a tempo change here.");
            return;
        };
        let start_ms = f64::from(g.beats[start].time_ms);
        // The tempo going in is the grid's up to the start beat; from the first beat, where nothing
        // comes before, the grid's there, which is steady until the ramp changes it.
        let from = f64::from(g.beats[start.saturating_sub(1)].tempo_x100) / 100.0;
        let end_ms = (start_ms + NEW_RAMP_BEATS * 60_000.0 / from).min(room.1);
        if end_ms < start_ms + 2.0 * 60_000.0 / from || start_ms < room.0 {
            self.ramp_refused("There's no room for a tempo change here: it's too close to the end of the track, or to another tempo change.");
            return;
        }
        // The tempo coming out starts as the grid's own at the end. On a grid that already
        // follows the change that's right; on a steady one it's still the old tempo, and has to
        // be typed.
        let to = crate::deck::grid_pos(&g.beats, end_ms).map_or(from, |p| p.bpm);
        let ramp = Ramp { start_ms, end_ms, from_bpm: from, to_bpm: to, beats: 0, feel: Feel::Gradual };
        g.ramp = Some(RampEdit { start, head: g.beats.clone(), tail: Vec::new(), ramp, shown: false, bend: None, pins: Vec::new(), times: Vec::new(), held: None, last_press: None, room, unbent_end: None });
        if (to - from).abs() >= 0.5 {
            self.set_ramp_to_bpm(to);
        }
        self.show_ramp_fields();
        self.show_grid_edit();
    }

    /// The room a ramp from `start_ms` to `end_ms` has: from the end of the tempo change before it
    /// (or the track's start) to the start of the one after (or just short of the track's end).
    fn room_around(&self, start_ms: f64, end_ms: f64, total: f64) -> (f64, f64) {
        let others = || self.deck_ramps().iter().map(|f| f.ramp).filter(|r| !(r.start_ms == start_ms && r.end_ms == end_ms));
        let before = others().filter(|r| r.end_ms <= start_ms).map(|r| r.end_ms).fold(0.0, f64::max);
        let after = others().filter(|r| r.start_ms >= end_ms && !(r.start_ms == end_ms && end_ms == start_ms)).map(|r| r.start_ms).fold(total - 1.0, f64::min);
        (before, after)
    }

    /// Opens a ramp found in the grid to shape. The grid doesn't change until something does.
    fn open_ramp(&mut self, f: Found, total: f64) {
        let room = self.room_around(f.ramp.start_ms, f.ramp.end_ms, total);
        let Some(g) = self.grid_edit.as_mut() else { return };
        let end = f.start + f.ramp.beats as usize;
        let (Some(head), Some(tail)) = (tempo_ramp::steady_from(&g.beats, f.start, f.ramp.from_bpm, total), g.beats.get(end..)) else { return };
        let mut edit = RampEdit { start: f.start, head, tail: tail.to_vec(), ramp: f.ramp, shown: true, bend: None, pins: f.pins, times: Vec::new(), held: None, last_press: None, room, unbent_end: None };
        match edit.build(edit.start, &edit.ramp, &edit.pins, total) {
            Some((times, _)) => {
                edit.times = times;
                g.ramp = Some(edit);
            }
            None => self.ramp_refused("That tempo change can't be opened."),
        }
        self.show_ramp_fields();
        self.show_grid_edit();
    }

    /// Whether the ramp is still waiting for the tempo it changes to, so nothing has changed yet.
    pub fn ramp_unplaced(&self) -> bool {
        self.ramp_edit().is_some_and(|a| !a.shown)
    }

    /// Back to the grid tools, leaving the ramp as it is.
    pub fn finish_ramp(&mut self) {
        if let Some(g) = self.grid_edit.as_mut() {
            g.ramp = None;
        }
        self.show_grid_edit();
    }

    /// Takes the ramp out: the grid carries on at the tempo going in from its start, as it was
    /// before a ramp placed just now. Undo brings it back.
    pub fn remove_ramp(&mut self) {
        let Some(g) = self.grid_edit.as_mut() else { return };
        if let Some(a) = g.ramp.take() {
            g.replace(a.head);
        }
        self.show_grid_edit();
    }

    /// Makes `change` to the ramp and puts the result in the grid, as one step of undo unless
    /// `live` (a drag, which counts as one step when it ends). A change that doesn't fit is
    /// refused, and says why.
    fn reramp(&mut self, live: bool, change: impl FnOnce(&mut Ramp)) {
        let Some(total) = self.deck_duration_ms().filter(|t| *t > 0.0) else { return };
        let Some(g) = self.grid_edit.as_mut() else { return };
        let Some(a) = g.ramp.as_mut() else { return };
        let mut next = a.ramp;
        change(&mut next);
        // Other bars change which beat is which after the start, so the pins there no longer mean
        // what they did and go.
        let pins = if next.beats == a.ramp.beats { a.pins.clone() } else { Vec::new() };
        let Some((times, beats)) = a.build(a.start, &next, &pins, total) else {
            if !live {
                // The pins may be what doesn't fit; say so rather than blaming the ramp.
                let why = if tempo_ramp::apply(&a.head, a.start, &next, total, &a.tail).is_some() {
                    "That would push beats past the pinned ones. Unpin some first.".to_owned()
                } else {
                    why_not(&next, total)
                };
                self.ramp_refused(&why);
                self.show_ramp_fields();
            }
            return;
        };
        a.ramp = next;
        a.pins = pins;
        a.times = times;
        a.shown = true;
        if live {
            g.beats = beats;
        } else {
            g.replace(beats);
        }
        self.show_ramp_fields();
        self.show_grid_edit();
    }

    /// Sets the tempo the ramp changes to. The first time, the bars are counted for it: the whole
    /// number that fits most evenly. After that the bars stay as they are.
    pub fn set_ramp_to_bpm(&mut self, to: f64) {
        self.settle_end();
        let first = self.ramp_unplaced();
        self.reramp(false, |r| {
            r.to_bpm = to;
            if first {
                r.beats = Ramp::count_beats(r.start_ms, r.end_ms, r.from_bpm, to).unwrap_or(0);
            }
        });
    }

    /// The tempo typed in the ramp's BPM box. One that can't be read puts the box back.
    pub fn submit_ramp_to_bpm(&mut self) {
        match self.ramp_to.text.trim().replace(',', ".").parse::<f64>() {
            Ok(bpm) if (40.0..=499.0).contains(&bpm) => self.set_ramp_to_bpm((bpm * 100.0).round() / 100.0),
            _ => {
                self.ramp_refused("Type a tempo from 40 to 499 BPM.");
                self.show_ramp_fields();
            }
        }
    }

    /// The bars typed in the ramp's bars box, to the quarter: a beat. 0 makes it an instant
    /// change on its start beat. Typed into an instant change, the end goes where that many bars
    /// come to at a tempo halfway between, short of the next tempo change.
    pub fn submit_ramp_bars(&mut self) {
        let beats = self.ramp_bars.text.trim().replace(',', ".").parse::<f64>().map(|b| (b * 4.0).round());
        let Some(room_end) = self.ramp_edit().map(|a| a.room.1) else { return };
        match beats {
            Ok(beats) if beats == 0.0 || (2.0..=1024.0).contains(&beats) => {
                self.settle_end();
                self.reramp(false, |r| {
                    if beats == 0.0 {
                        *r = Ramp { feel: r.feel, ..Ramp::instant_at(r.start_ms, r.from_bpm, r.to_bpm) };
                    } else {
                        if r.instant() {
                            let even = 120_000.0 / (r.from_bpm + r.to_bpm);
                            r.end_ms = (r.start_ms + beats * even).min(room_end);
                        }
                        r.beats = beats as u32;
                    }
                });
            }
            _ => {
                self.ramp_refused("Type a number of bars, like 8 or 7.5, or 0 for an instant change.");
                self.show_ramp_fields();
            }
        }
    }

    /// How sudden the change is.
    pub fn set_ramp_feel(&mut self, feel: Feel) {
        self.reramp(false, |r| r.feel = feel);
    }

    /// The press on the curve's handle, Alt held or not. An Alt-drag moves the end, which
    /// changes which beat is which, so it unpins them all.
    pub fn begin_bend(&mut self, alt: bool) {
        let Some(g) = self.grid_edit.as_mut() else { return };
        let before = g.beats.clone();
        let Some(a) = g.ramp.as_mut() else { return };
        let (Some(curve), Some(area)) = (a.curve(), a.ramp.area()) else { return };
        a.bend = Some(Bending { alt, sharpness: curve.sharpness, bend: Curve::bend_for(area), ramp: a.ramp, pins: a.pins.clone(), before });
        if alt {
            a.pins.clear();
            a.unbent_end.get_or_insert(a.ramp.end_ms);
        }
    }

    /// One step of dragging the curve's handle, `dy` points from the press.
    ///
    /// A plain drag keeps the ends where they are: up makes the change more sudden, down more
    /// gradual, and it stops at the most gradual curve that still fits.
    ///
    /// An Alt-drag bends the tempo line as a DAW bends automation: up bows it up, down bows it
    /// down. The start and the beats stay; the end moves to wherever the bent change takes them,
    /// and the grid after follows. Bowed up, a speed-up gets to the new tempo sooner, so its beats
    /// end sooner; a slow-down stays fast for longer.
    pub fn bend(&mut self, dy: f32) {
        let Some(total) = self.deck_duration_ms().filter(|t| *t > 0.0) else { return };
        let Some(a) = self.ramp_edit() else { return };
        let Some(b) = a.bend.as_ref() else { return };
        let up = f64::from(-dy);
        if !b.alt {
            // Sharpness grows by the same factor for each point dragged, so it feels even
            // whether the curve is gentle or steep.
            let sharpness = b.sharpness.max(0.5) * (up / 25.0).exp();
            self.reramp(true, |r| r.feel = Feel::Custom(sharpness));
            return;
        }
        // Bowing the tempo line up is bending the beat lengths early for a speed-up, late for a
        // slow-down.
        let r = a.ramp;
        let way = if r.to_bpm > r.from_bpm { -1.0 } else { 1.0 };
        let bend = (b.bend + way * up * BEND_PER_POINT).clamp(-MAX_BEND, MAX_BEND);
        let area = Curve::bent_area(bend).clamp(0.003, 0.997);
        let (p0, p1) = (60_000.0 / r.from_bpm, 60_000.0 / r.to_bpm);
        let end_ms = r.start_ms + f64::from(r.beats) * (p0 + (p1 - p0) * area);
        // Past the end of the track, or the next tempo change, it holds where it last fitted.
        if end_ms < total - 1.0 && end_ms <= a.room.1 {
            self.reramp(true, |r| {
                r.feel = Feel::Gradual;
                r.end_ms = end_ms;
            });
        }
    }

    /// The release: the whole drag becomes one step of undo. A cancelled drag puts it back.
    pub fn end_bend(&mut self, cancelled: bool) {
        let Some(total) = self.deck_duration_ms().filter(|t| *t > 0.0) else { return };
        let Some(g) = self.grid_edit.as_mut() else { return };
        let Some(a) = g.ramp.as_mut() else { return };
        let Some(b) = a.bend.take() else { return };
        if cancelled {
            (a.ramp, a.pins) = (b.ramp, b.pins);
            if let Some((times, _)) = a.build(a.start, &a.ramp, &a.pins, total) {
                a.times = times;
            }
            g.beats = b.before;
        } else if b.before != g.beats {
            g.undo_from(b.before);
        }
        self.show_ramp_fields();
        self.show_grid_edit();
    }

    /// A press on the zoomed waveform while shaping a ramp, with `at_ms` under the pointer and
    /// `reach_ms` as near as it has to be to an end to take hold of it. Near an end, it takes that
    /// end; outside the ramp, it brings the nearer end to the pointer and takes hold of it; inside,
    /// it takes the beat nearest the pointer, which is pinned once it moves. So a click, or a
    /// double-click to unpin, pins nothing.
    pub fn ramp_press(&mut self, at_ms: f64, reach_ms: f64) {
        let Some(g) = self.grid_edit.as_mut() else { return };
        let Some(a) = g.ramp.as_mut() else { return };
        let (to_start, to_end) = ((at_ms - a.ramp.start_ms).abs(), (at_ms - a.ramp.end_ms).abs());
        let near = to_start.min(to_end) <= reach_ms;
        let (grab, from_ms) = if (near && to_start <= to_end) || (!near && at_ms < a.ramp.start_ms) {
            (Grab::Start, if near { a.ramp.start_ms } else { at_ms })
        } else if near || at_ms > a.ramp.end_ms {
            (Grab::End, if near { a.ramp.end_ms } else { at_ms })
        } else {
            let k = crate::grid_edit::nearest(&g.beats, at_ms).filter(|k| *k > a.start && *k < a.times.len());
            a.last_press = k;
            match k.filter(|_| a.shown) {
                Some(k) => (Grab::Beat(k), a.times[k]),
                None => return,
            }
        };
        a.held = Some(Held { grab, from_ms, press_ms: at_ms, start: a.start, ramp: a.ramp, pins: a.pins.clone(), before: g.beats.clone() });
        // A press outside the ramp brings the end there straight away.
        if !near && matches!(grab, Grab::Start | Grab::End) {
            self.ramp_drag(at_ms);
        }
    }

    /// One step of a drag on the zoomed waveform while shaping a ramp.
    pub fn ramp_drag(&mut self, at_ms: f64) {
        let Some(total) = self.deck_duration_ms().filter(|t| *t > 0.0) else { return };
        let Some(g) = self.grid_edit.as_mut() else { return };
        let Some(a) = g.ramp.as_mut() else { return };
        let Some(grab) = a.held.as_ref().map(|h| h.grab) else { return };
        let t = a.held.as_ref().map_or(at_ms, |h| h.from_ms + at_ms - h.press_ms);
        let p0 = 60_000.0 / a.ramp.from_bpm;
        let (start, ramp, pins) = match grab {
            // The start sits on the beat nearest the pointer, at least two beats before the end,
            // and not before the tempo change before. An instant change moves whole.
            Grab::Start => {
                let (lo, hi) = a.room;
                let instant = a.ramp.instant();
                let fits = |b: &Beat| {
                    let at = f64::from(b.time_ms);
                    at >= lo && if instant { at <= hi } else { at + 2.0 * p0 <= a.ramp.end_ms }
                };
                let Some(earliest) = a.head.iter().position(fits) else { return };
                let Some(latest) = a.head.iter().rposition(fits) else { return };
                let Some(k) = crate::grid_edit::nearest(&a.head, t) else { return };
                let k = k.clamp(earliest, latest.max(earliest));
                let start_ms = f64::from(a.head[k].time_ms);
                let end_ms = if instant { start_ms } else { a.ramp.end_ms };
                (k, Ramp { start_ms, end_ms, ..a.ramp }, Vec::new())
            }
            // The end goes where the pointer is, at least two beats after the start and not past
            // the tempo change after. Brought back within a beat of the start, it lands on it: an
            // instant change.
            Grab::End if t < a.ramp.start_ms + p0 => (a.start, Ramp { feel: a.ramp.feel, ..Ramp::instant_at(a.ramp.start_ms, a.ramp.from_bpm, a.ramp.to_bpm) }, Vec::new()),
            Grab::End => (a.start, Ramp { end_ms: t.clamp(a.ramp.start_ms + 2.0 * p0, a.room.1), ..a.ramp }, Vec::new()),
            // A beat is pinned where the pointer has taken it, and the grid around it warped to
            // match. It can't pass the pinned beats either side, or come closer to them than
            // rekordbox's fastest tempo allows; at the limit it holds.
            Grab::Beat(k) => {
                let mut t = t;
                let anchors = a.anchors();
                if let Some(&prev) = anchors.iter().rev().find(|&&j| j < k) {
                    t = t.max(a.times[prev] + (k - prev) as f64 * MIN_GAP_MS);
                }
                if let Some(&next) = anchors.iter().find(|&&j| j > k) {
                    t = t.min(a.times[next] - (next - k) as f64 * MIN_GAP_MS);
                }
                let mut pins: Vec<(usize, f64)> = a.pins.iter().copied().filter(|p| p.0 != k).collect();
                pins.push((k, t));
                pins.sort_by_key(|p| p.0);
                (a.start, a.ramp, pins)
            }
        };
        if !a.shown {
            // Nothing's in the grid yet: only the ends move.
            a.start = start;
            a.ramp = ramp;
            self.show_grid_edit();
            return;
        }
        // Moved ends count the bars again, with the feel kept. An instant change has none.
        let ramp = if matches!(grab, Grab::Beat(_)) || ramp.instant() {
            ramp
        } else {
            match Ramp::count_beats(ramp.start_ms, ramp.end_ms, ramp.from_bpm, ramp.to_bpm) {
                Some(beats) => Ramp { beats, ..ramp },
                None => return,
            }
        };
        // A step that doesn't work out (a tempo out of range, say) holds the last one that did.
        if let Some((times, beats)) = a.build(start, &ramp, &pins, total) {
            a.start = start;
            a.ramp = ramp;
            a.pins = pins;
            a.times = times;
            g.beats = beats;
            self.show_ramp_fields();
            self.show_grid_edit();
        }
    }

    /// The release of a drag while shaping a ramp: the whole drag is one step of undo. A
    /// cancelled drag puts everything back.
    pub fn ramp_release(&mut self, cancelled: bool) {
        let Some(total) = self.deck_duration_ms().filter(|t| *t > 0.0) else { return };
        let Some(g) = self.grid_edit.as_mut() else { return };
        let Some(a) = g.ramp.as_mut() else { return };
        let Some(h) = a.held.take() else { return };
        if cancelled {
            (a.start, a.ramp, a.pins) = (h.start, h.ramp, h.pins);
            if let Some((times, _)) = a.build(a.start, &a.ramp, &a.pins, total) {
                a.times = times;
            }
            g.beats = h.before;
        } else if h.before != g.beats {
            // An end put somewhere new is the place a reset goes back to from now on.
            if matches!(h.grab, Grab::Start | Grab::End) {
                a.unbent_end = None;
            }
            g.undo_from(h.before);
        }
        self.show_ramp_fields();
        self.show_grid_edit();
    }

    /// A double-click on the curve's handle: the curve goes back to Gradual, and an end that
    /// Alt-dragging moved goes back to where it was. One step of undo.
    pub fn reset_bend(&mut self) {
        let Some(a) = self.ramp_edit().filter(|a| a.shown) else { return };
        let end = a.unbent_end.unwrap_or(a.ramp.end_ms);
        self.reramp(false, |r| {
            r.feel = Feel::Gradual;
            r.end_ms = end;
        });
        if let Some(a) = self.grid_edit.as_mut().and_then(|g| g.ramp.as_mut())
            && a.ramp.end_ms == end
        {
            a.unbent_end = None;
        }
    }

    /// Typing the bars or the tempo makes where the end is now the place a reset goes back to.
    fn settle_end(&mut self) {
        if let Some(a) = self.grid_edit.as_mut().and_then(|g| g.ramp.as_mut()) {
            a.unbent_end = None;
        }
    }

    /// A double-click while shaping a ramp: unpins the beat it was on, if it's pinned, and the
    /// grid around it goes back to following the curve. One step of undo.
    pub fn unpin(&mut self) {
        if let Some(k) = self.ramp_edit().and_then(|a| a.last_press) {
            self.unpin_beat(k);
        }
    }

    /// Unpins beat `k`, if it's pinned: the beats around it go back to following the curve. One
    /// step of undo.
    pub fn unpin_beat(&mut self, k: usize) {
        let Some(total) = self.deck_duration_ms().filter(|t| *t > 0.0) else { return };
        let Some(g) = self.grid_edit.as_mut() else { return };
        let Some(a) = g.ramp.as_mut() else { return };
        if !a.pins.iter().any(|p| p.0 == k) {
            return;
        }
        let pins: Vec<(usize, f64)> = a.pins.iter().copied().filter(|p| p.0 != k).collect();
        if let Some((times, beats)) = a.build(a.start, &a.ramp, &pins, total) {
            a.pins = pins;
            a.times = times;
            g.replace(beats);
        }
        self.show_grid_edit();
    }

    /// Fills the ramp's boxes from the ramp.
    pub(crate) fn show_ramp_fields(&mut self) {
        let Some(a) = self.ramp_edit() else { return };
        let bars = if a.ramp.beats == 0 && !a.shown { String::new() } else { bars_text(a.ramp.beats) };
        let to = format!("{:.2}", a.ramp.to_bpm);
        self.ramp_to = TextEdit::new(to);
        self.ramp_bars = TextEdit::new(bars);
    }

    /// Says in the status bar why a ramp change didn't happen, as the other grid edits do.
    fn ramp_refused(&mut self, why: &str) {
        let why = why.to_owned();
        self.jobs.spawn(JobKind::Edit, "Changing the tempo change", move |_| Outcome::Failed(why));
    }
}
