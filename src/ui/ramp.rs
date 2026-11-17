//! The tempo change editor's parts of the deck: the TEMPO button, the settings that take the grid
//! tools' place while a ramp is shaped, and on the zoomed waveform the tempo changes in the grid,
//! the one being shaped with its ends to drag, its tempo line and the handle that shapes it.

use rune::{Color, Drag, El, Phase, Tx, col, el, oklch, row, text};
use rune_kit::{Kit, TextInput};

use crate::app::App;
use crate::ramp_edit::RampEdit;
use crate::tempo_ramp::Feel;
use crate::ui::deck::{PAD, screen};

/// The colour of tempo changes, wherever they show: a mint green, apart from the accent blue of
/// the grid tools and cues.
pub fn tempo_colour() -> Color {
    oklch(0.84, 0.13, 165.0)
}

/// A small outlined button, as the grid tools are.
fn button(key: &'static str, label: &'static str, help: &'static str, k: &Kit) -> El {
    let b = row()
        .key(key)
        .h(PAD.1)
        .px(7.0)
        .radius(5.0)
        .center()
        .role("button")
        .focusable(true)
        .cursor(rune::Cursor::Pointer)
        .border(1.0, k.c.border_base)
        .label(help)
        .help(help)
        .child(text(label).font(k.ty.small_medium).color(k.c.text_base));
    k.interactive(b)
}

/// SET, beside a box whose text hasn't been put in: filled, as it's the next thing to do.
fn set_button(key: &'static str, help: &'static str, k: &Kit) -> El {
    let b = row()
        .key(key)
        .h(PAD.1)
        .px(7.0)
        .radius(5.0)
        .center()
        .role("button")
        .focusable(true)
        .cursor(rune::Cursor::Pointer)
        .fill(tempo_colour())
        .label(help)
        .help(help)
        .child(text("SET").font(k.ty.small_medium).color(oklch(0.2, 0.02, 0.0)));
    k.interactive(b)
}

/// The TEMPO button, among the grid tools.
pub fn tempo_button(k: &Kit) -> El {
    button("deck-grid-tempo", "TEMPO", "Add a tempo change at the playhead, or reshape the one it's in", k).on_press(|s: &mut App| s.start_ramp())
}

/// What stands in for the grid tools while a tempo change is shaped.
pub fn tools(s: &App, a: &RampEdit, k: &Kit) -> El {
    let c = &k.c;
    let faint = |t: &str| text(t.to_owned()).font(k.ty.small).color(c.text_faint);
    let field = |edit: &rune::TextEdit, label: &str, placeholder: &str, width: f32| {
        TextInput::new(edit).label(label).placeholder(placeholder).text_style(k.ty.small_medium.features("'tnum' 1")).height(PAD.1).width(Some(width))
    };
    // A box whose text hasn't been put in yet gets a SET button beside it, so it's plain that
    // Return (or SET) is what puts it in. Before the new tempo is in, that one always shows.
    let typed = |t: &rune::TextEdit| t.text.trim().replace(',', ".").parse::<f64>().ok();
    let to_unset = !a.shown || typed(&s.ramp_to).is_none_or(|v| (v - a.ramp.to_bpm).abs() >= 0.005);
    let bars_unset = a.shown && typed(&s.ramp_bars).is_none_or(|v| (v * 4.0).round() as u32 != a.ramp.beats);
    let set_to = set_button("deck-ramp-to-set", "Put in the tempo it changes to · Return", k).on_press(|s: &mut App| s.submit_ramp_to_bpm());
    let set_bars = set_button("deck-ramp-bars-set", "Put in the bars · Return", k).on_press(|s: &mut App| s.submit_ramp_bars());
    let to = field(&s.ramp_to, "The tempo it changes to, in BPM", "BPM", 54.0)
        .on_edit(|s: &mut App| &mut s.ramp_to)
        .on_submit(|s: &mut App, _| s.submit_ramp_to_bpm())
        .build(k)
        .key("deck-ramp-to")
        .help("Type the tempo it changes to and press Return");
    let bars = field(&s.ramp_bars, "How many bars the change takes", "–", 38.0)
        .on_edit(|s: &mut App| &mut s.ramp_bars)
        .on_submit(|s: &mut App, _| s.submit_ramp_bars())
        .build(k)
        .key("deck-ramp-bars")
        .help("Type how many bars the change takes and press Return. Its start and end stay where they are");
    // Before its tempo is typed, nothing has changed, so there's nothing to remove.
    let remove = if a.shown {
        button("deck-ramp-remove", "Remove", "Take the tempo change out", k).on_press(|s: &mut App| s.remove_ramp())
    } else {
        button("deck-ramp-remove", "Cancel", "Stop adding the tempo change · Esc", k).on_press(|s: &mut App| s.finish_ramp())
    };
    let done = button("deck-ramp-done", "Done", "Keep the tempo change and go back to the grid tools", k).on_press(|s: &mut App| s.finish_ramp());
    row()
        .key("deck-ramp-tools")
        .gap(4.0)
        .center_y()
        .child(text(format!("{:.2} →", a.ramp.from_bpm)).font(k.ty.small.features("'tnum' 1")).color(c.text_faint))
        .child(to)
        .child(faint("BPM"))
        .children(to_unset.then_some(set_to))
        .child(el().w(4.0))
        .child(faint("over"))
        .child(bars)
        .child(faint("bars"))
        .children(bars_unset.then_some(set_bars))
        .child(el().w(4.0))
        .children((a.shown && !a.ramp.instant()).then(|| feels(a, k)))
        .child(el().w(4.0))
        .child(remove)
        .children(a.shown.then_some(done))
}

/// How sudden the change is: three choices in one group, as a segmented control, plus Custom once
/// the curve's been shaped by hand.
fn feels(a: &RampEdit, k: &Kit) -> El {
    let c = &k.c;
    let segment = |key: &'static str, name: &'static str, help: &'static str, on: bool| {
        let s = row()
            .key(key)
            .h(PAD.1 - 4.0)
            .px(7.0)
            .radius(4.0)
            .center()
            .role("radio")
            .checked(on)
            .label(name)
            .help(help)
            .child(text(name).font(k.ty.small_medium).color(if on { c.text_base } else { c.text_faint }));
        if on { s.fill(c.layer_02).border(1.0, c.border_base) } else { s }
    };
    let choices = [
        ("deck-ramp-gradual", "Gradual", "The most even change that fits", Feel::Gradual),
        ("deck-ramp-medium", "Medium", "Most of the change over about half of it", Feel::Medium),
        ("deck-ramp-sudden", "Sudden", "Most of the change over about a fifth of it", Feel::Sudden),
    ];
    let group = row().key("deck-ramp-feel").role("radiogroup").focus_group().label("How sudden the change is").h(PAD.1).padding(2.0).gap(2.0).radius(5.0).fill(c.layer_01).center_y();
    let group = choices.into_iter().fold(group, |g, (key, name, help, feel)| {
        let on = a.ramp.feel == feel;
        g.child(k.interactive(segment(key, name, help, on).focusable(true).cursor(rune::Cursor::Pointer)).on_press(move |s: &mut App| s.set_ramp_feel(feel)))
    });
    group.children(matches!(a.ramp.feel, Feel::Custom(_)).then(|| segment("deck-ramp-custom", "Custom", "Shaped by dragging the handle on the tempo line", true)))
}

/// What the hint on the zoomed waveform says while a tempo change is shaped.
pub fn hint(s: &App) -> Option<&'static str> {
    let a = s.ramp_edit()?;
    Some(if !a.shown {
        "Drag the ends to where the tempo starts and settles, then type the new tempo"
    } else if a.ramp.instant() {
        "Drag the line to move the change · type bars, or click after it, to spread it out"
    } else if s.player.playing {
        "Drag an end, or a beat to pin it · pause to shape the change"
    } else {
        "Drag an end, or a beat to pin it · drag ● to shape, Alt-drag ● to bend"
    })
}

/// The tempo across a placed ramp, as points on the zoomed waveform: one at each end and one in
/// the middle of each beat, at that beat's tempo, lower down for slower. And where its handle
/// goes: where the tempo changes fastest, kept off the ends.
struct Lane {
    points: Vec<[f32; 2]>,
    handle: [f32; 2],
}

fn lane(a: &RampEdit, x_of: &dyn Fn(f32) -> f32, h: f32) -> Option<Lane> {
    // The beats as shown, warped through any pins, so the line shows the tempo the grid has.
    let (times, curve) = (a.times.get(a.start..=a.start + a.ramp.beats as usize)?, a.curve()?);
    let (from, to) = (a.ramp.from_bpm, a.ramp.to_bpm);
    let (lo, hi) = (from.min(to) - 2.0, from.max(to) + 2.0);
    let y = |bpm: f64| h - 14.0 - ((bpm - lo) / (hi - lo)) as f32 * (h - 44.0);
    let x = |ms: f64| x_of((ms / 1000.0) as f32);
    let mut points = Vec::with_capacity(times.len() + 1);
    points.push([x(times[0]), y(from)]);
    points.extend(times.windows(2).map(|w| [x((w[0] + w[1]) / 2.0), y(60_000.0 / (w[1] - w[0]))]));
    points.push([x(a.ramp.end_ms), y(to)]);
    let n = times.len() - 1;
    let middle = ((curve.centre.clamp(0.15, 0.85) * n as f64) as usize).min(n - 1);
    Some(Lane { handle: points[1 + middle], points })
}

/// A tempo as the labels on the waveform show it.
fn tempo_label(bpm: f64, k: &Kit) -> El {
    text(format!("{bpm:.2}")).font(rune::TextStyle { size: 10.0, ..k.ty.small_medium.features("'tnum' 1") }).color(tempo_colour())
}

/// The tint over a tempo change, from `x0` to `x1`: light across it, and glowing in from each
/// edge, strong at the edge and fading fast, as if the edges gave off light. `strength` scales it,
/// lower for one not yet placed.
fn glow(key: &str, x0: f32, x1: f32, h: f32, strength: f32) -> Vec<El> {
    const STEPS: usize = 14;
    const STEP_W: f32 = 2.0;
    let colour = tempo_colour();
    let w = (x1 - x0).max(0.0);
    let mut out = vec![el().key(format!("{key}-fill")).absolute(x0, 0.0).size(w, h).fill(colour.alpha(0.07 * strength))];
    // Slices stacked on the fill, each fainter than the last, no further in than half the band.
    let steps = STEPS.min((w / 2.0 / STEP_W) as usize);
    for i in 0..steps {
        let alpha = 0.34 * strength * (-(i as f32) / 3.0).exp();
        let inset = i as f32 * STEP_W;
        out.push(el().key(format!("{key}-glow-l{i}")).absolute(x0 + inset, 0.0).size(STEP_W, h).fill(colour.alpha(alpha)));
        out.push(el().key(format!("{key}-glow-r{i}")).absolute(x1 - inset - STEP_W, 0.0).size(STEP_W, h).fill(colour.alpha(alpha)));
    }
    out
}

/// An instant change: a line in the tempo colour at `x`, glowing out both ways, fading fast.
fn line_glow(key: &str, x: f32, h: f32, strength: f32) -> Vec<El> {
    const STEPS: usize = 10;
    const STEP_W: f32 = 2.0;
    let colour = tempo_colour();
    let mut out = Vec::with_capacity(STEPS * 2 + 1);
    for i in 0..STEPS {
        let alpha = 0.34 * strength * (-(i as f32) / 3.0).exp();
        let out_by = i as f32 * STEP_W;
        out.push(el().key(format!("{key}-glow-l{i}")).absolute(x - out_by - STEP_W, 0.0).size(STEP_W, h).fill(colour.alpha(alpha)));
        out.push(el().key(format!("{key}-glow-r{i}")).absolute(x + out_by, 0.0).size(STEP_W, h).fill(colour.alpha(alpha)));
    }
    out.push(el().key(format!("{key}-line")).absolute(x - 1.0, 0.0).size(2.0, h).fill(colour.alpha(0.9 * strength)));
    out
}

/// "120.00 → 140.00", for a tempo change's start.
fn change_label(key: String, from: f64, to: f64, k: &Kit) -> El {
    row().key(key).gap(3.0).px(3.0).radius(3.0).fill(screen().alpha(0.75)).child(tempo_label(from, k)).child(text("→").font(rune::TextStyle { size: 10.0, ..k.ty.small_medium }).color(tempo_colour().alpha(0.7))).child(tempo_label(to, k))
}

/// The tempo changes on the zoomed waveform, for its moving layer: those in the grid tinted, each
/// labelled with its tempos, whether or not Grid is on; and the one being shaped, edged in the
/// accent with a grip on each end to drag, its tempo as a line across it. `w` covers the layer.
pub fn bands(s: &App, x_of: &dyn Fn(f32) -> f32, w: f32, h: f32, k: &Kit) -> Option<El> {
    let accent = tempo_colour();
    let editing = s.ramp_edit();
    let mut layer = el().key("deck-ramp").absolute(0.0, 0.0).size(w, h).inert();
    let mut any = false;
    for (i, f) in s.deck_ramps().iter().enumerate() {
        // The one being shaped is drawn as that, below.
        if editing.is_some_and(|a| f.ramp.start_ms <= a.ramp.end_ms && a.ramp.start_ms <= f.ramp.end_ms) {
            continue;
        }
        let (x0, x1) = (x_of(f.ramp.start_ms as f32 / 1000.0), x_of(f.ramp.end_ms as f32 / 1000.0));
        if x1 < -30.0 || x0 > w {
            continue;
        }
        any = true;
        let key = format!("deck-ramp-found-{i}");
        if f.ramp.instant() {
            layer = layer.children(line_glow(&key, x0, h, 1.0));
        } else {
            layer = layer
                .children(glow(&key, x0, x1, h, 1.0))
                .child(el().key(format!("{key}-start")).absolute(x0 - 0.5, 0.0).size(1.0, h).fill(accent.alpha(0.6)))
                .child(el().key(format!("{key}-end")).absolute(x1 - 0.5, 0.0).size(1.0, h).fill(accent.alpha(0.6)));
        }
        layer = layer.child(change_label(format!("{key}-label"), f.ramp.from_bpm, f.ramp.to_bpm, k).absolute(x0 + 5.0, 16.0));
    }
    let Some(a) = editing else { return any.then_some(layer) };
    let (x0, x1) = (x_of(a.ramp.start_ms as f32 / 1000.0), x_of(a.ramp.end_ms as f32 / 1000.0));
    // An end: a line in the accent, and a grip halfway down to show it can be dragged, filled
    // in while it's held.
    let end = |key: &str, x: f32, held: bool| {
        let grip = el().key(format!("{key}-grip")).absolute(x - 4.0, h / 2.0 - 14.0).size(8.0, 28.0).radius(4.0).border(1.5, accent).fill(if held { accent } else { screen() });
        [el().key(key.to_owned()).absolute(x - 1.0, 0.0).size(2.0, h).fill(accent), grip]
    };
    let strength = if a.shown { 1.0 } else { 0.5 };
    // An instant change: one line, glowing both ways, with one grip that moves it whole.
    if a.ramp.instant() {
        return Some(
            layer
                .children(line_glow("deck-ramp-band", x0, h, strength))
                .children(a.shown.then(|| change_label("deck-ramp-label".to_owned(), a.ramp.from_bpm, a.ramp.to_bpm, k).absolute(x0 + 7.0, 16.0)))
                .children(end("deck-ramp-start", x0, a.holding_start())),
        );
    }
    // Before the new tempo is in, there's no change yet, only where it will be: fainter.
    layer = layer.children(glow("deck-ramp-band", x0, x1, h, strength));
    if a.shown
        && let Some(lane) = lane(a, x_of, h)
    {
        // A dark line under the accent one keeps it readable over the waveform.
        let line = rune::Shape::new().stroke(lane.points.clone(), 4.0, screen().alpha(0.85)).stroke(lane.points, 1.75, accent);
        layer = layer
            .child(rune::canvas(line.view_box(w, h)).key("deck-ramp-line").absolute(0.0, 0.0).size(w, h))
            .child(tempo_label(a.ramp.from_bpm, k).key("deck-ramp-from").absolute(x0 + 7.0, 16.0))
            .child(col().key("deck-ramp-to-label").absolute(x1 - 51.0, 16.0).w(44.0).items_end().child(tempo_label(a.ramp.to_bpm, k)));
        // Pinned beats: the beat's line in the accent, and a dot at its head. At the head, not
        // the foot where the grid tools mark theirs, as the hint sits along the foot.
        for &(k, t) in &a.pins {
            let x = x_of(pin_secs(s, k, t));
            layer = layer
                .child(el().key(format!("deck-ramp-pin-line-{k}")).absolute(x - 0.75, 0.0).size(1.5, h).fill(accent.alpha(0.8)))
                .child(el().key(format!("deck-ramp-pin-{k}")).absolute(x - 4.0, 2.0).size(8.0, 8.0).radius(4.0).fill(accent));
        }
    }
    Some(layer.children(end("deck-ramp-start", x0, a.holding_start())).children(end("deck-ramp-end", x1, a.holding_end())))
}

/// The tempo changes on the overview waveform: a tint over each.
pub fn overview_tints(s: &App, total_ms: f64, view_w: f32, h: f32) -> impl Iterator<Item = El> {
    let accent = tempo_colour();
    let x = move |ms: f64| (ms / total_ms).clamp(0.0, 1.0) as f32 * view_w;
    s.deck_ramps().iter().enumerate().map(move |(i, f)| {
        let (x0, x1) = (x(f.ramp.start_ms), x(f.ramp.end_ms));
        el().key(format!("deck-wave-ramp-{i}")).absolute(x0 - 1.0, 0.0).size((x1 - x0).max(0.0) + 2.0, h).fill(accent.alpha(0.3)).inert()
    })
}

/// The handle that shapes a placed ramp: a ring on its tempo line, dragged up for a more sudden
/// change and down for a more gradual one, or Alt-dragged to bend the line, moving the end.
/// Double-clicked, it resets: Gradual, with the end back where it was before any bending. Only while paused: playing, the waveform's moving layer
/// slides on its own, and the handle, which can't sit in that layer and still be grabbed, would
/// drift off the line.
/// Where to draw pinned beat `k`, in seconds: at the beat as the grid has it, rounded to the
/// millisecond as every beat is, so it sits exactly on the beat's own line. The pin's exact time
/// (`t`, in ms) is up to half a millisecond off that, a pixel or so when zoomed right in.
fn pin_secs(s: &App, k: usize, t: f64) -> f32 {
    s.deck_beats().get(k).map_or(t as f32, |b| b.time_ms as f32) / 1000.0
}

/// A spot over each pinned beat's dot, on the strip so it can be pressed: hovered, a red ⊗
/// takes the dot's place, and pressing it unpins the beat. Only the dot, not the beat's whole
/// line, so dragging the beat itself works as it did. Not while playing, as the handle isn't,
/// since the beats slide under the strip then.
pub fn pin_buttons(s: &App, x_of: &dyn Fn(f32) -> f32, k: &Kit) -> Vec<El> {
    let Some(a) = s.ramp_edit().filter(|a| a.shown && !s.player.playing) else { return Vec::new() };
    let red = k.c.danger.icon;
    a.pins
        .iter()
        .map(|&(beat, t)| {
            let x = x_of(pin_secs(s, beat, t));
            // An X drawn with two strokes, not a glyph, so it's centred whatever the font.
            let cross = rune::Shape::new().stroke(vec![[4.0, 4.0], [10.0, 10.0]], 1.75, Color::WHITE).stroke(vec![[10.0, 4.0], [4.0, 10.0]], 1.75, Color::WHITE);
            let button = el()
                .key(format!("deck-ramp-unpin-{beat}-x"))
                .absolute(1.0, 0.0)
                .size(14.0, 14.0)
                .radius(7.0)
                .fill(red)
                .group_opacity(0.0, 1.0)
                .child(rune::canvas(cross.view_box(14.0, 14.0)).size(14.0, 14.0));
            el()
                .key(format!("deck-ramp-unpin-{beat}"))
                .absolute(x - 8.0, 0.0)
                .size(16.0, 16.0)
                .hover_group()
                .cursor(rune::Cursor::Pointer)
                .role("button")
                .label("Unpin this beat")
                .help("Unpin this beat: it goes back to following the curve")
                .child(button)
                .on_press(move |s: &mut App| s.unpin_beat(beat))
        })
        .collect()
}

pub fn handle(s: &App, x_of: &dyn Fn(f32) -> f32, h: f32) -> Option<El> {
    if s.player.playing {
        return None;
    }
    let a = s.ramp_edit().filter(|a| a.shown)?;
    let [x, y] = lane(a, x_of, h)?.handle;
    let ring = el()
        .key("deck-ramp-handle")
        .absolute(x - 6.0, y - 6.0)
        .size(12.0, 12.0)
        .radius(6.0)
        .fill(screen())
        .border(2.0, tempo_colour())
        .cursor(rune::Cursor::ResizeRow)
        .label("How sudden the tempo change is")
        .help("Drag up for a more sudden change, down for a more gradual one; its start and end stay. Alt-drag to bend the line up or down as in Ableton; its end moves to fit. Double-click to reset");
    Some(ring.hover(tempo_colour()).on_double_press(|s: &mut App| s.reset_bend()).on_drag(|s: &mut App, d: &mut Drag, tx: &mut Tx| match d.phase {
        Phase::Down => s.begin_bend(tx.mods().alt),
        Phase::Move => s.bend(d.dy),
        Phase::Up => {
            if !d.cancelled() {
                s.bend(d.dy);
            }
            s.end_bend(d.cancelled());
        }
    }))
}
