//! Every write to the library goes through here.
//!
//! A writer is opened for each change and dropped straight after, so rekordbox can take the
//! file back between changes. Writes from different jobs take turns on one lock; the jobs
//! themselves don't wait on each other for anything else.

use std::path::PathBuf;
use std::sync::Mutex;

use rbl_db::LibraryLocation;
use rbl_db::write::Writer;

/// Why a write didn't happen.
#[derive(Debug)]
pub enum WriteError {
    /// The library can't be written right now (rekordbox is open, or this is a test run).
    /// Every later write will be refused too, so a job should stop rather than try the next item.
    Refused(String),
    /// This one change failed. Others may still work.
    Failed(rbl_db::DbError),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Refused(why) => write!(f, "{why}"),
            WriteError::Failed(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WriteError {}

impl From<rbl_db::DbError> for WriteError {
    fn from(e: rbl_db::DbError) -> Self {
        match e {
            rbl_db::DbError::WriteRefused(why) => WriteError::Refused(why),
            other => WriteError::Failed(other),
        }
    }
}

pub struct Store {
    pub location: LibraryLocation,
    backup_dir: PathBuf,
    /// Whether this session has already copied `master.db` aside. Held while writing.
    backed_up: Mutex<bool>,
}

impl Store {
    pub fn new(location: LibraryLocation) -> Store {
        // A demo library's backups go beside it, never among the real library's.
        let dir = if location.is_real_install {
            dirs_home().join("Library/Application Support/Sleeve/Backups")
        } else {
            location.share_root.with_file_name("sleeve-backups")
        };
        Store::new_in(location, dir)
    }

    pub fn new_in(location: LibraryLocation, backup_dir: PathBuf) -> Store {
        Store { location, backup_dir, backed_up: Mutex::new(false) }
    }

    /// Runs one change. The first change of the session copies the library aside first.
    pub fn write<T>(&self, change: impl FnOnce(&mut Writer) -> rbl_db::Result<T>) -> Result<T, WriteError> {
        let mut backed_up = self.backed_up.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut writer = Writer::open(self.location.clone(), &self.backup_dir)?;
        if *backed_up {
            writer.mark_backed_up();
        }
        let result = change(&mut writer).map_err(WriteError::from);
        *backed_up |= writer.backed_up();
        result
    }

    /// Why writing would be refused right now, in words for a person.
    pub fn refusal(&self) -> Option<&'static str> {
        (self.location.is_real_install && rbl_db::is_rekordbox_running()).then_some("rekordbox is open. Quit it first, so the two apps don't change your library at the same time.")
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(std::env::temp_dir, PathBuf::from)
}
