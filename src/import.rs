//! Adding songs from Finder. Files and folders dropped on the window go into the library, and
//! into a playlist when they're dropped on one. The app analyses the new tracks straight after.
//!
//! One job does the adding, in small batches, so its stop button takes effect between batches.
//! Each batch is one transaction: a batch is either all in or not in at all.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rbl_db::import::{WalkBudget, audio_files_in, is_audio};
use rbl_db::write::ImportOutcome;

use crate::analysis::Track;
use crate::edits::songs;
use crate::jobs::{Outcome, Reporter};
use crate::store::{Store, WriteError};

/// Files per transaction. Small enough that a stop lands quickly.
const BATCH: usize = 25;

/// Whether a drag carrying this path is worth taking: a song, or a folder that may hold some.
pub fn accepts(path: &Path) -> bool {
    path.is_dir() || is_audio(path)
}

/// A playlist to add the songs to as well, by id and name.
#[derive(Clone)]
pub struct Target {
    pub id: u64,
    pub name: String,
}

/// The job's work. The tracks it adds land in `added`, for the app to analyse once it's done.
pub fn run(store: &Store, dropped: &[PathBuf], into: Option<&Target>, added: &Mutex<Vec<Track>>, rep: &mut Reporter) -> Outcome {
    if let Some(why) = store.refusal() {
        return Outcome::Failed(why.into());
    }
    for p in dropped {
        rep.info(format!("Dropped: {}", p.display()));
    }

    // Folders are walked for songs; anything else that isn't a song is left out.
    rep.retitle("Looking for songs");
    let mut budget = WalkBudget::new(16, 500_000);
    let mut files = Vec::new();
    let mut ignored = 0usize;
    for p in dropped {
        if rep.is_cancelled() {
            return Outcome::Stopped("Stopped before adding anything".into());
        }
        if p.is_dir() {
            let found = audio_files_in(p, &mut budget);
            rep.info(format!("{} in {}", songs(found.len()), p.display()));
            files.extend(found);
        } else if is_audio(p) {
            files.push(p.clone());
        } else {
            ignored += 1;
        }
    }
    if budget.exhausted() {
        rep.warn("Some folders were too deep or too big to look through completely, so a few songs may have been missed.");
    }
    if ignored > 0 {
        rep.info(format!("Left out {ignored} files that aren't songs."));
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        return Outcome::Failed("None of those are songs Sleeve can add. It takes MP3, AAC, WAV, AIFF and FLAC.".into());
    }

    let total = files.len() as u64;
    rep.retitle(match into {
        Some(pl) => format!("Adding {} to {}", songs(files.len()), pl.name),
        None => format!("Adding {}", songs(files.len())),
    });
    rep.progress(0, Some(total), "");
    let (mut new, mut existing, mut skipped) = (0usize, 0usize, 0usize);
    let mut ids = Vec::new();
    for (n, batch) in files.chunks(BATCH).enumerate() {
        if rep.is_cancelled() {
            break;
        }
        let outcomes = match store.write(|w| w.import_files(batch)) {
            Ok(o) => o,
            Err(WriteError::Refused(why)) => {
                rep.warn(format!("The library refused the change: {why}"));
                return Outcome::Failed(if new == 0 { why } else { format!("Stopped after adding {}: {why}", songs(new)) });
            }
            Err(WriteError::Failed(e)) => {
                rep.error(format!("Writing a batch of {} failed, so none of that batch was added: {e} ({e:?})", songs(batch.len())));
                return Outcome::Failed(format!("Couldn't add songs to the library: {e}"));
            }
        };
        for (path, outcome) in batch.iter().zip(outcomes) {
            let name = path.file_stem().map_or_else(|| path.display().to_string(), |s| s.to_string_lossy().into_owned());
            match outcome {
                ImportOutcome::Added(id) => {
                    new += 1;
                    rep.info(format!("Added \"{name}\". File: {}", path.display()));
                    ids.push(id.clone());
                    added.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(Track { id, title: name, path: path.display().to_string(), analysis_path: String::new() });
                }
                ImportOutcome::Existing(id) => {
                    existing += 1;
                    rep.info(format!("\"{name}\" is already in the library."));
                    ids.push(id);
                }
                ImportOutcome::Refused(why) => {
                    skipped += 1;
                    rep.warn(format!("Left out \"{name}\": {why}. File: {}", path.display()));
                }
            }
        }
        let done = ((n + 1) * BATCH).min(files.len());
        rep.progress(done as u64, Some(total), batch.last().and_then(|p| p.file_stem()).map_or_else(String::new, |s| s.to_string_lossy().into_owned()));
    }

    // The playlist gets every song that was dropped, old or new, so the drop does what it shows.
    let mut listed = 0;
    if let Some(pl) = into
        && !ids.is_empty()
    {
        match store.write(|w| w.add_tracks(&pl.id.to_string(), &ids)) {
            Ok(changed) => {
                listed = changed.rows;
                rep.info(format!("Put {} in \"{}\". The rest were already there.", songs(listed), pl.name));
            }
            Err(e) => {
                rep.error(format!("Couldn't add the songs to \"{}\": {e} ({e:?})", pl.name));
                return Outcome::Failed(format!("Added {} to the library, but couldn't put them in {}: {e}", songs(new), pl.name));
            }
        }
    }

    let mut parts = Vec::new();
    if new > 0 {
        parts.push(format!("Added {}", songs(new)));
    }
    if existing > 0 {
        parts.push(if new == 0 { format!("{} already in your library", songs(existing)) } else { format!("{existing} already in your library") });
    }
    if skipped > 0 {
        parts.push(format!("{skipped} couldn't be read"));
    }
    let msg = parts.join(", ");
    rep.info(format!("{msg}.{}", into.map_or_else(String::new, |pl| format!(" {} newly in \"{}\".", songs(listed), pl.name))));
    if rep.is_cancelled() {
        return Outcome::Stopped(format!("Stopped. {msg}. They stay in your library."));
    }
    if new == 0 && existing == 0 {
        return Outcome::Failed(format!("Couldn't add any of the {}", songs(files.len())));
    }
    Outcome::Completed(match into {
        Some(pl) if new == 0 && listed == 0 => format!("Already in {}", pl.name),
        Some(pl) if new == 0 => format!("Put {} in {}", songs(listed), pl.name),
        Some(pl) => format!("{msg}, in {}", pl.name),
        None => msg,
    })
}


/// The tracks an import added, shared between its job and the app.
pub type Added = Arc<Mutex<Vec<Track>>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::tests::{click_track, setup, wait};
    use crate::jobs::{JobKind, Jobs, Phase};

    fn in_playlist(store: &Store, playlist: &str) -> i64 {
        let db = rbl_db::Library::open(store.location.clone(), rbl_db::OpenMode::ReadOnly).unwrap();
        db.connection().query_row("SELECT COUNT(*) FROM djmdSongPlaylist WHERE PlaylistID = ?1 AND rb_local_deleted = 0", [playlist], |r| r.get(0)).unwrap()
    }

    fn import(store: &Arc<Store>, dropped: Vec<PathBuf>, into: Option<Target>) -> (Phase, Vec<Track>) {
        let mut jobs = Jobs::default();
        let (s, added) = (store.clone(), Added::default());
        let out = added.clone();
        let id = jobs.spawn(JobKind::Import, "i", move |rep| run(&s, &dropped, into.as_ref(), &out, rep));
        let phase = wait(&mut jobs, id);
        let tracks = std::mem::take(&mut *added.lock().unwrap());
        (phase, tracks)
    }

    #[test]
    fn a_dropped_folder_lands_in_the_library_and_the_playlist() {
        let (dir, store) = setup("import", 0);
        let drop = dir.join("New music");
        std::fs::create_dir_all(drop.join("Deeper")).unwrap();
        click_track(&drop.join("one.wav"));
        click_track(&drop.join("Deeper/two.wav"));
        std::fs::write(drop.join("cover.txt"), "not a song").unwrap();
        let single = dir.join("three.wav");
        click_track(&single);
        let playlist = rbl_db::fixture::playlist_id(0);
        let before = in_playlist(&store, &playlist);
        let into = Target { id: playlist.parse().unwrap(), name: "Warm up".into() };

        let (phase, added) = import(&store, vec![drop.clone(), single.clone()], Some(into.clone()));
        assert!(matches!(&phase, Phase::Done { outcome: Outcome::Completed(m), .. } if m == "Added 3 songs, in Warm up"), "{phase:?}");
        assert_eq!(added.len(), 3);
        assert!(added.iter().all(|t| t.analysis_path.is_empty()));
        assert_eq!(in_playlist(&store, &playlist), before + 3);

        // What the app does next: analyse the new songs, which have no analysis path yet.
        let mut jobs = Jobs::default();
        let s = store.clone();
        let id = jobs.spawn(JobKind::Analysis, "a", move |rep| crate::analysis::run(&s, added, rep));
        let phase = wait(&mut jobs, id);
        assert!(matches!(&phase, Phase::Done { outcome: Outcome::Completed(m), .. } if m == "Analysed 3 tracks"), "{phase:?}");

        // Dropping the same songs again adds nothing twice.
        let (phase, added) = import(&store, vec![drop, single], Some(into));
        assert!(matches!(&phase, Phase::Done { outcome: Outcome::Completed(m), .. } if m == "Already in Warm up"), "{phase:?}");
        assert!(added.is_empty());
        assert_eq!(in_playlist(&store, &playlist), before + 3);
    }

    #[test]
    fn a_drop_with_no_songs_says_so() {
        let (dir, store) = setup("import-none", 0);
        let note = dir.join("readme.txt");
        std::fs::write(&note, "hi").unwrap();
        let (phase, _) = import(&store, vec![note], None);
        assert!(matches!(&phase, Phase::Done { outcome: Outcome::Failed(m), .. } if m.starts_with("None of those are songs")), "{phase:?}");
    }
}
