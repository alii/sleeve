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
