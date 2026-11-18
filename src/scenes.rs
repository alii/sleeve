//! Fixed states for headless snapshots: `sleeve --snap out.png --scene busy`.

use crate::app::App;
use crate::jobs::{JobKind, Level, Outcome, Phase};

pub fn selected(s: &mut App) {
    // The first track with a waveform, so the details panel shows one.
    let analysed = s.loaded().and_then(|l| s.rows.iter().position(|&r| !l.index.analysis_path.get(r as usize).is_empty()));
    if let Some(id) = analysed.map(|i| s.track_id(i)) {
        s.table.select(id);
        s.sync_artwork();
    }
}

/// A track on the deck, a third of the way in.
pub fn playing(s: &mut App) {
    selected(s);
    if let Some(id) = s.table.cursor().map(str::to_owned) {
        s.player.stage(&id, 0.34);
        s.sync_deck_wave();
    }
    // Then pick another track: the deck must keep showing what's playing.
    if let Some(other) = (!s.rows.is_empty()).then(|| s.track_id(2)) {
        s.table.clear_selection();
        s.table.select(other);
        s.sync_artwork();
    }
}

/// A track on the deck whose iCloud download failed, with its Try again button.
pub fn failed(s: &mut App) {
    selected(s);
    if let Some(id) = s.table.cursor().map(str::to_owned) {
        s.player.stage(&id, 0.0);
        s.player.playing = false;
        s.player.error = Some("Couldn't play Audio, Video, Disco: couldn't download it from iCloud. Check you're online and try again.".into());
        s.sync_deck_wave();
    }
}

/// The demo's one title from the far corners of Unicode, alone in the list, to check fallback.
pub fn glyphs(s: &mut App) {
    s.search = rune::TextEdit::new("✧");
    s.refresh_rows();
    if !s.rows.is_empty() {
        let id = s.track_id(0);
        s.table.select(id);
        s.sync_artwork();
    }
}

/// Four jobs, more than the footer holds, with the Activity panel open.
pub fn activity(s: &mut App) {
    busy(s);
    s.jobs.stage(JobKind::Export, "Exporting to SANDISK", "Undertow (Dub)", 40, Some(120), Phase::Running);
    s.activity_open = true;
}

/// A failed analysis with its log open, as clicking its chip shows it.
pub fn log(s: &mut App) {
    selected(s);
    s.jobs.stage(JobKind::Export, "Exporting to KINGSTON", "Halcyon Drift", 212, Some(540), Phase::Running);
    let failed = Outcome::Failed("Couldn't analyse All Around: couldn't download it from iCloud".into());
    let id = s.jobs.stage(JobKind::Analysis, "Analysing 1 track", "All Around", 1, Some(1), Phase::Done { outcome: failed, at: 0.0 });
    s.jobs.stage_log(
        id,
        &[
            (Level::Info, "Started: Analysing 1 track"),
            (Level::Info, "1 track to analyse, 1 at a time. Library: /Users/you/Library/Pioneer/rekordbox"),
            (Level::Info, "\"All Around\": only in iCloud (68.2 MB). Downloading it first."),
            (Level::Error, "\"All Around\": couldn't download it from iCloud. iCloud download failed: Resource deadlock avoided (os error 11) (errno 11, Deadlock). File: /Users/you/Music/Break/Symmetry/All Around.aiff"),
            (Level::Error, "Failed after 0.4s: Couldn't analyse All Around: couldn't download it from iCloud"),
        ],
    );
    s.activity_open = true;
    s.log_open = Some(id);
}

/// The sidebar closed: the toolbar sits right beside the traffic lights.
pub fn collapsed(s: &mut App) {
    s.sidebar.toggle();
}

pub fn busy(s: &mut App) {
    selected(s);
    s.jobs.stage(JobKind::Export, "Exporting to KINGSTON", "Halcyon Drift", 212, Some(540), Phase::Running);
    s.jobs.stage(JobKind::Analysis, "Analysing 48 tracks", "Paper Orbit", 17, Some(48), Phase::Running);
    s.jobs.stage(JobKind::Analysis, "Analysing 12 tracks", "", 3, Some(12), Phase::Stopping);
}

#[allow(dead_code)]
pub fn finished(s: &mut App) {
    s.jobs.stage(JobKind::Export, "", "", 1, Some(1), Phase::Done { outcome: Outcome::Completed("Exported 540 tracks to KINGSTON".into()), at: 0.0 });
}

/// Renaming the second playlist in place.
pub fn renaming(s: &mut App) {
    if let Some(id) = s.tree.get(1).map(|n| n.id) {
        s.start_rename(id);
    }
}

/// The question before a playlist is deleted.
pub fn confirm(s: &mut App) {
    if let Some(id) = s.tree.first().map(|n| n.id) {
        s.confirm = Some(crate::edits::Confirm::DeletePlaylist(id));
    }
}

/// A search that matches nothing.
pub fn empty(s: &mut App) {
    s.search = rune::TextEdit::new("zzzz nothing");
    s.refresh_rows();
}

/// A newer build coming down in the background.
pub fn update_downloading(s: &mut App) {
    selected(s);
    s.update.status = crate::update::Status::Downloading { build: 14, done: 17_400_000, total: 41_760_823 };
}

/// A newer build downloaded and checked, waiting for Restart.
pub fn update_ready(s: &mut App) {
    selected(s);
    s.update.status = crate::update::Status::Ready { build: 14 };
}

/// The download failed, with a way to try again.
pub fn update_failed(s: &mut App) {
    selected(s);
    s.update.status = crate::update::Status::Failed { build: 14, why: "couldn't reach GitHub. Check you're online".into() };
}

/// Sleeve is running from its disk image, so it can't replace itself.
pub fn update_manual(s: &mut App) {
    selected(s);
    s.update.status = crate::update::Status::Manual { build: 14, why: "Sleeve is running from its disk image. Drag it to your Applications folder and open it from there.".into() };
}

/// Check for Updates from the menu found nothing new.
pub fn update_current(s: &mut App) {
    selected(s);
    s.update.manual = Some(crate::update::Manual::UpToDate);
}

/// Check for Updates from the menu couldn't reach GitHub.
pub fn update_check_failed(s: &mut App) {
    selected(s);
    s.update.manual = Some(crate::update::Manual::Failed("couldn't reach GitHub. Check you're online".into()));
}

/// Sleeve's own title bar, as Windows shows it. A Mac draws no caption buttons in it.
pub fn windows_titlebar(s: &mut App) {
    selected(s);
    s.preview_titlebar = true;
}

/// The Sleeve menu open in the Windows title bar.
pub fn windows_menu(s: &mut App) {
    windows_titlebar(s);
    s.app_menu = true;
}

/// The Windows title bar with the sidebar closed.
pub fn windows_collapsed(s: &mut App) {
    windows_titlebar(s);
    s.sidebar.toggle();
}

/// The Windows chrome with the inspector closed: the caption buttons over the toolbar's end.
pub fn windows_no_inspector(s: &mut App) {
    windows_titlebar(s);
    s.inspector.toggle();
}

/// An update ready with the sidebar closed: the card floats in the corner instead.
pub fn update_collapsed(s: &mut App) {
    update_ready(s);
    s.sidebar.toggle();
}

/// The deck track's grid, read now. The app reads it on a worker and it lands in `poll`, after a
/// scene has run, but the ramp scenes need it to turn Grid on.
fn deck_grid_now(s: &mut App) {
    // The worker's copy, landing after the scene, would put back the grid as it was on disk, so
    // it's taken first and the scene's edits go on top.
    s.wait_for_deck_grid();
    let Some(l) = s.loaded() else { return };
    let Some(row) = s.deck_row() else { return };
    let beats = crate::deck::read_beats(&l.location.share_root, l.index.analysis_path.get(row as usize));
    if let Some(w) = s.deck_wave.as_mut() {
        w.ramps = crate::tempo_ramp::find(&beats);
        w.beats = beats;
    }

}

/// Grid on, with a tempo change from the deck track's tempo to 15% faster opened at the playhead,
/// its start moved four seconds back and its end four seconds on, and shaped Medium. Paused, so
/// the handle that shapes it shows.
pub fn ramp(s: &mut App) {
    ramp_unplaced(s);
    let from = match s.ramp_edit() {
        Some(a) => a.ramp.from_bpm,
        None => return,
    };
    s.set_ramp_to_bpm((from * 115.0).round() / 100.0);
    s.set_ramp_feel(crate::tempo_ramp::Feel::Medium);
}

/// The `ramp` scene with one undo: still shaping the ramp, the feel back to what it was before
/// Medium was picked.
pub fn ramp_undone(s: &mut App) {
    ramp(s);
    s.undo_grid();
}

/// A tempo change just opened at the playhead and its ends dragged out four seconds either side,
/// waiting for the tempo it changes to.
pub fn ramp_unplaced(s: &mut App) {
    playing(s);
    s.player.pause();
    s.zoom_secs = 16.0;
    deck_grid_now(s);
    s.toggle_grid_edit();
    let Some(total) = s.deck_duration_ms() else { return };
    let now = s.player.now() * total;
    s.start_ramp();
    let Some((start, end)) = s.ramp_edit().map(|a| (a.ramp.start_ms, a.ramp.end_ms)) else { return };
    for (from, to) in [(start, now - 4000.0), (end, now + 4000.0)] {
        s.grid_drag(rune::Phase::Down, from, 50.0, false, false);
        s.grid_drag(rune::Phase::Up, to, 50.0, false, false);
    }
}

/// The `ramp` scene with a beat in the middle of the tempo change dragged 150 ms later and pinned
/// there: the beats either side spread to meet it, and the tempo line kinks through it.
pub fn ramp_warped(s: &mut App) {
    ramp(s);
    let middle = s.ramp_edit().and_then(|a| a.times.get(a.start + a.ramp.beats as usize / 2).copied());
    let Some(t) = middle else { return };
    s.grid_drag(rune::Phase::Down, t, 50.0, false, false);
    s.grid_drag(rune::Phase::Move, t + 150.0, 50.0, false, false);
    s.grid_drag(rune::Phase::Up, t + 150.0, 50.0, false, false);
}

/// The `ramp-warped` scene once it's done: the tempo change is found in the grid and shows
/// tinted on both waveforms, labelled with its tempos. (With Grid off it shows the same, once the
/// saved grid has been read back, which a snapshot doesn't wait for.)
pub fn ramp_found(s: &mut App) {
    ramp_warped(s);
    s.finish_ramp();
    // The search a worker would finish after the snapshot, done now.
    s.find_ramps_now();
}

/// The `ramp-warped` scene once it's done, then TEMPO again with the playhead still in it: it
/// opens again as it was left, Medium, with its dragged beat pinned.
pub fn ramp_reopened(s: &mut App) {
    ramp_warped(s);
    s.finish_ramp();
    s.start_ramp();
}

/// The `ramp` scene with its handle Alt-dragged 60 points up: the tempo line bows up, getting to
/// the new tempo sooner, so the end comes earlier.
pub fn ramp_bent(s: &mut App) {
    ramp(s);
    s.begin_bend(true);
    s.bend(-60.0);
    s.end_bend(false);
}

/// The `ramp-bent` scene with its handle double-clicked: Gradual, and the end back where the
/// `ramp` scene put it.
pub fn ramp_reset(s: &mut App) {
    ramp_bent(s);
    s.reset_bend();
}

/// The `ramp` scene with 0 typed into its bars: an instant change on its start beat, the grid
/// after it at the new tempo straight away.
pub fn ramp_instant(s: &mut App) {
    ramp(s);
    s.ramp_bars = rune::TextEdit::new("0");
    s.submit_ramp_bars();
}

/// The `ramp-instant` scene once it's done: found in the grid as an instant change, a glowing
/// line labelled with its tempos.
pub fn ramp_instant_found(s: &mut App) {
    ramp_instant(s);
    s.finish_ramp();
    s.find_ramps_now();
}
