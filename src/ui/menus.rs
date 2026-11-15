//! Menus. Every item names what it acts on, so it's never a guess.

use rune::{PointerPress, PopupItem, PopupMenu, Tx};

use crate::app::App;
use crate::edits::{Confirm, songs};
use crate::library::{Browse, PlaylistKind};

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

/// Every plain playlist, indented under its folders, to add songs to.
fn playlist_picker(s: &App, except: Option<u64>) -> PopupMenu {
    let mut menu = PopupMenu::new().item(PopupItem::new("to-new", "New playlist").symbol("plus"));
    let lists: Vec<_> = s.tree.iter().filter(|n| n.kind == PlaylistKind::Playlist && Some(n.id) != except).collect();
    if !lists.is_empty() {
        menu = menu.separator();
    }
    for n in lists {
        let indent = "    ".repeat(usize::from(n.depth));
        menu = menu.item(PopupItem::new(format!("to:{}", n.id), format!("{indent}{}", n.name)));
    }
    menu
}

pub fn track_menu(s: &App, at: PointerPress, tx: &mut Tx) {
    let ids = s.selected_ids();
    let n = ids.len();
    let these = songs(n);
    let analyse = s.analyse_rows().len();
    let current = match s.browse {
        Browse::Playlist(id) => s.tree.iter().find(|p| p.id == id).map(|p| (id, p.name.clone(), p.kind)),
        Browse::Collection => None,
    };
    let reveal = if cfg!(windows) { "Show in Explorer" } else { "Show in Finder" };
    let mut menu = PopupMenu::new()
        .item(PopupItem::new("analyse", if analyse == 1 { "Analyse 1 track".to_owned() } else { format!("Analyse {analyse} tracks") }).symbol("waveform").enabled(analyse > 0))
        .item(PopupItem::new("finder", reveal).symbol("folder").enabled(n > 0))
        .separator()
        .submenu(format!("Add {these} to"), playlist_picker(s, current.as_ref().map(|c| c.0)));
    if let Some((_, name, PlaylistKind::Playlist)) = &current {
        menu = menu.item(PopupItem::new("out", format!("Take {these} out of “{name}”")).symbol("minus.circle").enabled(n > 0));
    }
    menu = menu.item(PopupItem::new("remove", format!("Remove {these} from library…")).symbol("trash").enabled(n > 0));
    if let Some((_, name, _)) = &current {
        menu = export_items(s, menu.separator(), &format!("“{name}”"));
    }
    let playlist = current.map(|c| c.0);
    tx.popup_menu(menu, at, move |s: &mut App, picked, _| match picked {
        Some("analyse") => s.analyse(),
        Some("finder") => s.show_in_finder(),
        Some("out") => s.remove_from_playlist(),
        Some("remove") => s.confirm = Some(Confirm::RemoveTracks(ids)),
        Some("to-new") => s.new_playlist(None, false, ids),
        Some(p) if p.starts_with("to:") => {
            if let Ok(id) = p["to:".len()..].parse() {
                s.add_to_playlist(id, ids);
            }
        }
        _ => {
            if let Some(id) = playlist {
                pick_export(s, picked, vec![id]);
            }
        }
    });
}

pub fn playlist_menu(s: &App, id: u64, at: PointerPress, tx: &mut Tx) {
    let Some(node) = s.tree.iter().find(|n| n.id == id) else { return };
    let folder = node.kind == PlaylistKind::Folder;
    let mut menu = PopupMenu::new();
    if folder {
        menu = menu
            .item(PopupItem::new("new-in", format!("New playlist in “{}”", node.name)).symbol("music.note.list"))
            .item(PopupItem::new("folder-in", format!("New folder in “{}”", node.name)).symbol("folder.badge.plus"))
            .separator();
    }
    menu = menu
        .item(PopupItem::new("rename", "Rename").symbol("pencil"))
        .item(PopupItem::new("delete", if folder { "Delete folder…" } else { "Delete playlist…" }).symbol("trash"))
        .separator();
    let menu = export_items(s, menu, &format!("“{}”", node.name));
    tx.popup_menu(menu, at, move |s: &mut App, picked, _| match picked {
        Some("new-in") => s.new_playlist(Some(id), false, Vec::new()),
        Some("folder-in") => s.new_playlist(Some(id), true, Vec::new()),
        Some("rename") => s.start_rename(id),
        Some("delete") => s.confirm = Some(Confirm::DeletePlaylist(id)),
        _ => {
            pick_export(s, picked, vec![id]);
        }
    });
}

/// The "+" by the Playlists heading.
pub fn new_menu(tx: &mut Tx) {
    let menu = PopupMenu::new()
        .item(PopupItem::new("playlist", "New playlist").symbol("music.note.list"))
        .item(PopupItem::new("folder", "New folder").symbol("folder.badge.plus"));
    tx.popup_menu(menu, "nav-new", |s: &mut App, picked, _| match picked {
        Some("playlist") => s.new_playlist(None, false, Vec::new()),
        Some("folder") => s.new_playlist(None, true, Vec::new()),
        _ => {}
    });
}

pub fn stick_menu(s: &App, mount: std::path::PathBuf, at: PointerPress, tx: &mut Tx) {
    let Some(stick) = s.sticks.iter().find(|d| d.mount == mount) else { return };
    let busy = s.exporting_to(&mount);
    let menu = PopupMenu::new().item(PopupItem::new("eject", if busy { format!("Eject {} (exporting…)", stick.name) } else { format!("Eject {}", stick.name) }).symbol("eject").enabled(!busy));
    tx.popup_menu(menu, at, move |s: &mut App, picked, _| {
        if picked == Some("eject") {
            s.eject(mount);
        }
    });
}

pub fn cue_menu(s: &App, slot: u8, at: PointerPress, tx: &mut Tx) {
    let letter = char::from(b'A' + slot);
    let set = s.deck_hot_cues()[slot as usize].is_some();
    let menu = PopupMenu::new()
        .item(PopupItem::new("jump", if set { format!("Jump to hot cue {letter}") } else { format!("Set hot cue {letter} here") }))
        .item(PopupItem::new("delete", format!("Delete hot cue {letter}")).enabled(set));
    tx.popup_menu(menu, at, move |s: &mut App, picked, tx| match picked {
        Some("jump") => s.hot_cue(slot, tx),
        Some("delete") => s.delete_hot_cue(slot),
        _ => {}
    });
}
