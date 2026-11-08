//! Files kept in iCloud Drive with "Optimise Mac Storage" may not be on this Mac. Opening one
//! for reading makes macOS download it first, which can take a while, so it never happens on
//! the UI thread.

use std::path::Path;

/// `SF_DATALESS`: the file's contents live in the cloud, not on disk.
#[cfg(target_os = "macos")]
const SF_DATALESS: u32 = 0x4000_0000;

/// Whether this file is only in iCloud right now. Cheap: one `stat`, no download.
pub fn is_evicted(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::darwin::fs::MetadataExt;
        std::fs::symlink_metadata(path).is_ok_and(|m| m.st_flags() & SF_DATALESS != 0)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        false
    }
}

/// Lets this process pull files down from iCloud when it reads them.
///
/// macOS starts processes launched from a terminal with downloads turned off: reading an
/// iCloud-only file then fails with EDEADLK, "Resource deadlock avoided", instead of fetching
/// it. Apps opened from Finder usually have it on, but not always, so the app asks either way.
pub fn allow_downloads() -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: i32 = 3;
        const IOPOL_SCOPE_PROCESS: i32 = 0;
        const IOPOL_MATERIALIZE_DATALESS_FILES_ON: i32 = 2;
        unsafe extern "C" {
            fn setiopolicy_np(kind: i32, scope: i32, policy: i32) -> i32;
        }
        // SAFETY: plain integer arguments; it only changes this process's I/O policy.
        let r = unsafe { setiopolicy_np(IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, IOPOL_SCOPE_PROCESS, IOPOL_MATERIALIZE_DATALESS_FILES_ON) };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[derive(Debug)]
pub enum DownloadError {
    /// Reading the file failed, so macOS couldn't bring it down.
    Io(std::io::Error),
    /// The read returned but the file is still only in iCloud.
    StillInCloud,
    /// Stop was pressed. macOS may still finish the download in the background.
    Stopped,
}

impl DownloadError {
    /// The technical detail for a log: the system error's name and number.
    pub fn detail(&self) -> String {
        match self {
            DownloadError::Io(e) => match e.raw_os_error() {
                Some(n) => format!("{} (errno {n}, {:?})", e, e.kind()),
                None => format!("{e} ({:?})", e.kind()),
            },
            DownloadError::StillInCloud => "read returned, but SF_DATALESS is still set".into(),
            DownloadError::Stopped => "stopped".into(),
        }
    }
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Io(e) if e.raw_os_error() == Some(11) => write!(f, "macOS wouldn't let Sleeve download it from iCloud"),
            DownloadError::Io(e) if e.kind() == std::io::ErrorKind::NotFound => write!(f, "its file has gone from iCloud Drive"),
            DownloadError::Io(_) => write!(f, "couldn't download it from iCloud"),
            DownloadError::StillInCloud => write!(f, "iCloud didn't finish downloading it. Check you're online and try again."),
            DownloadError::Stopped => write!(f, "the download was stopped"),
        }
    }
}

impl std::error::Error for DownloadError {}

/// Brings the file down from iCloud, calling `progress` with bytes on disk and the file's size
/// as it goes. Returns early with `Stopped` once `stop()` says so: the read itself can't be
/// interrupted, so it carries on in the background and the file just ends up downloaded.
pub fn download(path: &Path, stop: impl Fn() -> bool, mut progress: impl FnMut(u64, u64)) -> Result<(), DownloadError> {
    use std::io::Read;
    let _ = allow_downloads();
    let size = std::fs::metadata(path).map_err(DownloadError::Io)?.len();
    let owned = path.to_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("icloud-download".into())
        .spawn(move || {
            let read = std::fs::File::open(&owned).and_then(|mut f| f.read(&mut [0u8; 1]));
            let _ = tx.send(read);
        })
        .map_err(DownloadError::Io)?;
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(Ok(_)) => break,
            Ok(Err(e)) => return Err(DownloadError::Io(e)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(DownloadError::StillInCloud),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if stop() {
                    return Err(DownloadError::Stopped);
                }
                progress(on_disk(path).min(size), size);
            }
        }
    }
    if is_evicted(path) {
        return Err(DownloadError::StillInCloud);
    }
    progress(size, size);
    Ok(())
}

/// Bytes of the file actually stored on this Mac.
fn on_disk(path: &Path) -> u64 {
    #[cfg(target_os = "macos")]
    {
        use std::os::darwin::fs::MetadataExt;
        std::fs::symlink_metadata(path).map_or(0, |m| m.st_blocks() * 512)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        0
    }
}

/// Human file sizes for logs: "1.2 MB".
pub fn size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.0} KB", b / 1e3)
    }
}

/// A download job for a track you pressed play on. It shows in the status bar with its own
/// progress and stop button, and logs what it did.
pub fn run(path: &Path, title: &str, rep: &mut crate::jobs::Reporter) -> crate::jobs::Outcome {
    use crate::jobs::Outcome;
    let started = std::time::Instant::now();
    let size = std::fs::metadata(path).map_or(0, |m| m.len());
    rep.info(format!("\"{title}\" is only in iCloud ({}). File: {}", self::size(size), path.display()));
    if let Err(e) = allow_downloads() {
        rep.warn(format!("Couldn't turn on iCloud downloads for this process: {e}. Trying anyway."));
    }
    let token = rep.token();
    let result = download(path, || token.is_cancelled(), |done, total| rep.progress(done, Some(total.max(1)), format!("{} of {}", self::size(done), self::size(total))));
    match result {
        Ok(()) => {
            let secs = started.elapsed().as_secs_f64();
            rep.info(format!("Downloaded {} in {secs:.1}s ({}/s).", self::size(size), self::size((size as f64 / secs.max(0.001)) as u64)));
            Outcome::Completed(format!("Downloaded {title}"))
        }
        Err(DownloadError::Stopped) => {
            rep.info("Stopped waiting. macOS may still finish the download in the background, which is harmless.");
            Outcome::Stopped(format!("Stopped downloading {title}"))
        }
        Err(e) => {
            rep.error(format!("Download failed: {}", e.detail()));
            Outcome::Failed(format!("Couldn't play {title}: {e}"))
        }
    }
}
