//! The panel on the right: the focused track's artwork and details.

use rune::{El, col, el, row, spacer, text};
use rune_kit::Kit;

use crate::app::App;
use crate::library;
use crate::ui::{icons, tracks};

const ART: f32 = 268.0;
const WAVE_H: f32 = 56.0;
/// The waveform's pixel size: the panel's width and height at 2×.
pub const WAVE_PX: (usize, usize) = (536, 112);

/// `wave_size` is the waveform strip's size on the last frame, for its playhead.
pub fn build(s: &App, wave_size: Option<[f32; 2]>, k: &Kit) -> El {
    let c = &k.c;
    let (Some(l), Some(r)) = (s.loaded(), s.focused_row()) else {
        return col()
            .key("inspector-empty")
            .w_full()
            .h_full()
            .center()
            .gap(8.0)
            .child(rune::icon(&icons::WAVE, 20.0).color(c.icon_faint))
            .child(text("Select a track to see its details").font(k.ty.small).color(c.text_faint));
    };
    let f = library::fields(&l.index, r);
    let i = r as usize;

    let art = match &s.artwork {
        Some((row_ix, img)) if *row_ix == r => el().size(ART, ART).radius(k.radius.lg).fill(c.layer_02).pixels(img.pixels()),
        _ => el()
            .size(ART, ART)
            .radius(k.radius.lg)
            .linear_gradient(2.4, &[(0.0, c.layer_02), (1.0, c.layer_04)])
            .center()
            .child(rune::icon(&icons::COLLECTION, 40.0).color(c.icon_faint)),
    };

    let heading = col()
        .gap(2.0)
        .w_full()
        .child(text(f.title.to_owned()).key("insp-title").font(k.ty.body_medium).color(c.text_base).wrap())
        .child(text(f.artist.to_owned()).key("insp-artist").font(k.ty.ui).color(c.text_muted).wrap());

    let mut stats = row().gap(8.0).w_full();
    stats = stats.child(stat("BPM", library::bpm(f.bpm_x100), k));
    stats = stats.child(match tracks::camelot(f.key, k.dark) {
        Some((label, ink)) => stat_ink("Key", label, ink, k),
        None => stat("Key", f.key.to_owned(), k),
    });
    stats = stats.child(stat("Time", library::duration(f.length_sec), k));

    let file_type = match l.index.file_type[i] {
        1 => "MP3",
        4 => "M4A",
        5 => "FLAC",
        11 => "WAV",
        12 => "AIFF",
        _ => "",
    };
    let bitrate = l.index.bitrate[i];
    let format = match (file_type, bitrate) {
        ("", _) => String::new(),
        (t, 0) => t.to_owned(),
        (t, b) => format!("{t} · {b} kbps"),
    };
    let fields = col()
        .w_full()
        .gap(0.0)
        .child(field("Album", f.album, k))
        .child(field("Genre", f.genre, k))
        .child(field("Label", l.index.label_name(r), k))
        .child(field("Added", f.date_added, k))
        .child(field("Plays", &l.index.play_count[i].to_string(), k))
        .child(field("Format", &format, k))
        .child(field("Comment", l.index.comment.get(i), k));

    col()
        .key("inspector")
        .w_full()
        .padding(16.0)
        .gap(16.0)
        .child(art.key("insp-art"))
        .child(heading)
        .child(wave(s, l.index.analysis_path.get(i).is_empty(), wave_size, k))
        .child(stats)
        .child(fields)
        .scroll(rune::ScrollSpec { stick_bottom: false })
}

fn wave(s: &App, unanalysed: bool, size: Option<[f32; 2]>, k: &Kit) -> El {
    let c = &k.c;
    // The waveform sits on a dark panel in both themes, like a player's screen: its white
    // highs would vanish on a light one.
    let screen = rune::oklch(0.17, 0.005, 260.0);
    let frame = el().key("insp-wave").w_full().h(WAVE_H).radius(k.radius.md).fill(if unanalysed { c.layer_01 } else { screen }).border(1.0, c.border_muted).clip_rounded(k.radius.md);
    if unanalysed {
        return frame.child(
            row()
                .w_full()
                .h_full()
                .px(12.0)
                .center_y()
                .justify_between()
                .child(text("Not analysed yet").font(k.ty.small).color(c.text_faint))
                .child(
                    rune_kit::Button::new("Analyse")
                        .variant(rune_kit::ButtonVariant::Neutral)
                        .control_size(rune_kit::ControlSize::Small)
                        .on_press(|s: &mut App| s.analyse_focused())
                        .build(k)
                        .key("insp-analyse"),
                ),
        );
    }
    let Some((row_ix, generation, px)) = &s.wave else { return frame };
    let _ = (row_ix, generation);
    let on_deck = s.table.cursor().is_some_and(|id| s.player.is_on(id));
    let playing = on_deck && s.player.playing;
    // This track is downloading from iCloud and will play when it's down.
    let downloading = s.table.cursor().is_some_and(|id| s.fetch.as_ref().is_some_and(|f| f.1 == id));
    let mut strip = el().key("insp-wave-strip").grow(1.0).h_full().cursor(rune::Cursor::Pointer).child(el().w_full().h_full().pixels(px));
    if on_deck && let Some([w, h]) = size {
        // What's been played sits under a veil; the playhead is a white hairline. Both glide
        // with the deck's.
        let crate::ui::deck::Head { line: head, played } = crate::ui::deck::head(s, "insp-wave", s.deck_duration_ms().unwrap_or(0.0), w, h, 1.5);
        strip = strip
            .child(el().key("insp-played").absolute(0.0, 0.0).size(0.0, h).fill(screen.alpha(0.6)).live_rect(&played).inert())
            .child(el().key("insp-head").absolute(0.0, 0.0).size(1.5, h).fill(rune::Color::WHITE).live_rect(&head).inert());
    }
    strip = strip.on_drag(|s: &mut App, d: &mut rune::Drag, tx: &mut rune::Tx| {
        if d.w <= 0.0 {
            return;
        }
        let at = f64::from((d.x / d.w).clamp(0.0, 1.0));
        match d.phase {
            rune::Phase::Down => s.play_focused(Some(at), tx),
            _ => s.player.seek(at),
        }
    });
    // Drawn here rather than with the kit: the panel is dark in both themes, so the icon is
    // always light.
    let play = row()
        .key("insp-play")
        .role("button")
        .label(if downloading { "Downloading from iCloud" } else if playing { "Pause" } else { "Play" })
        .help(if downloading { "Downloading from iCloud. It plays when it's down." } else if playing { "Pause · Space" } else { "Play · Space" })
        .focusable(true)
        .size(28.0, 28.0)
        .radius(6.0)
        .center()
        .cursor(rune::Cursor::Pointer)
        .hover(rune::Color::WHITE.alpha(0.08))
        .child(if downloading {
            rune_kit::Spinner::new().size(12.0).color(rune::Color::WHITE.alpha(0.8)).build(k).key("insp-downloading")
        } else {
            rune::icon(if playing { &icons::PAUSE } else { &icons::PLAY }, 14.0).color(rune::Color::WHITE.alpha(0.9))
        })
        .on_press_tx(|s: &mut App, tx| s.play_focused(None, tx));
    frame.child(row().w_full().h_full().center_y().pl(4.0).pr(8.0).gap(2.0).child(play).child(strip))
}

fn stat(label: &str, value: String, k: &Kit) -> El {
    stat_ink(label, value, k.c.text_base, k)
}

fn stat_ink(label: &str, value: String, ink: rune::Color, k: &Kit) -> El {
    let value = if value.is_empty() { "–".to_owned() } else { value };
    col()
        .grow(1.0)
        .gap(2.0)
        .py(8.0)
        .px(10.0)
        .radius(k.radius.md)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_muted)
        .child(text(label.to_owned()).font(k.ty.small).color(k.c.text_faint))
        .child(text(value).font(k.ty.body_medium.features("'tnum' 1")).color(ink))
}

fn field(label: &str, value: &str, k: &Kit) -> El {
    let value = if value.is_empty() { "–" } else { value };
    row()
        .w_full()
        .py(6.0)
        .gap(12.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], k.c.border_muted)
        .child(text(label.to_owned()).font(k.ty.small).color(k.c.text_faint).w(64.0))
        .child(text(value.to_owned()).font(k.ty.small).color(k.c.text_base).wrap().grow(1.0))
        .child(spacer().w(0.0))
}
