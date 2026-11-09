//! Analysis: tempo, beat grid, key and waveforms, saved where rekordbox looks for them.
//!
//! One job analyses a list of tracks, a few at a time. Pressing its stop button stops this job
//! only: tracks still decoding give up at their next chunk and write nothing, and tracks
//! already saved stay saved.

use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::channel;

use rbl_index::{Library as Index, Row};

use crate::jobs::{CancelToken, Logger, Outcome, Reporter};
use crate::store::{Store, WriteError};

/// The first half hour feeds tempo and key; the rest only the waveform and grid.
const HEAD_SECS: f64 = 1800.0;

pub struct Track {
    pub id: String,
    pub title: String,
    pub path: String,
    pub analysis_path: String,
}

pub fn tracks(index: &Index, rows: &[Row]) -> Vec<Track> {
    rows.iter()
        .map(|&r| {
            let i = r as usize;
            Track {
                id: index.ids[i].to_string(),
                title: index.title.get(i).to_owned(),
                path: index.folder_path.get(i).to_owned(),
                analysis_path: index.analysis_path.get(i).to_owned(),
            }
        })
        .collect()
}

enum TrackResult {
    /// Saved, with a line for the log: what it found and how long it took.
    Saved(String),
    Skipped,
    /// Why, in words for the status bar, and the technical detail for the log.
    Failed(String, String),
    /// The library refused the write. Every other track would be refused too.
    Refused(String),
}

/// The job's work. Runs on the job's thread and starts its own workers.
pub fn run(store: &Arc<Store>, tracks: Vec<Track>, rep: &mut Reporter) -> Outcome {
    let total = tracks.len() as u64;
    if let Some(why) = store.refusal() {
        return Outcome::Failed(why.into());
    }
    rep.progress(0, Some(total), "");
    let workers = std::thread::available_parallelism().map_or(2, |n| (n.get() / 3).clamp(1, 4));
    rep.info(format!("{} to analyse, {workers} at a time. Library: {}", if total == 1 { "1 track".to_owned() } else { format!("{total} tracks") }, store.location.share_root.display()));
    let tracks = Arc::new(tracks);
    let next = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = channel::<(usize, TrackResult)>();
    let token = rep.token();
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let (tracks, next, tx, store, token, log) = (tracks.clone(), next.clone(), tx.clone(), store.clone(), token.clone(), rep.log().clone());
            std::thread::spawn(move || {
                loop {
                    if token.is_cancelled() {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(t) = tracks.get(i) else { break };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| one(&store, t, &token, Some(&log))))
                        .unwrap_or_else(|_| TrackResult::Failed("the analyser crashed on this file".into(), "panic inside the analyser".into()));
                    if tx.send((i, result)).is_err() {
                        break;
                    }
                }
            })
        })
        .collect();
    drop(tx);

    let (mut saved, mut failed, mut done, mut refused) = (0u64, Vec::<String>::new(), 0u64, None::<String>);
    for (i, result) in rx {
        done += 1;
        let t = &tracks[i];
        match result {
            TrackResult::Saved(line) => {
                saved += 1;
                rep.info(format!("\"{}\": {line}", t.title));
            }
            TrackResult::Skipped => rep.info(format!("\"{}\": stopped before saving. Nothing was written for it.", t.title)),
            TrackResult::Failed(why, detail) => {
                rep.error(format!("\"{}\": {why}. {detail}. File: {}", t.title, t.path));
                failed.push(format!("{}: {why}", t.title));
            }
            // Stop the workers: the rest would only be refused one by one.
            TrackResult::Refused(why) => {
                rep.warn(format!("The library refused a write, so the rest won't be tried: {why}"));
                token.cancel();
                refused.get_or_insert(why);
            }
        }
        rep.progress(done, Some(total), tracks[i].title.clone());
    }
    for h in handles {
        let _ = h.join();
    }

    let n = |k: u64| if k == 1 { "1 track".to_owned() } else { format!("{k} tracks") };
    if let Some(why) = refused {
        return Outcome::Failed(if saved == 0 { why } else { format!("Stopped after {}: {why}", n(saved)) });
    }
    if rep.is_cancelled() {
        return Outcome::Stopped(format!("Stopped after analysing {}", n(saved)));
    }
    match failed.len() {
        0 => Outcome::Completed(format!("Analysed {}", n(saved))),
        f if saved == 0 => Outcome::Failed(if f == 1 { format!("Couldn't analyse {}", failed[0]) } else { format!("Couldn't analyse any of the {f} tracks") }),
        f => Outcome::Completed(format!("Analysed {}. {f} couldn't be read.", n(saved))),
    }
}

/// Analyses one track on this thread, for building the demo library.
pub fn analyse_now(store: &Store, t: &Track) -> Result<(), String> {
    match one(store, t, &CancelToken::default(), None) {
        TrackResult::Saved(_) => Ok(()),
        TrackResult::Skipped => Err("skipped".into()),
        TrackResult::Failed(e, _) | TrackResult::Refused(e) => Err(e),
    }
}

fn one(store: &Store, t: &Track, cancel: &CancelToken, log: Option<&Logger>) -> TrackResult {
    let path = Path::new(&t.path);
    if t.path.is_empty() {
        return TrackResult::Failed("it has no file".into(), "The library row has an empty FolderPath".into());
    }
    if let Err(e) = std::fs::symlink_metadata(path) {
        return TrackResult::Failed("its file is missing".into(), format!("stat failed: {e}"));
    }
    if crate::icloud::is_evicted(path) {
        let started = std::time::Instant::now();
        let size = std::fs::metadata(path).map_or(0, |m| m.len());
        if let Some(l) = log {
            l.info(format!("\"{}\": only in iCloud ({}). Downloading it first.", t.title, crate::icloud::size(size)));
        }
        match crate::icloud::download(path, || cancel.is_cancelled(), |_, _| {}) {
            Ok(()) => {
                if let Some(l) = log {
                    l.info(format!("\"{}\": downloaded in {:.1}s.", t.title, started.elapsed().as_secs_f64()));
                }
            }
            Err(crate::icloud::DownloadError::Stopped) => return TrackResult::Skipped,
            Err(e) => return TrackResult::Failed(e.to_string(), format!("iCloud download failed: {}", e.detail())),
        }
    }
    let started = std::time::Instant::now();
    let (analysis, waves, secs) = match decode_and_analyse(path, cancel) {
        Ok(found) => found,
        Err(_) if cancel.is_cancelled() => return TrackResult::Skipped,
        Err(e) => return TrackResult::Failed("its audio couldn't be decoded".into(), e.to_string()),
    };
    // Last chance to stop before anything is written.
    if cancel.is_cancelled() {
        return TrackResult::Skipped;
    }
    let found = format!(
        "{:.2} BPM (confidence {:.2}), key {}, {}. Analysed in {:.1}s",
        analysis.tempo.bpm,
        analysis.tempo.confidence,
        analysis.key.as_ref().map_or_else(|| "not found".to_owned(), |k| format!("{} ({})", k.camelot(), k.name)),
        crate::library::duration(secs as u32),
        started.elapsed().as_secs_f64(),
    );
    match save(store, t, &analysis, &waves, secs) {
        Ok(()) => TrackResult::Saved(format!("{found}, saved.")),
        Err(SaveError::Write(WriteError::Refused(why))) => TrackResult::Refused(why),
        Err(SaveError::Write(WriteError::Failed(e))) => TrackResult::Failed("the library wouldn't take its results".into(), format!("database write failed: {e}")),
        Err(SaveError::Files(e)) => TrackResult::Failed("couldn't write its analysis files".into(), format!("ANLZ write failed: {e} ({:?})", e.kind())),
    }
}

#[derive(Debug)]
enum DecodeError {
    Open(rbl_audio::AudioError),
    Empty { rate: u32 },
    Stopped,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Open(e) => write!(f, "decoder couldn't open it: {e}"),
            DecodeError::Empty { rate } => write!(f, "decoder returned no samples at {rate} Hz"),
            DecodeError::Stopped => write!(f, "stopped"),
        }
    }
}

fn decode_and_analyse(path: &Path, cancel: &CancelToken) -> Result<(rbl_analysis::Analysis, rbl_anlz::RekordboxWaves, f64), DecodeError> {
    let mut stream = rbl_audio::MonoStream::open(path).map_err(DecodeError::Open)?;
    let rate = stream.sample_rate();
    let mut creator = rbl_analysis::wave_creator::WaveCreator::new(rate);
    if let Some(frames) = stream.expected_frames() {
        creator.reserve(frames);
    }
    let cap = (HEAD_SECS * f64::from(rate)) as usize;
    let mut head: Vec<f32> = Vec::with_capacity(cap.min(1 << 24));
    let mut tail: Option<rbl_analysis::waveform::Builder> = None;
    let mut past_head = false;
    while let Some((chunk, frames)) = stream.next_frames() {
        if cancel.is_cancelled() {
            return Err(DecodeError::Stopped);
        }
        creator.push(frames);
        if past_head {
            if let Some(b) = tail.as_mut() {
                b.push(chunk);
            }
        } else {
            head.extend_from_slice(chunk);
            if head.len() > cap {
                let mut b = rbl_analysis::waveform::Builder::new(rate);
                b.push(&head);
                tail = Some(b);
                head.truncate(cap);
                past_head = true;
            }
        }
    }
    if head.is_empty() {
        return Err(DecodeError::Empty { rate });
    }
    let options = rbl_analysis::AnalysisPreset::Rbxport.options();
    let mut analysis = rbl_analysis::analyse_with(&head, rate, options);
    let w = creator.finish();
    let waves = rbl_anlz::RekordboxWaves { pwav: w.pwav, pwv2: w.pwv2, pwvc: w.pwvc, pwv3: w.pwv3, pwv4: w.pwv4, pwv5: w.pwv5, pwv6: w.pwv6, pwv7: w.pwv7 };
    let secs = match tail {
        Some(b) => {
            let secs = b.samples() as f64 / f64::from(rate.max(1));
            analysis.peak = analysis.peak.max(b.peak());
            analysis.tempo.extend_to(secs);
            analysis.waveform = b.finish();
            secs
        }
        None => head.len() as f64 / f64::from(rate.max(1)),
    };
    Ok((analysis, waves, secs))
}

/// Writes the three analysis files and then the library row. If the row can't be written, the
/// files are put back as they were, so the two never disagree.
#[derive(Debug)]
enum SaveError {
    Write(WriteError),
    Files(std::io::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Write(e) => write!(f, "{e}"),
            SaveError::Files(e) => write!(f, "couldn't write its analysis files ({e})"),
        }
    }
}

impl From<WriteError> for SaveError {
    fn from(e: WriteError) -> Self {
        SaveError::Write(e)
    }
}

fn save(store: &Store, t: &Track, analysis: &rbl_analysis::Analysis, waves: &rbl_anlz::RekordboxWaves, secs: f64) -> Result<(), SaveError> {
    let share = &store.location.share_root;
    let relative = if t.analysis_path.is_empty() { store.write(|w| w.analysis_data_path_for(&t.id))? } else { t.analysis_path.clone() };
    let dat = rbl_anlz::resolve(share, &relative);
    let paths = [dat.clone(), rbl_anlz::sibling(&dat, "EXT"), rbl_anlz::sibling(&dat, "2EX")];
    let existing: Vec<Option<rbl_anlz::Anlz>> = paths.iter().map(|p| rbl_anlz::Anlz::read(p).ok()).collect();
    let beats: Vec<rbl_anlz::Beat> = analysis
        .tempo
        .beats
        .iter()
        .map(|b| rbl_anlz::Beat { beat_number: b.beat_number, tempo_x100: b.tempo_x100, time_ms: b.time_ms })
        .collect();
    let files = rbl_anlz::author_with_rekordbox_waves(
        &rbl_anlz::share_tree_ppth(&t.path),
        &beats,
        waves,
        rbl_anlz::Existing { dat: existing[0].as_ref(), ext: existing[1].as_ref(), two_ex: existing[2].as_ref() },
    );
    let to_u32 = |v: f64| v.round().clamp(0.0, f64::from(u32::MAX)) as u32;
    let bpm_x100 = to_u32(analysis.tempo.bpm * 100.0);
    let key = analysis.key.as_ref().map(|k| k.name.clone());

    let swap = FileSwap::write(&paths, [files.dat, files.ext, files.two_ex]).map_err(SaveError::Files)?;
    let written = store.write(|w| {
        if let Some(k) = key.as_deref() {
            w.ensure_detected_key(k)?;
        }
        w.set_analysis(&t.id, &rbl_db::write::AnalysisWrite { bpm_x100, key: key.as_deref(), analysis_path: &relative, length_sec: Some(to_u32(secs)) })
    });
    match written {
        Ok(_) => {
            swap.commit();
            Ok(())
        }
        Err(e) => {
            swap.undo();
            Err(e.into())
        }
    }
}

/// New files moved into place with the old ones kept aside until the library row is written.
struct FileSwap {
    moved: Vec<(std::path::PathBuf, Option<std::path::PathBuf>)>,
}

impl FileSwap {
    fn write(paths: &[std::path::PathBuf; 3], bytes: [Vec<u8>; 3]) -> std::io::Result<FileSwap> {
        let mut swap = FileSwap { moved: Vec::new() };
        for (path, data) in paths.iter().zip(bytes) {
            let step = || -> std::io::Result<Option<std::path::PathBuf>> {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let tmp = path.with_extension(format!("{}.sleeve-new", ext(path)));
                // Flushed through the handle that wrote it: Windows refuses `sync_all` on a
                // read-only one.
                let mut file = std::fs::File::create(&tmp)?;
                file.write_all(&data)?;
                file.sync_all()?;
                drop(file);
                let old = path.exists().then(|| path.with_extension(format!("{}.sleeve-old", ext(path))));
                if let Some(old) = &old {
                    std::fs::rename(path, old)?;
                }
                std::fs::rename(&tmp, path)?;
                Ok(old)
            };
            match step() {
                Ok(old) => swap.moved.push((path.clone(), old)),
                Err(e) => {
                    swap.undo();
                    return Err(e);
                }
            }
        }
        Ok(swap)
    }

    fn commit(self) {
        for (_, old) in self.moved {
            if let Some(old) = old {
                let _ = std::fs::remove_file(old);
            }
        }
    }

    fn undo(self) {
        for (path, old) in self.moved.into_iter().rev() {
            match old {
                Some(old) => {
                    let _ = std::fs::rename(old, &path);
                }
                None => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
}

fn ext(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::jobs::{JobKind, Jobs, Phase};
    use std::time::{Duration, Instant};

    /// 30 seconds of kicks at 120 BPM, 44.1 kHz stereo 16-bit.
    pub(crate) fn click_track(path: &Path) {
        let rate = 44_100u32;
        let frames = rate * 30;
        let mut data = Vec::with_capacity(frames as usize * 4);
        for n in 0..frames {
            let t = n as f32 / rate as f32;
            let since = t % 0.5;
            let kick = (-since * 30.0).exp() * (2.0 * std::f32::consts::PI * (55.0 + 90.0 * (-since * 40.0).exp()) * since).sin();
            let s = (kick * 0.8 * f32::from(i16::MAX)) as i16;
            data.extend_from_slice(&s.to_le_bytes());
            data.extend_from_slice(&s.to_le_bytes());
        }
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);
        std::fs::write(path, wav).unwrap();
    }

    pub(crate) fn setup(name: &str, n: usize) -> (std::path::PathBuf, Arc<Store>) {
        let dir = std::env::temp_dir().join(format!("sleeve-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let loc = rbl_db::fixture::build(&dir.join("lib"), rbl_db::fixture::Shape { tracks: n, ..Default::default() }).unwrap();
        for i in 0..n {
            let audio = dir.join(format!("beat{i}.wav"));
            click_track(&audio);
            rbl_db::fixture::point_at_audio(&loc, i, audio.to_str().unwrap(), 30).unwrap();
        }
        (dir, Arc::new(Store::new_in(loc, std::env::temp_dir().join(format!("sleeve-test-{name}-backups-{}", std::process::id())))))
    }

    fn index(store: &Store) -> rbl_index::Library {
        let db = rbl_db::Library::open(store.location.clone(), rbl_db::OpenMode::ReadOnly).unwrap();
        rbl_index::load(&db).unwrap().0
    }

    pub(crate) fn wait(jobs: &mut Jobs, id: crate::jobs::JobId) -> Phase {
        let start = Instant::now();
        loop {
            let _ = jobs.drain(0.0);
            let j = jobs.get(id).unwrap();
            if !j.is_active() {
                return j.phase.clone();
            }
            assert!(start.elapsed() < Duration::from_secs(120), "analysis timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn analysis_writes_the_grid_and_the_row() {
        let (_dir, store) = setup("analyse", 1);
        let before = index(&store);
        let row = before.row_of(&rbl_db::fixture::track_id(0)).unwrap();
        let tracks = tracks(&before, &[row]);
        let mut jobs = Jobs::default();
        let s = store.clone();
        let id = jobs.spawn(JobKind::Analysis, "a", move |rep| run(&s, tracks, rep));
        let phase = wait(&mut jobs, id);
        assert!(matches!(&phase, Phase::Done { outcome: Outcome::Completed(m), .. } if m == "Analysed 1 track"), "{phase:?}");

        let after = index(&store);
        let r = after.row_of(&rbl_db::fixture::track_id(0)).unwrap() as usize;
        let bpm = f64::from(after.bpm_x100[r]) / 100.0;
        assert!((bpm - 120.0).abs() < 0.6, "tempo was {bpm}");
        let rel = after.analysis_path.get(r);
        assert!(!rel.is_empty());
        let dat = rbl_anlz::resolve(&store.location.share_root, rel);
        for p in [dat.clone(), rbl_anlz::sibling(&dat, "EXT"), rbl_anlz::sibling(&dat, "2EX")] {
            assert!(rbl_anlz::Anlz::read(&p).is_ok(), "{} should parse", p.display());
        }
        // No temporary files left behind.
        let leftovers = std::fs::read_dir(dat.parent().unwrap()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains("sleeve-")).count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn stopping_writes_nothing_more() {
        let (_dir, store) = setup("stop", 3);
        let before = index(&store);
        let rows: Vec<_> = (0..3).map(|i| before.row_of(&rbl_db::fixture::track_id(i)).unwrap()).collect();
        let tracks = tracks(&before, &rows);
        let mut jobs = Jobs::default();
        let s = store.clone();
        let id = jobs.spawn(JobKind::Analysis, "a", move |rep| {
            rep.token().cancel();
            run(&s, tracks, rep)
        });
        let phase = wait(&mut jobs, id);
        assert!(matches!(&phase, Phase::Done { outcome: Outcome::Stopped(_), .. }), "{phase:?}");
        let after = index(&store);
        for &r in &rows {
            assert_eq!(after.analysis_path.get(r as usize), before.analysis_path.get(r as usize));
        }
    }
}
