//! Changes to the library you make by hand: playlists and folders made, renamed and deleted,
//! songs put in and taken out, and USB sticks ejected.
//!
//! Each change runs as a job, as everything that touches the disk does: it shows its status,
//! can be stopped before it writes, and logs what it did. The library is read again after.

use std::sync::{Arc, Mutex};

use rbl_db::write::{ROOT, Writer};
use rune::TextEdit;

use crate::app::App;
use crate::jobs::{JobId, JobKind, Outcome};
use crate::library::{Browse, PlaylistKind};
use crate::store::WriteError;

/// What a change hands back once it's done, for the app to pick up.
pub type Slot<T> = Arc<Mutex<Option<T>>>;

/// A question waiting for an answer before something is deleted.
#[derive(Clone, Debug, PartialEq)]
pub enum Confirm {
    DeletePlaylist(u64),
    /// Take these tracks out of the library. Their files stay where they are.
    RemoveTracks(Vec<String>),
}

impl App {
    /// Runs one change to the library as a job. `doing` names it for its chip ("Renaming
    /// Warm up"), `done` for when it's finished.
    pub(crate) fn change<T: Send + 'static>(
        &mut self,
        doing: String,
        done: String,
        change: impl FnOnce(&mut Writer) -> rbl_db::Result<T> + Send + 'static,
    ) -> Option<(JobId, Slot<T>)> {
        let l = self.loaded()?;
        let store = l.store.clone();
        let slot: Slot<T> = Arc::default();
        let out = slot.clone();
        let job = self.jobs.spawn(JobKind::Edit, doing.clone(), move |rep| {
            rep.info(format!("{doing}. Library: {}", store.location.share_root.display()));
            if rep.is_cancelled() {
                return Outcome::Stopped("Stopped before changing anything".into());
            }
            if let Some(why) = store.refusal() {
                return Outcome::Failed(why.into());
            }
            match store.write(change) {
                Ok(v) => {
                    *out.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(v);
                    rep.info(format!("{done}."));
                    Outcome::Completed(done)
                }
                Err(WriteError::Refused(why)) => {
                    rep.warn(format!("The library refused the change: {why}"));
                    Outcome::Failed(why)
                }
                Err(WriteError::Failed(e)) => {
                    rep.error(format!("Database write failed: {e} ({e:?})"));
                    Outcome::Failed(format!("Couldn't finish: {e}"))
                }
            }
        });
        Some((job, slot))
    }

    fn node_name(&self, id: u64) -> String {
        self.tree.iter().find(|n| n.id == id).map_or_else(|| "the playlist".to_owned(), |n| n.name.clone())
    }

    fn is_plain_playlist(&self, id: u64) -> bool {
        self.tree.iter().any(|n| n.id == id && n.kind == PlaylistKind::Playlist)
    }

    /// The selected tracks' ids, in list order.
    pub fn selected_ids(&self) -> Vec<String> {
        let Some(l) = self.loaded() else { return Vec::new() };
        self.selected_rows().into_iter().map(|r| l.index.ids[r as usize].to_string()).collect()
    }

    /// Makes a playlist (or a folder) inside `parent`, or at the top, with `tracks` in it, and
    /// then lets you name it in place.
    pub fn new_playlist(&mut self, parent: Option<u64>, folder: bool, tracks: Vec<String>) {
        let name = if folder { "New folder" } else { "New playlist" }.to_owned();
        let under = parent.map_or_else(|| ROOT.to_owned(), |p| p.to_string());
        let n = tracks.len();
        let doing = match (folder, n) {
            (true, _) => "Making a folder".to_owned(),
            (false, 0) => "Making a playlist".to_owned(),
            (false, 1) => "Making a playlist with 1 song".to_owned(),
            (false, n) => format!("Making a playlist with {n} songs"),
        };
        let done = if folder { "Made a folder" } else { "Made a playlist" }.to_owned();
        let made = self.change(doing, done, move |w| {
            let id = if folder { w.create_folder(&name, &under)? } else { w.create_playlist(&name, &under)? };
            if !tracks.is_empty() {
                w.add_tracks(&id, &tracks)?;
            }
            Ok(id)
        });
        if let Some(p) = parent {
            self.collapsed.remove(&p);
        }
        self.created = made;
    }

    /// Called once a new playlist's job is done: open it and start naming it when the library
    /// has been read again.
    pub(crate) fn take_created(&mut self, finished: &[(JobId, JobKind)]) {
        let Some((job, slot)) = &self.created else { return };
        if !finished.iter().any(|(f, _)| f == job) {
            return;
        }
        let id = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        self.created = None;
        self.rename_next = id.and_then(|id| id.parse().ok());
    }

    /// After a reload: a playlist just made is shown and its name is ready to type over.
    pub(crate) fn land_created(&mut self) {
        if let Some(id) = self.rename_next.take()
            && self.tree.iter().any(|n| n.id == id)
        {
            self.browse = Browse::Playlist(id);
            self.table.clear_selection();
            self.start_rename(id);
        }
    }

    pub fn start_rename(&mut self, id: u64) {
        let name = self.node_name(id);
        let mut edit = TextEdit::new(name.clone());
        edit.anchor = 0;
        edit.focus = name.len();
        edit.focused = true;
        self.rename = edit;
        self.renaming = Some(id);
    }

    pub fn commit_rename(&mut self) {
        let Some(id) = self.renaming.take() else { return };
        self.rename.focused = false;
        let name = self.rename.text.trim().to_owned();
        let old = self.node_name(id);
        if name.is_empty() || name == old {
            return;
        }
        // Shown straight away; the reload after the job confirms it.
        if let Some(n) = self.tree.iter_mut().find(|n| n.id == id) {
            n.name.clone_from(&name);
        }
        let (doing, done) = (format!("Renaming \"{old}\" to \"{name}\""), format!("Renamed \"{old}\" to \"{name}\""));
        self.change(doing, done, move |w| w.rename(&id.to_string(), &name).map(|_| ()));
    }

    pub fn cancel_rename(&mut self) {
        self.renaming = None;
        self.rename.focused = false;
    }

    pub fn delete_playlist(&mut self, id: u64) {
        self.confirm = None;
        let name = self.node_name(id);
        if self.browse == Browse::Playlist(id) {
            self.browse = Browse::Collection;
            self.table.clear_selection();
            self.refresh_rows();
        }
        self.change(format!("Deleting \"{name}\""), format!("Deleted \"{name}\""), move |w| w.delete_playlist(&id.to_string()).map(|_| ()));
    }

    pub fn add_to_playlist(&mut self, id: u64, tracks: Vec<String>) {
        if tracks.is_empty() || !self.is_plain_playlist(id) {
            return;
        }
        let name = self.node_name(id);
        let n = songs(tracks.len());
        self.change(format!("Adding {n} to \"{name}\""), format!("Added {n} to \"{name}\""), move |w| w.add_tracks(&id.to_string(), &tracks).map(|_| ()));
    }

    /// Takes the selected tracks out of the playlist on screen. They stay in the library.
    pub fn remove_from_playlist(&mut self) {
        let Browse::Playlist(id) = self.browse else { return };
        let tracks = self.selected_ids();
        if tracks.is_empty() || !self.is_plain_playlist(id) {
            return;
        }
        let name = self.node_name(id);
        let n = songs(tracks.len());
        self.table.clear_selection();
        self.change(format!("Taking {n} out of \"{name}\""), format!("Took {n} out of \"{name}\""), move |w| w.remove_tracks(&id.to_string(), &tracks).map(|_| ()));
    }

    /// Takes tracks out of the library and every playlist. Their files stay on disk.
    pub fn remove_from_library(&mut self, tracks: Vec<String>) {
        self.confirm = None;
        if tracks.is_empty() {
            return;
        }
        if tracks.iter().any(|t| self.player.is_on(t)) {
            self.close_deck();
        }
        let n = songs(tracks.len());
        self.table.clear_selection();
        self.change(format!("Removing {n} from your library"), format!("Removed {n} from your library"), move |w| w.delete_tracks(&tracks).map(|_| ()));
    }

    /// Shows the focused track's file in Finder.
    pub fn show_in_finder(&self) {
        let Some(l) = self.loaded() else { return };
        let Some(row) = self.focused_row() else { return };
        let path = l.index.folder_path.get(row as usize).to_owned();
        let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    }

    /// Ejects a USB stick, as a job: it can take a moment, and it fails if something has a file
    /// open on the stick.
    pub fn eject(&mut self, mount: std::path::PathBuf) {
        let name = self.sticks.iter().find(|s| s.mount == mount).map_or_else(|| mount.display().to_string(), |s| s.name.clone());
        self.jobs.spawn(JobKind::Eject, format!("Ejecting {name}"), move |rep| {
            rep.info(format!("Ejecting {}", mount.display()));
            // macOS can sit on an eject for a long time. The job waits on a worker, so its stop
            // button works and it gives up rather than hanging.
            let (tx, rx) = std::sync::mpsc::channel();
            let target = mount.clone();
            std::thread::spawn(move || {
                let _ = tx.send(rbl_devices::eject::eject(&target));
            });
            let started = std::time::Instant::now();
            loop {
                match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(Ok(())) => return Outcome::Completed(format!("Ejected {name}. It's safe to unplug.")),
                    Ok(Err(e)) => {
                        rep.error(format!("Eject failed: {e} ({e:?})"));
                        return Outcome::Failed(format!("Couldn't eject {name}. Close anything using it and try again. ({e})"));
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Outcome::Failed(format!("Couldn't eject {name}")),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if rep.is_cancelled() {
                            rep.info("Stopped waiting. macOS may still eject it.");
                            return Outcome::Stopped(format!("Stopped waiting for {name} to eject"));
                        }
                        if started.elapsed() > EJECT_WAIT {
                            rep.error(format!("macOS hadn't ejected it after {}s.", EJECT_WAIT.as_secs()));
                            return Outcome::Failed(format!("{name} didn't eject. Something may be using it: close it and try again."));
                        }
                    }
                }
            }
        });
    }
}

/// How long an eject may take before the job gives up on it.
const EJECT_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

pub fn songs(n: usize) -> String {
    if n == 1 { "1 song".to_owned() } else { format!("{n} songs") }
}
