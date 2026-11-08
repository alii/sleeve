//! USB export: writing library playlists to a stick a CDJ can read.
//!
//! An export adds playlists to what the stick already holds, the way rekordbox's Export
//! Playlist does. Playlists an earlier export from this library put there are written again
//! with what they hold now, and anything else on the stick is left where it is.
//!
//! The heavy lifting is `rbl_export`'s. It stages everything beside the old files, checks the
//! result, and only then swaps it in. Stopping before that swap leaves the stick as it was. A
//! stick pulled out halfway is finished or rolled back the next time it's exported to.
//!
//! Nothing here writes to `master.db`. The library is only read.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock};

use rbl_core::durable::VolumeIdentity;
use rbl_db::OpenMode;
use rbl_db::export_info::{self, MyTagRow, TrackExtras};
use rbl_export::{ExportError, ExportProgress, ExportReport, SourceMyTag, SourcePlaylist, SourceTrack, SyncNode, SyncSource};
use rbl_index::{Library as Index, TrackSource};

use crate::jobs::{Outcome, Reporter};
use crate::store::Store;

/// A volume an export could go to, and what an earlier export left on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stick {
    pub name: String,
    pub mount: PathBuf,
    pub free_bytes: u64,
    pub total_bytes: u64,
    pub file_system: String,
    /// What's already exported there. None for a stick with no export on it.
    pub export: Option<rbl_devices::DeviceExport>,
}

/// The volumes worth offering as export destinations.
///
/// With `RB_LITE_FAKE_VOLUMES` set, its `:`-separated folders stand in for real volumes. This
/// reads each stick to see what it holds, so call it when the list is opened, not on a timer.
pub fn sticks() -> Vec<Stick> {
    rbl_devices::list().into_iter().map(describe).collect()
}

fn describe(device: rbl_devices::Device) -> Stick {
    let export = rbl_devices::inspect(&device.mount_point);
    Stick {
        name: device.name,
        mount: device.mount_point,
        free_bytes: device.free_bytes,
        total_bytes: device.total_bytes,
        file_system: device.file_system,
        export,
    }
}

/// Conversion and probe results kept between exports, so a second export doesn't read every
/// file again. It's disposable: an export never depends on it being there.
static CACHE: LazyLock<Option<rbl_export::cache::PreparationCache>> = LazyLock::new(|| {
    // Tests never write outside their own folders.
    if cfg!(test) {
        return None;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let cache = rbl_export::cache::PreparationCache::new(home.join("Library/Caches/Sleeve/usb-export"));
    let _ = cache.prune(CACHE_BUDGET);
    Some(cache)
});

const CACHE_BUDGET: u64 = 8 * 1024 * 1024 * 1024;

/// Exports these playlists (by database id; a folder brings everything under it) to the stick
/// mounted at `stick`.
pub fn run(store: &Arc<Store>, index: Arc<Index>, playlists: Vec<u64>, stick: PathBuf, rep: &mut Reporter) -> Outcome {
    let name = stick_name(&stick);
    if rep.is_cancelled() {
        return unchanged(&name);
    }
    if !stick.is_dir() {
        return Outcome::Failed(format!("{name} isn't plugged in."));
    }
    if rbl_db::is_rekordbox_app_running() {
        return Outcome::Failed(format!("rekordbox is open. Quit it first, so only one app writes to {name}."));
    }
    // Held for the whole export, so a different drive mounted at the same path is noticed.
    rep.info(format!("Drive: {name} at {}", stick.display()));
    let volume = match VolumeIdentity::capture(&stick) {
        Ok(v) => v,
        Err(e) => {
            rep.error(format!("Couldn't read the drive's identity: {e:?}"));
            return Outcome::Failed(format!("Couldn't read {name}. Check it's plugged in and try again."));
        }
    };

    rep.retitle(format!("Getting ready to export to {name}"));
    rep.progress(0, None, "");
    // A pulled-out export is finished or rolled back first, so the stick is read as it is.
    if rbl_export::recovery_pending(&stick) {
        rep.warn("The last export to this drive didn't finish. Finishing or undoing it first.");
        rep.retitle(format!("Finishing the last export to {name}"));
    }
    if let Err(e) = rbl_export::recover(&stick) {
        rep.error(format!("Recovery failed: {e:?}"));
        return Outcome::Failed(format!("Couldn't finish the last export to {name}: {e}"));
    }

    let db = match rbl_db::Library::open(store.location.clone(), OpenMode::ReadOnly) {
        Ok(db) => db,
        Err(e) => {
            rep.error(format!("Opening the library read-only failed: {e:?}"));
            return Outcome::Failed(format!("Couldn't open the library: {e}"));
        }
    };
    let selection = match Selection::build(&db, &index, &store.location.share_root, &stick, &playlists) {
        Ok(s) => s,
        Err(message) => return Outcome::Failed(message),
    };
    drop(db);
    rep.info(format!("Selected {} and {} to export.", count(selection.tracks.len(), "track"), count(selection.playlists.len(), "playlist")));
    if rep.is_cancelled() {
        return unchanged(&name);
    }

    // Songs the stick needs that are only in iCloud come down first, where the stop button
    // works. Read inside the exporter, each would hold it for its whole download.
    let plan = rbl_export::estimate::estimate(&stick, selection.sync.db_id, &selection.tracks, false, None);
    let needed: Vec<PathBuf> = selection
        .tracks
        .iter()
        .filter(|t| matches!(plan.actions.get(&rbl_export::manifest::track_key(t.id, &t.source_path.to_string_lossy())), Some(rbl_export::estimate::AudioAction::Copy | rbl_export::estimate::AudioAction::Convert)))
        .map(|t| t.source_path.clone())
        .collect();
    match crate::icloud::fetch_all(&needed, rep) {
        Ok(failed) if failed.is_empty() => {}
        Ok(failed) => {
            let n = failed.len();
            return Outcome::Failed(format!("Couldn't download {} from iCloud, so nothing was written to {name}. The log says which.", count(n, "song")));
        }
        Err(crate::icloud::Stopped) => return unchanged(&name),
    }
    if rep.is_cancelled() {
        return unchanged(&name);
    }

    let result = write(&selection, &stick, &name, &volume, rep);
    if let Some(cache) = CACHE.as_ref() {
        let _ = cache.prune(CACHE_BUDGET);
    }
    match result {
        Ok(report) => {
            log_report(&report, rep);
            finished(&name, &report)
        }
        Err(WriteFailure::Unplugged) => {
            rep.error(format!("{name} disappeared from {} during the export.", stick.display()));
            Outcome::Failed(format!("{name} was unplugged during the export. Plug it back in and export again: Sleeve finishes or undoes the half-done export first, so nothing on it is left broken."))
        }
        Err(WriteFailure::Export(ExportError::Cancelled)) => {
            rep.info("The exporter stopped and rolled back. Nothing on the drive changed.");
            unchanged(&name)
        }
        Err(WriteFailure::Export(e)) => {
            rep.error(format!("Export error: {e} ({e:?})"));
            Outcome::Failed(failure(&name, &stick, &volume, &e))
        }
    }
}

/// Writes what an export did into its log, in full.
fn log_report(r: &ExportReport, rep: &Reporter) {
    let mb = |b: u64| crate::icloud::size(b);
    rep.info(format!(
        "Done in {:.1}s: {} and {}; copied {} ({}), reused {} already there ({}), converted {}; {} analysis files, {} artwork files, export.pdb {}. Verified: {}.",
        r.elapsed_ms as f64 / 1000.0,
        count(r.tracks, "track"),
        count(r.playlists, "playlist"),
        count(r.tracks.saturating_sub(r.reused), "file"),
        mb(r.bytes_copied),
        r.reused,
        mb(r.bytes_reused),
        r.converted,
        r.analysis_files,
        r.artwork_files,
        mb(r.pdb_bytes as u64),
        if r.verified { "yes" } else { "no" },
    ));
    for w in &r.warnings {
        rep.warn(format!("{w:?}"));
    }
    for t in &r.skipped {
        rep.warn(format!("Left out, file missing or unreadable: {t}"));
    }
    for t in &r.failed {
        rep.error(format!("Left out: {t}"));
    }
}

/// Why writing to the stick stopped short.
enum WriteFailure {
    /// The stick went away mid-export.
    Unplugged,
    Export(ExportError),
}

fn write(selection: &Selection, stick: &Path, name: &str, volume: &VolumeIdentity, rep: &mut Reporter) -> Result<ExportReport, WriteFailure> {
    // A fresh HFS+ stick gets the hidden `.PIONEER` root, as rekordbox gives it.
    let root = rbl_devices::list()
        .into_iter()
        .find(|d| d.mount_point == stick)
        .and_then(|d| rbl_export::ExportRoot::for_file_system(&d.file_system));
    let options = rbl_export::ExportOptions {
        sync: Some(&selection.sync),
        root,
        keep_unlisted: true,
        cache: CACHE.as_ref(),
        expected_volume: Some(volume),
        ..Default::default()
    };
    let flag = rep.token().flag();
    // A stick pulled out mid-export stops it at once rather than leaving it to fail file by
    // file. The watcher asks the disk, so it runs on its own thread: a dying drive can be slow
    // to answer.
    let gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (gone, done, stick) = (gone.clone(), done.clone(), stick.to_owned());
        std::thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                if !stick.is_dir() {
                    gone.store(true, Ordering::Release);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });
    }
    let unplugged = gone.clone();
    let cancelled = move || flag.load(Ordering::Acquire) || unplugged.load(Ordering::Acquire);
    let mut title = String::new();
    let mut last_track = String::new();
    let mut progress = |p: &ExportProgress| {
        let stage = stage_title(p.stage, name);
        if stage != title {
            rep.retitle(stage.as_str());
            title = stage;
        }
        if matches!(p.stage, "copying" | "converting") && !p.title.is_empty() && p.title != last_track {
            rep.info(format!("{} \"{}\"", if p.stage == "converting" { "Converting" } else { "Copying" }, p.title));
            last_track.clone_from(&p.title);
        }
        let (done, total) = if p.unit == "tracks" { (p.done, p.total) } else { (p.tracks_done, p.tracks_total) };
        let total = (total > 0).then_some(total as u64);
        rep.progress(done as u64, total, p.title.as_str());
    };
    let report = rbl_export::export_cancellable(stick, &selection.tracks, &selection.playlists, &selection.my_tags, &options, &mut progress, &cancelled);
    done.store(true, Ordering::Release);
    // The watcher may not have looked yet when a write to the missing stick failed first.
    if gone.load(Ordering::Acquire) || (report.is_err() && !stick.is_dir()) {
        return Err(WriteFailure::Unplugged);
    }
    let report = report.map_err(WriteFailure::Export)?;
    rep.progress(selection.tracks.len() as u64, Some(selection.tracks.len() as u64), "");
    Ok(report)
}

/// The job's title for each of the exporter's stages.
fn stage_title(stage: &str, name: &str) -> String {
    match stage {
        "recovering" => format!("Finishing the last export to {name}"),
        "verifying" | "planning" => format!("Checking {name}"),
        "database" => format!("Writing the library to {name}"),
        "publishing" => format!("Saving {name}"),
        // "checking", "copying", "converting": one pass over the tracks, copying what's new.
        _ => format!("Copying music to {name}"),
    }
}

fn unchanged(name: &str) -> Outcome {
    Outcome::Stopped(format!("Stopped. {name} is as it was before."))
}

fn finished(name: &str, report: &ExportReport) -> Outcome {
    if !report.verified {
        return Outcome::Failed(format!("The export to {name} didn't check out. Export again to repair it."));
    }
    if report.up_to_date {
        return Outcome::Completed(format!("{name} was already up to date."));
    }
    let mut summary = format!("Exported {} to {name}", count(report.tracks, "track"));
    let (missing, broken) = (report.skipped.len(), report.failed.len());
    match (missing, broken) {
        (0, 0) => {}
        (m, 0) => summary.push_str(&format!(", leaving out {m} whose {} missing", if m == 1 { "file is" } else { "files are" })),
        (0, b) => summary.push_str(&format!(", leaving out {b} that couldn't be copied")),
        (m, b) => summary.push_str(&format!(", leaving out {m} missing and {b} that couldn't be copied")),
    }
    summary.push('.');
    Outcome::Completed(summary)
}

/// Why an export failed, in words a person can act on.
fn failure(name: &str, stick: &Path, volume: &VolumeIdentity, e: &ExportError) -> String {
    if matches!(e, ExportError::DeviceGone) || !stick.is_dir() {
        return format!("{name} was unplugged. Plug it back in and export again to finish.");
    }
    if volume.check(stick).is_err() {
        return format!("{name} isn't the drive the export started on, so it stopped.");
    }
    match e {
        ExportError::Io(io) if io.kind() == std::io::ErrorKind::StorageFull => format!("{name} is full. Make some room on it and try again."),
        ExportError::Io(io) if io.kind() == std::io::ErrorKind::ReadOnlyFilesystem => format!("{name} is read-only, so nothing could be written to it."),
        ExportError::Conflict(_) => format!("Something else changed {name} during the export. Close other apps using it and try again."),
        ExportError::Empty => "Those playlists have nothing in them to export.".to_owned(),
        _ => format!("Couldn't export to {name}: {e}"),
    }
}

/// The stick's volume name, or its folder's name when the OS doesn't list it.
fn stick_name(stick: &Path) -> String {
    rbl_devices::list()
        .into_iter()
        .find(|d| d.mount_point == stick)
        .map(|d| d.name)
        .filter(|n| !n.is_empty())
        .or_else(|| stick.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "the USB".to_owned())
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 { format!("1 {noun}") } else { format!("{n} {noun}s") }
}

/// What one export writes: the tracks, the playlists that name them by index, and the
/// library's My Tags.
struct Selection {
    tracks: Vec<SourceTrack>,
    playlists: Vec<SourcePlaylist>,
    my_tags: Vec<SourceMyTag>,
    sync: SyncSource,
}

impl Selection {
    /// The playlists asked for, after the ones the stick already holds from this library,
    /// plus any loose tracks an earlier export put there.
    fn build(db: &rbl_db::Library, index: &Index, share: &Path, stick: &Path, asked: &[u64]) -> Result<Selection, String> {
        let conn = db.connection();
        let read = |e: rbl_db::DbError| format!("Couldn't read the library: {e}");
        let db_id = export_info::db_id(conn).map_err(read)?;
        let held = Held::read(index, stick, db_id);

        let mut wanted = held.playlists;
        for &id in asked {
            if index.playlists().index_of(id).is_none() {
                return Err("One of those playlists isn't in the library any more.".to_owned());
            }
            if !wanted.contains(&id) {
                wanted.push(id);
            }
        }

        let (named, chosen_folders) = leaves(index, &wanted);
        // `source_rows` takes the playlist lock itself, so nothing here holds it.
        let rows_of: Vec<Vec<u32>> = named.iter().map(|n| index.source_rows(&n.source)).collect();
        let loose_rows: Vec<u32> = held.loose.iter().filter_map(|&id| index.row_of_id(id)).collect();

        let content_ids: Vec<String> = rows_of
            .iter()
            .flatten()
            .chain(&loose_rows)
            .map(|&row| index.ids.get(row as usize).copied().unwrap_or(0).to_string())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let plain: Vec<u64> = named.iter().filter(|n| matches!(n.source, TrackSource::Playlist(_))).map(|n| n.id).collect();
        let my_tags = export_info::my_tags(conn).map_err(read)?;
        let extras = export_info::track_extras(conn, &content_ids).map_err(read)?;
        let cloud_db_id = export_info::cloud_db_id(conn).map_err(read)?;
        let entries_changed = export_info::playlist_entries_changed(conn, &plain).map_err(read)?;

        let tree = tree(index);
        let parent_of: HashMap<u64, u64> = tree.iter().map(|n| (n.id, n.parent)).collect();
        let sync = SyncSource { db_id, tree, automatic: held.automatic, cloud_db_id };

        // Each track goes in once, however many playlists hold it.
        let mut tracks = Vec::new();
        let mut position: HashMap<u32, usize> = HashMap::new();
        let mut add = |row: u32, tracks: &mut Vec<SourceTrack>| {
            *position.entry(row).or_insert_with(|| {
                let content = index.ids.get(row as usize).copied().unwrap_or(0).to_string();
                tracks.push(source_track(index, share, row, extras.get(&content).unwrap_or(&TrackExtras::default())));
                tracks.len() - 1
            })
        };
        let mut playlists = Vec::with_capacity(named.len());
        for (n, rows) in named.into_iter().zip(rows_of) {
            let track_indices = rows.into_iter().map(|row| add(row, &mut tracks)).collect();
            playlists.push(SourcePlaylist {
                id: n.id,
                name: n.name,
                parent_id: parent_of.get(&n.id).copied().unwrap_or(0),
                track_indices,
                entries_changed_ms: entries_changed.get(&n.id).copied(),
                ..Default::default()
            });
        }
        for row in loose_rows {
            add(row, &mut tracks);
        }

        // The folders above each playlist go on the stick as folders, and so do folders asked
        // for by name even when nothing under them is a playlist.
        let mut folders = HashSet::new();
        let starts = playlists.iter().map(|p| p.parent_id).chain(chosen_folders);
        for start in starts {
            let mut next = start;
            while next != 0 && folders.insert(next) {
                next = parent_of.get(&next).copied().unwrap_or(0);
            }
        }
        {
            let pl = index.playlists();
            for id in folders {
                let Some(i) = pl.index_of(id) else { continue };
                playlists.push(SourcePlaylist {
                    id,
                    name: pl.name(i).to_owned(),
                    parent_id: parent_of.get(&id).copied().unwrap_or(0),
                    folder: true,
                    ..Default::default()
                });
            }
        }
        // The exporter numbers siblings in the order given, so give them the tree's order.
        let rank = tree_rank(index);
        playlists.sort_by_key(|p| rank.get(&p.id).copied().unwrap_or(usize::MAX));

        Ok(Selection { tracks, playlists, my_tags: my_tags.iter().map(source_my_tag).collect(), sync })
    }
}

/// What the stick already holds from this library.
struct Held {
    /// Library playlists, in the order the stick lists them.
    playlists: Vec<u64>,
    /// Tracks put there in no playlist.
    loose: Vec<u64>,
    /// Whether the stick asks to be synced again on its own.
    automatic: bool,
}

impl Held {
    /// Only a stick written from this library counts: another library's ids name other
    /// playlists. An older stick with no library id in its manifest counts when rekordbox's
    /// own sync record names this library. The stick's own playlists, and ones since deleted
    /// from the library, are left to `keep_unlisted`, which keeps them as they are.
    fn read(index: &Index, stick: &Path, db_id: u64) -> Held {
        let none = Held { playlists: Vec::new(), loose: Vec::new(), automatic: false };
        let Some(manifest) = rbl_export::Manifest::load(stick) else { return none };
        let record = rbl_export::sync_record::read(stick);
        let ours = manifest.db_id == db_id || (manifest.db_id == 0 && db_id != 0 && record.as_ref().is_some_and(|r| r.db_id == db_id));
        if !ours {
            return none;
        }
        let pl = index.playlists();
        let playlists = manifest
            .playlists
            .iter()
            .filter(|p| !p.folder && !p.device_only && p.library_id != 0 && pl.index_of(p.library_id).is_some())
            .map(|p| p.library_id)
            .collect();
        Held { playlists, loose: manifest.loose, automatic: record.is_some_and(|r| r.automatic) }
    }
}

/// A playlist that holds tracks, as the export will name it.
struct Named {
    id: u64,
    name: String,
    source: TrackSource,
}

/// The playlists these ids stand for, a folder standing for every playlist under it, each
/// once. Also returns every folder met on the way.
fn leaves(index: &Index, ids: &[u64]) -> (Vec<Named>, Vec<u64>) {
    let pl = index.playlists();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); pl.len()];
    for i in 0..pl.len() {
        if let Some(bucket) = pl.parent.get(i).and_then(|&p| children.get_mut(p as usize)) {
            bucket.push(i);
        }
    }
    let (mut named, mut folders) = (Vec::new(), Vec::new());
    let mut seen = vec![false; pl.len()];
    for &id in ids {
        let Some(start) = pl.index_of(id) else { continue };
        // A visited set, so a corrupt parent cycle can't loop forever.
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            match seen.get_mut(i) {
                Some(s) if !*s => *s = true,
                _ => continue,
            }
            if pl.is_folder(i) {
                folders.push(pl.ids[i]);
                stack.extend(children[i].iter().rev());
            } else {
                let source = if pl.is_smart(i) { TrackSource::SmartPlaylist(i) } else { TrackSource::Playlist(i) };
                named.push(Named { id: pl.ids[i], name: pl.name(i).to_owned(), source });
            }
        }
    }
    (named, folders)
}

/// The library's whole playlist tree, which the stick's sync record keeps.
fn tree(index: &Index) -> Vec<SyncNode> {
    let pl = index.playlists();
    (0..pl.len())
        .map(|i| SyncNode {
            id: pl.ids[i],
            parent: pl.parent.get(i).and_then(|&p| pl.ids.get(p as usize)).copied().unwrap_or(0),
            attribute: pl.attribute.get(i).copied().unwrap_or(0),
        })
        .collect()
}

/// Each playlist and folder's place in the tree read top to bottom.
fn tree_rank(index: &Index) -> HashMap<u64, usize> {
    let pl = index.playlists();
    let mut children: HashMap<u32, Vec<usize>> = HashMap::new();
    for i in 0..pl.len() {
        let parent = pl.parent.get(i).copied().filter(|&p| (p as usize) < pl.len()).unwrap_or(rbl_index::NO_ID);
        children.entry(parent).or_default().push(i);
    }
    for siblings in children.values_mut() {
        siblings.sort_by_key(|&i| (pl.seq.get(i).copied().unwrap_or(0), i));
    }
    let mut rank = HashMap::new();
    let mut seen = vec![false; pl.len()];
    let mut stack: Vec<usize> = children.get(&rbl_index::NO_ID).map(|c| c.iter().rev().copied().collect()).unwrap_or_default();
    while let Some(i) = stack.pop() {
        match seen.get_mut(i) {
            Some(s) if !*s => *s = true,
            _ => continue,
        }
        rank.insert(pl.ids[i], rank.len());
        if let Some(under) = u32::try_from(i).ok().and_then(|k| children.get(&k)) {
            stack.extend(under.iter().rev());
        }
    }
    rank
}

fn source_my_tag(tag: &MyTagRow) -> SourceMyTag {
    SourceMyTag {
        id: tag.id.parse().unwrap_or(0),
        seq: u32::try_from(tag.seq.max(0)).unwrap_or(u32::MAX),
        name: tag.name.clone(),
        attribute: u8::try_from(tag.attribute.clamp(0, 255)).unwrap_or(0),
        parent: tag.parent.parse().unwrap_or(0),
    }
}

/// One library row as the exporter wants it. Analysis facts come from `extra` when the
/// database has newer ones than the index, so an export during an analysis run doesn't treat
/// freshly analysed tracks as unanalysed.
fn source_track(index: &Index, share: &Path, row: u32, extra: &TrackExtras) -> SourceTrack {
    let i = row as usize;
    let analysed = extra.analysis.as_ref();
    let analysis_path = analysed.map_or_else(|| index.analysis_path.get(i), |a| a.analysis_path.as_str());
    let artwork_path = analysed.map_or_else(|| index.artwork_path.get(i), |a| a.artwork_path.as_str());
    let in_share = |p: &str| share.join(p.trim_start_matches(['/', '\\']));
    SourceTrack {
        metadata: extra.metadata.clone(),
        cues: Some(extra.cues.clone()),
        // The content id is how a later export recognises a track it already wrote.
        id: index.ids.get(i).copied().unwrap_or(0),
        source_path: source_audio(index.folder_path.get(i), &extra.alternate_paths),
        artwork: Some(artwork_path).filter(|p| !p.is_empty()).map(in_share),
        my_tags: extra.my_tags.iter().filter_map(|t| t.parse().ok()).collect(),
        title: index.title.get(i).to_owned(),
        artist: index.artist_name(row).to_owned(),
        album: index.album_name(row).to_owned(),
        genre: index.genre_name(row).to_owned(),
        label: index.label_name(row).to_owned(),
        key: analysed.map_or_else(|| index.key_name(row).to_owned(), |a| a.key.clone()),
        comment: index.comment.get(i).to_owned(),
        date_added: index.date_added.get(i).to_owned(),
        release_date: index.release_date.get(i).to_owned(),
        bpm_x100: analysed.map_or_else(|| index.bpm_x100.get(i).copied().unwrap_or(0), |a| a.bpm_x100),
        duration_sec: u16::try_from(analysed.map_or_else(|| index.length_sec.get(i).copied().unwrap_or(0), |a| a.length_sec)).unwrap_or(u16::MAX),
        rating: index.rating.get(i).copied().unwrap_or(0),
        color_id: index.color.get(i).copied().unwrap_or(0),
        bitrate: index.bitrate.get(i).copied().unwrap_or(0),
        sample_rate: index.sample_rate.get(i).copied().unwrap_or(0),
        file_size: index.file_size.get(i).copied().unwrap_or(0),
        year: index.year.get(i).copied().unwrap_or(0),
        analysis_source: (!analysis_path.is_empty()).then(|| in_share(analysis_path)),
        ..Default::default()
    }
}

/// Where a track's audio is read from. A cloud-synced track's `FolderPath` can name a copy
/// that isn't on this machine, so the row's other paths are tried in turn. When none exists,
/// `FolderPath` stays, and the export reports the track as missing under that name.
fn source_audio(folder_path: &str, alternates: &[String]) -> PathBuf {
    let first = PathBuf::from(folder_path);
    if first.is_file() {
        return first;
    }
    alternates.iter().map(PathBuf::from).find(|p| p.is_file()).unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobKind, Jobs, Phase};
    use rbl_db::fixture;
    use std::time::{Duration, Instant};

    /// A folder under the system temp dir, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("sleeve-export-{}-{n}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Every file under `dir` with its bytes, to tell whether anything changed.
    fn files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_owned()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push((path.strip_prefix(dir).unwrap().to_owned(), std::fs::read(&path).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    fn export(store: &Arc<Store>, index: &Arc<Index>, playlists: Vec<u64>, stick: &Path, cancel_first: bool) -> Outcome {
        let mut jobs = Jobs::default();
        let (store, index, stick) = (store.clone(), index.clone(), stick.to_owned());
        let id = jobs.spawn(JobKind::Export, "Exporting", move |rep| {
            if cancel_first {
                rep.token().cancel();
            }
            run(&store, index, playlists, stick, rep)
        });
        let start = Instant::now();
        loop {
            let _ = jobs.drain(0.0);
            if let Phase::Done { outcome, .. } = &jobs.get(id).unwrap().phase {
                return outcome.clone();
            }
            assert!(start.elapsed() < Duration::from_secs(60), "the export timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn pulling_the_stick_out_fails_cleanly_and_the_next_export_finishes() {
        let library = TempDir::new("library-pull");
        let music = TempDir::new("music-pull");
        let stick = TempDir::new("stick-pull");
        let away = stick.0.with_extension("away");
        let _ = std::fs::remove_dir_all(&away);

        // Big enough files that the export is still copying when the stick goes.
        let shape = fixture::Shape { tracks: 30, playlists: 1, tracks_per_playlist: 30, ..Default::default() };
        let location = fixture::build(&library.0, shape).unwrap();
        for i in 0..shape.tracks {
            let path = music.0.join(format!("track-{i}.mp3"));
            std::fs::write(&path, vec![i as u8; 24 * 1024 * 1024]).unwrap();
            fixture::point_at_audio(&location, i, path.to_str().unwrap(), 200).unwrap();
        }
        let db = rbl_db::Library::open(location.clone(), OpenMode::ReadOnly).unwrap();
        let index = Arc::new(rbl_index::load(&db).unwrap().0);
        drop(db);
        let store = Arc::new(Store::new(location));
        let list: u64 = fixture::playlist_id(0).parse().unwrap();

        // "Unplug" it shortly after the export starts: its folder goes away.
        let (from, to) = (stick.0.clone(), away.clone());
        let puller = std::thread::spawn(move || {
            // Wait for the copying to be under way: some bytes of audio on the stick.
            let start = Instant::now();
            let copying = |dir: &Path| -> bool {
                let mut stack = vec![dir.to_owned()];
                while let Some(d) = stack.pop() {
                    for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            stack.push(p);
                        } else if p.extension().is_some_and(|x| x == "mp3") && e.metadata().is_ok_and(|m| m.len() > 0) {
                            return true;
                        }
                    }
                }
                false
            };
            while !copying(&from) && start.elapsed() < Duration::from_secs(20) {
                std::thread::sleep(Duration::from_millis(2));
            }
            std::fs::rename(&from, &to).unwrap();
        });
        let outcome = export(&store, &index, vec![list], &stick.0, false);
        puller.join().unwrap();
        let Outcome::Failed(why) = &outcome else { panic!("pulling the stick out must fail the export, got {outcome:?}") };
        assert!(why.contains("unplugged"), "{why}");

        // Plugged back in, the next export finishes the job and the stick checks out.
        std::fs::rename(&away, &stick.0).unwrap();
        let outcome = export(&store, &index, vec![list], &stick.0, false);
        assert!(matches!(outcome, Outcome::Completed(_)), "{outcome:?}");
        assert!(!rbl_export::recovery_pending(&stick.0));
        let seen = rbl_devices::inspect(&stick.0).expect("the stick holds an export");
        assert_eq!(seen.tracks, 30);
    }

    #[test]
    fn export_writes_a_stick_and_a_stopped_one_leaves_it_alone() {
        let library = TempDir::new("library");
        let music = TempDir::new("music");
        let stick = TempDir::new("stick");
        let fresh = TempDir::new("fresh");

        let shape = fixture::Shape { tracks: 8, playlists: 2, tracks_per_playlist: 3, ..Default::default() };
        let location = fixture::build(&library.0, shape).unwrap();
        // The exporter copies real files, so every track gets one.
        for i in 0..shape.tracks {
            let path = music.0.join(format!("track-{i}.mp3"));
            std::fs::write(&path, vec![i as u8; 4096]).unwrap();
            fixture::point_at_audio(&location, i, path.to_str().unwrap(), 200).unwrap();
        }
        let db = rbl_db::Library::open(location.clone(), OpenMode::ReadOnly).unwrap();
        let index = Arc::new(rbl_index::load(&db).unwrap().0);
        drop(db);
        let store = Arc::new(Store::new(location));
        let first: u64 = fixture::playlist_id(0).parse().unwrap();
        let second: u64 = fixture::playlist_id(1).parse().unwrap();

        // Stopped before it starts, a fresh stick stays empty.
        let outcome = export(&store, &index, vec![first], &fresh.0, true);
        assert!(matches!(outcome, Outcome::Stopped(_)), "{outcome:?}");
        assert!(files(&fresh.0).is_empty(), "a stopped export must not write anything");

        let outcome = export(&store, &index, vec![first], &stick.0, false);
        let Outcome::Completed(summary) = &outcome else { panic!("{outcome:?}") };
        assert!(summary.starts_with("Exported 3 tracks to "), "{summary}");
        assert!(stick.0.join("PIONEER/rekordbox/export.pdb").is_file());
        let seen = rbl_devices::inspect(&stick.0).expect("the stick holds an export");
        assert_eq!((seen.tracks, seen.ours), (3, true));

        // Stopped before it starts, an exported stick is byte for byte as it was.
        let before = files(&stick.0);
        let outcome = export(&store, &index, vec![second], &stick.0, true);
        assert!(matches!(outcome, Outcome::Stopped(_)), "{outcome:?}");
        assert_eq!(files(&stick.0), before, "a stopped export must leave the stick as it was");
        assert!(!rbl_export::recovery_pending(&stick.0));

        // A second playlist is added beside the first, not in its place.
        let outcome = export(&store, &index, vec![second], &stick.0, false);
        assert!(matches!(outcome, Outcome::Completed(_)), "{outcome:?}");
        let both: HashSet<u32> = [first, second]
            .iter()
            .flat_map(|&id| index.source_rows(&TrackSource::Playlist(index.playlists().index_of(id).unwrap())))
            .collect();
        let seen = rbl_devices::inspect(&stick.0).unwrap();
        assert_eq!((seen.tracks, seen.playlists), (both.len(), 2));

        // The volume list reads what's on a stick.
        let device = rbl_devices::Device {
            name: "KINGSTON".into(),
            mount_point: stick.0.clone(),
            total_bytes: 0,
            free_bytes: 0,
            file_system: String::new(),
            removable: true,
            volume_id: String::new(),
        };
        let listed = describe(device);
        assert_eq!(listed.export.map(|e| e.tracks), Some(both.len()));

        // Exporting the same thing again changes nothing.
        let outcome = export(&store, &index, vec![second], &stick.0, false);
        assert_eq!(outcome, Outcome::Completed(format!("{} was already up to date.", stick_name(&stick.0))));
    }

    #[test]
    fn outcome_sentences_are_plain() {
        let report = ExportReport { verified: true, tracks: 540, ..Default::default() };
        assert_eq!(finished("KINGSTON", &report), Outcome::Completed("Exported 540 tracks to KINGSTON.".into()));
        let report = ExportReport { verified: true, tracks: 538, skipped: vec!["a".into(), "b".into()], ..Default::default() };
        assert_eq!(finished("KINGSTON", &report), Outcome::Completed("Exported 538 tracks to KINGSTON, leaving out 2 whose files are missing.".into()));
        assert_eq!(unchanged("KINGSTON"), Outcome::Stopped("Stopped. KINGSTON is as it was before.".into()));
    }
}
