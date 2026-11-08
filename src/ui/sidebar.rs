//! The sidebar: the collection, the playlist tree and the sticks plugged in.

use rune::El;
use rune_kit::Kit;
use rune_kit::shell::{Sidebar, SidebarRow, SidebarSection};

use crate::app::App;
use crate::library::{Browse, PlaylistKind};
use crate::ui::icons;

pub fn build(s: &App, k: &Kit) -> El {
    let total = s.loaded().map_or(0, |l| l.index.len());
    let collection = SidebarRow::new("Collection")
        .icon(&icons::COLLECTION)
        .detail(thousands(total))
        .height(30.0)
        .selected(s.browse == Browse::Collection)
        .on_select(|s: &mut App, tx| s.browse_to(Browse::Collection, tx))
        .build(k)
        .key("nav-collection");

    let rows = s.tree.iter().map(|n| {
        let id = n.id;
        let icon = match n.kind {
            PlaylistKind::Folder => &icons::FOLDER,
            PlaylistKind::Smart => &icons::SMART,
            PlaylistKind::Playlist => &icons::PLAYLIST,
        };
        let mut row = SidebarRow::new(n.name.clone())
            .icon(icon)
            .depth(n.depth)
            .height(28.0)
            .selected(s.browse == Browse::Playlist(id))
            .on_select(move |s: &mut App, tx| s.browse_to(Browse::Playlist(id), tx));
        if n.kind == PlaylistKind::Folder {
            // A folder opens or closes on a double click and selects on a single one, as Finder's
            // sidebar does.
            row = row.on_double_click(move |s: &mut App, _| {
                if !s.collapsed.remove(&id) {
                    s.collapsed.insert(id);
                }
                s.refresh_tree();
            });
        } else if n.kind == PlaylistKind::Playlist {
            row = row.detail(n.count.to_string());
        }
        row.build(k).key(format!("nav-pl-{id}")).on_secondary_press(move |s: &mut App, press, tx: &mut rune::Tx| {
            crate::ui::menus::playlist_menu(s, id, press, tx);
        })
    });
    let playlists = SidebarSection::new("Playlists")
        .key("nav-playlists")
        .count(s.tree.iter().filter(|n| n.kind != PlaylistKind::Folder).count())
        .collapsed(s.playlists_closed)
        .on_toggle(|s: &mut App, _| s.playlists_closed ^= true)
        .rows(rows);

    let sticks = s.sticks.iter().map(|d| {
        let busy = s.exporting_to(&d.mount);
        let mut row = SidebarRow::new(d.name.clone()).icon(&icons::USB).height(28.0);
        row = if busy {
            row.trailing(rune_kit::Spinner::new().size(12.0).build(k))
        } else {
            row.detail(format!("{} free", bytes(d.free_bytes)))
        };
        let tracks = d.export.as_ref().map_or(0, |e| e.tracks);
        let help = if tracks > 0 { format!("{} · {tracks} tracks exported", d.file_system) } else { d.file_system.clone() };
        row.build(k).key(format!("nav-stick-{}", d.mount.display())).help(help)
    });
    let devices = SidebarSection::new("Devices").key("nav-devices").count(s.sticks.len()).rows(sticks);
    // The same 8 points either side as the gap around the main panel.
    let mut sb = Sidebar::new().label("Library").inset(0.0).header(collection).section(playlists);
    if !s.sticks.is_empty() {
        sb = sb.section(devices);
    }
    sb.build(k)
}

fn bytes(b: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    let g = b as f64 / GB;
    if g >= 10.0 { format!("{g:.0} GB") } else if g >= 1.0 { format!("{g:.1} GB") } else { format!("{:.0} MB", b as f64 / 1_000_000.0) }
}

pub fn thousands(n: usize) -> String {
    let d = n.to_string();
    d.chars().enumerate().fold(String::new(), |mut out, (i, ch)| {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
        out
    })
}
