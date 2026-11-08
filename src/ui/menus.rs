//! Menus. Every item names what it acts on, so it's never a guess.

use rune::{PointerPress, PopupItem, PopupMenu, Tx};

use crate::app::App;
use crate::library::Browse;

/// Items that export `what` to each plugged-in drive. A drive already being written to is
/// shown but can't be picked, and says why.
fn export_items(s: &App, mut menu: PopupMenu, what: &str) -> PopupMenu {
    if s.sticks.is_empty() {
        return menu.item(PopupItem::new("none", "No USB drive plugged in").enabled(false));
    }
    for stick in &s.sticks {
        let busy = s.exporting_to(&stick.mount);
        let title = if busy { format!("Export {what} to {} (exporting…)", stick.name) } else { format!("Export {what} to {}", stick.name) };
        menu = menu.item(PopupItem::new(format!("export:{}", stick.mount.display()), title).symbol("externaldrive").enabled(!busy));
    }
    menu
}

fn pick_export(s: &mut App, picked: Option<&str>, playlists: Vec<u64>) -> bool {
    match picked.and_then(|p| p.strip_prefix("export:")) {
        Some(mount) => {
            s.export(playlists, mount.into());
            true
        }
        None => false,
    }
}

pub fn track_menu(s: &App, at: PointerPress, tx: &mut Tx) {
    let n = s.analyse_rows().len();
    let noun = if n == 1 { "1 track".to_owned() } else { format!("{n} tracks") };
    let mut menu = PopupMenu::new().item(PopupItem::new("analyse", format!("Analyse {noun}")).symbol("waveform").enabled(n > 0));
    let playlist = match s.browse {
        Browse::Playlist(id) => Some(id),
        Browse::Collection => None,
    };
    if let Some(id) = playlist {
        let name = s.tree.iter().find(|n| n.id == id).map_or("this playlist", |n| n.name.as_str());
        menu = export_items(s, menu.separator(), &format!("“{name}”"));
    }
    tx.popup_menu(menu, at, move |s: &mut App, picked, _| {
        if picked.as_deref() == Some("analyse") {
            s.analyse();
        } else if let Some(id) = playlist {
            pick_export(s, picked.as_deref(), vec![id]);
        }
    });
}

pub fn playlist_menu(s: &App, id: u64, at: PointerPress, tx: &mut Tx) {
    let Some(node) = s.tree.iter().find(|n| n.id == id) else { return };
    let menu = export_items(s, PopupMenu::new(), &format!("“{}”", node.name));
    tx.popup_menu(menu, at, move |s: &mut App, picked, _| {
        pick_export(s, picked.as_deref(), vec![id]);
    });
}

pub fn cue_menu(s: &App, slot: u8, at: PointerPress, tx: &mut Tx) {
    let letter = char::from(b'A' + slot);
    let set = s.deck_hot_cues()[slot as usize].is_some();
    let menu = PopupMenu::new()
        .item(PopupItem::new("jump", if set { format!("Jump to hot cue {letter}") } else { format!("Set hot cue {letter} here") }))
        .item(PopupItem::new("delete", format!("Delete hot cue {letter}")).enabled(set));
    tx.popup_menu(menu, at, move |s: &mut App, picked, tx| match picked.as_deref() {
        Some("jump") => s.hot_cue(slot, tx),
        Some("delete") => s.delete_hot_cue(slot),
        _ => {}
    });
}
