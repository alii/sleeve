//! The window.

pub mod deck;
pub mod filters;
pub mod icons;
pub mod inspector;
pub mod menus;
pub mod sidebar;
pub mod statusbar;
pub mod titlebar;
pub mod tracks;
pub mod update;

use rune::{Cx, El, KeyEvent, Spring, Tx, col, el, row, text};
use rune_kit::shell::{self, PaneMotion, SidebarToggle, SplitView, Toolbar};
use rune_kit::{Button, ButtonVariant, ControlSize, Dialog, IconButton, IconButtonVariant, Kit, TextInput};

use crate::app::{App, DropTarget, LibState};
use crate::library::Browse;

pub fn kit(dark: bool) -> &'static Kit {
    rune_theme_opencode::kit(dark)
}

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    let k = kit(cx.system_dark());
    // The waveforms are pixels drawn in the theme's colours, so a new theme draws them again.
    if crate::waveform::follow_theme(cx.system_dark()) {
        s.redraw_waves();
    }
    let c = &k.c;

    let main: El = match &s.lib {
        LibState::Loading => centred(text("Opening your library…").font(k.ty.ui).color(c.text_muted)),
        LibState::Failed(e) => centred(
            col()
                .gap(6.0)
                .items_center()
                .max_w(420.0)
                .child(rune::icon(&icons::WARNING, 20.0).color(c.danger.icon))
                .child(text(e.clone()).font(k.ty.ui).color(c.text_base).wrap().align(rune::Align::Center)),
        ),
        LibState::Ready(_) if s.rows.is_empty() => empty(s, k),
        LibState::Ready(_) => tracks::build(s, cx.layout_size("tracks-area").map_or(1200.0, |z| z[0]), k),
    };
    // The panel's width, read off the footer, which always spans it.
    let panel_w = cx.layout_size("statusbar").map_or(740.0, |z| z[0]);
    let mut main_col = col().key("main").w_full().h_full().min_h(0.0);
    if let Some(d) = deck::build(s, panel_w, k) {
        main_col = main_col.child(d);
    }
    // The status bar is the panel's footer, inside its frame, lined up with the list.
    let mut main = main_col.child(main);
    if let Some(a) = statusbar::activity(s, k) {
        main = main.child(a);
    }
    let main = main.child(statusbar::build(s, panel_w, k));

    let search = TextInput::new(&s.search)
        .placeholder("Search")
        .leading_icon(&icons::SEARCH)
        .width(Some(if panel_w < 760.0 { 160.0 } else { 240.0 }))
        .on_edit(|s: &mut App| &mut s.search)
        .on_change(|s: &mut App, _| s.refresh_rows());
    // The clear button shows only with something to clear. Rune draws it; what it does is ours.
    let search = if s.search.text.is_empty() {
        search
    } else {
        search.clear_button().on_action(|s: &mut App, _| {
            s.search = rune::TextEdit::new("");
            s.refresh_rows();
        })
    };
    let search = search.build(k).key("search");
    let inspector_toggle = IconButton::new(&shell::INSPECTOR)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(s.inspector.is_open())
        .label(if s.inspector.is_open() { "Hide details" } else { "Show details" })
        .on_press(|s: &mut App| s.inspector.toggle())
        .build(k)
        .key("inspector-toggle");
    let n = s.analyse_rows().len();
    let fresh = s.selected_rows().is_empty();
    let analyse = Button::new(s.analyse_label())
    .icon(&icons::WAVE)
    .variant(ButtonVariant::Ghost)
    .rolling("analyse-label")
    .disabled(n == 0)
    .help(if fresh { "Analyse the tracks here that have never been analysed" } else { "Analyse the selected tracks again, replacing their grids" })
    .on_press(|s: &mut App| s.analyse())
    .build(k)
    .key("analyse");
    // Overlays (the Filter menu, the filter popover, the confirm dialog) go on one layer above
    // everything. Made here, as the toolbar's Filter button puts its menu on it.
    let mut layer = Vec::new();
    // The frame inset round the main pane. It also pushes the title bar row down.
    let frame = 8.0;
    let chrome = titlebar::build(s, cx, k, frame, &mut layer);
    let filter = filters::button(s, k, &mut layer);
    // On Windows the caption buttons sit over the window's top-right corner. With the inspector
    // open they're over its top, which starts lower; with it closed, over the toolbar's end,
    // which makes room.
    let caption = chrome.as_ref().map(|ch| ch.caption);
    let clear_caption = caption.filter(|_| !s.inspector.is_open()).map(|(w, _)| el().key("caption-clear").w((w - frame).max(0.0)).a11y_hidden());
    let mut inspector = inspector::build(s, cx.layout_size("insp-wave-strip"), k);
    if let Some((_, h)) = caption {
        inspector = col().key("inspector-below-caption").w_full().h_full().pt(h).child(inspector);
    }
    let heading = text(title(s)).key("toolbar-title").font(k.ty.body_medium).color(c.text_base).truncate().shrink();
    let toolbar = Toolbar::new()
        .leading(heading)
        .trailing(row().gap(8.0).center_y().child(analyse).child(filter).child(search).child(inspector_toggle).children(clear_caption));

    let split = SplitView::new(main)
        .sidebar(&s.sidebar, |s: &mut App| &mut s.sidebar, sidebar::build(s, k))
        .inspector(&s.inspector, |s: &mut App| &mut s.inspector, inspector)
        .toolbar(toolbar)
        // On Windows the Sleeve menu sits where the traffic lights do on a Mac, so the row clears it.
        .titlebar_inset(chrome.as_ref().map_or_else(|| cx.titlebar_inset(), |ch| ch.inset))
        .frame_inset(frame)
        .toolbar_in_panel()
        .sidebar_padding(8.0, 8.0)
        .sidebar_toggle(SidebarToggle::new().size(ControlSize::Large).past_lights(16.0).edge(18.0).gap(4.0).in_header())
        .pane_motion(PaneMotion::new().spring(Spring::new(0.3, 0.0)).scale_with_width(480.0, 0.15, 0.3).fade_tail(0.15))
        .reduced_motion(cx.is_reduced_motion())
        .build(k);

    // On Windows, the Sleeve menu goes over the split view's title bar row.
    let mut root = col().w_full().h_full().fill(c.page).child(split).children(chrome.map(|ch| ch.menu));
    if s.drop_over == Some(DropTarget::Library) {
        root = root.child(drop_veil(s, k));
    }
    menus::build(s, cx.window_layout("nav-new"), k, &mut layer);
    confirm(s, k, &mut layer);
    filters::popover(s, k, &mut layer);
    // The update card lives at the foot of the sidebar. With the sidebar closed it floats in
    // the window's bottom left corner instead, above the status bar, so it's never hidden.
    if !s.sidebar.is_open()
        && let Some(card) = update::card(s, k)
    {
        layer.push(col().key("update-float").pin(Some(16.0), None, None, Some(52.0)).w(280.0).radius(k.radius.md).shadow(8.0, 24.0, rune::Color::BLACK.alpha(0.3)).child(card));
    }
    root = root.children(layer);
    // Songs and folders dragged in from Finder land anywhere in the window.
    root.on_drop_files(|s: &mut App, files, _| s.import(files.to_vec(), s.drop_playlist()))
        .accepts_files(crate::import::accepts)
        .on_drop_hover(|s: &mut App, over, _| s.drop_over = over.then_some(DropTarget::Library))
}

/// The question before anything is deleted: what goes, what stays, and a red button.
fn confirm(s: &App, k: &Kit, layer: &mut Vec<El>) {
    use crate::edits::{Confirm, songs};
    let Some(ask) = &s.confirm else { return };
    let (title, about, verb) = match ask {
        Confirm::DeletePlaylist(id) => {
            let node = s.tree.iter().find(|n| n.id == *id);
            let name = node.map_or("this playlist", |n| n.name.as_str());
            let folder = node.is_some_and(|n| n.kind == crate::library::PlaylistKind::Folder);
            if folder {
                (format!("Delete the folder “{name}”?"), "Everything inside it goes too. The songs stay in your library.".to_owned(), "Delete folder")
            } else {
                (format!("Delete “{name}”?"), "The songs in it stay in your library.".to_owned(), "Delete playlist")
            }
        }
        Confirm::RemoveTracks(ids) => (
            format!("Remove {} from your library?", songs(ids.len())),
            "They come out of every playlist too, and lose their cues and analysis. The files stay where they are on your computer.".to_owned(),
            "Remove",
        ),
    };
    let ask = ask.clone();
    let buttons = row()
        .w_full()
        .gap(8.0)
        .px(16.0)
        .pb(16.0)
        .justify_end()
        .child(Button::new("Cancel").variant(ButtonVariant::Outline).on_press(|s: &mut App| s.confirm = None).build(k).key("confirm-cancel"))
        .child(
            Button::new(verb)
                .variant(ButtonVariant::Danger)
                .on_press(move |s: &mut App| match &ask {
                    crate::edits::Confirm::DeletePlaylist(id) => s.delete_playlist(*id),
                    crate::edits::Confirm::RemoveTracks(ids) => s.remove_from_library(ids.clone()),
                })
                .build(k)
                .key("confirm-ok"),
        );
    // The explanation goes in the body rather than `.description()`, which rune_kit draws on one
    // line, so a long one runs off the dialog. Pulled up to sit just under the title.
    let about = col().w_full().px(16.0).mt(-12.0).pb(16.0).child(text(about).font(k.ty.ui.lh(20.0)).color(k.c.text_muted).w_full().wrap());
    Dialog::new("confirm", title).width(420.0).fit().open(true).on_close(|s: &mut App| s.confirm = None).child(about).child(buttons).build(k, layer);
}

/// What a drag over the window shows: the window tinted, and where the songs will go.
fn drop_veil(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let target = s.drop_playlist().and_then(|id| s.tree.iter().find(|n| n.id == id)).map(|n| n.name.clone());
    let headline = match &target {
        Some(name) => format!("Add to {name}"),
        None => "Add to your library".to_owned(),
    };
    let card = col()
        .gap(6.0)
        .items_center()
        .px(28.0)
        .py(22.0)
        .radius(14.0)
        .fill(c.layer_01)
        .border(1.0, c.border_base)
        .shadow(12.0, 32.0, rune::Color::BLACK.alpha(0.3))
        .child(rune::icon(&icons::DROP, 28.0).color(c.icon_accent))
        .child(text(headline).font(k.ty.body_medium).color(c.text_base))
        .child(text("Drop songs or folders. They'll be analysed straight away.").font(k.ty.small).color(c.text_muted));
    el().key("drop-veil")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .px(10.0)
        .py(10.0)
        .inert()
        .transition(rune::Transition::opacity())
        .fill(c.page.alpha(0.45))
        .child(col().w_full().h_full().radius(16.0).fill(c.icon_accent.alpha(0.1)).border(2.0, c.icon_accent.alpha(0.7)).center().child(card))
}

fn title(s: &App) -> String {
    match (s.browse, s.loaded()) {
        (Browse::Collection, _) => "Collection".into(),
        (Browse::Playlist(id), Some(_)) => s.tree.iter().find(|n| n.id == id).map_or_else(String::new, |n| n.name.clone()),
        _ => String::new(),
    }
}

/// What an empty list says instead: why it's empty, and how to fill it.
fn empty(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let searching = !s.search.text.trim().is_empty();
    // Filters hide every row: with the list gone, its headers are too, so the way out is here.
    if !s.base_rows.is_empty() && !s.filters.is_empty() {
        let clear = Button::new("Clear filters").variant(ButtonVariant::Neutral).on_press(|s: &mut App| s.clear_filters()).build(k).key("empty-clear-filters");
        return centred(
            col()
                .key("empty")
                .gap(8.0)
                .items_center()
                .max_w(360.0)
                .child(rune::icon(&icons::FILTER, 22.0).color(c.icon_faint))
                .child(text("No tracks match the filters").font(k.ty.ui_medium).color(c.text_base))
                .child(clear),
        );
    }
    let (head, tip) = match s.browse {
        _ if searching => ("Nothing matches your search".to_owned(), "Try fewer words, or clear the search.".to_owned()),
        Browse::Playlist(_) if s.drop_playlist().is_some() => ("This playlist is empty".to_owned(), "Drag songs here from Finder, or right-click songs in your collection and choose Add to.".to_owned()),
        Browse::Playlist(_) => ("Nothing in here yet".to_owned(), String::new()),
        Browse::Collection => ("Your library is empty".to_owned(), "Drag songs or folders here from Finder to add them.".to_owned()),
    };
    centred(
        col()
            .key("empty")
            .gap(6.0)
            .items_center()
            .max_w(360.0)
            .child(rune::icon(&icons::DROP, 22.0).color(c.icon_faint))
            .child(text(head).font(k.ty.ui_medium).color(c.text_base))
            .child(text(tip).font(k.ty.small).color(c.text_muted).wrap().align(rune::Align::Center)),
    )
}

fn centred(e: El) -> El {
    col().w_full().grow(1.0).min_h(0.0).center().child(e)
}

pub fn on_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    // Space plays and pauses unless a text field has it. Fields take it before this runs.
    let plain = !ev.mods.cmd && !ev.mods.alt && !ev.mods.ctrl;
    if ev.key == rune::Key::Text(" ".into()) && plain {
        if s.player.track.is_some() {
            s.toggle_deck(tx);
        } else {
            s.play_focused(None, tx);
        }
        return true;
    }
    // While fixing the grid, undo takes back the last drag.
    if s.grid_editing() && ev.is_command('z') && !ev.mods.shift && s.undo_grid() {
        return true;
    }
    // Ctrl (⌘ on a Mac) with = and - zooms the waveform too, as it zooms most apps and as Ctrl
    // and the scroll wheel already do here; with 0 it resets.
    if s.player.track.is_some()
        && (ev.mods.cmd || ev.mods.ctrl)
        && !ev.mods.alt
        && let rune::Key::Text(t) = &ev.key
    {
        match t.as_str() {
            "=" | "+" => s.zoom(-1),
            "-" => s.zoom(1),
            "0" => s.reset_zoom(),
            _ => {}
        }
        if matches!(t.as_str(), "=" | "+" | "-" | "0") {
            return true;
        }
    }
    // On the deck: ← and → step a beat, with ⇧ a bar; - and = zoom the waveform, as in
    // rekordbox, and ⇧= resets it.
    if s.player.track.is_some() && !ev.mods.cmd && !ev.mods.alt && !ev.mods.ctrl {
        let bar = if ev.mods.shift { 4 } else { 1 };
        match &ev.key {
            // On a US keyboard ⇧= types "+", so a shifted "+" is the reset too. A "+" key of its
            // own (a keypad's, or another layout's) zooms in.
            rune::Key::Text(t) if ev.mods.shift && (t == "=" || t == "+") => {
                s.reset_zoom();
                return true;
            }
            rune::Key::Left => {
                s.nudge_beats(-bar);
                return true;
            }
            rune::Key::Right => {
                s.nudge_beats(bar);
                return true;
            }
            rune::Key::Text(t) if t == "-" => {
                s.zoom(1);
                return true;
            }
            rune::Key::Text(t) if t == "=" || t == "+" => {
                s.zoom(-1);
                return true;
            }
            // C saves a memory cue at the playhead, as in rekordbox.
            rune::Key::Text(t) if t.eq_ignore_ascii_case("c") => {
                s.memory_cue();
                return true;
            }
            _ => {}
        }
    }
    // 1 to 8 are the hot cue pads, as on a player.
    if let rune::Key::Text(t) = &ev.key
        && plain
        && let Some(d @ '1'..='8') = t.chars().next().filter(|_| t.len() == 1)
        && s.player.track.is_some()
    {
        s.hot_cue(d as u8 - b'1', tx);
        return true;
    }
    // Delete takes the selected songs out of the playlist on screen, or, in the collection,
    // asks before removing them from the library.
    if matches!(ev.key, rune::Key::Backspace | rune::Key::Delete) && !ev.mods.alt && !ev.mods.ctrl && s.renaming.is_none() {
        let ids = s.selected_ids();
        if !ids.is_empty() {
            match s.browse {
                Browse::Playlist(_) if !ev.mods.cmd => s.remove_from_playlist(),
                Browse::Playlist(_) | Browse::Collection => s.confirm = Some(crate::edits::Confirm::RemoveTracks(ids)),
            }
            return true;
        }
    }
    if shell::is_sidebar_toggle(ev) {
        s.sidebar.toggle();
        return true;
    }
    if ev.is_command('i') && ev.mods.alt {
        s.inspector.toggle();
        return true;
    }
    false
}
