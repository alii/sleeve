//! The deck along the top of the track list: the track that's playing, wherever the selection
//! is. Its title, a full-width waveform with the playhead and hot cues, the hot cue pads, the
//! time, and a way back to the track in the list.

use rune::{Color, Drag, El, Phase, Transition, Tx, col, el, oklch, row, spacer, text};
use rune_kit::{IconButton, IconButtonVariant, Kit};

use crate::app::App;
use crate::library;
use crate::ui::icons;

const WAVE_H: f32 = 64.0;
const PAD: (f32, f32) = (26.0, 22.0);

/// rekordbox's "colourful" hot cue defaults, near enough: A red, B sky, C green, D amber, E
/// violet, F orange, G cyan, H pink.
pub fn cue_colour(slot: usize) -> Color {
    let hue = [25.0, 235.0, 145.0, 85.0, 300.0, 55.0, 200.0, 355.0][slot % 8];
    oklch(0.72, 0.16, hue)
}

/// The panel's dark "screen", the same in both themes, as a player's display is.
pub fn screen() -> Color {
    oklch(0.17, 0.005, 260.0)
}

/// The deck's moving parts for the player's current anchor. Each glides from the anchor at
/// playing speed, stepped by the renderer every frame, so a playing track needs no view
/// rebuilds and nothing it shows can lag a frame behind another. Made the first time the view
/// asks for one, and dropped when the anchor changes.
#[derive(Default)]
pub struct Glides {
    anchor: (u64, u64),
    /// The zoomed waveform's slide, by zoom and width.
    slides: std::collections::HashMap<(u64, u32), rune::Tween<[f32; 2]>>,
    /// A playhead and its "played" veil across a strip, by strip and size.
    heads: std::collections::HashMap<(&'static str, u32, u32), Head>,
}

/// A strip's playhead and the veil over what's been played, as rects for `live_rect`.
#[derive(Clone)]
pub struct Head {
    pub line: rune::Tween<[f32; 4]>,
    pub played: rune::Tween<[f32; 4]>,
}

/// How far the anchor glides, in seconds: to the end of its run or of the track.
fn glide_secs(s: &App, total_ms: f64) -> f64 {
    let a = s.player.anchor;
    if !a.moving || total_ms <= 0.0 {
        return 0.0;
    }
    crate::player::GLIDE_SECS.min(((1.0 - a.pos) * total_ms / 1000.0).max(0.0))
}

fn with_glides<T>(s: &App, total_ms: f64, f: impl FnOnce(&mut Glides) -> T) -> T {
    let mut cell = s.glides.borrow_mut();
    let g = cell.get_or_insert_with(Glides::default);
    let anchor = (s.player.anchor.seq, total_ms.to_bits());
    if g.anchor != anchor {
        *g = Glides { anchor, ..Glides::default() };
    }
    f(g)
}

/// The zoomed waveform's slide: left at `pps` points a second while the track plays.
fn slide(s: &App, total_ms: f64, view_w: f32) -> rune::Tween<[f32; 2]> {
    let secs = glide_secs(s, total_ms);
    let pps = view_w / s.zoom_secs as f32;
    with_glides(s, total_ms, |g| {
        g.slides
            .entry((s.zoom_secs.to_bits(), view_w.to_bits()))
            .or_insert_with(|| rune::Tween::linear([0.0, 0.0], [-pps * secs as f32, 0.0], std::time::Duration::from_secs_f64(secs)))
            .clone()
    })
}

/// The playhead, `line` points wide, and its veil across a strip `w` by `h`.
pub fn head(s: &App, strip: &'static str, total_ms: f64, w: f32, h: f32, line: f32) -> Head {
    let secs = glide_secs(s, total_ms);
    let a = s.player.anchor;
    with_glides(s, total_ms, |g| {
        g.heads
            .entry((strip, w.to_bits(), h.to_bits()))
            .or_insert_with(|| {
                let x0 = a.pos as f32 * w;
                let x1 = if total_ms > 0.0 { (a.pos + secs * 1000.0 / total_ms).min(1.0) as f32 * w } else { x0 };
                let d = std::time::Duration::from_secs_f64(secs);
                Head { line: rune::Tween::linear([x0 - line / 2.0, 0.0, line, h], [x1 - line / 2.0, 0.0, line, h], d), played: rune::Tween::linear([0.0, 0.0, x0, h], [0.0, 0.0, x1, h], d) }
            })
            .clone()
    })
}

/// Below this panel width the pads move to their own row under the title.
const ONE_ROW_W: f32 = 860.0;

pub fn build(s: &App, width: f32, k: &Kit) -> Option<El> {
    let (l, track) = (s.loaded()?, s.deck_row()?);
    let c = &k.c;
    let f = library::fields(&l.index, track);
    let total_ms = s.deck_duration_ms().unwrap_or(0.0);
    // The readouts show the playhead now; the waveforms glide from the anchor on their own.
    let pos = s.player.now();
    let cues = s.deck_hot_cues();

    let art = match s.deck_wave.as_ref().and_then(|w| w.art.as_ref()) {
        Some(img) => el().size(44.0, 44.0).radius(6.0).fill(c.layer_02).pixels(img.pixels()),
        None => el().size(44.0, 44.0).radius(6.0).fill(c.layer_02).center().child(rune::icon(&icons::COLLECTION, 18.0).color(c.icon_faint)),
    };
    let names = col()
        .min_w(0.0)
        .shrink()
        .gap(2.0)
        .child(text(f.title.to_owned()).key("deck-title").font(k.ty.ui_medium).color(c.text_base).truncate())
        .child(text(f.artist.to_owned()).key("deck-artist").font(k.ty.small).color(c.text_muted).truncate());

    let pads = (0..8).fold(row().key("deck-pads").gap(4.0), |r, slot| r.child(pad(slot, cues[slot], total_ms, k)));
    let has_grid = !s.deck_beats().is_empty();
    let q = row()
        .key("deck-quantize")
        .size(PAD.0, PAD.1)
        .radius(5.0)
        .center()
        .role("switch")
        .focusable(true)
        .cursor(rune::Cursor::Pointer)
        .label(if s.quantize { "Quantise on" } else { "Quantise off" })
        .help(match (s.quantize, has_grid) {
            (_, false) => "Quantise needs a beat grid. Analyse this track to get one.",
            (true, true) => "Quantise is on: new hot cues and clicks on the waveform snap to the nearest beat",
            (false, true) => "Quantise is off: cues go exactly where the playhead is",
        });
    let q = if s.quantize && has_grid {
        q.fill(c.icon_accent).child(text("Q").font(k.ty.small_medium).color(oklch(0.2, 0.02, 0.0)))
    } else {
        q.border(1.0, c.border_base).child(text("Q").font(k.ty.small_medium).color(c.text_faint))
    };
    let q = k.interactive(q).on_press(|s: &mut App| s.quantize ^= true);
    let grid_at = crate::deck::grid_pos(s.deck_beats(), pos * total_ms);
    let counter = col()
        .key("deck-grid-pos")
        .w(64.0)
        .items_end()
        .gap(1.0)
        .child(text(grid_at.map_or_else(|| "–".to_owned(), |g| format!("{}.{}", g.bar, g.beat))).font(k.ty.body_medium.features("'tnum' 1")).color(c.text_base))
        .child(text(grid_at.map_or_else(|| "no grid".to_owned(), |g| format!("{:.2} BPM", g.bpm))).font(k.ty.small.features("'tnum' 1")).color(c.text_faint));

    let elapsed = (pos * total_ms / 1000.0).max(0.0) as u32;
    let left = ((1.0 - pos) * total_ms / 1000.0).max(0.0) as u32;
    let clock = col()
        .items_end()
        .gap(1.0)
        .child(text(library::duration(elapsed)).key("deck-elapsed").font(k.ty.body_medium.features("'tnum' 1")).color(c.text_base))
        .child(text(format!("−{}", library::duration(left))).key("deck-left").font(k.ty.small.features("'tnum' 1")).color(c.text_faint));

    let clock = if s.player.fetching {
        row().gap(6.0).center_y().child(rune_kit::Spinner::new().size(12.0).build(k)).child(text("Downloading from iCloud…").key("deck-fetching").font(k.ty.small).color(c.text_muted))
    } else if let Some(e) = &s.player.error {
        text(e.clone()).key("deck-error").font(k.ty.small).color(c.danger.fg).truncate().max_w(260.0)
    } else {
        clock
    };
    let play = IconButton::new(if s.player.playing { &icons::PAUSE } else { &icons::PLAY })
        .variant(IconButtonVariant::Contrast)
        .label(if s.player.playing { "Pause" } else { "Play" })
        .shortcut("Space")
        .on_press_tx(|s: &mut App, tx| s.toggle_deck(tx))
        .build(k)
        .key("deck-play");
    let locate = IconButton::new(&icons::LOCATE)
        .variant(IconButtonVariant::GhostMuted)
        .label("Show in list")
        .help("Show in list")
        .on_press_tx(|s: &mut App, tx| s.locate_deck(tx))
        .build(k)
        .key("deck-locate");
    let close = IconButton::new(&icons::CLOSE)
        .variant(IconButtonVariant::GhostMuted)
        .label("Stop and close the deck")
        .help("Stop")
        .on_press(|s: &mut App| s.close_deck())
        .build(k)
        .key("deck-close");

    let controls = row().key("deck-controls").gap(12.0).center_y().child(pads).child(q);
    let readouts = row().gap(12.0).center_y().child(counter).child(clock).child(play).child(row().gap(0.0).child(locate).child(close));
    let lead = row().gap(12.0).center_y().min_w(0.0).shrink().child(art.key("deck-art")).child(names);
    let top = if width >= ONE_ROW_W {
        col().w_full().child(
            row().w_full().gap(12.0).center_y().child(lead).child(spacer()).child(controls).child(el().w(1.0).h(28.0).fill(c.border_muted)).child(readouts),
        )
    } else {
        col().w_full().gap(10.0).child(row().w_full().gap(12.0).center_y().child(lead).child(spacer()).child(readouts)).child(controls)
    };

    let strip_w = width - 2.0 * crate::ui::tracks::INSET;
    let wave = wave(s, &cues, total_ms, strip_w, k);
    let zoomed = zoomed(s, &cues, total_ms, strip_w, k);

    Some(
        col()
            .key("deck")
            .w_full()
            .px(crate::ui::tracks::INSET)
            .py(12.0)
            .gap(10.0)
            .border_sides([0.0, 0.0, 1.0, 0.0], c.border_muted)
            .transition(Transition::offset(0.0, -8.0))
            .child(top)
            .child(zoomed)
            .child(wave),
    )
}

fn pad(slot: usize, cue: Option<rbl_index::Cue>, total_ms: f64, k: &Kit) -> El {
    let c = &k.c;
    let letter = char::from(b'A' + slot as u8).to_string();
    let ink = cue_colour(slot);
    let slot8 = slot as u8;
    let base = row().key(format!("deck-pad-{letter}")).size(PAD.0, PAD.1).radius(5.0).center().cursor(rune::Cursor::Pointer).focusable(true).role("button");
    let at = |ms: u32| library::duration(ms / 1000);
    let pad = match cue {
        Some(cue) => base
            .fill(ink)
            .label(format!("Hot cue {letter} at {}", at(cue.position_ms)))
            .help(format!("Jump to hot cue {letter} · {} · press {}", at(cue.position_ms), slot + 1))
            .child(text(letter).font(k.ty.small_medium).color(oklch(0.2, 0.02, 0.0))),
        None => base
            .border(1.0, c.border_base)
            .label(format!("Set hot cue {letter}"))
            .help(format!("Set hot cue {letter} at the playhead · press {}", slot + 1))
            .child(text(letter).font(k.ty.small_medium).color(c.text_faint)),
    };
    let _ = total_ms;
    k.interactive(pad)
        .on_press_tx(move |s: &mut App, tx| s.hot_cue(slot8, tx))
        .on_secondary_press(move |s: &mut App, press, tx: &mut Tx| crate::ui::menus::cue_menu(s, slot8, press, tx))
}

fn wave(s: &App, cues: &[Option<rbl_index::Cue>; 8], total_ms: f64, view_w: f32, k: &Kit) -> El {
    let Head { line: head, played } = head(s, "deck-wave", total_ms, view_w, WAVE_H, 2.0);
    let mut strip = el().key("deck-wave").instant_changes().w_full().h(WAVE_H).radius(k.radius.md).fill(screen()).clip_rounded(k.radius.md).cursor(rune::Cursor::Pointer);
    if let Some(w) = &s.deck_wave {
        strip = strip.child(el().w_full().h_full().pixels(&w.pixels));
    }
    if total_ms > 0.0 {
        // Built once per track: the grid doesn't move.
        let beats = s.deck_beats();
        let id = s.player.track.clone().unwrap_or_default();
        strip = strip.child(rune::memo("deck-grid", (id, beats.len(), total_ms.to_bits()), || grid(beats, total_ms).w_full().h_full().absolute(0.0, 0.0).inert()));
    }
    strip = strip.child(el().key("deck-played").absolute(0.0, 0.0).h_full().w(0.0).fill(screen().alpha(0.55)).live_rect(&played).inert());
    // Hot cue markers: a hairline and a lettered flag in the cue's colour.
    if total_ms > 0.0 {
        for (slot, cue) in cues.iter().enumerate() {
            let Some(cue) = cue else { continue };
            let x = (f64::from(cue.position_ms) / total_ms).clamp(0.0, 1.0) as f32;
            let ink = cue_colour(slot);
            let letter = char::from(b'A' + slot as u8).to_string();
            let flag = row().size(12.0, 12.0).radius(3.0).fill(ink).center().child(text(letter).font(rune::TextStyle { size: 9.0, ..k.ty.small_medium }).color(oklch(0.2, 0.02, 0.0)));
            strip = strip.child(
                el().key(format!("deck-cue-{slot}"))
                    .absolute(0.0, 0.0)
                    .h_full()
                    .w_frac(x)
                    .inert()
                    .child(el().pin(None, Some(0.0), Some(-0.5), Some(0.0)).w(1.0).fill(ink.alpha(0.85)))
                    .child(el().pin(None, Some(3.0), Some(-6.0), None).child(flag)),
            );
        }
    }
    strip = strip.child(el().key("deck-head").absolute(0.0, 0.0).size(2.0, WAVE_H).fill(Color::WHITE).live_rect(&head).inert());
    strip.on_drag(|s: &mut App, d: &mut Drag, tx: &mut Tx| {
        if d.w <= 0.0 {
            return;
        }
        let at = s.snap_frac(f64::from((d.x / d.w).clamp(0.0, 1.0)));
        match (d.phase, s.player.track.clone()) {
            (Phase::Down, Some(id)) if !s.player.playing => s.play_track(&id, Some(at), tx),
            _ => s.player.seek(at),
        }
    })
}

/// The beat grid over the whole track: a faint line at every bar and a brighter one every four
/// bars, where phrases usually turn. Drawn as one shape, so a long track costs one element.
fn grid(beats: &[rbl_anlz::Beat], total_ms: f64) -> El {
    const W: f32 = 2000.0;
    const H: f32 = 64.0;
    let mut shape = rune::Shape::new();
    let mut bar = 0;
    for b in beats.iter().filter(|b| b.beat_number == 1) {
        bar += 1;
        let x = (f64::from(b.time_ms) / total_ms) as f32 * W;
        let phrase = bar % 4 == 1;
        let ink = Color::WHITE.alpha(if phrase { 0.32 } else { 0.12 });
        shape = shape.line([x, 0.0], [x, H], if phrase { 1.4 } else { 1.0 }, rune::LineCap::Butt, ink);
    }
    rune::canvas(shape.view_box(W, H))
}

const ZOOM_H: f32 = 96.0;

/// The zoomed waveform: a few seconds either side of the playhead, which stays in the middle
/// while the track moves under it. Every beat has a line, every bar its number, so a cue can
/// be put exactly on a beat. Dragging moves the track; with quantise on, it lands on a beat.
fn zoomed(s: &App, cues: &[Option<rbl_index::Cue>; 8], total_ms: f64, view_w: f32, k: &Kit) -> El {
    let c = &k.c;
    let mut strip = el().key("deck-zoom").instant_changes().w_full().h(ZOOM_H).radius(k.radius.md).fill(screen()).clip_rounded(k.radius.md).cursor(rune::Cursor::Grab);
    let pps = view_w / s.zoom_secs as f32;
    // Everything that moves sits on one layer, laid out around the anchor and slid left by the
    // renderer as the track plays. It covers the view plus the whole glide.
    let pos_s = (s.player.anchor.pos * total_ms / 1000.0) as f32;
    let ahead = glide_secs(s, total_ms) as f32;
    let centre = view_w / 2.0;
    let x_of = |secs: f32| centre + (secs - pos_s) * pps;
    let reach = view_w + ahead * pps;
    let mut world = el().key("deck-zoom-world").absolute(0.0, 0.0).size(view_w, ZOOM_H).live_offset(&slide(s, total_ms, view_w)).inert();
    let detail = s.deck_wave.as_ref().filter(|w| s.player.is_on(&w.id)).and_then(|w| w.detail.as_ref());
    if let Some(d) = detail {
        let per = crate::waveform::DETAIL_PER_SEC as f32;
        for (i, tile) in d.tiles.iter().enumerate() {
            let first = i * crate::waveform::TILE;
            let n = crate::waveform::TILE.min(d.cols - first);
            let left = x_of(first as f32 / per);
            let w = n as f32 / per * pps;
            if left + w < 0.0 || left > reach {
                continue;
            }
            world = world.child(el().key(format!("deck-zoom-tile-{i}")).absolute(left, 0.0).w(w).h_full().pixels(tile));
        }
    } else {
        strip = strip.child(
            row().w_full().h_full().center().inert().child(text(if total_ms > 0.0 { "Analyse this track to zoom in on it" } else { "" }).font(k.ty.small).color(Color::WHITE.alpha(0.5))),
        );
    }
    // The grid: a line per beat, brighter on the bar, with bar numbers along the top.
    let half = s.zoom_secs as f32 / 2.0 + 0.5;
    let beats = s.deck_beats();
    let from = beats.partition_point(|b| (b.time_ms as f32) / 1000.0 < pos_s - half);
    let mut bar = beats[..from].iter().filter(|b| b.beat_number == 1).count();
    let every = if pps * 2.0 < 40.0 { 4 } else { 1 };
    for (i, b) in beats[from..].iter().enumerate() {
        let t = b.time_ms as f32 / 1000.0;
        if t > pos_s + half + ahead {
            break;
        }
        let x = x_of(t);
        let key = format!("deck-zoom-beat-{}", from + i);
        if b.beat_number == 1 {
            bar += 1;
            world = world.child(el().key(key).absolute(x - 0.75, 0.0).size(1.5, ZOOM_H).fill(Color::WHITE.alpha(0.55)));
            if (bar - 1) % every == 0 {
                world = world.child(text(bar.to_string()).key(format!("deck-zoom-bar-{bar}")).font(rune::TextStyle { size: 10.0, ..k.ty.small_medium.features("'tnum' 1") }).color(Color::WHITE.alpha(0.7)).absolute(x + 4.0, 3.0));
            }
        } else if pps * 0.25 > 6.0 {
            // Beats only once they're far enough apart to read.
            world = world.child(el().key(key).absolute(x - 0.5, 0.0).size(1.0, ZOOM_H).fill(Color::WHITE.alpha(0.2)));
        }
    }
    // Hot cues: a full-height line and a lettered flag.
    for (slot, cue) in cues.iter().enumerate() {
        let Some(cue) = cue else { continue };
        let x = x_of(cue.position_ms as f32 / 1000.0);
        if !(-20.0..reach + 20.0).contains(&x) {
            continue;
        }
        let ink = cue_colour(slot);
        let letter = char::from(b'A' + slot as u8).to_string();
        world = world
            .child(el().key(format!("deck-zoom-cue-{slot}")).absolute(x - 1.0, 0.0).w(2.0).h_full().fill(ink))
            .child(row().key(format!("deck-zoom-flag-{slot}")).absolute(x + 1.0, ZOOM_H - 18.0).size(14.0, 14.0).radius(3.0).fill(ink).center().child(text(letter).font(rune::TextStyle { size: 10.0, ..k.ty.small_medium }).color(oklch(0.2, 0.02, 0.0))));
    }
    strip = strip.child(world);
    // The playhead, fixed in the middle.
    strip = strip
        .child(el().key("deck-zoom-head").absolute(centre - 1.0, 0.0).w(2.0).h_full().fill(c.danger.icon).inert())
        .child(el().absolute(centre - 5.0, 0.0).size(10.0, 4.0).radius(1.0).fill(c.danger.icon).inert());
    // Zoom buttons, top right.
    let zoom_btn = |label: &'static str, step: i32, key: &'static str, help: &'static str| {
        row()
            .key(key)
            .size(22.0, 18.0)
            .radius(4.0)
            .center()
            .fill(Color::WHITE.alpha(0.08))
            .hover(Color::WHITE.alpha(0.16))
            .cursor(rune::Cursor::Pointer)
            .role("button")
            .label(help)
            .help(help)
            .child(text(label).font(k.ty.small_medium).color(Color::WHITE.alpha(0.85)))
            .on_press(move |s: &mut App| s.zoom(step))
    };
    strip = strip.child(
        row()
            .absolute(view_w - 56.0, 4.0)
            .gap(4.0)
            .child(zoom_btn("−", 1, "deck-zoom-out", "Zoom out · -"))
            .child(zoom_btn("+", -1, "deck-zoom-in", "Zoom in · =")),
    );
    strip.on_drag(move |s: &mut App, d: &mut Drag, _tx: &mut Tx| {
        let Some(total) = s.deck_duration_ms().filter(|t| *t > 0.0) else { return };
        let pps = d.w / s.zoom_secs as f32;
        match d.phase {
            Phase::Down => d.data = (s.player.now() * total / 1000.0) as f32,
            _ => {
                let secs = f64::from(d.data - d.dx / pps.max(1.0));
                let mut at = (secs * 1000.0 / total).clamp(0.0, 1.0);
                if matches!(d.phase, Phase::Up) {
                    at = s.snap_frac(at);
                }
                s.player.seek(at);
            }
        }
    })
}
