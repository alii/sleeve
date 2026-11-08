//! The app's state, and the changes handlers make to it.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, channel};

use rbl_index::Row;
use rune::{ImageCache, TextEdit, Tx, Waker};
use rune_kit::shell::PaneState;
use rune_kit::{SortDir, TableState};

use crate::jobs::{JobKind, Jobs};
use crate::library::{self, Browse, Loaded, PlaylistNode, Source};

pub enum LibState {
    Loading,
    Ready(Loaded),
    Failed(String),
}

/// Where a file drag from Finder would land.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropTarget {
    /// The window: into the library, and the playlist on screen if there is one.
    Library,
    /// The Collection row in the sidebar: into the library only.
    Collection,
    /// A playlist in the sidebar.
    Playlist(u64),
}

pub struct App {
    pub source: Source,
    pub lib: LibState,
    load_rx: Option<Receiver<Result<Loaded, library::LoadError>>>,
    /// A job changed the library while a reload was already running: read it once more after.
    reload_again: bool,
    pub waker: Option<Waker>,

    pub browse: Browse,
    /// The rows in view, in order. Rebuilt whenever the source, sort or search changes.
    pub rows: Vec<Row>,
    pub table: TableState,
    pub search: TextEdit,
    pub sort: (String, bool),

    pub tree: Vec<PlaylistNode>,
    pub collapsed: HashSet<u64>,
    pub playlists_closed: bool,

    pub sidebar: PaneState,
    pub inspector: PaneState,

    pub jobs: Jobs,
    pub player: crate::player::Player,
    pub deck_wave: Option<crate::deck::DeckWave>,
    /// The deck track's grid and zoomed waveform, coming from a worker.
    pub(crate) deck_grid_rx: Option<Receiver<crate::deck::DeckGrid>>,
    /// The rows whose files are only in iCloud, found by a worker after each load so the list
    /// never asks the disk while drawing. Empty until the first scan lands.
    pub evicted: HashSet<Row>,
    evicted_rx: Option<Receiver<(u64, HashSet<Row>)>>,
    /// Quantise: new hot cues and clicks on the deck waveform snap to the nearest beat.
    pub quantize: bool,
    /// Seconds of track the zoomed waveform shows across its width.
    pub zoom_secs: f64,
    /// The deck's glides for the current anchor, made by the view the first time it draws one.
    pub glides: std::cell::RefCell<Option<crate::ui::deck::Glides>>,
    /// What the deck's clock and bar counter last showed, so the view rebuilds only when they
    /// change rather than on every tick.
    shown: (u32, u32, i32, u16),
    /// Imports still running, with the tracks each has added so far. They're analysed when
    /// their import finishes.
    imports: Vec<(crate::jobs::JobId, crate::import::Added)>,
    /// Where a file drag from Finder would land right now, for the drop highlight.
    pub drop_over: Option<DropTarget>,
    /// The playlist being renamed in the sidebar, and the field holding its new name.
    pub renaming: Option<u64>,
    pub rename: TextEdit,
    /// A question on screen before something is deleted.
    pub confirm: Option<crate::edits::Confirm>,
    /// A playlist being made, and the id its job hands back.
    pub(crate) created: Option<(crate::jobs::JobId, crate::edits::Slot<String>)>,
    /// A playlist just made, to open and name once the library is read again.
    pub(crate) rename_next: Option<u64>,
    /// The iCloud download the deck is waiting on: its job, the track and where to start.
    pub fetch: Option<(crate::jobs::JobId, String, Option<f64>)>,
    /// The job whose log is open in the Activity panel.
    pub log_open: Option<crate::jobs::JobId>,
    /// The Activity panel above the footer, listing every job.
    pub activity_open: bool,
    /// USB drives plugged in, newest reading.
    pub sticks: Vec<crate::export::Stick>,
    sticks_rx: Option<Receiver<Vec<crate::export::Stick>>>,
    mounts: Option<(rbl_devices::mounts::MountWatcher, Receiver<()>)>,
    /// Which drive each export job is writing to, so a drive can't be given two at once.
    pub export_targets: std::collections::HashMap<crate::jobs::JobId, std::path::PathBuf>,
    pub images: Option<ImageCache>,
    pub artwork: Option<(Row, rune::Image)>,
    /// The focused track's waveform, for the library generation it was read from.
    pub wave: Option<(Row, u64, rune::Pixels)>,
    pub clock: f64,
}

pub fn source_from_args() -> Source {
    let args = rune::extra_args();
    let forced = std::env::var("SLEEVE_LIBRARY").ok();
    if forced.as_deref() == Some("installed") {
        return Source::Installed;
    }
    if args.iter().any(|a| a == "demo") || rune::tooling::headless_run().is_some() || forced.as_deref() == Some("demo") {
        let dir = std::env::temp_dir().join(format!("sleeve-demo-{}", std::process::id()));
        Source::Fixture(dir)
    } else {
        Source::Installed
    }
}

impl App {
    pub fn new() -> App {
        let mut table = TableState::new().key("tracks").multi_select(true);
        table.set_sort("title", SortDir::Ascending);
        let mut app = App {
            source: source_from_args(),
            lib: LibState::Loading,
            load_rx: None,
            reload_again: false,
            waker: None,
            browse: Browse::Collection,
            rows: Vec::new(),
            table,
            search: TextEdit::new(""),
            sort: ("title".into(), false),
            tree: Vec::new(),
            collapsed: HashSet::new(),
            playlists_closed: false,
            sidebar: PaneState::new(240.0).range(180.0, 420.0),
            inspector: PaneState::new(300.0).range(260.0, 440.0),
            jobs: Jobs::default(),
            player: Default::default(),
            deck_wave: None,
            quantize: true,
            zoom_secs: 4.0,
            glides: std::cell::RefCell::new(None),
            deck_grid_rx: None,
            evicted: HashSet::new(),
            evicted_rx: None,
            imports: Vec::new(),
            drop_over: None,
            renaming: None,
            rename: TextEdit::new(""),
            confirm: None,
            created: None,
            rename_next: None,
            shown: (0, 0, 0, 0),
            fetch: None,
            log_open: None,
            activity_open: false,
            sticks: Vec::new(),
            sticks_rx: None,
            mounts: None,
            export_targets: Default::default(),
            images: None,
            artwork: None,
            wave: None,
            clock: 0.0,
        };
        // Headless runs (snapshots, scripted tests) load at once so the first frame has data.
        if rune::tooling::headless_run().is_some() {
            // Images decode on Rune's worker; headless frames wait for them below.
            app.images = Some(ImageCache::new(std::sync::Arc::new(|| {})));
            let loaded = library::load(&app.source);
            app.land(loaded);
            app.sticks = crate::export::sticks();
        }
        app
    }

    pub fn setup(&mut self, waker: Waker) {
        self.jobs.set_waker(waker.clone());
        if rune::tooling::headless_run().is_none()
            && let Some(path) = crate::jobs::log_path()
        {
            self.jobs.log_to(&path);
        }
        self.images = Some(ImageCache::new(waker.clone()));
        self.player.open(Some(waker.clone()));
        self.waker = Some(waker);
        if matches!(self.lib, LibState::Loading) {
            self.start_load();
        }
        if rune::tooling::headless_run().is_none() {
            let (tx, rx) = channel();
            let w = self.waker.clone();
            let watcher = rbl_devices::mounts::MountWatcher::start(rbl_devices::mounts::INTERVAL, move || {
                let _ = tx.send(());
                if let Some(w) = &w {
                    w();
                }
            });
            self.mounts = Some((watcher, rx));
            self.refresh_sticks();
        }
    }

    fn start_load(&mut self) {
        let source = self.source.clone();
        self.spawn_load(move || library::load(&source));
    }

    /// Reads the library again after a job changed it. The list keeps its place, sort, search
    /// and selection; only the values in it change.
    pub fn start_reload(&mut self) {
        if self.load_rx.is_some() {
            self.reload_again = true;
            return;
        }
        let Some(l) = self.loaded() else { return };
        let (location, generation) = (l.location.clone(), l.generation + 1);
        self.spawn_load(move || library::reload(location, generation));
    }

    /// Reads the plugged-in drives again on a worker. Each one is inspected once, not polled.
    pub fn refresh_sticks(&mut self) {
        let (tx, rx) = channel();
        let w = self.waker.clone();
        std::thread::spawn(move || {
            let _ = tx.send(crate::export::sticks());
            if let Some(w) = w {
                w();
            }
        });
        self.sticks_rx = Some(rx);
    }

    pub fn exporting_to(&self, mount: &std::path::Path) -> bool {
        self.export_targets.iter().any(|(id, m)| m == mount && self.jobs.get(*id).is_some_and(|j| j.is_active()))
    }

    /// Exports these playlists to a drive, as a job of its own.
    pub fn export(&mut self, playlists: Vec<u64>, mount: std::path::PathBuf) {
        if playlists.is_empty() || self.exporting_to(&mount) {
            return;
        }
        let Some(l) = self.loaded() else { return };
        let (store, index) = (l.store.clone(), l.index.clone());
        let name = self.sticks.iter().find(|s| s.mount == mount).map_or_else(|| mount.display().to_string(), |s| s.name.clone());
        let target = mount.clone();
        let id = self.jobs.spawn(JobKind::Export, format!("Exporting to {name}"), move |rep| crate::export::run(&store, index, playlists, target, rep));
        self.export_targets.insert(id, mount);
    }

    fn spawn_load(&mut self, work: impl FnOnce() -> Result<Loaded, library::LoadError> + Send + 'static) {
        let (tx, rx) = channel();
        let waker = self.waker.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work());
            if let Some(w) = waker {
                w();
            }
        });
        self.load_rx = Some(rx);
    }

    pub fn poll(&mut self, clock: f64, tx: &mut Tx) -> bool {
        self.clock = clock;
        let mut changed = false;
        if let Some(rx) = &self.load_rx
            && let Ok(result) = rx.try_recv()
        {
            self.load_rx = None;
            match (result, &self.lib) {
                // A reload that fails keeps the library on screen; the next one may work.
                (Err(_), LibState::Ready(_)) => {}
                (result, _) => self.land(result),
            }
            if std::mem::take(&mut self.reload_again) {
                self.start_reload();
            }
            changed = true;
            // For timing the deck by hand: `SLEEVE_AUTOPLAY=1` plays the first analysed local
            // track once the library is in.
            if std::env::var_os("SLEEVE_AUTOPLAY").is_some() && self.player.track.is_none() {
                let pick = self.loaded().and_then(|l| {
                    (0..self.rows.len()).find(|&i| {
                        let r = self.rows[i];
                        library::fields(&l.index, r).bpm_x100 > 0 && !crate::icloud::is_evicted(std::path::Path::new(l.index.folder_path.get(r as usize)))
                    })
                });
                if let Some(i) = pick {
                    let id = self.track_id(i);
                    self.play_track(&id, Some(0.3), tx);
                }
            }
        }
        changed |= self.player.tick();
        changed |= self.land_deck_grid();
        if let Some(rx) = &self.evicted_rx
            && let Ok((generation, rows)) = rx.try_recv()
        {
            self.evicted_rx = None;
            if self.loaded().is_some_and(|l| l.generation == generation) {
                self.evicted = rows;
                changed = true;
            }
        }
        if self.player.playing {
            // The waveforms glide by themselves; only the readouts need the view, a few times a
            // second.
            let shown = self.deck_readout();
            changed |= shown != self.shown;
            self.shown = shown;
            tx.wake_after(std::time::Duration::from_millis(33));
        }
        if let Some((_, rx)) = &self.mounts
            && rx.try_iter().count() > 0
        {
            self.refresh_sticks();
        }
        if let Some(rx) = &self.sticks_rx
            && let Ok(sticks) = rx.try_recv()
        {
            self.sticks = sticks;
            self.sticks_rx = None;
            changed = true;
        }
        let (finished, moved) = self.jobs.drain(clock);
        changed |= moved;
        if let Some((job, id, at)) = self.fetch.clone()
            && finished.iter().any(|(f, _)| *f == job)
        {
            // Still in `fetch`, so nothing else was asked for since: this track is still wanted.
            self.fetch = None;
            let shown = self.player.fetching && self.player.is_on(&id);
            if shown {
                self.player.fetching = false;
            }
            match self.jobs.get(job).map(|j| j.phase.clone()) {
                Some(crate::jobs::Phase::Done { outcome: crate::jobs::Outcome::Completed(_), .. }) => {
                    if let Some(row) = self.loaded().and_then(|l| l.index.row_of(&id)) {
                        self.evicted.remove(&row);
                    }
                    self.play_track(&id, at, tx);
                }
                // The job's chip carries the error either way; the deck shows it too if it was
                // showing this track.
                Some(crate::jobs::Phase::Done { outcome: crate::jobs::Outcome::Failed(why), .. }) if shown => self.player.error = Some(why),
                Some(crate::jobs::Phase::Done { outcome: crate::jobs::Outcome::Failed(_) | crate::jobs::Outcome::Stopped(_), .. } | crate::jobs::Phase::Running | crate::jobs::Phase::Stopping) | None => {}
            }
        }
        self.take_created(&finished);
        if finished.iter().any(|(_, kind)| matches!(kind, JobKind::Export | JobKind::Eject)) {
            self.refresh_sticks();
        }
        let live: Vec<_> = self.jobs.list().iter().map(|j| j.id).collect();
        self.export_targets.retain(|id, _| live.contains(id));
        // Every song an import added gets analysed straight away, in a job of its own.
        let imported: Vec<_> = self.imports.extract_if(.., |(job, _)| finished.iter().any(|(f, _)| f == job)).collect();
        for (job, added) in imported {
            let tracks = std::mem::take(&mut *added.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
            let completed = matches!(self.jobs.get(job).map(|j| &j.phase), Some(crate::jobs::Phase::Done { outcome: crate::jobs::Outcome::Completed(_), .. }));
            if completed && !tracks.is_empty() {
                let title = if tracks.len() == 1 { format!("Analysing {}", tracks[0].title) } else { format!("Analysing {} new songs", tracks.len()) };
                self.spawn_analysis(title, tracks);
            }
        }
        if finished.iter().any(|(_, kind)| matches!(kind, JobKind::Analysis | JobKind::Edit | JobKind::Import)) {
            self.start_reload();
        }
        // Keep "running 0:42" in the Activity panel ticking.
        if self.activity_open && self.jobs.list().iter().any(crate::jobs::Job::is_active) {
            tx.wake_after(std::time::Duration::from_secs(1));
        }
        changed
    }

    /// Finds which tracks are only in iCloud, on a worker. A slow or stuck disk (a network
    /// share gone quiet) holds up only the worker.
    fn scan_evicted(&mut self) {
        let Some(l) = self.loaded() else { return };
        let (index, generation, waker) = (l.index.clone(), l.generation, self.waker.clone());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let rows: HashSet<Row> = (0..index.len()).filter(|&i| crate::icloud::is_evicted(std::path::Path::new(index.folder_path.get(i)))).map(|i| i as Row).collect();
            if tx.send((generation, rows)).is_ok()
                && let Some(w) = waker
            {
                w();
            }
        });
        self.evicted_rx = Some(rx);
    }

    /// The deck's clock (elapsed and left, in seconds) and bar.beat, as shown now.
    fn deck_readout(&self) -> (u32, u32, i32, u16) {
        let total = self.deck_duration_ms().unwrap_or(0.0);
        let at = self.player.now() * total;
        let grid = crate::deck::grid_pos(self.deck_beats(), at).map_or((0, 0), |g| (g.bar, g.beat));
        ((at / 1000.0) as u32, ((total - at) / 1000.0).max(0.0) as u32, grid.0, grid.1)
    }

    fn land(&mut self, result: Result<Loaded, library::LoadError>) {
        self.lib = match result {
            Ok(l) => LibState::Ready(l),
            Err(e) => LibState::Failed(e.to_string()),
        };
        self.refresh_tree();
        self.scan_evicted();
        self.land_created();
        self.refresh_rows();
        self.sync_artwork();
        self.sync_deck_wave();
    }

    pub fn loaded(&self) -> Option<&Loaded> {
        match &self.lib {
            LibState::Ready(l) => Some(l),
            _ => None,
        }
    }

    pub fn refresh_tree(&mut self) {
        self.tree = match &self.lib {
            LibState::Ready(l) => library::playlist_tree(&l.index, &self.collapsed),
            _ => Vec::new(),
        };
    }

    /// Rebuilds the rows in view. Keeps the selection for tracks that are still there.
    pub fn refresh_rows(&mut self) {
        let Some(l) = self.loaded() else {
            self.rows.clear();
            return;
        };
        let view = library::open_view(&l.index, self.browse, &self.sort.0, self.sort.1, self.search.text.trim());
        self.rows = view.window(0, view.len()).to_vec();
    }

    pub fn browse_to(&mut self, b: Browse, tx: &mut Tx) {
        if self.browse == b {
            return;
        }
        self.browse = b;
        self.table.clear_selection();
        self.refresh_rows();
        // A new list starts at its top.
        tx.scroll_to_instant("tracks", 0.0, 0.0);
    }

    pub fn set_sort(&mut self, col: &str, dir: SortDir) {
        self.sort = (col.to_owned(), dir == SortDir::Descending);
        self.table.set_sort(col, dir);
        self.refresh_rows();
    }

    pub fn track_id(&self, i: usize) -> String {
        match (self.loaded(), self.rows.get(i)) {
            (Some(l), Some(&r)) => l.index.ids[r as usize].to_string(),
            _ => String::new(),
        }
    }

    /// The row of the track the cursor is on.
    pub fn focused_row(&self) -> Option<Row> {
        let l = self.loaded()?;
        let id = self.table.cursor()?;
        l.index.row_of(id)
    }

    pub fn selected_rows(&self) -> Vec<Row> {
        let Some(l) = self.loaded() else { return Vec::new() };
        let mut rows: Vec<Row> = self.table.selected().filter_map(|id| l.index.row_of(id)).collect();
        // Keep the list's order, not the order they were clicked.
        let pos: std::collections::HashMap<Row, usize> = self.rows.iter().enumerate().map(|(i, &r)| (r, i)).collect();
        rows.sort_by_key(|r| pos.get(r).copied().unwrap_or(usize::MAX));
        rows
    }

    /// Loads the artwork for the focused track, if it changed.
    pub fn sync_artwork(&mut self) {
        self.sync_wave();
        let row = self.focused_row();
        if self.artwork.as_ref().map(|a| a.0) == row {
            return;
        }
        self.artwork = row.and_then(|r| self.artwork_for(r, 268.0).map(|i| (r, i)));
    }

    /// A track's cover, read on the image worker, drawn at `size` points.
    pub fn artwork_for(&self, row: Row, size: f32) -> Option<rune::Image> {
        let (l, cache) = (self.loaded()?, self.images.as_ref()?);
        let rel = l.index.artwork_path.get(row as usize);
        if rel.is_empty() || rel.contains("..") {
            return None;
        }
        let mut path = l.location.share_root.clone();
        path.extend(rel.split(['/', '\\']).filter(|p| !p.is_empty()));
        let image = cache.file(&path, size, 2.0);
        if rune::tooling::headless_run().is_some() {
            // A snapshot shows the cover, not the moment before it decoded.
            let _ = image.wait();
        }
        Some(image)
    }

    /// What Analyse would act on: the selection, re-analysed on purpose, or with nothing
    /// selected only the tracks in view that have never been analysed. A click with nothing
    /// selected must never redo grids someone fixed by hand.
    pub fn analyse_rows(&self) -> Vec<Row> {
        let sel = self.selected_rows();
        if !sel.is_empty() {
            return sel;
        }
        let Some(l) = self.loaded() else { return Vec::new() };
        self.rows.iter().copied().filter(|&r| l.index.analysis_path.get(r as usize).is_empty()).collect()
    }

    /// Starts a job that analyses `analyse_rows`. Each press makes its own job with its own stop
    /// button.
    /// Clicking a job's chip: opens the Activity panel, with the log showing if the job failed
    /// or logged a problem. Clicking it again closes the panel.
    pub fn open_job(&mut self, id: crate::jobs::JobId) {
        if self.activity_open && (self.log_open == Some(id) || self.log_open.is_none()) {
            self.activity_open = false;
            return;
        }
        self.activity_open = true;
        let troubled = self.jobs.get(id).is_some_and(|j| {
            matches!(j.phase, crate::jobs::Phase::Done { outcome: crate::jobs::Outcome::Failed(_), .. }) || j.log.iter().any(|l| l.level == crate::jobs::Level::Error)
        });
        if troubled {
            self.log_open = Some(id);
        }
    }

    fn spawn_analysis(&mut self, title: String, tracks: Vec<crate::analysis::Track>) {
        let Some(l) = self.loaded() else { return };
        let store = l.store.clone();
        self.jobs.spawn(JobKind::Analysis, title, move |rep| crate::analysis::run(&store, tracks, rep));
    }

    /// Adds songs dropped from Finder to the library, and to `playlist` if it's given and is a
    /// plain playlist. New songs are analysed when the import is done.
    pub fn import(&mut self, files: Vec<std::path::PathBuf>, playlist: Option<u64>) {
        self.drop_over = None;
        let Some(l) = self.loaded() else { return };
        let store = l.store.clone();
        let target = playlist
            .and_then(|id| self.tree.iter().find(|n| n.id == id && n.kind == crate::library::PlaylistKind::Playlist))
            .map(|n| crate::import::Target { id: n.id, name: n.name.clone() });
        let added = crate::import::Added::default();
        let out = added.clone();
        let title = target.as_ref().map_or_else(|| "Adding songs".to_owned(), |t| format!("Adding songs to {}", t.name));
        let job = self.jobs.spawn(JobKind::Import, title, move |rep| crate::import::run(&store, &files, target.as_ref(), &out, rep));
        self.imports.push((job, added));
    }

    /// The playlist a drop on the track list goes into: the one on screen, if it's a plain
    /// playlist rather than a folder or a smart one.
    pub fn drop_playlist(&self) -> Option<u64> {
        match self.browse {
            Browse::Playlist(id) => self.tree.iter().find(|n| n.id == id && n.kind == crate::library::PlaylistKind::Playlist).map(|n| n.id),
            Browse::Collection => None,
        }
    }

    pub fn analyse(&mut self) {
        let Some(l) = self.loaded() else { return };
        let rows = self.analyse_rows();
        if rows.is_empty() {
            return;
        }
        let tracks = crate::analysis::tracks(&l.index, &rows);
        let title = if tracks.len() == 1 { format!("Analysing {}", tracks[0].title) } else { format!("Analysing {} tracks", tracks.len()) };
        self.spawn_analysis(title, tracks);
    }

    fn sync_wave(&mut self) {
        let row = self.focused_row();
        let Some(l) = self.loaded() else {
            self.wave = None;
            return;
        };
        let generation = l.generation;
        if self.wave.as_ref().map(|w| (w.0, w.1)) == row.map(|r| (r, generation)) {
            return;
        }
        let Some(row) = row else {
            self.wave = None;
            return;
        };
        let pixels = rune::Pixels::new();
        let rel = l.index.analysis_path.get(row as usize).to_owned();
        crate::waveform::load_into(pixels.clone(), l.location.share_root.clone(), rel, crate::ui::inspector::WAVE_PX.0, crate::ui::inspector::WAVE_PX.1);
        self.wave = Some((row, generation, pixels));
    }

    /// Plays or pauses the track in the details panel. `at` (0 to 1) starts it from there.
    pub fn play_focused(&mut self, at: Option<f64>, tx: &mut Tx) {
        if let Some(id) = self.table.cursor().map(str::to_owned) {
            self.play_track(&id, at, tx);
        }
    }

    /// Analyses just the track in the details panel, whatever else is selected.
    pub fn analyse_focused(&mut self) {
        let Some(row) = self.focused_row() else { return };
        let Some(l) = self.loaded() else { return };
        let tracks = crate::analysis::tracks(&l.index, &[row]);
        let title = format!("Analysing {}", tracks[0].title);
        self.spawn_analysis(title, tracks);
    }

    pub fn summary(&self) -> String {
        match &self.lib {
            LibState::Loading => "Opening your library…".into(),
            LibState::Failed(e) => e.clone(),
            LibState::Ready(l) => {
                let sel = self.selected_rows();
                let secs = |rows: &[Row]| rows.iter().map(|&r| u64::from(l.index.length_sec[r as usize])).sum::<u64>();
                if sel.len() > 1 {
                    format!("{} of {} selected · {}", crate::ui::sidebar::thousands(sel.len()), crate::ui::sidebar::thousands(self.rows.len()), library::long_duration(secs(&sel)))
                } else {
                    let n = self.rows.len();
                    let noun = if n == 1 { "track" } else { "tracks" };
                    format!("{} {noun} · {}", crate::ui::sidebar::thousands(n), library::long_duration(secs(&self.rows)))
                }
            }
        }
    }
}
