//! The panel on the right: the focused track's artwork and details.

use rune::{El, col, el, row, spacer, text};
use rune_kit::Kit;

use crate::app::App;
use crate::metadata::Field;
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

    // Title and artist edit in place on a double click, as the fields below do.
    let id = l.index.ids[i].to_string();
    let editable = |field: Field, key: &'static str, font: rune::TextStyle, ink: rune::Color| -> El {
        if s.is_editing(&id, field, true) {
            return edit_box(s, font, k).key(key);
        }
        let value = s.shown(r, field).map_or_else(|| s.stored_value(r, field), str::to_owned);
        let id = id.clone();
        text(value).key(key).font(font).color(ink).wrap().help("Double-click to edit").on_double_press(move |s: &mut App| s.start_edit(&id, field, true))
    };
    let heading = col()
        .gap(2.0)
        .w_full()
        .child(editable(Field::Title, "insp-title", k.ty.body_medium, c.text_base))
        .child(editable(Field::Artist, "insp-artist", k.ty.ui, c.text_muted));

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
        .child(edit_field(s, "Album", r, &id, Field::Album, k))
        .child(edit_field(s, "Genre", r, &id, Field::Genre, k))
        .child(edit_field(s, "Label", r, &id, Field::Label, k))
        .child(field("Added", f.date_added, k))
        .child(field("Plays", &l.index.play_count[i].to_string(), k))
        .child(field("Format", &format, k))
        .child(edit_field(s, "Comment", r, &id, Field::Comment, k));

    col()
        .key("inspector")
        // As on the deck: Space plays rather than pressing the button last clicked (a cue's ×).
        .on_key(|s: &mut App, ev: &rune::KeyEvent, tx: &mut rune::Tx| crate::ui::on_key(s, ev, tx))
        .w_full()
        .padding(16.0)
        .gap(16.0)
        .child(art.key("insp-art"))
        .child(heading)
        .child(wave(s, l.index.analysis_path.get(i).is_empty(), wave_size, k))
        .child(stats)
        .child(fields)
        .child(cues(s, r, k))
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

/// The track's hot cues (A to H) and memory cues, each under its own heading.
fn cues(s: &App, r: rbl_index::Row, k: &Kit) -> El {
    let dark = rune::oklch(0.2, 0.02, 0.0);
    let hot = s
        .hot_cues_of(r)
        .into_iter()
        .enumerate()
        .filter_map(|(slot, cue)| {
            let letter = char::from(b'A' + slot as u8);
            let flag = row()
                .size(14.0, 14.0)
                .radius(3.0)
                .fill(crate::ui::deck::cue_colour(slot))
                .center()
                .child(text(letter.to_string()).font(rune::TextStyle { size: 10.0, ..k.ty.small_medium }).color(dark));
            cue.map(|cue| CueItem { mark: flag, name: format!("hot cue {letter}"), cue })
        })
        .collect();
    let memory = s
        .memory_cues_of(r)
        .into_iter()
        .enumerate()
        .map(|(i, cue)| {
            let mark = row().gap(6.0).center_y().child(crate::ui::deck::memory_mark()).child(text((i + 1).to_string()).font(k.ty.small.features("'tnum' 1")).color(k.c.text_faint));
            CueItem { mark, name: format!("memory cue {}", i + 1), cue }
        })
        .collect();
    col()
        .w_full()
        .gap(16.0)
        .child(cue_list(s, r, "Hot cues", "insp-hot", hot, 8, "None yet. Put the track on the deck and press 1 to 8 to set one.", k))
        .child(cue_list(s, r, "Memory cues", "insp-mem", memory, crate::deck::MAX_MEMORY_CUES, "None yet. Put the track on the deck and press C to save one.", k))
}

/// One cue in a list: its mark (a lettered flag or a numbered triangle), what to call it, and
/// the cue itself.
struct CueItem {
    mark: El,
    name: String,
    cue: rbl_index::Cue,
}

/// A heading with a count out of `max`, then one row per cue. Clicking a row goes to the cue;
/// × deletes it.
#[allow(clippy::too_many_arguments)]
fn cue_list(s: &App, r: rbl_index::Row, title: &str, key: &str, items: Vec<CueItem>, max: usize, empty: &str, k: &Kit) -> El {
    let c = &k.c;
    let id = s.loaded().map(|l| l.index.ids[r as usize].to_string()).unwrap_or_default();
    let head = row()
        .w_full()
        .pb(4.0)
        .justify_between()
        .child(text(title.to_owned()).font(k.ty.small_medium).color(c.text_base))
        .child(text(format!("{}/{max}", items.len())).font(k.ty.small.features("'tnum' 1")).color(c.text_faint));
    let mut list = col().key(key.to_owned()).w_full().child(head);
    if items.is_empty() {
        return list.child(text(empty.to_owned()).font(k.ty.small).color(c.text_faint).wrap());
    }
    for (i, CueItem { mark, name, cue }) in items.into_iter().enumerate() {
        let when = library::duration(cue.position_ms / 1000);
        let id = id.clone();
        let go = row()
            .grow(1.0)
            .gap(8.0)
            .center_y()
            .cursor(rune::Cursor::Pointer)
            .role("button")
            .label(format!("Go to {name} at {when}"))
            .help("Go to this cue")
            .child(row().w(28.0).center_y().child(mark))
            .child(text(when.clone()).font(k.ty.small.features("'tnum' 1")).color(c.text_base))
            .on_press_tx(move |s: &mut App, tx| s.go_to_cue(&id, cue.position_ms, tx));
        let delete = rune_kit::IconButton::new(&icons::CLOSE)
            .variant(rune_kit::IconButtonVariant::GhostMuted)
            .label(format!("Delete {name}"))
            .help("Delete")
            .on_press(move |s: &mut App| s.delete_cue(r, cue))
            .build(k)
            .key(format!("{key}-del-{i}"));
        list = list.child(row().key(format!("{key}-{i}")).w_full().py(2.0).gap(8.0).center_y().border_sides([0.0, 0.0, 1.0, 0.0], c.border_muted).child(go).child(delete));
    }
    list
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

/// A detail you can double-click to edit. While it's being edited, its value is a text box.
fn edit_field(s: &App, label: &str, r: rbl_index::Row, id: &str, f: Field, k: &Kit) -> El {
    if !s.is_editing(id, f, true) {
        let value = s.shown(r, f).map_or_else(|| s.stored_value(r, f), str::to_owned);
        let id = id.to_owned();
        return field(label, &value, k).key(format!("insp-field-{label}")).help("Double-click to edit").on_double_press(move |s: &mut App| s.start_edit(&id, f, true));
    }
    row()
        .w_full()
        .py(3.0)
        .gap(12.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], k.c.border_muted)
        .child(text(label.to_owned()).font(k.ty.small).color(k.c.text_faint).w(64.0))
        .child(edit_box(s, k.ty.small, k).key(format!("insp-edit-{label}")))
}

/// The text box a detail turns into while it's edited, here and in the track list. Return
/// saves, Escape puts it back, and clicking anywhere else saves too.
pub fn edit_box(s: &App, font: rune::TextStyle, k: &Kit) -> El {
    // A plain `fn`, not a closure: `on_edit` needs one that hands back a borrow of the state it
    // was given, which a closure can't always promise.
    fn edit_of(s: &mut App) -> &mut rune::TextEdit {
        &mut s.edit
    }
    let c = &k.c;
    let input = rune::text_input(&s.edit, font)
        .single_line()
        .grow(1.0)
        .shrink()
        .color(c.text_base)
        .caret(c.text_base, k.selection())
        .on_edit(edit_of)
        .on_submit(|s: &mut App, _| s.commit_edit())
        .on_input_key(|s: &mut App, ev: &rune::KeyEvent, _| {
            if ev.key == rune::Key::Escape {
                s.cancel_edit();
                return true;
            }
            false
        });
    row().grow(1.0).min_w(0.0).px(8.0).py(2.0).radius(4.0).fill(c.layer_02).border(1.0, c.border_base).center_y().child(input)
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
