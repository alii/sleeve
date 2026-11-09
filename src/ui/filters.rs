//! The column filters' controls: the Filter button in the toolbar, and the popover that opens
//! under a column's header (or under the button, for a column a narrow window has hidden).

use rune::{El, PointerPress, Tx, col, row, spacer, text};
use rune_kit::{Button, ButtonVariant, Checkbox, ControlSize, Kit, Menu, MenuItem, Placement, TextInput, popover};

use crate::app::App;
use crate::filters::{COLUMNS, label};
use crate::ui::icons;

/// The most values a filter lists at once. An artist list can run to thousands; the search box
/// narrows it.
const MAX_SHOWN: usize = 300;

fn title_of(col: &str) -> &'static str {
    COLUMNS.iter().find(|c| c.0 == col).map_or("", |c| c.1)
}

/// The toolbar's Filter button and its menu: each column, ticked when it's filtered, then Clear
/// all. Drawn by Rune like the filter itself, not handed to the system as a native menu (which
/// looks out of place, and on Windows holds the window still while it's open).
pub fn button(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let n = s.filters.count();
    let title = if n == 0 { "Filter".to_owned() } else { format!("Filters · {n}") };
    let trigger = Button::new(title)
        .icon(&icons::FILTER)
        .variant(if n == 0 { ButtonVariant::Ghost } else { ButtonVariant::Neutral })
        .help("Filter the list by artist, album, genre, BPM, key or rating. Right-clicking a column's header opens its filter too.")
        .on_press(|s: &mut App| s.filter_menu ^= true)
        .build(k)
        .key("filter-button");
    let mut menu = Menu::new("filter-menu").trigger(trigger).placement(Placement::BottomEnd).min_width(200.0).open(s.filter_menu).on_close(|s: &mut App| s.filter_menu = false);
    for (id, title) in COLUMNS {
        menu = menu.item(MenuItem::new(title).checked(s.filters.is_on(id)).on_select(move |s: &mut App| {
            s.filter_menu = false;
            s.open_filter(id);
        }));
    }
    menu.separator()
        .item(MenuItem::new("Clear all filters").disabled(s.filters.is_empty()).on_select(|s: &mut App| {
            s.filter_menu = false;
            s.clear_filters();
        }))
        .build(k, layer)
}

/// A right-click on a column header: that column's filter, if it has one.
pub fn header_pressed(s: &mut App, col: &str, _press: PointerPress, _tx: &mut Tx) {
    if COLUMNS.iter().any(|c| c.0 == col) {
        s.open_filter(col);
    }
}

/// The open filter's popover, if one is open. Pressing outside it or Escape closes it.
pub fn popover(s: &App, k: &Kit, layer: &mut Vec<El>) {
    let Some(column) = s.filter_open.clone() else { return };
    let c = &k.c;
    let on = s.filters.is_on(&column);
    let clear_col = column.clone();
    let head = row()
        .w_full()
        .center_y()
        .child(text(format!("Filter by {}", title_of(&column).to_lowercase())).font(k.ty.ui_medium).color(c.text_base))
        .child(spacer())
        .child(
            Button::new("Clear")
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .disabled(!on)
                .on_press(move |s: &mut App| s.clear_filter(&clear_col))
                .build(k)
                .key("filter-clear"),
        );
    let body = if column == "bpm" { bpm_body(s, k) } else { values_body(s, &column, k) };
    let surface = popover::raised(k, col().key("filter-popover").w(280.0).max_h(460.0).min_h(0.0).padding(12.0).gap(10.0).child(head).child(body), k.radius.md)
        .modal(true, |s: &mut App| s.filter_open = None);
    // Under the column's header when it's on screen, else under the Filter button.
    let anchor = if s.shown_columns.borrow().iter().any(|c| *c == column) { column.as_str() } else { "filter-button" };
    layer.push(popover::anchored(k, surface, anchor, Placement::BottomStart, 4.0));
}

/// A search box and a checkbox for each value, with how many tracks have it.
fn values_body(s: &App, column: &str, k: &Kit) -> El {
    let c = &k.c;
    let search = TextInput::new(&s.filter_search)
        .placeholder("Search")
        .leading_icon(&icons::SEARCH)
        .width(None)
        .on_edit(|s: &mut App| &mut s.filter_search)
        .build(k)
        .key("filter-search")
        .w_full();
    let needle = s.filter_search.text.trim().to_lowercase();
    let picked = s.filters.picks.get(column);
    let matching: Vec<(String, usize)> = s.filter_values(column).into_iter().filter(|(v, _)| needle.is_empty() || label(column, v).to_lowercase().contains(&needle)).collect();
    let total = matching.len();
    let mut rows = col().w_full().gap(2.0);
    for (i, (value, n)) in matching.into_iter().take(MAX_SHOWN).enumerate() {
        let checked = picked.is_some_and(|p| p.contains(&value));
        let (col_id, v) = (column.to_owned(), value.clone());
        let check = Checkbox::new(checked).label(label(column, &value)).on_toggle(move |s: &mut App| s.toggle_filter(&col_id, &v)).build(k).shrink().min_w(0.0);
        // The whole row ticks, not only the box: it's a big target in a long list.
        let (col_id, v) = (column.to_owned(), value.clone());
        rows = rows.child(
            row()
                .key(format!("filter-value-{i}"))
                .w_full()
                .px(4.0)
                .py(2.0)
                .gap(8.0)
                .radius(4.0)
                .center_y()
                .cursor(rune::Cursor::Pointer)
                .hover(c.hover)
                .on_press(move |s: &mut App| s.toggle_filter(&col_id, &v))
                .child(check)
                .child(spacer())
                .child(text(n.to_string()).font(k.ty.small.features("'tnum' 1")).color(c.text_faint)),
        );
    }
    // Ticked values the search hides still count, so say so rather than leaving them unseen.
    let note = match (total, picked.map_or(0, std::collections::BTreeSet::len)) {
        (0, _) => Some("Nothing matches.".to_owned()),
        (t, _) if t > MAX_SHOWN => Some(format!("Showing {MAX_SHOWN} of {t}. Type to narrow it down.")),
        _ => None,
    };
    let mut body = col().w_full().min_h(0.0).shrink().gap(8.0).child(search).child(popover::fit_scroll(rows));
    if let Some(note) = note {
        body = body.child(text(note).font(k.ty.small).color(c.text_faint).wrap());
    }
    body
}

/// From and to boxes. Either can be left empty for no limit on that side.
fn bpm_body(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let from = TextInput::new(&s.bpm_from).placeholder("From").width(None).on_edit(|s: &mut App| &mut s.bpm_from).on_change(|s: &mut App, _| s.bpm_filter_changed()).build(k).key("filter-bpm-from");
    let to = TextInput::new(&s.bpm_to).placeholder("To").width(None).on_edit(|s: &mut App| &mut s.bpm_to).on_change(|s: &mut App, _| s.bpm_filter_changed()).build(k).key("filter-bpm-to");
    col()
        .w_full()
        .gap(8.0)
        .child(row().w_full().gap(8.0).center_y().child(from.grow(1.0)).child(text("–").font(k.ty.ui).color(c.text_faint)).child(to.grow(1.0)))
        .child(text("Leave one empty for no limit on that side. Tracks without a BPM are hidden while this is on.").font(k.ty.small).color(c.text_faint).wrap())
}
