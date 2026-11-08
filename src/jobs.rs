//! Long-running work: analysis, USB export, iCloud downloads.
//!
//! Every job gets its own id, its own cancel token and its own row in the status bar. A stop
//! button is built from the job it sits next to and captures only that job's id, so it can
//! only ever stop that job. Nothing here is shared between jobs: no global "busy" flag, no
//! single progress channel per kind, no "the current export".
//!
//! Workers run on their own threads and talk to the UI through a channel. They never touch
//! app state. The UI thread drains the channel in `poll`.
//!
//! Every job keeps a log: when it started, each step it took, every problem with the file and
//! the system error behind it, when stop was pressed and how it ended. The Activity panel shows
//! it, and each line also goes to `~/Library/Logs/Sleeve/activity.log`.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use rune::Waker;

/// Workers report progress at most this often, so a fast loop doesn't flood the UI.
const REPORT_EVERY: Duration = Duration::from_millis(33);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(pub u64);

impl JobId {
    /// The element key for this job's status row. Keys are per job, never per kind.
    pub fn key(self, part: &str) -> String {
        format!("job-{}-{part}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Analysis,
    Export,
    Download,
    /// A small change to the library, such as a hot cue.
    Edit,
    /// Songs dropped in from Finder, going into the library.
    Import,
    /// A USB stick being ejected.
    Eject,
}

impl JobKind {
    /// The verb a stop button's tooltip uses: "Stop analysis".
    pub fn noun(self) -> &'static str {
        match self {
            JobKind::Analysis => "analysis",
            JobKind::Export => "export",
            JobKind::Download => "download",
            JobKind::Edit => "change",
            JobKind::Import => "import",
            JobKind::Eject => "eject",
        }
    }
}

/// A handle a worker checks to see if its job was asked to stop.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    /// The shared flag, for crates that take an `Arc<AtomicBool>` themselves.
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.0.clone()
    }
}

/// How much a log line matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    /// Local wall-clock time, "14:12:15.123".
    pub clock: String,
    /// Seconds since the job started.
    pub secs: f64,
    pub level: Level,
    pub text: String,
}

impl LogLine {
    /// The line as it reads in a copied log or the log file.
    pub fn render(&self) -> String {
        format!("{} +{:>7.2}s {:<5} {}", self.clock, self.secs, self.level.tag(), self.text)
    }
}

/// A job keeps this many lines. Older ones stay in the log file.
const LOG_LINES: usize = 2000;

/// The log file every job's lines go to as well, shared by every worker.
#[derive(Clone, Default)]
struct LogFile(Option<Arc<Mutex<std::fs::File>>>);

impl LogFile {
    fn open(path: &std::path::Path) -> Self {
        let file = path.parent().map(std::fs::create_dir_all).transpose().ok().and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(path).ok());
        LogFile(file.map(|f| Arc::new(Mutex::new(f))))
    }
    fn write(&self, id: JobId, kind: JobKind, line: &LogLine) {
        if let Some(f) = &self.0
            && let Ok(mut f) = f.lock()
        {
            let _ = writeln!(f, "{} {} job-{} {:<8} {:<5} {}", date(), line.clock, id.0, kind.noun(), line.level.tag(), line.text);
        }
    }
}

/// Where the log file lives on this Mac.
pub fn log_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Logs/Sleeve/activity.log"))
}

/// Writes log lines for one job from any thread. Cheap to clone, so analysis can hand one to
/// each of its workers.
#[derive(Clone)]
pub struct Logger {
    id: JobId,
    kind: JobKind,
    started: Instant,
    tx: Sender<Event>,
    waker: Option<Waker>,
    file: LogFile,
}

impl Logger {
    pub fn info(&self, text: impl Into<String>) {
        self.log(Level::Info, text.into());
    }
    pub fn warn(&self, text: impl Into<String>) {
        self.log(Level::Warn, text.into());
    }
    pub fn error(&self, text: impl Into<String>) {
        self.log(Level::Error, text.into());
    }
    fn log(&self, level: Level, text: String) {
        let line = LogLine { clock: clock(), secs: self.started.elapsed().as_secs_f64(), level, text };
        self.file.write(self.id, self.kind, &line);
        if self.tx.send(Event::Log { id: self.id, line }).is_ok()
            && let Some(w) = &self.waker
        {
            w();
        }
    }
}

/// How a job ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// It did everything it was asked. The summary says what, in a short sentence.
    Completed(String),
    /// Someone pressed its stop button and it stopped cleanly.
    Stopped(String),
    /// It couldn't go on. The message says why, in words a person can act on.
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Running,
    /// Stop was pressed and the worker hasn't finished winding down yet.
    Stopping,
    /// Finished, at this many seconds on the app clock.
    Done { outcome: Outcome, at: f64 },
}


#[derive(Clone, Debug)]
pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    /// What the job is doing, in a few words: "Exporting to KINGSTON".
    pub title: String,
    /// The item in hand: a track's title, a file name.
    pub detail: String,
    pub done: u64,
    /// None while the job is still working out how much there is to do.
    pub total: Option<u64>,
    pub phase: Phase,
    /// Everything the job did, oldest first.
    pub log: Vec<LogLine>,
    pub started: Instant,
    cancel: CancelToken,
}

impl Job {
    /// The log as plain text, for copying into a bug report.
    pub fn log_text(&self) -> String {
        let mut out = format!("job-{} {} \"{}\"\n", self.id.0, self.kind.noun(), self.title);
        for l in &self.log {
            out.push_str(&l.render());
            out.push('\n');
        }
        out
    }
    fn push(&mut self, line: LogLine) {
        if self.log.len() >= LOG_LINES {
            self.log.drain(..LOG_LINES / 4);
        }
        self.log.push(line);
    }
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total?;
        if total == 0 {
            return Some(1.0);
        }
        Some((self.done as f64 / total as f64).clamp(0.0, 1.0) as f32)
    }
    pub fn is_active(&self) -> bool {
        matches!(self.phase, Phase::Running | Phase::Stopping)
    }
}

enum Event {
    Log { id: JobId, line: LogLine },
    Progress { id: JobId, done: u64, total: Option<u64>, detail: String },
    Title { id: JobId, title: String },
    Finished { id: JobId, outcome: Outcome },
}

/// What a worker holds. It can report progress and check for cancellation, nothing else.
pub struct Reporter {
    id: JobId,
    tx: Sender<Event>,
    waker: Option<Waker>,
    cancel: CancelToken,
    last: Option<Instant>,
    log: Logger,
}

impl Reporter {
    pub fn log(&self) -> &Logger {
        &self.log
    }
    pub fn info(&self, text: impl Into<String>) {
        self.log.info(text);
    }
    pub fn warn(&self, text: impl Into<String>) {
        self.log.warn(text);
    }
    pub fn error(&self, text: impl Into<String>) {
        self.log.error(text);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
    pub fn token(&self) -> CancelToken {
        self.cancel.clone()
    }
    /// Reports progress. Calls closer together than `REPORT_EVERY` are dropped, except the
    /// last one of a job (`done == total`), so the bar always reaches its end.
    pub fn progress(&mut self, done: u64, total: Option<u64>, detail: impl Into<String>) {
        let now = Instant::now();
        let finished = total.is_some_and(|t| done >= t);
        if !finished && self.last.is_some_and(|l| now.duration_since(l) < REPORT_EVERY) {
            return;
        }
        self.last = Some(now);
        self.send(Event::Progress { id: self.id, done, total, detail: detail.into() });
    }
    /// Renames the job's row and logs the new step.
    pub fn retitle(&mut self, title: impl Into<String>) {
        let title = title.into();
        self.log.info(format!("Step: {title}"));
        self.send(Event::Title { id: self.id, title });
    }
    fn send(&self, e: Event) {
        if self.tx.send(e).is_ok()
            && let Some(w) = &self.waker
        {
            w();
        }
    }
}

pub struct Jobs {
    list: Vec<Job>,
    next: u64,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    waker: Option<Waker>,
    file: LogFile,
}

impl Default for Jobs {
    fn default() -> Self {
        let (tx, rx) = channel();
        Jobs { list: Vec::new(), next: 1, tx, rx, waker: None, file: LogFile::default() }
    }
}

impl Jobs {
    /// Sends every job's log lines to this file too. Without it, logs live only in the app.
    pub fn log_to(&mut self, path: &std::path::Path) {
        self.file = LogFile::open(path);
    }

    fn logger(&self, id: JobId, kind: JobKind, started: Instant) -> Logger {
        Logger { id, kind, started, tx: self.tx.clone(), waker: self.waker.clone(), file: self.file.clone() }
    }

    /// Adds a line to a job's log from the UI thread.
    fn note(&mut self, id: JobId, level: Level, text: String) {
        let file = self.file.clone();
        if let Some(j) = self.list.iter_mut().find(|j| j.id == id) {
            let line = LogLine { clock: clock(), secs: j.started.elapsed().as_secs_f64(), level, text };
            file.write(id, j.kind, &line);
            j.push(line);
        }
    }
    pub fn set_waker(&mut self, w: Waker) {
        self.waker = Some(w);
    }

    pub fn list(&self) -> &[Job] {
        &self.list
    }

    pub fn get(&self, id: JobId) -> Option<&Job> {
        self.list.iter().find(|j| j.id == id)
    }


    /// Starts `work` on its own thread. Whatever it returns becomes the job's outcome. If it
    /// panics, the job fails with a message rather than taking the app down.
    pub fn spawn<F>(&mut self, kind: JobKind, title: impl Into<String>, work: F) -> JobId
    where
        F: FnOnce(&mut Reporter) -> Outcome + Send + 'static,
    {
        let id = JobId(self.next);
        self.next += 1;
        let cancel = CancelToken::default();
        let title = title.into();
        let started = Instant::now();
        self.list.push(Job {
            id,
            kind,
            title: title.clone(),
            detail: String::new(),
            done: 0,
            total: None,
            phase: Phase::Running,
            log: Vec::new(),
            started,
            cancel: cancel.clone(),
        });
        self.note(id, Level::Info, format!("Started: {title}"));
        let log = self.logger(id, kind, started);
        let mut rep = Reporter { id, tx: self.tx.clone(), waker: self.waker.clone(), cancel, last: None, log };
        let spawned = std::thread::Builder::new().name(format!("job-{}", id.0)).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&mut rep)));
            let outcome = result.unwrap_or_else(|panic| {
                let why = panic.downcast_ref::<&str>().map(|s| (*s).to_owned()).or_else(|| panic.downcast_ref::<String>().cloned()).unwrap_or_default();
                rep.error(format!("The worker panicked: {why}"));
                Outcome::Failed("Something went wrong inside this job. Its log has the details.".into())
            });
            rep.send(Event::Finished { id: rep.id, outcome });
        });
        if let Err(e) = spawned {
            self.note(id, Level::Error, format!("Couldn't start a worker thread: {e}"));
            self.finish(id, Outcome::Failed("Couldn't start a worker thread.".into()), 0.0);
        }
        id
    }

    /// Adds a job row with no worker behind it, for headless scenes that show the status bar in
    /// a fixed state.
    pub fn stage(&mut self, kind: JobKind, title: &str, detail: &str, done: u64, total: Option<u64>, phase: Phase) -> JobId {
        let id = JobId(self.next);
        self.next += 1;
        let job = Job { id, kind, title: title.into(), detail: detail.into(), done, total, phase, log: Vec::new(), started: Instant::now(), cancel: CancelToken::default() };
        self.list.push(job);
        id
    }

    /// Adds log lines to a staged job, for scenes.
    pub fn stage_log(&mut self, id: JobId, lines: &[(Level, &str)]) {
        for (level, text) in lines {
            self.note(id, *level, (*text).to_owned());
        }
    }

    /// Asks one job to stop. Only the job with this id is touched.
    pub fn cancel(&mut self, id: JobId) {
        let Some(j) = self.list.iter_mut().find(|j| j.id == id) else { return };
        if j.phase != Phase::Running {
            return;
        }
        j.cancel.cancel();
        j.phase = Phase::Stopping;
        let what = if j.detail.is_empty() { "the current step".to_owned() } else { format!("\"{}\"", j.detail) };
        self.note(id, Level::Info, format!("Stop pressed. Winding down after {what}, so nothing is left half-written."));
    }

    /// Removes a finished job's row. Running jobs can't be dismissed, only stopped.
    pub fn dismiss(&mut self, id: JobId) {
        self.list.retain(|j| j.id != id || j.is_active());
    }

    /// Takes every event the workers sent. Returns the jobs that finished, and whether anything
    /// visible changed.
    pub fn drain(&mut self, now: f64) -> (Vec<(JobId, JobKind)>, bool) {
        let mut changed = false;
        let mut finished = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            changed = true;
            match e {
                Event::Log { id, line } => {
                    if let Some(j) = self.list.iter_mut().find(|j| j.id == id) {
                        j.push(line);
                    }
                }
                Event::Progress { id, done, total, detail } => {
                    if let Some(j) = self.list.iter_mut().find(|j| j.id == id) {
                        j.done = done;
                        j.total = total;
                        j.detail = detail;
                    }
                }
                Event::Title { id, title } => {
                    if let Some(j) = self.list.iter_mut().find(|j| j.id == id) {
                        j.title = title;
                    }
                }
                Event::Finished { id, outcome } => {
                    if let Some(j) = self.get(id) {
                        finished.push((id, j.kind));
                    }
                    self.finish(id, outcome, now);
                }
            }
        }
        (finished, changed)
    }

    fn finish(&mut self, id: JobId, outcome: Outcome, now: f64) {
        if let Some(j) = self.get(id) {
            let took = j.started.elapsed().as_secs_f64();
            let (level, how, m) = match &outcome {
                Outcome::Completed(m) => (Level::Info, "Finished", m),
                Outcome::Stopped(m) => (Level::Info, "Stopped", m),
                Outcome::Failed(m) => (Level::Error, "Failed", m),
            };
            self.note(id, level, format!("{how} after {took:.1}s: {m}"));
        }
        if let Some(j) = self.list.iter_mut().find(|j| j.id == id) {
            if matches!(outcome, Outcome::Completed(_))
                && let Some(t) = j.total
            {
                j.done = t;
            }
            j.phase = Phase::Done { outcome, at: now };
        }
    }

    /// Removes every finished job's row, when you ask. Nothing is ever removed on its own.
    pub fn clear_finished(&mut self) {
        self.list.retain(Job::is_active);
    }

    pub fn any_finished(&self) -> bool {
        self.list.iter().any(|j| !j.is_active())
    }
}

/// The local time of day, "14:12:15.123".
fn clock() -> String {
    let (tm, ms) = local_now();
    format!("{:02}:{:02}:{:02}.{ms:03}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// The local date, "2026-11-07".
fn date() -> String {
    let (tm, _) = local_now();
    format!("{}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
}

#[repr(C)]
#[derive(Default)]
struct Tm {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    tm_gmtoff: i64,
    tm_zone: usize,
}

unsafe extern "C" {
    fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
}

fn local_now() -> (Tm, u32) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs() as i64;
    let mut tm = Tm::default();
    // SAFETY: both pointers are to live locals of the right type; localtime_r is thread-safe.
    unsafe { localtime_r(&secs, &mut tm) };
    (tm, now.subsec_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(jobs: &mut Jobs, mut until: impl FnMut(&Jobs) -> bool) {
        let start = Instant::now();
        while !until(jobs) {
            let _ = jobs.drain(0.0);
            assert!(start.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::yield_now();
        }
    }

    #[test]
    fn stopping_one_job_leaves_the_other_running() {
        let mut jobs = Jobs::default();
        let spin = |rep: &mut Reporter| {
            while !rep.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            Outcome::Stopped("stopped".into())
        };
        let export = jobs.spawn(JobKind::Export, "Exporting", spin);
        let analysis = jobs.spawn(JobKind::Analysis, "Analysing", spin);

        jobs.cancel(analysis);
        wait(&mut jobs, |j| !j.get(analysis).unwrap().is_active());

        assert!(matches!(jobs.get(analysis).unwrap().phase, Phase::Done { outcome: Outcome::Stopped(_), .. }));
        assert_eq!(jobs.get(export).unwrap().phase, Phase::Running, "the export must keep going");

        jobs.cancel(export);
        wait(&mut jobs, |j| !j.get(export).unwrap().is_active());
    }

    #[test]
    fn a_panicking_job_fails_instead_of_crashing() {
        let mut jobs = Jobs::default();
        let id = jobs.spawn(JobKind::Export, "Exporting", |_| panic!("boom"));
        wait(&mut jobs, |j| !j.get(id).unwrap().is_active());
        assert!(matches!(jobs.get(id).unwrap().phase, Phase::Done { outcome: Outcome::Failed(_), .. }));
    }

    #[test]
    fn finished_jobs_stay_until_you_clear_them() {
        let mut jobs = Jobs::default();
        let ok = jobs.spawn(JobKind::Analysis, "a", |_| Outcome::Completed("done".into()));
        let bad = jobs.spawn(JobKind::Export, "b", |_| Outcome::Failed("no".into()));
        let running = jobs.spawn(JobKind::Download, "c", |rep| {
            while !rep.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            Outcome::Stopped("stopped".into())
        });
        wait(&mut jobs, |j| !j.get(ok).unwrap().is_active() && !j.get(bad).unwrap().is_active());
        let _ = jobs.drain(1_000_000.0);
        assert!(jobs.get(ok).is_some() && jobs.get(bad).is_some(), "nothing leaves on its own");
        jobs.dismiss(ok);
        assert!(jobs.get(ok).is_none());
        jobs.clear_finished();
        assert!(jobs.get(bad).is_none());
        assert!(jobs.get(running).is_some(), "clearing never touches a running job");
        jobs.cancel(running);
        wait(&mut jobs, |j| !j.get(running).unwrap().is_active());
    }

    #[test]
    fn a_job_logs_its_start_its_work_its_stop_and_its_end() {
        let mut jobs = Jobs::default();
        let id = jobs.spawn(JobKind::Analysis, "Analysing 2 tracks", |rep| {
            rep.info("one");
            rep.log().clone().warn("two, from a worker");
            while !rep.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            Outcome::Stopped("Stopped after analysing 0 tracks".into())
        });
        wait(&mut jobs, |j| j.get(id).unwrap().log.len() >= 3);
        jobs.cancel(id);
        wait(&mut jobs, |j| !j.get(id).unwrap().is_active());
        let text: Vec<_> = jobs.get(id).unwrap().log.iter().map(|l| l.text.clone()).collect();
        assert!(text[0].starts_with("Started"), "{text:?}");
        assert!(text.iter().any(|t| t == "two, from a worker"), "{text:?}");
        assert!(text.iter().any(|t| t.starts_with("Stop pressed")), "{text:?}");
        assert!(text.last().unwrap().starts_with("Stopped after"), "{text:?}");
        assert!(jobs.get(id).unwrap().log_text().contains("info  one"));
    }

    #[test]
    fn a_panic_lands_in_the_log() {
        let mut jobs = Jobs::default();
        let id = jobs.spawn(JobKind::Export, "Exporting", |_| panic!("disk on fire"));
        wait(&mut jobs, |j| !j.get(id).unwrap().is_active());
        assert!(jobs.get(id).unwrap().log.iter().any(|l| l.level == Level::Error && l.text.contains("disk on fire")));
    }
}
