//! The window.

pub mod deck;
pub mod icons;
pub mod inspector;
pub mod menus;
pub mod sidebar;
pub mod statusbar;
pub mod tracks;

use rune::{Cx, El, KeyEvent, Spring, Tx, col, row, text};
use rune_kit::shell::{self, PaneMotion, SidebarToggle, SplitView, Toolbar};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, TextInput};

use crate::app::{App, LibState};
use crate::library::Browse;

pub fn kit(dark: bool) -> &'static Kit {
    rune_theme_opencode::kit(dark)
}

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    let k = kit(cx.system_dark());
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
        .clear_button()
        .on_edit(|s: &mut App| &mut s.search)
        .on_change(|s: &mut App, _| s.refresh_rows())
        .build(k)
        .key("search");
    let inspector_toggle = IconButton::new(&shell::INSPECTOR)
        .variant(IconButtonVariant::GhostMuted)
        .expanded(s.inspector.is_open())
        .label(if s.inspector.is_open() { "Hide details" } else { "Show details" })
        .on_press(|s: &mut App| s.inspector.toggle())
        .build(k)
        .key("inspector-toggle");
    let n = s.analyse_rows().len();
    let fresh = s.selected_rows().is_empty();
    let analyse = Button::new(match (n, fresh) {
        (0, true) => "All analysed".to_owned(),
        (0, false) => "Analyse".to_owned(),
        (1, true) => "Analyse 1 new".to_owned(),
        (n, true) => format!("Analyse {n} new"),
        (1, false) => "Analyse 1 track".to_owned(),
        (n, false) => format!("Analyse {n} tracks"),
    })
    .icon(&icons::WAVE)
    .variant(ButtonVariant::Ghost)
    .rolling("analyse-label")
    .disabled(n == 0)
    .help(if fresh { "Analyse the tracks here that have never been analysed" } else { "Analyse the selected tracks again, replacing their grids" })
    .on_press(|s: &mut App| s.analyse())
    .build(k)
    .key("analyse");
    let heading = text(title(s)).key("toolbar-title").font(k.ty.body_medium).color(c.text_base).truncate().shrink();
    let toolbar = Toolbar::new()
        .leading(heading)
        .trailing(row().gap(8.0).center_y().child(analyse).child(search).child(inspector_toggle));

    let split = SplitView::new(main)
        .sidebar(&s.sidebar, |s: &mut App| &mut s.sidebar, sidebar::build(s, k))
        .inspector(&s.inspector, |s: &mut App| &mut s.inspector, inspector::build(s, cx.layout_size("insp-wave-strip"), k))
        .toolbar(toolbar)
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(8.0)
        .toolbar_in_panel()
        .sidebar_padding(8.0, 8.0)
        .sidebar_toggle(SidebarToggle::new().size(ControlSize::Large).past_lights(16.0).edge(18.0).gap(4.0).in_header())
        .pane_motion(PaneMotion::new().spring(Spring::new(0.3, 0.0)).scale_with_width(480.0, 0.15, 0.3).fade_tail(0.15))
        .reduced_motion(cx.is_reduced_motion())
        .build(k);

    col().w_full().h_full().fill(c.page).child(split)
}

fn title(s: &App) -> String {
    match (s.browse, s.loaded()) {
        (Browse::Collection, _) => "Collection".into(),
        (Browse::Playlist(id), Some(_)) => s.tree.iter().find(|n| n.id == id).map_or_else(String::new, |n| n.name.clone()),
        _ => String::new(),
    }
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
    // On the deck: ← and → step a beat, with ⇧ a bar; - and = zoom the waveform.
    if s.player.track.is_some() && !ev.mods.cmd && !ev.mods.alt && !ev.mods.ctrl {
        let bar = if ev.mods.shift { 4 } else { 1 };
        match &ev.key {
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
