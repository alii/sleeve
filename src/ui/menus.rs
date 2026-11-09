//! Menus. Every item names what it acts on, so it's never a guess.
//!
//! They're drawn by Rune, like the rest of the window, not handed to the system as native
//! menus: those look out of place, and on Windows hold the whole window still while open. A
//! right-click (or the "+" button) only records which menu is open and where, in
//! `App::menu`; `build` draws it from that each frame, and picking an item closes it.

use std::path::PathBuf;

use rune::{El, PointerPress, Tx, el};
use rune_kit::{Kit, Menu, MenuEntry, MenuItem};

use crate::app::App;
use crate::edits::{Confirm, songs};
use crate::library::{Browse, PlaylistKind};

/// Which menu is open.
#[derive(Clone, Debug)]
pub enum Context {
    /// The track list's, for the tracks selected when it opened.
    Tracks(Vec<String>),
    Playlist(u64),
    /// The "+" by the Playlists heading.
    New,
    Stick(PathBuf),
    HotCue(u8),
    MemoryCue(rbl_index::Row, rbl_index::Cue),
}

/// An open menu: which, where in the window it opened, and whether its submenu is showing.
#[derive(Clone, Debug)]
pub struct OpenMenu {
    pub what: Context,
    pub at: (f32, f32),
    pub sub: bool,
}

fn open(s: &mut App, what: Context, at: (f32, f32)) {
    s.menu = Some(OpenMenu { what, at, sub: false });
}

pub fn track_menu(s: &mut App, at: PointerPress) {
    let ids = s.selected_ids();
    open(s, Context::Tracks(ids), (at.x, at.y));
}

pub fn playlist_menu(s: &mut App, id: u64, at: PointerPress) {
    open(s, Context::Playlist(id), (at.x, at.y));
}

/// Opens under the "+" button; `build` finds where it is.
pub fn new_menu(s: &mut App) {
    open(s, Context::New, (0.0, 0.0));
}

pub fn stick_menu(s: &mut App, mount: PathBuf, at: PointerPress) {
    open(s, Context::Stick(mount), (at.x, at.y));
}

pub fn cue_menu(s: &mut App, slot: u8, at: PointerPress) {
    open(s, Context::HotCue(slot), (at.x, at.y));
}

pub fn memory_cue_menu(s: &mut App, row: rbl_index::Row, cue: rbl_index::Cue, at: PointerPress) {
    open(s, Context::MemoryCue(row, cue), (at.x, at.y));
}

/// An item that closes the menu, then does `f`.
fn act(label: impl Into<String>, f: impl Fn(&mut App, &mut Tx) + 'static) -> MenuItem {
    MenuItem::new(label).on_select_tx(move |s: &mut App, tx: &mut Tx| {
        s.menu = None;
        f(s, tx);
    })
}

/// Items that export `what` to each plugged-in drive. A drive already being written to is
/// shown but can't be picked, and says why.
fn export_items(s: &App, mut menu: Menu, what: &str, playlists: &[u64]) -> Menu {
    if s.sticks.is_empty() {
        return menu.item(MenuItem::new("No USB drive plugged in").disabled(true));
    }
    for stick in &s.sticks {
        let busy = s.exporting_to(&stick.mount);
        let title = if busy { format!("Export {what} to {} (exporting…)", stick.name) } else { format!("Export {what} to {}", stick.name) };
        let (lists, mount) = (playlists.to_vec(), stick.mount.clone());
        menu = menu.item(act(title, move |s, _| s.export(lists.clone(), mount.clone())).disabled(busy));
    }
    menu
}

/// Every plain playlist, indented under its folders, to add songs to.
fn playlist_picker(s: &App, ids: &[String], except: Option<u64>) -> Vec<MenuEntry> {
    let to_new = ids.to_vec();
    let mut out = vec![MenuEntry::item(act("New playlist", move |s, _| s.new_playlist(None, false, to_new.clone())))];
    let lists: Vec<_> = s.tree.iter().filter(|n| n.kind == PlaylistKind::Playlist && Some(n.id) != except).collect();
    if !lists.is_empty() {
        out.push(MenuEntry::Separator);
    }
    for n in lists {
        let indent = "    ".repeat(usize::from(n.depth));
        let (id, tracks) = (n.id, ids.to_vec());
        out.push(MenuEntry::item(act(format!("{indent}{}", n.name), move |s, _| s.add_to_playlist(id, tracks.clone()))));
    }
    out
}

/// Draws the open menu, if there is one. `new_at` is where the "+" button is on screen.
pub fn build(s: &App, new_at: Option<[f32; 4]>, k: &Kit, layer: &mut Vec<El>) {
    let Some(open) = &s.menu else { return };
    let at = match open.what {
        Context::New => match new_at {
            Some([x, y, _, h]) => (x, y + h + 4.0),
            None => return,
        },
        _ => open.at,
    };
    let menu = Menu::new("context").trigger(el()).at(at.0, at.1).min_width(220.0).open(true).on_close(|s: &mut App| s.menu = None);
    let menu = match &open.what {
        Context::Tracks(ids) => tracks(s, ids, open.sub, menu),
        Context::Playlist(id) => playlist(s, *id, menu),
        Context::New => menu
            .item(act("New playlist", |s, _| s.new_playlist(None, false, Vec::new())))
            .item(act("New folder", |s, _| s.new_playlist(None, true, Vec::new()))),
        Context::Stick(mount) => stick(s, mount, menu),
        Context::HotCue(slot) => hot_cue(s, *slot, menu),
        Context::MemoryCue(row, cue) => memory_cue(*row, *cue, menu),
    };
    // A menu at a point draws from there; the trigger it returns is only a placeholder.
    let anchor = menu.build(k, layer);
    layer.push(anchor);
}

fn tracks(s: &App, ids: &[String], sub: bool, menu: Menu) -> Menu {
    let n = ids.len();
    let these = songs(n);
    let analyse = s.analyse_rows().len();
    let current = match s.browse {
        Browse::Playlist(id) => s.tree.iter().find(|p| p.id == id).map(|p| (id, p.name.clone(), p.kind)),
        Browse::Collection => None,
    };
    let reveal = if cfg!(windows) { "Show in Explorer" } else { "Show in Finder" };
    // The submenu opens on a click: a Rune menu item has no hover to open it on.
    let add_to = MenuItem::new(format!("Add {these} to")).submenu(playlist_picker(s, ids, current.as_ref().map(|c| c.0)), sub).on_select(|s: &mut App| {
        if let Some(m) = &mut s.menu {
            m.sub ^= true;
        }
    });
    let mut menu = menu
        .item(act(if analyse == 1 { "Analyse 1 track".to_owned() } else { format!("Analyse {analyse} tracks") }, |s, _| s.analyse()).disabled(analyse == 0))
        .item(act(reveal, |s, _| s.show_in_finder()).disabled(n == 0))
        .separator()
        .item(add_to);
    if let Some((_, name, PlaylistKind::Playlist)) = &current {
        menu = menu.item(act(format!("Take {these} out of “{name}”"), |s, _| s.remove_from_playlist()).disabled(n == 0));
    }
    let remove = ids.to_vec();
    menu = menu.item(act(format!("Remove {these} from library…"), move |s, _| s.confirm = Some(Confirm::RemoveTracks(remove.clone()))).danger(true).disabled(n == 0));
    if let Some((id, name, _)) = &current {
        menu = export_items(s, menu.separator(), &format!("“{name}”"), &[*id]);
    }
    menu
}

fn playlist(s: &App, id: u64, mut menu: Menu) -> Menu {
    let Some(node) = s.tree.iter().find(|n| n.id == id) else { return menu };
    let folder = node.kind == PlaylistKind::Folder;
    if folder {
        menu = menu
            .item(act(format!("New playlist in “{}”", node.name), move |s, _| s.new_playlist(Some(id), false, Vec::new())))
            .item(act(format!("New folder in “{}”", node.name), move |s, _| s.new_playlist(Some(id), true, Vec::new())))
            .separator();
    }
    menu = menu
        .item(act("Rename", move |s, _| s.start_rename(id)))
        .item(act(if folder { "Delete folder…" } else { "Delete playlist…" }, move |s, _| s.confirm = Some(Confirm::DeletePlaylist(id))).danger(true))
        .separator();
    export_items(s, menu, &format!("“{}”", node.name), &[id])
}

fn stick(s: &App, mount: &std::path::Path, menu: Menu) -> Menu {
    let Some(stick) = s.sticks.iter().find(|d| d.mount == mount) else { return menu };
    let busy = s.exporting_to(mount);
    // The top of the tree is enough: a folder brings everything inside it.
    let all: Vec<u64> = s.tree.iter().filter(|n| n.depth == 0).map(|n| n.id).collect();
    let none = all.is_empty();
    let export_all = if busy { format!("Export all playlists to {} (exporting…)", stick.name) } else { format!("Export all playlists to {}", stick.name) };
    let eject = if busy { format!("Eject {} (exporting…)", stick.name) } else { format!("Eject {}", stick.name) };
    let (to, out) = (mount.to_owned(), mount.to_owned());
    menu.item(act(export_all, move |s, _| s.export(all.clone(), to.clone())).disabled(busy || none))
        .separator()
        .item(act(eject, move |s, _| s.eject(out.clone())).disabled(busy))
}

fn hot_cue(s: &App, slot: u8, menu: Menu) -> Menu {
    let letter = char::from(b'A' + slot);
    let set = s.deck_hot_cues()[slot as usize].is_some();
    menu.item(act(if set { format!("Jump to hot cue {letter}") } else { format!("Set hot cue {letter} here") }, move |s, tx| s.hot_cue(slot, tx)))
        .item(act(format!("Delete hot cue {letter}"), move |s, _| s.delete_hot_cue(slot)).danger(true).disabled(!set))
}

fn memory_cue(row: rbl_index::Row, cue: rbl_index::Cue, menu: Menu) -> Menu {
    let when = crate::library::duration(cue.position_ms / 1000);
    menu.item(act(format!("Jump to the memory cue at {when}"), move |s, _| s.jump_to(cue.position_ms)))
        .item(act(format!("Delete the memory cue at {when}"), move |s, _| s.delete_cue(row, cue)).danger(true).disabled(cue.id == 0))
}
