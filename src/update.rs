//! Keeps Sleeve up to date from its GitHub releases.
//!
//! A release build knows its own number, baked in by CI as `SLEEVE_BUILD`. A worker thread reads
//! the latest release's `latest.json` when Sleeve opens and every hour after. When there's a
//! newer build it downloads it in the background, checks it, and gets it ready beside the app.
//! Nothing changes until you press Restart: then the new version is swapped in and opened, and
//! this one quits. A build without a number, such as `cargo run`, never updates.
//!
//! What it guards against:
//!
//! - **Bad internet.** Every request has timeouts, including one for a download that stalls.
//!   A failed check is quiet and tries again later, sooner at first and less often after. A
//!   download that stops part way carries on from where it got to next time, even after a
//!   restart, as long as the file it was writing is still there.
//! - **Captive portals and broken downloads.** A login page instead of JSON fails to parse. A
//!   file is only used if its size and SHA-256 match what GitHub lists for it.
//! - **Rate limits.** GitHub's API allows 60 unsigned requests an hour, shared by everything on
//!   the same internet connection, so other apps can use them all up. Sleeve reads `latest.json`,
//!   a file the release workflow uploads with each release, which has no such limit. It asks the
//!   API only for a release made before there was one, sending the last answer's ETag so an
//!   unchanged answer doesn't count, and waits out a limit if hit.
//! - **Anything but our own app.** On a Mac the new app must pass `codesign --verify --strict`,
//!   be signed by Sleeve's own Developer ID team and say it's the build it should be.
//! - **Two Sleeves at once.** A lock file lets one of them check, download or install at a time.
//!   A download the other one finished is used rather than fetched again.
//! - **Never no app.** The new version is copied in beside the old one first. The old one is
//!   moved aside, the new one moved into its place, and if that second step fails the old one
//!   goes back. What's left over is deleted the next time Sleeve opens.
//! - **Places Sleeve can't replace itself**, like the disk image or a folder you can't write to.
//!   Then it says so and offers the download page instead.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rune::Waker;
use serde::{Deserialize, Serialize};

/// The latest release's `latest.json`: its build number, and each file's size and SHA-256.
const MANIFEST: &str = "https://github.com/alii/sleeve/releases/latest/download/latest.json";
/// GitHub's API, for a latest release without a `latest.json`.
const LATEST: &str = "https://api.github.com/repos/alii/sleeve/releases/latest";
/// Where a release's files are downloaded from. Anything else GitHub's answer points at is
/// refused.
const DOWNLOADS: &str = "https://github.com/alii/sleeve/releases/download/";
/// The page with the newest release, for when Sleeve can't update itself.
pub const RELEASE_PAGE: &str = "https://github.com/alii/sleeve/releases/latest";

#[cfg(target_os = "macos")]
const ASSET: &str = "Sleeve.dmg";
#[cfg(windows)]
const ASSET: &str = "Sleeve-windows.zip";
#[cfg(not(any(target_os = "macos", windows)))]
const ASSET: &str = "";

/// The Apple Developer ID team that signs Sleeve. A new app signed by anyone else is refused.
#[cfg(target_os = "macos")]
const TEAM_ID: &str = "RD994J874S";
#[cfg(target_os = "macos")]
const BUNDLE_ID: &str = "com.alii.sleeve";

/// The first check waits a little, so it doesn't compete with the library opening.
const FIRST_CHECK: Duration = Duration::from_secs(20);
/// How long between checks once one has worked.
const EVERY: Duration = Duration::from_secs(60 * 60);
/// How often the worker looks at the clock. Short, because a Mac's monotonic clock stops while
/// it sleeps, so a single long wait could run hours late.
const TICK: Duration = Duration::from_secs(5 * 60);
/// A download that gets nothing for this long has stalled.
const STALL: Duration = Duration::from_secs(30);

/// This build's number, set by the release workflow. `None` for a build made any other way.
pub fn this_build() -> Option<u32> {
    option_env!("SLEEVE_BUILD").and_then(|s| s.trim().parse().ok()).filter(|&n| n > 0)
}

/// Whether this copy of Sleeve updates itself: a numbered release build on a Mac or Windows,
/// not a headless test run, and not turned off with `SLEEVE_NO_UPDATES=1`.
pub fn enabled() -> bool {
    cfg!(any(target_os = "macos", windows))
        && this_build().is_some()
        && rune::tooling::headless_run().is_none()
        && std::env::var_os("SLEEVE_NO_UPDATES").is_none_or(|v| v.is_empty() || v == "0")
}

/// What the update card shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    /// Nothing to show: up to date, not checked yet, or a check failed quietly.
    #[default]
    Idle,
    /// Downloading a newer build.
    Downloading { build: u32, done: u64, total: u64 },
    /// Downloaded. Now checking the file, unpacking it and checking its signature.
    Preparing { build: u32 },
    /// The download failed. It's tried again by itself later, or now with Try again.
    Failed { build: u32, why: String },
    /// Downloaded and checked. Restart to use it.
    Ready { build: u32 },
    /// Restart was pressed and the new version is going in.
    Installing { build: u32 },
    /// Sleeve can't replace itself where it is, so the download page is the way.
    Manual { build: u32, why: String },
}

impl Status {
    pub fn build(&self) -> Option<u32> {
        match self {
            Status::Idle => None,
            Status::Downloading { build, .. } | Status::Preparing { build } | Status::Failed { build, .. } | Status::Ready { build } | Status::Installing { build } | Status::Manual { build, .. } => Some(*build),
        }
    }
}

/// How a check from the menu went, when it found nothing to download.
#[derive(Clone, Debug, PartialEq)]
pub enum Manual {
    Checking,
    UpToDate,
    /// Why it couldn't check, or why this build can't update.
    Failed(String),
}

enum Cmd {
    /// Check now, as Try again does.
    Check,
    /// Swap the new version in.
    Install,
}

enum Msg {
    Status(Status),
    /// A check finished: `None` if it worked, or why it didn't.
    Checked(Option<String>),
    /// The new version is in place at this path. Open it and quit.
    Installed(PathBuf),
}

/// The update state the app holds. The worker does the work; this just shows what it says.
#[derive(Default)]
pub struct Updater {
    pub status: Status,
    /// Restart was pressed while a job was running. It happens when the jobs are done.
    pub restart_waiting: bool,
    /// "Later" was pressed for this build, so its card stays hidden until a newer one.
    pub hidden: Option<u32>,
    /// A check asked for from the menu, so its result shows even when there's nothing new.
    pub manual: Option<Manual>,
    cmd: Option<Sender<Cmd>>,
    rx: Option<Receiver<Msg>>,
    installed: Option<PathBuf>,
}

impl Updater {
    /// Starts the worker, if this build updates itself at all.
    pub fn start(&mut self, waker: Waker) {
        if !enabled() || self.cmd.is_some() {
            return;
        }
        let (cmd_tx, cmd_rx) = channel();
        let (msg_tx, msg_rx) = channel();
        let worker = Worker::new(msg_tx, cmd_rx, waker);
        let started = std::thread::Builder::new().name("updates".into()).spawn(move || worker.run());
        if started.is_ok() {
            self.cmd = Some(cmd_tx);
            self.rx = Some(msg_rx);
        }
    }

    /// Takes in what the worker said. True when the card needs drawing again.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut changed = false;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Checked(problem) => {
                    if self.manual == Some(Manual::Checking) {
                        self.manual = match (&self.status, problem) {
                            (Status::Idle, Some(why)) => Some(Manual::Failed(why)),
                            (Status::Idle, None) => Some(Manual::UpToDate),
                            // Something to download or install: its own card says so.
                            _ => None,
                        };
                    }
                }
                Msg::Status(s) => {
                    if !matches!(s, Status::Idle) {
                        self.manual = None;
                    }
                    // A failed install goes back to Ready, so Restart can be pressed again.
                    if !matches!(s, Status::Ready { .. } | Status::Installing { .. }) {
                        self.restart_waiting = false;
                    }
                    self.status = s;
                }
                Msg::Installed(path) => self.installed = Some(path),
            }
            changed = true;
        }
        changed
    }

    /// Check now, from the menu. Unlike the checks on their own, this one always says how it
    /// went: up to date, couldn't check, or the usual card when there's something new.
    pub fn check_now(&mut self) {
        if self.status.build().is_some() {
            // Already found one: show its card again, even if Later hid it.
            self.hidden = None;
            return;
        }
        let Some(tx) = &self.cmd else {
            self.manual = Some(Manual::Failed(match this_build() {
                None => "This copy of Sleeve was built by hand, so it can't update itself. Download the latest from GitHub.".into(),
                Some(_) => "Updates are turned off. Download the latest from GitHub.".into(),
            }));
            return;
        };
        if tx.send(Cmd::Check).is_ok() {
            self.manual = Some(Manual::Checking);
        }
    }

    /// Whether the card is on screen.
    pub fn visible(&self) -> bool {
        if self.manual.is_some() && matches!(self.status, Status::Idle) {
            return true;
        }
        match &self.status {
            Status::Idle => false,
            Status::Installing { .. } => true,
            s => self.hidden != s.build(),
        }
    }

    /// Try again: check and download now.
    pub fn retry(&mut self) {
        if let Some(tx) = &self.cmd {
            let _ = tx.send(Cmd::Check);
        }
    }

    /// Restart was pressed. With jobs running it waits for them; `restart_now` does the rest.
    pub fn restart(&mut self, jobs_running: bool) {
        if !matches!(self.status, Status::Ready { .. }) {
            return;
        }
        if jobs_running {
            self.restart_waiting ^= true;
        } else {
            self.restart_now();
        }
    }

    /// Starts swapping the new version in. Pressing Restart twice only does it once.
    pub fn restart_now(&mut self) {
        let Status::Ready { build } = self.status else { return };
        self.restart_waiting = false;
        if let Some(tx) = &self.cmd
            && tx.send(Cmd::Install).is_ok()
        {
            self.status = Status::Installing { build };
        }
    }

    /// The new version's path, once it's in place, to open before quitting.
    pub fn take_installed(&mut self) -> Option<PathBuf> {
        self.installed.take()
    }
}

/// Opens the new version once this process has gone, so the two never run at once. Returns
/// false if it couldn't, in which case the app shouldn't quit.
pub fn relaunch(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::process::CommandExt;
        // Waits up to 30 seconds for this process to end, then opens the app either way.
        const SCRIPT: &str = r#"i=0; while kill -0 "$1" 2>/dev/null && [ "$i" -lt 300 ]; do sleep 0.1; i=$((i+1)); done; exec /usr/bin/open -n "$2""#;
        std::process::Command::new("/bin/sh")
            .args(["-c", SCRIPT, "sh", &std::process::id().to_string()])
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // Its own process group, so it isn't caught up in anything sent to this one.
            .process_group(0)
            .spawn()
            .is_ok()
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x8;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
        std::process::Command::new(path).creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP).spawn().is_ok()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = path;
        false
    }
}

/// Where downloads and the worker's notes go: `Updates` in Sleeve's own folder.
fn updates_dir() -> PathBuf {
    crate::store::app_dir().join("Updates")
}

/// A release, as much of it as an update needs.
#[derive(Clone, Debug, PartialEq)]
struct Release {
    build: u32,
    url: String,
    size: u64,
    sha256: String,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    size: u64,
    #[serde(default)]
    state: String,
    #[serde(default)]
    digest: Option<String>,
    browser_download_url: String,
}

/// Reads GitHub's answer for the latest release. `Ok(None)` means it has no file for this
/// system yet, as when a release is still being put together.
fn parse_release(body: &str) -> Result<Option<Release>, String> {
    let r: ApiRelease = serde_json::from_str(body).map_err(|e| format!("GitHub's answer wasn't a release ({e}). A Wi-Fi login page can do this."))?;
    if r.draft || r.prerelease {
        return Ok(None);
    }
    let Some(build) = r.tag_name.strip_prefix("build-").and_then(|n| n.parse::<u32>().ok()) else {
        return Err(format!("the latest release is called \"{}\", not build-N", r.tag_name));
    };
    let Some(a) = r.assets.iter().find(|a| a.name == ASSET) else { return Ok(None) };
    if a.state != "uploaded" || a.size == 0 {
        return Ok(None);
    }
    if !a.browser_download_url.starts_with(DOWNLOADS) {
        return Err(format!("the download is somewhere unexpected: {}", a.browser_download_url));
    }
    let sha256 = a.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")).filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()));
    let Some(sha256) = sha256 else { return Err("GitHub didn't list a SHA-256 for the download, so it can't be checked".into()) };
    Ok(Some(Release { build, url: a.browser_download_url.clone(), size: a.size, sha256: sha256.to_ascii_lowercase() }))
}

/// The latest release's `latest.json`, which the release workflow writes.
#[derive(Deserialize)]
struct Manifest {
    build: u32,
    files: std::collections::HashMap<String, ManifestFile>,
}

#[derive(Deserialize)]
struct ManifestFile {
    size: u64,
    sha256: String,
}

/// Reads `latest.json`. `Ok(None)` means the release has no file for this system.
fn parse_manifest(body: &str) -> Result<Option<Release>, String> {
    let m: Manifest = serde_json::from_str(body).map_err(|e| format!("the release's latest.json wasn't readable ({e}). A Wi-Fi login page can do this."))?;
    if m.build == 0 {
        return Err("the release's latest.json has no build number".into());
    }
    let Some(f) = m.files.get(ASSET).filter(|f| f.size > 0) else { return Ok(None) };
    if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("the release's latest.json has no proper SHA-256 for the download, so it can't be checked".into());
    }
    Ok(Some(Release { build: m.build, url: format!("{DOWNLOADS}build-{}/{ASSET}", m.build), size: f.size, sha256: f.sha256.to_ascii_lowercase() }))
}

/// Why GitHub said no, when it's asking Sleeve to wait. `None` for any other no.
fn rate_limit(status: u16, header: impl Fn(&str) -> Option<String>) -> Option<SystemTime> {
    let now = SystemTime::now();
    let reset = header("x-ratelimit-reset").and_then(|s| s.parse::<u64>().ok()).map(|t| UNIX_EPOCH + Duration::from_secs(t));
    let after = header("retry-after").and_then(|s| s.parse::<u64>().ok()).map(|s| now + Duration::from_secs(s));
    (header("x-ratelimit-remaining").as_deref() == Some("0") || after.is_some() || status == 429).then(|| after.or(reset).unwrap_or(now + Duration::from_secs(60 * 60)))
}

/// What the worker remembers between runs, shared by every Sleeve on this computer.
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    /// The last answer from GitHub and its ETag, so an unchanged one can be asked for cheaply.
    etag: Option<String>,
    body: Option<String>,
    /// When the next check is due, in seconds since 1970.
    next_check: u64,
}

/// Why a check or download didn't work.
enum Error {
    /// GitHub's rate limit is used up until this time.
    RateLimited(SystemTime),
    /// Anything else, in words for a person.
    Failed(String),
    /// Restart was pressed, so the download stopped to let the ready build go in. What was
    /// downloaded is kept for next time.
    Interrupted,
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Failed(s)
    }
}

/// A failed request in plain words. The full error goes in the log.
fn net(e: ureq::Error) -> Error {
    let why = match &e {
        ureq::Error::Timeout(_) => "the connection timed out".to_owned(),
        ureq::Error::HostNotFound => "couldn't reach GitHub. Check you're online".to_owned(),
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => "the connection timed out".to_owned(),
        ureq::Error::Io(_) | ureq::Error::ConnectionFailed => "the connection dropped".to_owned(),
        _ => format!("the download failed ({e})"),
    };
    Error::Failed(why)
}

struct Worker {
    dir: PathBuf,
    tx: Sender<Msg>,
    wake: Waker,
    agent: ureq::Agent,
    /// This Sleeve's build. A field rather than `this_build()`, so a test can be any build.
    this: u32,
    /// The build that's downloaded and checked, ready to go in.
    ready: Option<u32>,
    /// Checks that have failed in a row, for backing off.
    failures: u32,
    /// Why the last check didn't work, for a check from the menu to show.
    problem: Option<String>,
    cmds: Receiver<Cmd>,
    /// Restart was pressed during a download, which stopped so it could go in.
    install_asked: bool,
}

impl Worker {
    fn new(tx: Sender<Msg>, cmds: Receiver<Cmd>, wake: Waker) -> Worker {
        let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .user_agent(format!("Sleeve/{}", this_build().unwrap_or(0)))
            .timeout_resolve(Some(Duration::from_secs(15)))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_per_read(Some(STALL))
            .max_redirects(5)
            .build()
            .new_agent();
        Worker { dir: updates_dir(), tx, wake, agent, this: this_build().unwrap_or(0), ready: None, failures: 0, problem: None, cmds, install_asked: false }
    }

    fn say(&self, s: Status) {
        // With a build ready to go in, a newer one downloads quietly. The card keeps its
        // Restart button and switches to the newer build once that's ready.
        if self.ready.is_some() && matches!(s, Status::Downloading { .. } | Status::Preparing { .. } | Status::Failed { .. }) {
            return;
        }
        let _ = self.tx.send(Msg::Status(s));
        (self.wake)();
    }

    /// A line in `Updates/update.log`, which keeps about the last half megabyte.
    fn log(&self, line: impl AsRef<str>) {
        let path = self.dir.join("update.log");
        if fs::metadata(&path).is_ok_and(|m| m.len() > 512 * 1024) {
            let _ = fs::rename(&path, self.dir.join("update.log.old"));
        }
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{secs} [{}] {}", std::process::id(), line.as_ref());
        }
    }

    fn run(mut self) {
        if let Err(e) = fs::create_dir_all(&self.dir) {
            eprintln!("sleeve: updates are off, as {} can't be made: {e}", self.dir.display());
            return;
        }
        self.log(format!("Sleeve build {} started.", this_build().unwrap_or(0)));
        let mut due = Instant::now() + FIRST_CHECK;
        let mut first = true;
        loop {
            let wait = due.saturating_duration_since(Instant::now()).min(TICK);
            match self.cmds.recv_timeout(wait) {
                Ok(Cmd::Install) => {
                    self.install();
                    continue;
                }
                Ok(Cmd::Check) => {}
                Err(RecvTimeoutError::Timeout) => {
                    // Due by the monotonic clock, or by the wall clock after the Mac slept.
                    if Instant::now() < due && !(!first && self.wall_clock_due()) {
                        continue;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
            first = false;
            let next = self.check_once();
            let _ = self.tx.send(Msg::Checked(self.problem.take()));
            (self.wake)();
            due = Instant::now() + next;
            self.save_next(next);
            if std::mem::take(&mut self.install_asked) {
                self.install();
            }
        }
    }

    /// Whether the saved next-check time has passed, by the wall clock.
    fn wall_clock_due(&self) -> bool {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let next = self.load().next_check;
        next != 0 && now >= next
    }

    fn save_next(&self, wait: Duration) {
        let Some(_lock) = Lock::try_take(&self.dir) else { return };
        let mut saved = self.load();
        saved.next_check = (SystemTime::now() + wait).duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        self.save(&saved);
    }

    /// One check, and a download if there's something new. Returns how long until the next.
    fn check_once(&mut self) -> Duration {
        let Some(lock) = Lock::try_take(&self.dir) else {
            self.log("Another Sleeve is checking for updates. Trying again later.");
            self.problem = Some("another Sleeve window is checking right now".into());
            return Duration::from_secs(10 * 60);
        };
        cleanup(&self.dir, |l| self.log(l));
        let result = self.check();
        drop(lock);
        match result {
            Ok(()) => {
                self.failures = 0;
                EVERY
            }
            Err(Error::RateLimited(until)) => {
                let wait = until.duration_since(SystemTime::now()).unwrap_or_default().clamp(Duration::from_secs(60), EVERY);
                self.log(format!("GitHub's rate limit is used up. Waiting {} minutes.", wait.as_secs() / 60));
                self.problem = Some(format!("GitHub asked Sleeve to wait. Try again in {} minutes", (wait.as_secs() / 60).max(1)));
                wait
            }
            Err(Error::Interrupted) => {
                self.problem = Some("the check was interrupted".into());
                Duration::from_secs(60)
            }
            Err(Error::Failed(why)) => {
                self.failures += 1;
                self.log(format!("Check failed: {why}"));
                self.problem = Some(why);
                // 5 minutes, then 15, and every hour after that.
                [5u64, 15].get(self.failures as usize - 1).map_or(EVERY, |m| Duration::from_secs(m * 60))
            }
        }
    }

    fn check(&mut self) -> Result<(), Error> {
        let this = self.this;
        if this == 0 {
            return Ok(());
        }
        let Some(release) = self.latest()? else {
            self.log("The latest release has no file for this computer yet.");
            return Ok(());
        };
        if release.build <= this {
            self.log(format!("Up to date: build {this}, latest {}.", release.build));
            if self.ready.is_none() {
                self.say(Status::Idle);
            }
            return Ok(());
        }
        if self.ready == Some(release.build) || staged(&self.dir, release.build).is_some() {
            self.ready = Some(release.build);
            self.say(Status::Ready { build: release.build });
            return Ok(());
        }
        self.log(format!("Build {} is out. Downloading {} bytes from {}.", release.build, release.size, release.url));
        match self.fetch(&release) {
            Ok(()) => {
                self.log(format!("Build {} is ready.", release.build));
                self.ready = Some(release.build);
                self.say(Status::Ready { build: release.build });
                // Older downloads aren't needed now.
                cleanup(&self.dir, |l| self.log(l));
                Ok(())
            }
            Err(Error::Failed(why)) => {
                // An older build that's ready stays ready; there's no need to lose it.
                match self.ready {
                    Some(build) => self.say(Status::Ready { build }),
                    None => self.say(Status::Failed { build: release.build, why: why.clone() }),
                }
                Err(Error::Failed(why))
            }
            Err(e) => Err(e),
        }
    }

    fn load(&self) -> Saved {
        fs::read(self.dir.join("state.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Saves by writing a new file and moving it over the old one, so a crash part way through
    /// leaves the old one whole.
    fn save(&self, saved: &Saved) {
        let tmp = self.dir.join(format!("state.json.{}.tmp", std::process::id()));
        if serde_json::to_vec(saved).ok().is_some_and(|b| fs::write(&tmp, b).is_ok()) {
            let _ = fs::rename(&tmp, self.dir.join("state.json"));
        }
        let _ = fs::remove_file(&tmp);
    }

    /// Finds the latest release: from its `latest.json`, or from GitHub's API for a release
    /// made before there was one.
    fn latest(&mut self) -> Result<Option<Release>, Error> {
        let mut resp = self.agent.get(MANIFEST).call().map_err(net)?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
        match status {
            200 => {
                let body = resp.body_mut().with_config().limit(64 * 1024).read_to_string().map_err(net)?;
                Ok(parse_manifest(&body)?)
            }
            404 => {
                self.log("The latest release has no latest.json. Asking GitHub's API instead.");
                self.latest_from_api()
            }
            403 | 429 => Err(rate_limit(status, header).map_or_else(|| Error::Failed(format!("GitHub said no ({status})")), Error::RateLimited)),
            s => Err(Error::Failed(format!("GitHub answered {s}"))),
        }
    }

    /// Asks GitHub's API for the latest release.
    fn latest_from_api(&mut self) -> Result<Option<Release>, Error> {
        let mut saved = self.load();
        let mut req = self.agent.get(LATEST).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28");
        if let (Some(etag), Some(_)) = (&saved.etag, &saved.body) {
            req = req.header("If-None-Match", etag);
        }
        let mut resp = req.call().map_err(net)?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let body = match status {
            304 => saved.body.clone().unwrap_or_default(),
            200 => {
                let etag = header("etag");
                let body = resp.body_mut().with_config().limit(2 * 1024 * 1024).read_to_string().map_err(net)?;
                saved.etag = etag;
                saved.body = Some(body.clone());
                self.save(&saved);
                body
            }
            403 | 429 => return Err(rate_limit(status, header).map_or_else(|| Error::Failed(format!("GitHub said no ({status})")), Error::RateLimited)),
            s => return Err(Error::Failed(format!("GitHub answered {s}"))),
        };
        Ok(parse_release(&body)?)
    }

    /// Downloads the release and gets it ready to go in.
    fn fetch(&mut self, r: &Release) -> Result<(), Error> {
        let file = self.dir.join(format!("build-{}-{ASSET}", r.build));
        let have = fs::metadata(&file).map_or(0, |m| m.len());
        if have != r.size || sha256(&file).as_deref() != Some(&r.sha256) {
            let _ = fs::remove_file(&file);
            self.download(r, &file)?;
        }
        self.say(Status::Preparing { build: r.build });
        stage(&self.dir, &file, r.build).map_err(|why| {
            self.log(format!("Build {} didn't check out: {why}", r.build));
            Error::Failed(why)
        })
    }

    /// Downloads to a `.part` file, carrying on from where an earlier try stopped, and moves it
    /// into place only once its size and SHA-256 match.
    fn download(&mut self, r: &Release, dest: &Path) -> Result<(), Error> {
        let part = dest.with_extension(format!("{}.part", dest.extension().and_then(|e| e.to_str()).unwrap_or("")));
        let mut tries_without_progress = 0;
        loop {
            let have = fs::metadata(&part).map_or(0, |m| m.len());
            if have > r.size {
                let _ = fs::remove_file(&part);
                continue;
            }
            if have == r.size {
                break;
            }
            self.say(Status::Downloading { build: r.build, done: have, total: r.size });
            if have > 0 {
                self.log(format!("Carrying on from byte {have}."));
            }
            match self.download_from(r, &part, have) {
                Ok(()) => {}
                Err(Error::Failed(why)) => {
                    let now = fs::metadata(&part).map_or(0, |m| m.len());
                    self.log(format!("Download stopped at {now} of {} bytes: {why}", r.size));
                    tries_without_progress = if now > have { 0 } else { tries_without_progress + 1 };
                    if tries_without_progress >= 3 {
                        return Err(Error::Failed(why));
                    }
                    std::thread::sleep(Duration::from_secs(2 << tries_without_progress));
                }
                Err(e) => return Err(e),
            }
        }
        self.say(Status::Preparing { build: r.build });
        match sha256(&part) {
            Some(h) if h == r.sha256 => {}
            got => {
                let _ = fs::remove_file(&part);
                self.log(format!("SHA-256 was {got:?}, wanted {}.", r.sha256));
                return Err(Error::Failed("the download was damaged on the way, so it was thrown away".into()));
            }
        }
        fs::rename(&part, dest).map_err(|e| Error::Failed(format!("couldn't save the download: {e}")))?;
        Ok(())
    }

    /// One request, from byte `have` on, appended to `part`.
    fn download_from(&mut self, r: &Release, part: &Path, have: u64) -> Result<(), Error> {
        let mut req = self.agent.get(&r.url).header("Accept", "application/octet-stream");
        if have > 0 {
            req = req.header("Range", format!("bytes={have}-"));
        }
        let mut resp = req.call().map_err(net)?;
        let status = resp.status().as_u16();
        let mut start = have;
        match status {
            206 => {
                let range = resp.headers().get("content-range").and_then(|v| v.to_str().ok()).unwrap_or("");
                if !range.starts_with(&format!("bytes {have}-")) {
                    let _ = fs::remove_file(part);
                    return Err(Error::Failed(format!("the server sent the wrong part of the file ({range})")));
                }
            }
            // The whole file, from the start, whether or not part was asked for.
            200 => {
                if have > 0 {
                    self.log("The server sent the whole file again, so it starts from the beginning.");
                }
                start = 0;
            }
            // What's on disk doesn't fit the file any more. Start again.
            416 => {
                let _ = fs::remove_file(part);
                return Err(Error::Failed("the partial download didn't fit, so it was started again".into()));
            }
            s => return Err(Error::Failed(format!("the download answered {s}"))),
        }
        let mut out = fs::OpenOptions::new().create(true).write(true).truncate(start == 0).append(start > 0).open(part).map_err(|e| Error::Failed(format!("couldn't write the download: {e}")))?;
        let left = r.size - start;
        let mut body = resp.body_mut().with_config().limit(left + 1).reader();
        let mut buf = vec![0u8; 256 * 1024];
        let (mut done, mut told) = (start, Instant::now());
        loop {
            let n = match body.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    let _ = out.sync_data();
                    return Err(if e.kind() == std::io::ErrorKind::TimedOut { Error::Failed("the download stalled".into()) } else { Error::Failed(format!("the connection dropped ({e})")) });
                }
            };
            done += n as u64;
            if done > r.size {
                drop(out);
                let _ = fs::remove_file(part);
                return Err(Error::Failed("the download was bigger than it should be".into()));
            }
            out.write_all(&buf[..n]).map_err(|e| Error::Failed(format!("couldn't write the download: {e}")))?;
            if told.elapsed() >= Duration::from_millis(200) {
                if self.cmds.try_iter().any(|c| matches!(c, Cmd::Install)) {
                    self.install_asked = true;
                    let _ = out.sync_data();
                    return Err(Error::Interrupted);
                }
                told = Instant::now();
                self.say(Status::Downloading { build: r.build, done, total: r.size });
            }
        }
        out.sync_data().map_err(|e| Error::Failed(format!("couldn't write the download: {e}")))?;
        if done < r.size {
            return Err(Error::Failed("the connection closed early".into()));
        }
        self.say(Status::Downloading { build: r.build, done, total: r.size });
        Ok(())
    }

    /// Swaps the ready build in, then tells the app to open it and quit.
    fn install(&mut self) {
        let Some(build) = self.ready else { return };
        // Try again checks once more, which finds this build still ready and shows Restart.
        let fail = |w: &Worker, why: String| {
            w.log(format!("Install failed: {why}"));
            let _ = w.tx.send(Msg::Status(Status::Failed { build, why }));
            (w.wake)();
        };
        // Another Sleeve may be part way through a download. Give it a minute.
        let started = Instant::now();
        let lock = loop {
            if let Some(l) = Lock::try_take(&self.dir) {
                break Some(l);
            }
            if started.elapsed() > Duration::from_secs(60) {
                break None;
            }
            std::thread::sleep(Duration::from_millis(250));
        };
        let Some(_lock) = lock else {
            return fail(self, "another Sleeve window is busy updating. Try again in a minute".into());
        };
        let Some(staged) = staged(&self.dir, build) else {
            self.ready = None;
            self.log("The ready build has gone. Downloading it again.");
            self.say(Status::Idle);
            return;
        };
        self.log(format!("Installing build {build} from {}.", staged.display()));
        match swap_in(&staged, build) {
            Ok(target) => {
                self.log(format!("Installed build {build} at {}.", target.display()));
                let _ = self.tx.send(Msg::Installed(target));
                (self.wake)();
            }
            Err(Swap::Manual(why)) => {
                self.log(format!("Can't install here: {why}"));
                self.say(Status::Manual { build, why });
            }
            Err(Swap::Failed(why)) => fail(self, why),
        }
    }
}

/// One Sleeve at a time checks, downloads or installs. Let go when dropped, or when the
/// process ends however it ends.
struct Lock(#[allow(dead_code)] fs::File);

impl Lock {
    fn try_take(dir: &Path) -> Option<Lock> {
        let f = fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(dir.join("lock")).ok()?;
        f.try_lock().ok()?;
        Some(Lock(f))
    }
}

/// The file's SHA-256 in lowercase hex.
fn sha256(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut f = fs::File::open(path).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => h.update(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The build number at the start of a name like `build-12` or `build-12-Sleeve.dmg`.
fn build_in(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("build-")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Deletes what's no longer needed: downloads of builds no newer than this one, all but the
/// newest download otherwise, disk images left mounted by a crash, and the old app or exe an
/// install moved aside. Anything still in use is left for next time.
fn cleanup(dir: &Path, log: impl Fn(String)) {
    let this = this_build().unwrap_or(0);
    let Ok(entries) = fs::read_dir(dir) else { return };
    let entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    // The newest build that's ready. Older ones can go; a newer one still downloading can't
    // push out the one that's ready.
    let newest = entries.iter().filter_map(|p| build_in(&p.file_name()?.to_string_lossy())).filter(|&b| staged(dir, b).is_some()).max().unwrap_or(0);
    for p in &entries {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let stale = build_in(&name).is_some_and(|b| b <= this || b < newest);
        if name.starts_with("mount-") {
            #[cfg(target_os = "macos")]
            detach(p);
            let _ = fs::remove_dir(p);
        } else if stale || name.ends_with(".tmp") {
            let gone = if p.is_dir() { fs::remove_dir_all(p) } else { fs::remove_file(p) };
            if gone.is_ok() {
                log(format!("Deleted {name}."));
            }
        }
    }
    // What an install moved aside, next to the app.
    #[cfg(target_os = "macos")]
    if let Some(app) = running_bundle()
        && let (Some(parent), Some(stem)) = (app.parent(), app.file_stem().map(|s| s.to_string_lossy().into_owned()))
        && let Ok(siblings) = fs::read_dir(parent)
    {
        for e in siblings.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if (name.starts_with(&format!(".{stem}-old-")) || name.starts_with(&format!(".{stem}-new-"))) && name.ends_with(".app") && fs::remove_dir_all(e.path()).is_ok() {
                log(format!("Deleted {name}."));
            }
        }
    }
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe()
        && let (Some(parent), Some(file)) = (exe.parent(), exe.file_name().map(|s| s.to_string_lossy().into_owned()))
        && let Ok(siblings) = fs::read_dir(parent)
    {
        for e in siblings.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            // Fails while the old exe is still closing. The next launch tries again.
            if (name.starts_with(&format!("{file}.old")) || name.starts_with(&format!("{file}.new"))) && fs::remove_file(e.path()).is_ok() {
                log(format!("Deleted {name}."));
            }
        }
    }
}

/// The folder a build is unpacked into, ready to go in.
fn stage_dir(dir: &Path, build: u32) -> PathBuf {
    dir.join(format!("build-{build}"))
}

/// What goes in: the app bundle on a Mac, the exe on Windows.
fn staged_payload(dir: &Path, build: u32) -> PathBuf {
    let d = stage_dir(dir, build);
    if cfg!(windows) { d.join("Sleeve.exe") } else { d.join("Sleeve.app") }
}

/// The unpacked build, if it was finished and checked. The `ok` file is written last, so a
/// crash while unpacking never looks finished.
fn staged(dir: &Path, build: u32) -> Option<PathBuf> {
    let ok = fs::read_to_string(stage_dir(dir, build).join("ok")).ok()?;
    let payload = staged_payload(dir, build);
    (ok.trim() == build.to_string() && payload.exists()).then_some(payload)
}

/// Unpacks a downloaded build and checks it, then marks it ready.
fn stage(dir: &Path, file: &Path, build: u32) -> Result<(), String> {
    let out = stage_dir(dir, build);
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| format!("couldn't clear {}: {e}", out.display()))?;
    }
    fs::create_dir_all(&out).map_err(|e| format!("couldn't make {}: {e}", out.display()))?;
    unpack(dir, file, &out)?;
    let payload = staged_payload(dir, build);
    check_payload(&payload, build)?;
    let tmp = out.join("ok.tmp");
    fs::write(&tmp, build.to_string()).and_then(|()| fs::rename(&tmp, out.join("ok"))).map_err(|e| format!("couldn't mark it ready: {e}"))
}

/// Runs a command to the end, quietly. Its error output is the reason if it fails.
fn run(cmd: &mut std::process::Command) -> Result<String, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.stdin(std::process::Stdio::null()).output().map_err(|e| format!("couldn't run {:?}: {e}", cmd.get_program()))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(text) } else { Err(format!("{:?} failed: {}", cmd.get_program(), text.trim())) }
}

/// Copies Sleeve.app out of the disk image. The image is always detached after, even if the
/// copy fails.
#[cfg(target_os = "macos")]
fn unpack(dir: &Path, dmg: &Path, out: &Path) -> Result<(), String> {
    let mnt = dir.join(format!("mount-{}", std::process::id()));
    detach(&mnt);
    fs::create_dir_all(&mnt).map_err(|e| format!("couldn't make {}: {e}", mnt.display()))?;
    struct Detach<'a>(&'a Path);
    impl Drop for Detach<'_> {
        fn drop(&mut self) {
            detach(self.0);
            let _ = fs::remove_dir(self.0);
        }
    }
    let _guard = Detach(&mnt);
    run(std::process::Command::new("/usr/bin/hdiutil").args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint"]).arg(&mnt).arg(dmg))?;
    let app = mnt.join("Sleeve.app");
    if !app.is_dir() {
        return Err("the disk image has no Sleeve.app in it".into());
    }
    run(std::process::Command::new("/usr/bin/ditto").arg(&app).arg(out.join("Sleeve.app")))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn detach(mnt: &Path) {
    if mnt.exists() {
        let _ = run(std::process::Command::new("/usr/bin/hdiutil").args(["detach", "-force"]).arg(mnt));
    }
}

/// Takes Sleeve.exe out of the zip, with the `tar` that comes with Windows 10 and 11.
#[cfg(windows)]
fn unpack(_dir: &Path, zip: &Path, out: &Path) -> Result<(), String> {
    let root = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    run(std::process::Command::new(root.join(r"System32\tar.exe")).arg("-xf").arg(zip).arg("-C").arg(out).arg("Sleeve.exe"))?;
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn unpack(_dir: &Path, _file: &Path, _out: &Path) -> Result<(), String> {
    Err("updates only work on a Mac or Windows".into())
}

/// Checks the new app is really Sleeve, signed by Sleeve's team, and the build it should be.
#[cfg(target_os = "macos")]
fn check_payload(app: &Path, build: u32) -> Result<(), String> {
    run(std::process::Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(app)).map_err(|e| format!("the new app's signature didn't check out: {e}"))?;
    let info = run(std::process::Command::new("/usr/bin/codesign").args(["-dv", "--verbose=2"]).arg(app))?;
    if !info.lines().any(|l| l.trim() == format!("TeamIdentifier={TEAM_ID}")) {
        return Err("the new app isn't signed by Sleeve's developer".into());
    }
    let id = plist_value(app, "CFBundleIdentifier");
    if id.as_deref() != Some(BUNDLE_ID) {
        return Err(format!("the new app isn't Sleeve (it's {})", id.as_deref().unwrap_or("unnamed")));
    }
    let version = plist_value(app, "CFBundleVersion");
    if version.as_deref() != Some(&build.to_string()) {
        return Err(format!("the new app says it's build {}, not {build}", version.as_deref().unwrap_or("nothing")));
    }
    Ok(())
}

/// Checks the exe at least looks like one. Its SHA-256 was already checked against GitHub's.
#[cfg(windows)]
fn check_payload(exe: &Path, _build: u32) -> Result<(), String> {
    let mut head = [0u8; 2];
    fs::File::open(exe).and_then(|mut f| f.read_exact(&mut head)).map_err(|e| format!("couldn't read the new exe: {e}"))?;
    if &head != b"MZ" { Err("the new exe isn't a program".into()) } else { Ok(()) }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn check_payload(_p: &Path, _build: u32) -> Result<(), String> {
    Err("updates only work on a Mac or Windows".into())
}

#[cfg(target_os = "macos")]
fn plist_value(app: &Path, key: &str) -> Option<String> {
    run(std::process::Command::new("/usr/libexec/PlistBuddy").args(["-c", &format!("Print :{key}")]).arg(app.join("Contents/Info.plist"))).ok().map(|s| s.trim().to_owned())
}

/// The app bundle this process is running from, wherever it is now. The kernel follows the
/// file, so this is right even if the app was moved after it opened.
#[cfg(target_os = "macos")]
fn running_bundle() -> Option<PathBuf> {
    unsafe extern "C" {
        fn proc_pidpath(pid: i32, buf: *mut u8, size: u32) -> i32;
    }
    let mut buf = vec![0u8; 4096];
    // SAFETY: the buffer is as long as the size passed, and the call writes at most that much.
    let n = unsafe { proc_pidpath(std::process::id() as i32, buf.as_mut_ptr(), buf.len() as u32) };
    let exe = if n > 0 {
        buf.truncate(n as usize);
        PathBuf::from(String::from_utf8(buf).ok()?)
    } else {
        std::env::current_exe().ok()?
    };
    // …/Sleeve.app/Contents/MacOS/sleeve
    let app = exe.parent()?.parent()?.parent()?.to_path_buf();
    (app.extension().is_some_and(|e| e == "app") && exe.parent()?.file_name()? == "MacOS").then_some(app)
}

enum Swap {
    /// It can't go in where Sleeve is. Download it by hand instead.
    Manual(String),
    /// Something went wrong. Trying again may work.
    Failed(String),
}

/// An error from writing next to the app, as a reason to download by hand if it's about where
/// the app is rather than a one-off.
fn where_it_is(e: &std::io::Error, folder: &Path) -> Swap {
    // EROFS is 30 on both Mac and Linux. Windows says permission denied.
    if e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(30) || e.kind() == std::io::ErrorKind::ReadOnlyFilesystem {
        Swap::Manual(format!("Sleeve can't replace itself in {}. Download the new version and put it there yourself.", folder.display()))
    } else {
        Swap::Failed(format!("couldn't put the new version in {}: {e}", folder.display()))
    }
}

/// Puts the new app where the running one is: copies it in beside it, moves the old one aside,
/// and moves the new one into its place. If the last step fails the old one goes back.
#[cfg(target_os = "macos")]
fn swap_in(staged: &Path, build: u32) -> Result<PathBuf, Swap> {
    let Some(target) = running_bundle() else { return Err(Swap::Manual("Sleeve isn't running from an app, so it can't replace itself.".into())) };
    let path = target.to_string_lossy();
    if path.contains("/AppTranslocation/") {
        return Err(Swap::Manual("macOS is running Sleeve from a temporary copy. Move Sleeve to your Applications folder, open it from there, and it can update itself.".into()));
    }
    if path.starts_with("/Volumes/") && !writable(target.parent().unwrap_or(Path::new("/"))) {
        return Err(Swap::Manual("Sleeve is running from its disk image. Drag it to your Applications folder and open it from there.".into()));
    }
    // Another Sleeve window may have put it in already.
    if plist_value(&target, "CFBundleVersion").and_then(|v| v.parse::<u32>().ok()).is_some_and(|v| v >= build) {
        return Ok(target);
    }
    // Checked again just before it goes in, in case it changed on disk since it was downloaded.
    check_payload(staged, build).map_err(Swap::Failed)?;
    replace_app(staged, &target)?;
    Ok(target)
}

/// Exchanges what's at two paths in one step, with `renamex_np`'s `RENAME_SWAP`. Fails with
/// ENOTSUP (45) or EINVAL (22) on a disk that can't.
#[cfg(target_os = "macos")]
fn swap_paths(a: &Path, b: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    const RENAME_SWAP: u32 = 0x2;
    unsafe extern "C" {
        fn renamex_np(from: *const std::ffi::c_char, to: *const std::ffi::c_char, flags: u32) -> i32;
    }
    let a = std::ffi::CString::new(a.as_os_str().as_bytes())?;
    let b = std::ffi::CString::new(b.as_os_str().as_bytes())?;
    // SAFETY: both are NUL-terminated paths that live past the call.
    if unsafe { renamex_np(a.as_ptr(), b.as_ptr(), RENAME_SWAP) } == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// Copies `staged` in beside `target`, then swaps the two. Where the disk can't swap, it moves
/// `target` aside and the copy into its place, and if that last move fails `target` goes back.
#[cfg(target_os = "macos")]
fn replace_app(staged: &Path, target: &Path) -> Result<(), Swap> {
    let folder = target.parent().ok_or_else(|| Swap::Failed("the app has no folder".into()))?.to_path_buf();
    let stem = target.file_stem().map_or_else(|| "Sleeve".into(), |s| s.to_string_lossy().into_owned());
    let pid = std::process::id();
    let new = folder.join(format!(".{stem}-new-{pid}.app"));
    let old = folder.join(format!(".{stem}-old-{pid}.app"));
    let _ = fs::remove_dir_all(&new);
    let _ = fs::remove_dir_all(&old);
    // ditto keeps the signature, permissions and extended attributes exactly.
    if let Err(e) = run(std::process::Command::new("/usr/bin/ditto").arg(staged).arg(&new)) {
        let _ = fs::remove_dir_all(&new);
        return Err(if writable(&folder) { Swap::Failed(e) } else { where_it_is(&std::io::Error::from(std::io::ErrorKind::PermissionDenied), &folder) });
    }
    // Swaps the two in one step, so there's no moment without an app, even if Sleeve is
    // killed right then. The old app ends up under the new one's temporary name, and is
    // deleted next launch. A disk that can't swap falls through to two moves.
    match swap_paths(&new, target) {
        Ok(()) => {
            let _ = fs::rename(&new, &old);
            return Ok(());
        }
        Err(e) if e.raw_os_error() == Some(45) || e.raw_os_error() == Some(22) => {}
        Err(e) => {
            let _ = fs::remove_dir_all(&new);
            return Err(where_it_is(&e, &folder));
        }
    }
    if let Err(e) = fs::rename(target, &old) {
        let _ = fs::remove_dir_all(&new);
        return Err(where_it_is(&e, &folder));
    }
    if let Err(e) = fs::rename(&new, target) {
        // Put the old one back. If even that fails, say where it is so nothing is lost.
        if let Err(back) = fs::rename(&old, target) {
            return Err(Swap::Failed(format!("couldn't put the new version in ({e}), or the old one back ({back}). It's at {}.", old.display())));
        }
        let _ = fs::remove_dir_all(&new);
        return Err(Swap::Failed(format!("couldn't put the new version in: {e}")));
    }
    Ok(())
}

/// Windows won't let a running exe be overwritten, but it can be renamed. So the running one
/// is renamed aside and the new one takes its name.
#[cfg(windows)]
fn swap_in(staged: &Path, build: u32) -> Result<PathBuf, Swap> {
    let target = std::env::current_exe().map_err(|e| Swap::Failed(format!("couldn't find Sleeve.exe: {e}")))?;
    check_payload(staged, build).map_err(Swap::Failed)?;
    let folder = target.parent().ok_or_else(|| Swap::Failed("the exe has no folder".into()))?.to_path_buf();
    let file = target.file_name().map_or_else(|| "Sleeve.exe".into(), |s| s.to_string_lossy().into_owned());
    let pid = std::process::id();
    let new = folder.join(format!("{file}.new-{pid}"));
    let old = folder.join(format!("{file}.old-{pid}"));
    let _ = fs::remove_file(&new);
    let _ = fs::remove_file(&old);
    if let Err(e) = fs::copy(staged, &new) {
        let _ = fs::remove_file(&new);
        return Err(where_it_is(&e, &folder));
    }
    if let Err(e) = fs::rename(&target, &old) {
        let _ = fs::remove_file(&new);
        return Err(where_it_is(&e, &folder));
    }
    if let Err(e) = fs::rename(&new, &target) {
        if let Err(back) = fs::rename(&old, &target) {
            return Err(Swap::Failed(format!("couldn't put the new version in ({e}), or the old one back ({back}). It's at {}.", old.display())));
        }
        let _ = fs::remove_file(&new);
        return Err(Swap::Failed(format!("couldn't put the new version in: {e}")));
    }
    Ok(target)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn swap_in(_staged: &Path, _build: u32) -> Result<PathBuf, Swap> {
    Err(Swap::Manual("updates only work on a Mac or Windows".into()))
}

/// Whether a file can be made in this folder, found by making one.
#[cfg(target_os = "macos")]
fn writable(folder: &Path) -> bool {
    let probe = folder.join(format!(".sleeve-write-test-{}", std::process::id()));
    let ok = fs::write(&probe, b"").is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_json(tag: &str, asset: &str, state: &str, digest: Option<&str>, url: &str) -> String {
        serde_json::json!({
            "tag_name": tag, "draft": false, "prerelease": false,
            "assets": [{ "name": asset, "size": 123, "state": state, "digest": digest, "browser_download_url": url }]
        })
        .to_string()
    }

    const HASH: &str = "sha256:a97750cf94d759465c89d46cf5443031c8db7b9663abb224cd8d70562e046670";

    #[test]
    fn reads_a_release() {
        let url = format!("{DOWNLOADS}build-12/{ASSET}");
        let r = parse_release(&release_json("build-12", ASSET, "uploaded", Some(HASH), &url)).unwrap().unwrap();
        assert_eq!(r.build, 12);
        assert_eq!(r.size, 123);
        assert_eq!(r.sha256, &HASH[7..]);
    }

    #[test]
    fn waits_for_a_file_still_uploading() {
        let url = format!("{DOWNLOADS}build-12/{ASSET}");
        assert_eq!(parse_release(&release_json("build-12", ASSET, "starter", Some(HASH), &url)).unwrap(), None);
        assert_eq!(parse_release(&release_json("build-12", "Other.zip", "uploaded", Some(HASH), &url)).unwrap(), None);
    }

    #[test]
    fn refuses_what_it_cant_check() {
        let url = format!("{DOWNLOADS}build-12/{ASSET}");
        assert!(parse_release(&release_json("build-12", ASSET, "uploaded", None, &url)).is_err());
        assert!(parse_release(&release_json("build-12", ASSET, "uploaded", Some("sha256:nothex"), &url)).is_err());
        assert!(parse_release(&release_json("build-12", ASSET, "uploaded", Some(HASH), "https://example.com/Sleeve.dmg")).is_err());
        assert!(parse_release(&release_json("v1.0", ASSET, "uploaded", Some(HASH), &url)).is_err());
    }

    fn manifest_json(build: u32, asset: &str, sha256: &str) -> String {
        serde_json::json!({ "build": build, "files": { asset: { "size": 123, "sha256": sha256 } } }).to_string()
    }

    #[test]
    fn reads_latest_json() {
        let r = parse_manifest(&manifest_json(14, ASSET, &HASH[7..].to_ascii_uppercase())).unwrap().unwrap();
        assert_eq!(r, Release { build: 14, url: format!("{DOWNLOADS}build-14/{ASSET}"), size: 123, sha256: HASH[7..].into() });
    }

    #[test]
    fn latest_json_without_this_systems_file() {
        assert_eq!(parse_manifest(&manifest_json(14, "Other.zip", &HASH[7..])).unwrap(), None);
    }

    #[test]
    fn refuses_latest_json_it_cant_check() {
        assert!(parse_manifest(&manifest_json(14, ASSET, "nothex")).is_err());
        assert!(parse_manifest(&manifest_json(0, ASSET, &HASH[7..])).is_err());
        assert!(parse_manifest("<html><body>Sign in to the Wi-Fi</body></html>").is_err());
    }

    #[test]
    fn a_login_page_is_not_a_release() {
        assert!(parse_release("<html><body>Sign in to the Wi-Fi</body></html>").is_err());
    }

    #[test]
    fn finds_build_numbers_in_names() {
        assert_eq!(build_in("build-12"), Some(12));
        assert_eq!(build_in("build-12-Sleeve.dmg"), Some(12));
        assert_eq!(build_in("build-12-Sleeve.dmg.part"), Some(12));
        assert_eq!(build_in("state.json"), None);
    }

    #[test]
    fn a_half_unpacked_build_is_not_ready() {
        let dir = std::env::temp_dir().join(format!("sleeve-update-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(staged_payload(&dir, 7)).unwrap();
        assert_eq!(staged(&dir, 7), None, "no ok file yet");
        fs::write(stage_dir(&dir, 7).join("ok"), "7").unwrap();
        assert_eq!(staged(&dir, 7), Some(staged_payload(&dir, 7)));
        assert_eq!(staged(&dir, 8), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hashes_files() {
        let p = std::env::temp_dir().join(format!("sleeve-hash-test-{}", std::process::id()));
        fs::write(&p, b"abc").unwrap();
        assert_eq!(sha256(&p).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let _ = fs::remove_file(&p);
    }

    #[cfg(target_os = "macos")]
    fn fake_app(at: &Path, says: &str) {
        fs::create_dir_all(at.join("Contents/MacOS")).unwrap();
        fs::write(at.join("Contents/MacOS/sleeve"), says).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn swaps_the_app_and_keeps_the_old_one_aside() {
        let dir = std::env::temp_dir().join(format!("sleeve-swap-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (target, staged) = (dir.join("Apps/Sleeve.app"), dir.join("staged/Sleeve.app"));
        fake_app(&target, "old");
        fake_app(&staged, "new");
        assert!(replace_app(&staged, &target).is_ok());
        assert_eq!(fs::read_to_string(target.join("Contents/MacOS/sleeve")).unwrap(), "new");
        let old = dir.join(format!("Apps/.Sleeve-old-{}.app", std::process::id()));
        assert_eq!(fs::read_to_string(old.join("Contents/MacOS/sleeve")).unwrap(), "old");
        assert!(!dir.join(format!("Apps/.Sleeve-new-{}.app", std::process::id())).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_folder_it_cant_write_to_means_downloading_by_hand() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("sleeve-locked-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (target, staged) = (dir.join("Apps/Sleeve.app"), dir.join("staged/Sleeve.app"));
        fake_app(&target, "old");
        fake_app(&staged, "new");
        fs::set_permissions(dir.join("Apps"), fs::Permissions::from_mode(0o555)).unwrap();
        let result = replace_app(&staged, &target);
        fs::set_permissions(dir.join("Apps"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(result, Err(Swap::Manual(_))));
        assert_eq!(fs::read_to_string(target.join("Contents/MacOS/sleeve")).unwrap(), "old", "the old app is untouched");
        assert_eq!(fs::read_dir(dir.join("Apps")).unwrap().count(), 1, "nothing left beside it");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Asks GitHub for the real latest release as build 1, downloads it, and unpacks and checks
    /// it, in a folder of its own. Needs the internet, so it only runs when asked:
    /// `cargo test live_update -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_update() {
        let dir = std::env::temp_dir().join("sleeve-live-update-test");
        fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = channel();
        let (_cmd_tx, cmd_rx) = channel();
        let mut w = Worker::new(tx, cmd_rx, std::sync::Arc::new(|| {}));
        w.dir = dir.clone();
        w.this = 1;
        let first = Instant::now();
        let result = w.check();
        let said: Vec<_> = rx.try_iter().filter_map(|m| if let Msg::Status(s) = m { Some(s) } else { None }).collect();
        eprintln!("first check took {:?}: {:?}", first.elapsed(), result.as_ref().err().map(|e| match e {
            Error::Failed(w) => w.clone(),
            Error::RateLimited(_) => "rate limited".into(),
            Error::Interrupted => "interrupted".into(),
        }));
        eprintln!("said {} things, last {:?}", said.len(), said.last());
        eprintln!("{}", fs::read_to_string(dir.join("update.log")).unwrap_or_default());
        // Asking again sends the ETag, and a finished download isn't fetched twice.
        let again = Instant::now();
        let _ = w.check();
        eprintln!("second check took {:?}", again.elapsed());
    }

    #[test]
    fn restart_only_starts_once() {
        let (tx, rx) = channel();
        let mut u = Updater { status: Status::Ready { build: 5 }, cmd: Some(tx), ..Default::default() };
        u.restart(false);
        u.restart(false);
        assert_eq!(u.status, Status::Installing { build: 5 });
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn restart_waits_for_jobs() {
        let (tx, rx) = channel();
        let mut u = Updater { status: Status::Ready { build: 5 }, cmd: Some(tx), ..Default::default() };
        u.restart(true);
        assert!(u.restart_waiting);
        assert_eq!(rx.try_iter().count(), 0);
        // Pressing it again calls it off.
        u.restart(true);
        assert!(!u.restart_waiting);
    }
}
