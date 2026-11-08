//! The track list.

use std::borrow::Cow;

use rune::{Align, Color, El, Tx, col, el, oklch, row, text};
use rune_kit::{Cell, Kit, Table, TableColumn, TableStyle};

/// The panel's one horizontal inset: the toolbar title, the deck, every cell and the footer
/// start this far in, so their text lines up.
pub const INSET: f32 = 12.0;

use crate::app::App;
use crate::library;

/// Columns in the order they're shown, with the room each needs and its priority. When the
/// list is narrow, the lowest priorities (highest numbers) go first.
fn columns() -> Vec<(TableColumn, f32, u8)> {
    vec![
        (TableColumn::new("title", "Title").flex(2.4).min(150.0).sortable(), 150.0, 0),
        (TableColumn::new("artist", "Artist").flex(1.5).min(90.0).sortable(), 90.0, 1),
        (TableColumn::new("genre", "Genre").flex(0.9).min(64.0).max(220.0).sortable(), 64.0, 5),
        (TableColumn::new("bpm", "BPM").width(64.0).min(56.0).max(90.0).align(Align::End).sortable(), 64.0, 2),
        (TableColumn::new("key", "Key").width(56.0).min(48.0).max(90.0).align(Align::Center).sortable(), 56.0, 2),
        (TableColumn::new("time", "Time").width(58.0).min(52.0).max(90.0).align(Align::End).sortable(), 58.0, 3),
        (TableColumn::new("rating", "Rating").width(78.0).resizable(false).sortable(), 78.0, 4),
        (TableColumn::new("added", "Added").width(104.0).min(96.0).max(140.0).sortable(), 104.0, 6),
    ]
}

pub fn build(s: &App, width: f32, k: &Kit) -> El {
    let kit = k.clone();
    let mut cols = columns();
    // Drop the least important column until the rest fit, cell padding included.
    let need = |cols: &[(TableColumn, f32, u8)]| cols.iter().map(|c| c.1 + 2.0 * INSET).sum::<f32>();
    while need(&cols) > width && cols.len() > 2 {
        let drop = cols.iter().enumerate().max_by_key(|(_, c)| c.2).map_or(0, |(i, _)| i);
        cols.remove(drop);
    }
    let base = Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |s: &App| s.rows.len(), |s: &App, i| Cow::Owned(s.track_id(i)))
        .name("Tracks")
        .style(TableStyle { cell_pad: INSET, ..TableStyle::from_kit(k) });
    let table = cols
        .into_iter()
        .fold(base, |t, c| t.column(c.0))
        .cell(move |s: &App, i, col| cell(s, i, col, &kit))
        .on_sort(|s: &mut App, col, dir, _| s.set_sort(col, dir))
        .on_select(|s: &mut App, _, _| s.sync_artwork())
        .build(s, k)
        .key("tracks")
        .grow(1.0)
        .min_h(0.0);
    col().key("tracks-area").w_full().grow(1.0).min_h(0.0).child(table).on_secondary_press(|_: &mut App, press, tx: &mut Tx| {
        // Right-clicking a row that isn't selected selects it first, as Finder does, so the menu
        // always acts on what's highlighted.
        tx.element_at(press.x, press.y, move |s: &mut App, picked, tx| {
            if let Some(id) = picked.as_ref().and_then(|p| row_of_path(p.path()))
                && !s.table.is_selected(&id)
            {
                s.table.clear_selection();
                s.table.select(id);
                s.sync_artwork();
            }
            crate::ui::menus::track_menu(s, press, tx);
        });
    })
}

/// The track id in an element path inside the table: the segment after `rows`.
fn row_of_path(path: &str) -> Option<String> {
    let mut parts = path.split('/');
    parts.by_ref().find(|p| *p == "rows")?;
    parts.next().filter(|id| id.chars().all(|c| c.is_ascii_digit()) && !id.is_empty()).map(str::to_owned)
}

fn cell(s: &App, i: usize, col: &str, k: &Kit) -> Cell {
    let (Some(l), Some(&row_ix)) = (s.loaded(), s.rows.get(i)) else { return Cell::text("") };
    let f = library::fields(&l.index, row_ix);
    match col {
        "title" => {
            // The colour follows the title, so titles line up with their header.
            let mut cell = row().w_full().gap(6.0).center_y().min_w(0.0).child(text(f.title.to_owned()).font(k.ty.ui).color(k.c.text_base).truncate().shrink());
            if let Some(ink) = track_color(f.color) {
                cell = cell.child(el().size(6.0, 6.0).radius(3.0).fill(ink));
            }
            if crate::icloud::is_evicted(std::path::Path::new(l.index.folder_path.get(row_ix as usize))) {
                cell = cell.child(rune::icon(&crate::ui::icons::CLOUD, 13.0).color(k.c.icon_faint).help("In iCloud. It downloads when you play it."));
            }
            Cell::el(cell)
        }
        "artist" => Cell::text(f.artist),
        "album" => Cell::text(f.album).muted(),
        "genre" => Cell::text(f.genre).muted(),
        "bpm" => Cell::el(num(library::bpm(f.bpm_x100), k.c.text_base, k)),
        "key" => match camelot(f.key, k.dark) {
            Some((label, ink)) => Cell::el(key_pill(label, ink, k)),
            None => Cell::text(f.key).muted(),
        },
        "time" => Cell::el(num(library::duration(f.length_sec), k.c.text_muted, k)),
        "rating" => Cell::el(stars(f.rating, k)),
        "added" => Cell::el(num(f.date_added.to_owned(), k.c.text_muted, k)),
        _ => Cell::text(""),
    }
}

/// Numbers in the UI face with tabular figures, so columns of them line up without the weight
/// of a monospaced face.
fn num(s: String, ink: Color, k: &Kit) -> El {
    text(s).font(k.ty.ui.features("'tnum' 1")).color(ink)
}

fn stars(n: u8, k: &Kit) -> El {
    (0..5).fold(row().gap(1.0).center_y(), |r, i| {
        let on = i < n;
        let icon = if on { k.glyph_star_filled() } else { k.glyph_star() };
        r.child(rune::icon(icon, 11.0).color(if on { k.c.text_base } else { k.c.text_faint.alpha(0.35) }))
    })
}

/// A key in Camelot notation, tinted with its place on the wheel so keys that mix well share a
/// colour family.
fn key_pill(label: String, ink: Color, k: &Kit) -> El {
    row()
        .px(6.0)
        .h(18.0)
        .center()
        .radius(4.0)
        .fill(ink.alpha(if k.dark { 0.16 } else { 0.14 }))
        .child(text(label).font(rune::TextStyle { size: 11.0, ..k.ty.mono }).color(ink))
}

/// Converts rekordbox's key names ("Am", "F#m", "Db", "8A") to Camelot and a wheel colour.
pub fn camelot(key: &str, dark: bool) -> Option<(String, Color)> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let (num, letter) = if let Some(n) = key.strip_suffix(['A', 'B']).and_then(|n| n.parse::<u8>().ok()) {
        (n, key.chars().last()?)
    } else {
        let minor = key.ends_with('m');
        let root = key.trim_end_matches('m');
        let pc = match root {
            "C" => 0,
            "C#" | "Db" => 1,
            "D" => 2,
            "D#" | "Eb" => 3,
            "E" => 4,
            "F" => 5,
            "F#" | "Gb" => 6,
            "G" => 7,
            "G#" | "Ab" => 8,
            "A" => 9,
            "A#" | "Bb" => 10,
            "B" => 11,
            _ => return None,
        };
        // Major C is 8B; each fifth up adds one. Minor A is 8A.
        let fifths = |pc: i32| (pc * 7).rem_euclid(12);
        if minor {
            (((fifths(pc) - fifths(9) + 7).rem_euclid(12) + 1) as u8, 'A')
        } else {
            (((fifths(pc) + 7).rem_euclid(12) + 1) as u8, 'B')
        }
    };
    if !(1..=12).contains(&num) {
        return None;
    }
    let hue = (f32::from(num) - 1.0) * 30.0 + 140.0;
    let ink = match (dark, letter == 'A') {
        (true, true) => oklch(0.74, 0.13, hue),
        (true, false) => oklch(0.80, 0.11, hue),
        (false, true) => oklch(0.52, 0.14, hue),
        (false, false) => oklch(0.58, 0.12, hue),
    };
    Some((format!("{num}{letter}"), ink))
}

/// rekordbox's eight track colours.
pub fn track_color(c: u8) -> Option<Color> {
    Some(match c {
        1 => oklch(0.72, 0.17, 350.0),
        2 => oklch(0.64, 0.21, 25.0),
        3 => oklch(0.74, 0.16, 55.0),
        4 => oklch(0.88, 0.16, 100.0),
        5 => oklch(0.76, 0.17, 145.0),
        6 => oklch(0.80, 0.11, 200.0),
        7 => oklch(0.62, 0.17, 260.0),
        8 => oklch(0.62, 0.19, 300.0),
        _ => return None,
    })
}

trait StarGlyphs {
    fn glyph_star(&self) -> &'static rune::Icon;
    fn glyph_star_filled(&self) -> &'static rune::Icon;
}

impl StarGlyphs for Kit {
    fn glyph_star(&self) -> &'static rune::Icon {
        &rune_kit::glyphs::STAR
    }
    fn glyph_star_filled(&self) -> &'static rune::Icon {
        &rune_kit::glyphs::STAR_FILLED
    }
}

#[cfg(test)]
mod tests {
    use super::camelot;

    #[test]
    fn keys_map_to_camelot() {
        assert_eq!(camelot("Am", true).unwrap().0, "8A");
        assert_eq!(camelot("C", true).unwrap().0, "8B");
        assert_eq!(camelot("Em", true).unwrap().0, "9A");
        assert_eq!(camelot("G", true).unwrap().0, "9B");
        assert_eq!(camelot("F#m", true).unwrap().0, "11A");
        assert_eq!(camelot("Db", true).unwrap().0, "3B");
        assert_eq!(camelot("5A", true).unwrap().0, "5A");
        assert!(camelot("", true).is_none());
    }
}
