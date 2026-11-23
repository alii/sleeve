//! Auto-cue: hot cues at the points that matter in a track's structure, or every so many bars
//! when the structure can't be found. New cues go on the empty pads only. Cues already there are
//! never moved, replaced or deleted, and memory cues play no part.

use std::path::Path;

use rbl_index::{Library as Index, Row};

use crate::deck::kind_of_slot;
use crate::jobs::{Outcome, Reporter};
use crate::store::{Store, WriteError};
use crate::structure::{Label, Structure};

const PADS: usize = 8;

/// Bar-count fallback intervals, tried shortest first.
const FALLBACK_EVERY: [usize; 4] = [8, 16, 32, 64];

/// Fill-in cues step through a section every 16 bars first, then every 8.
const FILL_EVERY: [usize; 2] = [16, 8];

/// A cue on the first beat of bar `bar`, at `ms`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PlannedCue {
    bar: usize,
    ms: u32,
    comment: String,
}

/// The closest two new cues may sit, in bars.
fn min_gap(bars: usize) -> usize {
    if bars < 64 { 4 } else { 8 }
}

/// The fallback's interval: the shortest whose cues fit on the pads.
fn fallback_every(bars: usize) -> usize {
    FALLBACK_EVERY.into_iter().find(|n| bars.div_ceil(*n) <= PADS).unwrap_or(64)
}

fn label_name(label: Label) -> &'static str {
    match label {
        Label::Intro => "Intro",
        Label::Drop => "Drop",
        Label::Breakdown => "Breakdown",
        Label::Outro => "Outro",
        Label::Section => "Section",
    }
}

/// Where cues could go, most useful first. With a structure: the first beat, the first drop, the
/// outro and the mix-in point, then every other section start in time order, then phrase points
/// inside the sections, longest sections first, so a track with few sections still gets a full
/// set. Without one: the first downbeat and then every so many bars.
fn candidates(bar_ms: &[u32], structure: Option<&Structure>) -> Vec<(usize, String)> {
    let Some(s) = structure else {
        return (0..bar_ms.len()).step_by(fallback_every(bar_ms.len())).map(|bar| (bar, if bar == 0 { "First beat".to_owned() } else { format!("Bar {}", bar + 1) })).collect();
    };
    let first = |label: Label| s.sections.iter().find(|x| x.label == label).map(|x| x.start_bar);
    let mut out: Vec<(usize, String)> = vec![(0, "First beat".to_owned())];
    out.extend(first(Label::Drop).map(|d| (d, "Drop".to_owned())));
    out.extend(first(Label::Outro).map(|o| (o, "Outro".to_owned())));
    // With a long intro, 16 bars before the first drop is where to start mixing in.
    out.extend(first(Label::Drop).filter(|&d| d >= 32).map(|d| (d - 16, "Mix in".to_owned())));
    out.extend(s.sections.iter().map(|x| (x.start_bar, label_name(x.label).to_owned())));
    let mut longest: Vec<_> = s.sections.iter().collect();
    longest.sort_by_key(|x| std::cmp::Reverse(x.bars));
    for every in FILL_EVERY {
        for x in &longest {
            out.extend((every..x.bars).step_by(every).map(|off| (x.start_bar + off, format!("{} +{off}", label_name(x.label)))));
        }
    }
    out
}

/// Picks up to `room` cues from the candidates, in time order. A point less than a bar from a hot
/// cue the track already has (`taken`), or too close to one picked before it, is skipped.
fn plan(bar_ms: &[u32], structure: Option<&Structure>, taken: &[u32], room: usize) -> Vec<PlannedCue> {
    let bar_len = |bar: usize| match (bar_ms.get(bar), bar_ms.get(bar + 1), bar.checked_sub(1).and_then(|p| bar_ms.get(p))) {
        (Some(&a), Some(&b), _) => b - a,
        (Some(&a), None, Some(&p)) => a - p,
        _ => 0,
    };
    let gap = min_gap(bar_ms.len());
    let mut chosen: Vec<PlannedCue> = Vec::new();
    for (bar, comment) in candidates(bar_ms, structure) {
        let Some(&ms) = bar_ms.get(bar) else { continue };
        let clear = taken.iter().all(|&e| e.abs_diff(ms) >= bar_len(bar).max(1));
        if chosen.len() < room && clear && chosen.iter().all(|c| c.bar.abs_diff(bar) >= gap) {
            chosen.push(PlannedCue { bar, ms, comment });
        }
    }
    chosen.sort_by_key(|c| c.bar);
    chosen
}

/// What the job needs of a track, read from the library index on the main thread. Its cues
/// aren't here: they're read fresh from the library just before writing, as the index can be out
/// of date by then.
pub struct Track {
    pub id: String,
    pub title: String,
    pub analysis_path: String,
}

pub fn tracks(index: &Index, rows: &[Row]) -> Vec<Track> {
    rows.iter()
        .map(|&r| {
            let i = r as usize;
            Track { id: index.ids[i].to_string(), title: index.title.get(i).to_owned(), analysis_path: index.analysis_path.get(i).to_owned() }
        })
        .collect()
}

/// Where hot cues A to H are, from the library's `(Kind, InMsec)` rows; None for an empty pad.
fn pads(cues: &[(u8, u32)]) -> [Option<u32>; PADS] {
    let mut hot = [None; PADS];
    for slot in 0..PADS as u8 {
        hot[slot as usize] = cues.iter().find(|c| c.0 == kind_of_slot(slot)).map(|c| c.1);
    }
    hot
}

/// One hot cue to save: its `djmdCue.Kind`, where, and its comment.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CueWrite {
    kind: u8,
    ms: u32,
    comment: String,
}

/// The new hot cues, on the empty pads only, the earliest on the first free one.
fn writes(bar_ms: &[u32], structure: Option<&Structure>, hot: &[Option<u32>; PADS]) -> Vec<CueWrite> {
    let taken: Vec<u32> = hot.iter().flatten().copied().collect();
    let free = (0..PADS as u8).filter(|&slot| hot[slot as usize].is_none());
    free.zip(plan(bar_ms, structure, &taken, PADS - taken.len())).map(|(slot, c)| CueWrite { kind: kind_of_slot(slot), ms: c.ms, comment: c.comment }).collect()
}

/// How the cues were found, for the log: "Intro 1, Drop 33, ...", each section by the bar it
/// starts on as the deck numbers them, or why it fell back.
fn found(structure: Option<&Structure>, bars: usize, has_waveform: bool) -> String {
    if let Some(s) = structure {
        let map: Vec<String> = s.sections.iter().map(|x| format!("{} {}", label_name(x.label), x.start_bar + 1)).collect();
        return format!("{}. Cued from its structure", map.join(", "));
    }
    let why = if has_waveform { "No clear structure" } else { "No detailed waveform" };
    match fallback_every(bars) {
        every if bars > every => format!("{why}, so cued every {every} bars"),
        _ => format!("{why}, so cued on its first beat"),
    }
}

/// What was added, for the log, or why nothing was. `free` is the empty pads before.
fn added(hot: usize, free: usize) -> String {
    match (hot, free) {
        (0, 0) => "nothing added, as every hot cue pad is taken".to_owned(),
        (0, _) => "nothing added, as it already has hot cues at those points".to_owned(),
        (1, _) => "added 1 hot cue".to_owned(),
        (n, _) => format!("added {n} hot cues"),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum TrackResult {
    Cued(String),
    /// Nothing to add; `full` when every pad was taken.
    Nothing { line: String, full: bool },
    NotAnalysed,
    NoGrid,
    Failed(String),
    /// The library refused the write, so every other track would be refused too.
    Refused(String),
}

#[derive(Default, Debug)]
struct Tally {
    cued: usize,
    not_analysed: usize,
    no_grid: usize,
    full: usize,
    nothing: usize,
    failed: usize,
}

fn tracks_word(k: usize) -> String {
    if k == 1 { "1 track".to_owned() } else { format!("{k} tracks") }
}

/// The job's outcome: "Added cues to 12 tracks; 3 had no analysis".
fn summary(t: &Tally) -> String {
    let mut parts = vec![if t.cued == 0 { "No cues added".to_owned() } else { format!("Added cues to {}", tracks_word(t.cued)) }];
    for (k, what) in [
        (t.not_analysed, "had no analysis"),
        (t.no_grid, "had no beat grid"),
        (t.full, "had no free hot cue pads"),
        (t.nothing, "already had hot cues at those points"),
        (t.failed, "couldn't be saved"),
    ] {
        if k > 0 {
            parts.push(format!("{k} {what}"));
        }
    }
    parts.join("; ")
}

/// The job, one track at a time. Stop is checked between tracks, so none is left half cued.
pub fn run(store: &Store, tracks: Vec<Track>, rep: &mut Reporter) -> Outcome {
    if let Some(why) = store.refusal() {
        return Outcome::Failed(why.into());
    }
    let total = tracks.len() as u64;
    rep.progress(0, Some(total), "");
    rep.info(format!("{} to cue. Library: {}", tracks_word(tracks.len()), store.location.share_root.display()));
    let mut tally = Tally::default();
    for (i, t) in tracks.iter().enumerate() {
        if rep.is_cancelled() {
            break;
        }
        match one(store, t) {
            TrackResult::Cued(line) => {
                tally.cued += 1;
                rep.info(format!("\"{}\": {line}.", t.title));
            }
            TrackResult::Nothing { line, full } => {
                if full { tally.full += 1 } else { tally.nothing += 1 }
                rep.info(format!("\"{}\": {line}.", t.title));
            }
            TrackResult::NotAnalysed => {
                tally.not_analysed += 1;
                rep.info(format!("\"{}\": not analysed yet, so nothing was added. Analyse it first.", t.title));
            }
            TrackResult::NoGrid => {
                tally.no_grid += 1;
                rep.warn(format!("\"{}\": its analysis has no beat grid, or couldn't be read, so nothing was added.", t.title));
            }
            TrackResult::Failed(why) => {
                tally.failed += 1;
                rep.error(format!("\"{}\": {why}", t.title));
            }
            TrackResult::Refused(why) => {
                rep.warn(format!("The library refused a write, so the rest won't be tried: {why}"));
                return Outcome::Failed(if tally.cued == 0 { why } else { format!("Stopped after cueing {}: {why}", tracks_word(tally.cued)) });
            }
        }
        rep.progress(i as u64 + 1, Some(total), t.title.clone());
    }
    if rep.is_cancelled() {
        return Outcome::Stopped(format!("Stopped. {}", summary(&tally)));
    }
    if tally.failed > 0 && tally.failed == tracks.len() {
        return Outcome::Failed(if tally.failed == 1 { "Couldn't save the cues. The log has the details.".to_owned() } else { format!("Couldn't save cues on any of the {} tracks", tally.failed) });
    }
    Outcome::Completed(summary(&tally))
}

fn one(store: &Store, t: &Track) -> TrackResult {
    if t.analysis_path.is_empty() {
        return TrackResult::NotAnalysed;
    }
    let share: &Path = &store.location.share_root;
    let beats = crate::deck::read_beats(share, &t.analysis_path);
    let bar_ms = crate::structure::bars(&beats);
    if bar_ms.is_empty() {
        return TrackResult::NoGrid;
    }
    let cols = crate::waveform::read_detail(share, &t.analysis_path);
    let structure = cols.as_deref().and_then(|c| crate::structure::detect(&beats, c, crate::waveform::DETAIL_PER_SEC));
    let found = found(structure.as_ref(), bar_ms.len(), cols.is_some());
    // The pads are read in the same writer that fills them, so cues set since the library was
    // last read, by hand or by an earlier auto-cue, are never doubled up.
    let (mut free, mut planned, mut saved) = (0, 0, 0);
    let written = store.write(|w| {
        let hot = pads(&w.hot_cues(&t.id)?);
        free = hot.iter().filter(|h| h.is_none()).count();
        let todo = writes(&bar_ms, structure.as_ref(), &hot);
        planned = todo.len();
        for c in &todo {
            let id = w.add_cue(&t.id, c.kind, c.ms)?;
            w.set_cue_comment(&id, &c.comment)?;
            saved += 1;
        }
        Ok(())
    });
    match written {
        Ok(()) if planned == 0 => TrackResult::Nothing { line: format!("{found}: {}", added(0, free)), full: free == 0 },
        Ok(()) => TrackResult::Cued(format!("{found}: {}", added(planned, free))),
        Err(WriteError::Refused(why)) => TrackResult::Refused(why),
        Err(WriteError::Failed(e)) => TrackResult::Failed(format!("saved {saved} of its {planned} new cues before a write failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{added, found, plan, summary, writes, CueWrite, PlannedCue, Tally};
    use crate::deck::kind_of_slot;
    use crate::structure::{Label, Section, Structure};

    /// 128 BPM: a bar is 1875 ms. The first downbeat is at 250 ms.
    fn bars(n: usize) -> Vec<u32> {
        (0..n as u32).map(|i| 250 + i * 1875).collect()
    }

    /// A structure from (start bar, label) pairs, each section running to the next.
    fn structure(total: usize, parts: &[(usize, Label)]) -> Structure {
        let sections = parts
            .iter()
            .enumerate()
            .map(|(i, &(start_bar, label))| Section { start_bar, bars: parts.get(i + 1).map_or(total, |p| p.0) - start_bar, label })
            .collect();
        Structure { bar_ms: bars(total), sections }
    }

    fn placed(cues: &[PlannedCue]) -> Vec<(usize, &str)> {
        cues.iter().map(|c| (c.bar, c.comment.as_str())).collect()
    }

    /// The plan for a track with no hot cues yet.
    fn fresh(s: &Structure) -> Vec<PlannedCue> {
        plan(&s.bar_ms, Some(s), &[], 8)
    }

    fn three_minutes() -> Structure {
        use Label::*;
        structure(96, &[(0, Intro), (32, Drop), (48, Breakdown), (64, Drop), (80, Outro)])
    }

    #[test]
    fn a_three_minute_track_gets_its_section_starts_then_phrase_points_to_make_eight() {
        let p = fresh(&three_minutes());
        assert_eq!(placed(&p), [(0, "First beat"), (8, "Intro +8"), (16, "Mix in"), (24, "Intro +24"), (32, "Drop"), (48, "Breakdown"), (64, "Drop"), (80, "Outro")]);
        assert_eq!(p[4].ms, 250 + 32 * 1875);
    }

    #[test]
    fn a_track_with_few_sections_still_gets_eight() {
        use Label::*;
        let s = structure(128, &[(0, Intro), (32, Drop), (96, Outro)]);
        assert_eq!(placed(&fresh(&s)), [(0, "First beat"), (16, "Mix in"), (32, "Drop"), (48, "Drop +16"), (64, "Drop +32"), (80, "Drop +48"), (96, "Outro"), (112, "Outro +16")]);
    }

    #[test]
    fn a_long_track_keeps_the_most_useful_points() {
        use Label::*;
        let s = structure(288, &[(0, Intro), (48, Drop), (96, Breakdown), (144, Drop), (192, Breakdown), (224, Drop), (240, Section), (256, Outro)]);
        assert_eq!(placed(&fresh(&s)), [(0, "First beat"), (32, "Mix in"), (48, "Drop"), (96, "Breakdown"), (144, "Drop"), (192, "Breakdown"), (224, "Drop"), (256, "Outro")]);
        assert_eq!(placed(&plan(&s.bar_ms, Some(&s), &[], 3)), [(0, "First beat"), (48, "Drop"), (256, "Outro")]);
    }

    #[test]
    fn a_very_short_track_gets_fewer_as_the_spacing_runs_out() {
        use Label::*;
        // Under 64 bars new cues sit at least 4 bars apart.
        let s = structure(20, &[(0, Intro), (8, Drop), (16, Outro)]);
        assert_eq!(placed(&fresh(&s)), [(0, "First beat"), (8, "Drop"), (16, "Outro")]);
        // A drop 4 bars in is too close to the first beat on a 96-bar track.
        let s = structure(96, &[(0, Intro), (4, Drop), (40, Breakdown), (56, Drop), (88, Outro)]);
        assert!(!fresh(&s).iter().any(|c| c.bar == 4));
    }

    #[test]
    fn the_fallback_picks_the_shortest_interval_that_fits_eight() {
        assert_eq!(placed(&plan(&bars(40), None, &[], 8)), [(0, "First beat"), (8, "Bar 9"), (16, "Bar 17"), (24, "Bar 25"), (32, "Bar 33")]);
        assert_eq!(placed(&plan(&bars(20), None, &[], 8)), [(0, "First beat"), (8, "Bar 9"), (16, "Bar 17")]);
        // Every 32 would be 9 cues, so every 64.
        assert_eq!(plan(&bars(288), None, &[], 8).iter().map(|c| c.bar).collect::<Vec<_>>(), [0, 64, 128, 192, 256]);
        // Even every 64 is too many, so it stops at eight.
        assert_eq!(plan(&bars(1000), None, &[], 8).len(), 8);
        assert!(plan(&[], None, &[], 8).is_empty());
        assert!(plan(&[], Some(&Structure { bar_ms: Vec::new(), sections: Vec::new() }), &[], 8).is_empty());
    }

    fn written(w: &[CueWrite]) -> Vec<(u8, u32, &str)> {
        w.iter().map(|c| (c.kind, c.ms, c.comment.as_str())).collect()
    }

    #[test]
    fn new_hot_cues_fill_only_the_empty_pads_and_keep_clear_of_the_ones_there() {
        let s = three_minutes();
        let b = &s.bar_ms;
        // Pad A has a cue on the first beat, C one at bar 10 and F one half a bar after the drop.
        let mut hot = [None; 8];
        hot[0] = Some(b[0]);
        hot[2] = Some(b[10]);
        hot[5] = Some(b[32] + 900);
        let w = writes(b, Some(&s), &hot);
        // Five pads free, so five cues, on B, D, E, G and H in time order. The first beat and the
        // drop already have a cue within a bar, so they're left out.
        let kinds: Vec<u8> = [1, 3, 4, 6, 7].map(kind_of_slot).to_vec();
        assert_eq!(w.iter().map(|c| c.kind).collect::<Vec<_>>(), kinds);
        assert_eq!(written(&w).iter().map(|c| c.2).collect::<Vec<_>>(), ["Intro +8", "Mix in", "Breakdown", "Drop", "Outro"]);
        assert!(w.windows(2).all(|p| p[0].ms < p[1].ms));
        // Every pad taken: nothing.
        assert!(writes(b, Some(&s), &[Some(1); 8]).is_empty());
        // Seven taken: just the most useful point that's clear of them.
        let mut seven = [Some(900_000); 8];
        seven[3] = None;
        assert_eq!(written(&writes(b, Some(&s), &seven)), [(kind_of_slot(3), b[0], "First beat")]);
    }

    #[test]
    fn the_log_says_what_was_found_and_what_was_added() {
        let s = three_minutes();
        assert_eq!(found(Some(&s), 96, true), "Intro 1, Drop 33, Breakdown 49, Drop 65, Outro 81. Cued from its structure");
        assert_eq!(found(None, 40, true), "No clear structure, so cued every 8 bars");
        assert_eq!(found(None, 40, false), "No detailed waveform, so cued every 8 bars");
        assert_eq!(found(None, 2, true), "No clear structure, so cued on its first beat");
        assert_eq!(added(5, 5), "added 5 hot cues");
        assert_eq!(added(1, 8), "added 1 hot cue");
        assert_eq!(added(0, 0), "nothing added, as every hot cue pad is taken");
        assert_eq!(added(0, 3), "nothing added, as it already has hot cues at those points");
    }

    #[test]
    fn the_outcome_sums_it_up() {
        assert_eq!(summary(&Tally { cued: 12, not_analysed: 3, ..Default::default() }), "Added cues to 12 tracks; 3 had no analysis");
        assert_eq!(summary(&Tally { cued: 1, ..Default::default() }), "Added cues to 1 track");
        assert_eq!(
            summary(&Tally { cued: 0, not_analysed: 1, no_grid: 1, full: 2, nothing: 1, failed: 1 }),
            "No cues added; 1 had no analysis; 1 had no beat grid; 2 had no free hot cue pads; 1 already had hot cues at those points; 1 couldn't be saved"
        );
    }

    /// Runs the job against a fixture library built in a temporary folder, never a real one.
    #[test]
    fn the_job_adds_cues_and_keeps_the_ones_already_there() {
        use crate::jobs::{JobKind, Jobs, Outcome, Phase};
        use std::sync::Arc;

        let dir = std::env::temp_dir().join(format!("sleeve-test-autocue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let loc = rbl_db::fixture::build(&dir.join("lib"), rbl_db::fixture::Shape { tracks: 2, ..Default::default() }).unwrap();
        let store = Arc::new(crate::store::Store::new_in(loc.clone(), dir.join("backups")));

        // Track 0: 128 bars at 120 BPM (a bar is 2000 ms) with an intro, drop, breakdown, drop
        // and outro in its waveform. Track 1 is left unanalysed.
        let beats: Vec<rbl_anlz::Beat> = (0..512u32).map(|i| rbl_anlz::Beat { beat_number: (i % 4 + 1) as u16, tempo_x100: 12000, time_ms: i * 500 }).collect();
        let columns: Vec<rbl_anlz::BandColumn> = (0..256 * 150)
            .map(|c| {
                let (low, mid, high) = match c / 300 {
                    0..32 => (10, 40, 40),
                    32..64 => (115, 90, 80),
                    64..96 => (8, 50, 60),
                    96..112 => (115, 90, 80),
                    _ => (30, 30, 30),
                };
                rbl_anlz::BandColumn { low, mid, high, peak: low.max(mid).max(high) }
            })
            .collect();
        let files = rbl_anlz::author(&rbl_anlz::share_tree_ppth("/music/a.wav"), &beats, &columns, rbl_anlz::Existing { dat: None, ext: None, two_ex: None });
        let rel = "/PIONEER/USBANLZ/aaa/bbbb/ANLZ0000.DAT";
        let dat = rbl_anlz::resolve(&loc.share_root, rel);
        std::fs::create_dir_all(dat.parent().unwrap()).unwrap();
        std::fs::write(&dat, files.dat).unwrap();
        std::fs::write(rbl_anlz::sibling(&dat, "EXT"), files.ext).unwrap();
        std::fs::write(rbl_anlz::sibling(&dat, "2EX"), files.two_ex).unwrap();
        rbl_db::fixture::set_analysis_path(&loc, 0, rel).unwrap();

        // Cues set by hand: hot cue A on the first beat, B mid-intro, and a memory cue on the
        // first drop, which auto-cue leaves alone and pays no heed to.
        let id = rbl_db::fixture::track_id(0);
        for (kind, ms) in [(1u8, 0u32), (2, 20_000), (0, 64_000)] {
            store.write(|w| w.add_cue(&id, kind, ms).map(|_| ())).unwrap();
        }

        let index = |store: &crate::store::Store| {
            let db = rbl_db::Library::open(store.location.clone(), rbl_db::OpenMode::ReadOnly).unwrap();
            rbl_index::load(&db).unwrap().0
        };
        let before = index(&store);
        let rows: Vec<_> = (0..2).map(|i| before.row_of(&rbl_db::fixture::track_id(i)).unwrap()).collect();
        let kept = before.cues_of(rows[0]);
        let tracks = super::tracks(&before, &rows);
        let again = super::tracks(&before, &rows);

        let mut jobs = Jobs::default();
        let s = store.clone();
        let job = jobs.spawn(JobKind::AutoCue, "a", move |rep| super::run(&s, tracks, rep));
        let phase = crate::analysis::tests::wait(&mut jobs, job);
        let log: Vec<String> = jobs.get(job).unwrap().log.iter().map(|l| l.text.clone()).collect();
        assert!(matches!(&phase, Phase::Done { outcome: Outcome::Completed(m), .. } if m == "Added cues to 1 track; 1 had no analysis"), "{phase:?}\n{log:#?}");
        assert!(log.iter().any(|l| l.contains("not analysed yet")), "{log:#?}");
        assert!(log.iter().any(|l| l.contains("Intro 1, Drop 33") && l.ends_with("Cued from its structure: added 6 hot cues.")), "{log:#?}");

        // Run again from the same, now stale, read of the library: every pad is taken, so
        // nothing is added and no pad gets a second cue.
        let s = store.clone();
        let job = jobs.spawn(JobKind::AutoCue, "b", move |rep| super::run(&s, again, rep));
        crate::analysis::tests::wait(&mut jobs, job);

        let after = index(&store);
        let cues = after.cues_of(after.row_of(&id).unwrap());
        // Every cue that was there is still there, as it was.
        for k in &kept {
            assert!(cues.iter().any(|c| c.id == k.id && c.position_ms == k.position_ms && c.kind == k.kind), "{k:?} went missing");
        }
        // Six new hot cues fill pads C to H, each on a bar line, and no memory cue is added.
        let new: Vec<_> = cues.iter().filter(|c| !kept.iter().any(|k| k.id == c.id)).collect();
        let mut letters: Vec<char> = new.iter().filter_map(|c| c.hot_letter()).collect();
        letters.sort_unstable();
        assert_eq!(letters, ['C', 'D', 'E', 'F', 'G', 'H'], "{new:?}");
        assert_eq!(new.len(), 6);
        assert!(new.iter().all(|c| c.position_ms % 2000 == 0), "{new:?}");
        // The memory cue on the drop doesn't keep a hot cue off it.
        assert!(new.iter().any(|c| c.position_ms == 64_000), "{new:?}");
        let db = rbl_db::Library::open(store.location.clone(), rbl_db::OpenMode::ReadOnly).unwrap();
        for c in &new {
            let comment: String = db.connection().query_row("SELECT Comment FROM djmdCue WHERE ID = ?1", [c.id.to_string()], |r| r.get(0)).unwrap();
            assert!(!comment.is_empty(), "cue {} has no comment", c.id);
        }
        assert_eq!(after.cues_of(after.row_of(&rbl_db::fixture::track_id(1)).unwrap()).len(), before.cues_of(rows[1]).len());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
