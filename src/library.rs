//! Opening a rekordbox library and holding its in-memory index.
//!
//! The installed library is only ever opened read-only here. Edits open a short-lived
//! `Writer` of their own (see `edit.rs`), so nothing holds a write handle for long.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rbl_db::{LibraryLocation, OpenMode};
use rbl_index::{Library as Index, Row, SortColumn, TrackSource, View, ViewSpec};

/// A loaded library: where it lives and the index of everything in it.
pub struct Loaded {
    pub location: LibraryLocation,
    pub store: Arc<crate::store::Store>,
    pub index: Arc<Index>,
    /// Bumped on every reload, so views built from an older index are known to be stale.
    pub generation: u64,
}

/// Where to load from.
#[derive(Clone, Debug)]
pub enum Source {
    /// The library rekordbox is set up to use.
    Installed,
    /// A throwaway library built in this folder, for demos and tests.
    Fixture(PathBuf),
}

/// Why the library couldn't be shown. Each says, in words, what to do about it.
#[derive(Debug)]
pub enum LoadError {
    /// No rekordbox library on this Mac, or its settings couldn't be read.
    NotFound(rbl_db::DbError),
    /// The database file is there but wouldn't open.
    Open(rbl_db::DbError),
    /// It opened but reading it failed.
    Read(rusqlite::Error),
    /// The demo library couldn't be built.
    Demo(crate::demo::DemoError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::NotFound(e) => write!(f, "Couldn't find your rekordbox library: {e}"),
            LoadError::Open(e) => write!(f, "Couldn't open the library: {e}"),
            LoadError::Read(e) => write!(f, "Couldn't read the library: {e}"),
            LoadError::Demo(e) => write!(f, "Couldn't build the demo library: {e}"),
        }
    }
}

pub fn load(source: &Source) -> Result<Loaded, LoadError> {
    let location = match source {
        Source::Installed => rbl_db::detect().map_err(LoadError::NotFound)?,
        Source::Fixture(dir) => fixture(dir).map_err(LoadError::Demo)?,
    };
    reload(location, 1)
}

/// Reads the library again from where it already is.
pub fn reload(location: LibraryLocation, generation: u64) -> Result<Loaded, LoadError> {
    let db = rbl_db::Library::open(location.clone(), OpenMode::ReadOnly).map_err(LoadError::Open)?;
    let (index, _) = rbl_index::load(&db).map_err(LoadError::Read)?;
    let store = Arc::new(crate::store::Store::new(location.clone()));
    Ok(Loaded { location, store, index: Arc::new(index), generation })
}

fn fixture(dir: &Path) -> Result<LibraryLocation, crate::demo::DemoError> {
    std::fs::create_dir_all(dir)?;
    let shape = rbl_db::fixture::Shape { tracks: 240, playlists: 8, tracks_per_playlist: 24, ..Default::default() };
    let loc = rbl_db::fixture::build(dir, shape)?;
    crate::demo::enrich(&loc, 240)?;
    crate::demo::with_artwork(&loc)?;
    crate::demo::with_audio(&loc, dir, 3)?;
    Ok(loc)
}

/// What the track list is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Browse {
    Collection,
    /// A playlist, folder or smart playlist, by its database id.
    Playlist(u64),
}

/// The track list's sort, by our column id.
pub fn sort_column(id: &str) -> SortColumn {
    match id {
        "no" => SortColumn::TrackNo,
        "artist" => SortColumn::Artist,
        "album" => SortColumn::Album,
        "genre" => SortColumn::Genre,
        "bpm" => SortColumn::Bpm,
        "key" => SortColumn::KeyCamelot,
        "time" => SortColumn::Duration,
        "rating" => SortColumn::Rating,
        "added" => SortColumn::DateAdded,
        _ => SortColumn::Title,
    }
}

pub fn open_view(index: &Index, browse: Browse, sort: &str, descending: bool, query: &str) -> View {
    let source = match browse {
        Browse::Collection => TrackSource::Collection,
        Browse::Playlist(id) => {
            let pl = index.playlists();
            match pl.index_of(id) {
                Some(i) if pl.is_smart(i) => TrackSource::SmartPlaylist(i),
                Some(i) if pl.is_folder(i) => TrackSource::PlaylistFolder(i),
                Some(i) => TrackSource::Playlist(i),
                None => TrackSource::Collection,
            }
        }
    };
    let spec = ViewSpec { source, sort: sort_column(sort), descending, query: query.to_owned(), filter: Default::default() };
    index.open_view(&spec)
}

/// A playlist tree node, flattened in display order with its depth.
#[derive(Clone, Debug)]
pub struct PlaylistNode {
    pub id: u64,
    pub name: String,
    pub depth: u8,
    pub kind: PlaylistKind,
    pub count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaylistKind {
    Playlist,
    Folder,
    Smart,
}

/// The playlist tree, depth first, siblings in rekordbox's order. Children of a folder in
/// `collapsed` are left out.
pub fn playlist_tree(index: &Index, collapsed: &std::collections::HashSet<u64>) -> Vec<PlaylistNode> {
    let pl = index.playlists();
    // `parent` indexes into the same table, NO_ID at the root.
    let mut children: std::collections::HashMap<u32, Vec<usize>> = Default::default();
    for i in 0..pl.len() {
        let parent = pl.parent[i];
        let key = if (parent as usize) < pl.len() { parent } else { rbl_index::NO_ID };
        children.entry(key).or_default().push(i);
    }
    for list in children.values_mut() {
        list.sort_by_key(|&i| pl.seq[i]);
    }
    let mut out = Vec::new();
    let mut stack: Vec<(usize, u8)> = children.get(&rbl_index::NO_ID).map(|v| v.iter().rev().map(|&i| (i, 0)).collect()).unwrap_or_default();
    while let Some((i, depth)) = stack.pop() {
        let kind = if pl.is_folder(i) {
            PlaylistKind::Folder
        } else if pl.is_smart(i) {
            PlaylistKind::Smart
        } else {
            PlaylistKind::Playlist
        };
        let id = pl.ids[i];
        out.push(PlaylistNode { id, name: pl.name(i).to_owned(), depth, kind, count: pl.members[i].len() });
        if kind == PlaylistKind::Folder
            && !collapsed.contains(&id)
            && let Some(kids) = u32::try_from(i).ok().and_then(|k| children.get(&k))
        {
            stack.extend(kids.iter().rev().map(|&c| (c, depth.saturating_add(1))));
        }
    }
    out
}

/// One track's fields as the list and inspector show them.
pub struct TrackFields<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub genre: &'a str,
    pub key: &'a str,
    pub bpm_x100: u32,
    pub length_sec: u32,
    pub rating: u8,
    pub color: u8,
    pub date_added: &'a str,
}

pub fn fields(index: &Index, row: Row) -> TrackFields<'_> {
    let i = row as usize;
    TrackFields {
        title: index.title.get(i),
        artist: index.artist_name(row),
        album: index.album_name(row),
        genre: index.genre_name(row),
        key: index.key_name(row),
        bpm_x100: index.bpm_x100[i],
        length_sec: index.length_sec[i],
        rating: index.rating[i],
        color: index.color[i],
        date_added: index.date_added.get(i),
    }
}

pub fn duration(sec: u32) -> String {
    format!("{}:{:02}", sec / 60, sec % 60)
}

pub fn long_duration(sec: u64) -> String {
    let (h, m) = (sec / 3600, (sec % 3600) / 60);
    if h > 0 { format!("{h} h {m} min") } else { format!("{m} min") }
}

pub fn bpm(x100: u32) -> String {
    match x100 {
        0 => String::new(),
        x if x % 100 == 0 => (x / 100).to_string(),
        x if x % 10 == 0 => format!("{:.1}", f64::from(x) / 100.0),
        x => format!("{:.2}", f64::from(x) / 100.0),
    }
}

#[cfg(test)]
mod probe {
    /// Lists the unusual characters in the installed library's titles and artists, to see which
    /// fonts the fallback needs. Read-only. `cargo test probe -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn odd_characters_in_the_installed_library() {
        let l = super::load(&super::Source::Installed).unwrap();
        let mut seen = std::collections::BTreeMap::<char, usize>::new();
        for i in 0..l.index.len() {
            let r = i as u32;
            for ch in l.index.title.get(i).chars().chain(l.index.artist_name(r).chars()) {
                if !ch.is_ascii() && !ch.is_alphabetic() {
                    *seen.entry(ch).or_default() += 1;
                }
            }
        }
        for (ch, n) in seen {
            println!("U+{:04X} {ch} x{n}", ch as u32);
        }
    }
}

#[cfg(test)]
mod icloud_probe {
    /// Counts the installed library's tracks that are only in iCloud. Read-only, downloads
    /// nothing. `cargo test icloud_probe -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn count_evicted() {
        let l = super::load(&super::Source::Installed).unwrap();
        let (mut evicted, mut local, mut missing) = (0, 0, 0);
        for i in 0..l.index.len() {
            let p = std::path::Path::new(l.index.folder_path.get(i));
            if !p.exists() {
                missing += 1;
            } else if crate::icloud::is_evicted(p) {
                evicted += 1;
            } else {
                local += 1;
            }
        }
        println!("local {local}, in iCloud only {evicted}, missing {missing}");
    }
}

#[cfg(test)]
mod icloud_probe_paths {
    /// Prints the smallest iCloud-only track's path. Read-only.
    #[test]
    #[ignore]
    fn smallest_evicted() {
        let l = super::load(&super::Source::Installed).unwrap();
        let best = (0..l.index.len())
            .map(|i| std::path::PathBuf::from(l.index.folder_path.get(i)))
            .filter(|p| crate::icloud::is_evicted(p))
            .min_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(u64::MAX));
        println!("PATH {}", best.unwrap().display());
    }
}
