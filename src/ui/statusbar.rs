//! The panel's footer: what's in view on the left, and one chip per job on the right. Chips
//! that don't fit fold into "+N more", and the Activity panel above the footer lists every job.
//!
//! Each chip and each Activity row is built from one `Job`, and everything in it is keyed by
//! that job's id. Its stop button captures that id and nothing else, so it can only ever stop
//! the job it sits in.

use rune::{El, Icon, Transition, col, el, row, spacer, text};
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit, Spinner};

use crate::app::App;
use crate::jobs::{Job, Level, Outcome, Phase};
use crate::ui::icons;
use crate::ui::tracks::INSET;

pub const HEIGHT: f32 = 32.0;
/// A compact chip's width, so the footer knows how many fit.
const CHIP_W: f32 = 236.0;
/// Room the summary on the left keeps before chips take over.
const SUMMARY_W: f32 = 150.0;
/// The "+N more" button's width.
const MORE_W: f32 = 76.0;

pub fn build(s: &App, width: f32, k: &Kit) -> El {
    let c = &k.c;
    let summary = text(s.summary()).key("status-summary").font(k.ty.small).color(c.text_muted).truncate();

    let jobs = s.jobs.list();
    let free = width - 2.0 * INSET - SUMMARY_W;
    let fits = |n: usize| n as f32 * (CHIP_W + 6.0) <= free;
    let shown = if fits(jobs.len()) { jobs.len() } else { (0..jobs.len()).rev().find(|&n| n as f32 * (CHIP_W + 6.0) + MORE_W <= free).unwrap_or(0).max(1) };
    let hidden = jobs.len() - shown;
    let mut chips = row().key("status-jobs").gap(6.0).center_y();
    if hidden > 0 {
        let label = format!("+{hidden} more");
        chips = chips.child(
            rune_kit::Button::new(label)
                .variant(rune_kit::ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .expanded(s.activity_open)
                .on_press(|s: &mut App| s.activity_open ^= true)
                .build(k)
                .key("status-more"),
        );
    }
    // The newest jobs stay in the footer; older ones fold away first.
    for job in &jobs[hidden..] {
        chips = chips.child(chip(job, k));
    }

    row()
        .key("statusbar")
        .w_full()
        .h(HEIGHT)
        .px(INSET)
        .gap(12.0)
        .center_y()
        .border_sides([1.0, 0.0, 0.0, 0.0], c.border_muted)
        .child(col().min_w(0.0).shrink().child(summary))
        .child(spacer())
        .child(chips)
}

/// How a job looks right now: its leading mark, its line of text and that text's colour.
fn face(job: &Job, k: &Kit) -> (El, String, rune::Color) {
    let c = &k.c;
    match &job.phase {
        Phase::Running => (Spinner::new().size(12.0).build(k), job.title.clone(), c.text_base),
        Phase::Stopping => (Spinner::new().size(12.0).color(c.icon_faint).build(k), format!("Stopping {}…", job.kind.noun()), c.text_muted),
        Phase::Done { outcome: Outcome::Completed(m), .. } => (glyph(&icons::CHECK, c.success.icon), m.clone(), c.text_base),
        Phase::Done { outcome: Outcome::Stopped(m), .. } => (glyph(&icons::STOP, c.icon_muted), m.clone(), c.text_muted),
        Phase::Done { outcome: Outcome::Failed(m), .. } => (glyph(&icons::WARNING, c.danger.icon), m.clone(), c.danger.fg),
    }
}

/// The job's one button: stop while it runs, nothing while it stops, dismiss once it's done.
fn action(job: &Job, k: &Kit) -> El {
    let id = job.id;
    match job.phase {
        Phase::Running => IconButton::new(&icons::STOP)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .label(format!("Stop {}", job.kind.noun()))
            .help(format!("Stop {}", job.kind.noun()))
            .on_press(move |s: &mut App| s.jobs.cancel(id))
            .build(k),
        // Nothing to press while it winds down. Keep the space so nothing shifts.
        Phase::Stopping => el().size(20.0, 20.0),
        Phase::Done { .. } => IconButton::new(&icons::CLOSE)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .label("Dismiss")
            .on_press(move |s: &mut App| s.jobs.dismiss(id))
            .build(k),
    }
}

fn chip(job: &Job, k: &Kit) -> El {
    let c = &k.c;
    let id = job.id;
    let (lead, title, ink) = face(job, k);
    let mut body = row().grow(1.0).min_w(0.0).gap(8.0).center_y().child(lead.key(id.key("lead"))).child(
        text(title).key(id.key("title")).font(k.ty.small_medium).color(ink).truncate().shrink(),
    );
    if job.is_active() {
        body = body.child(bar(job, 40.0, k));
        if let Some(f) = job.fraction() {
            body = body.child(text(format!("{:.0}%", f * 100.0)).key(id.key("pct")).font(k.ty.small.features("'tnum' 1")).color(c.text_muted).w(30.0).align(rune::Align::End));
        }
    }
    let fill = match &job.phase {
        Phase::Done { outcome: Outcome::Failed(_), .. } => c.danger.bg,
        Phase::Running | Phase::Stopping | Phase::Done { outcome: Outcome::Completed(_) | Outcome::Stopped(_), .. } => c.layer_01,
    };
    let help = if job.detail.is_empty() { chip_label(job) } else { format!("{} · {}", chip_label(job), job.detail) };
    let chip = row()
        .key(id.key("chip"))
        .role("status")
        .label(chip_label(job))
        .w(CHIP_W)
        .h(24.0)
        .pl(8.0)
        .pr(2.0)
        .gap(4.0)
        .center_y()
        .radius(k.radius.md)
        .fill(fill)
        .border(1.0, c.border_muted)
        .transition(Transition::offset(0.0, 6.0))
        .child(body.help(help).on_press(move |s: &mut App| s.open_job(id)).cursor(rune::Cursor::Pointer))
        .child(action(job, k).key(id.key("action")));
    k.interactive(chip)
}

/// Every job, one per row with room for the item in hand, above the footer. Each row can open
/// its log.
pub fn activity(s: &App, k: &Kit) -> Option<El> {
    if !s.activity_open || s.jobs.list().is_empty() {
        return None;
    }
    let c = &k.c;
    let rows = s.jobs.list().iter().rev().map(|job| {
        let id = job.id;
        let open = s.log_open == Some(id);
        let (lead, title, ink) = face(job, k);
        let detail = match (&job.phase, job.detail.is_empty()) {
            (Phase::Running | Phase::Stopping, false) => format!("{} · {}", job.detail, elapsed(job)),
            (Phase::Running | Phase::Stopping, true) => elapsed(job),
            (Phase::Done { .. }, _) => job.title.clone(),
        };
        let mut r = row().key(id.key("activity")).w_full().h(40.0).px(INSET).gap(10.0).center_y().child(lead).child(
            col()
                .min_w(0.0)
                .shrink()
                .gap(1.0)
                .child(text(title).font(k.ty.small_medium).color(ink).truncate())
                .child(text(detail).key(id.key("detail")).font(k.ty.small.features("'tnum' 1")).color(c.text_faint).truncate()),
        );
        r = r.child(spacer());
        if job.is_active() {
            r = r.child(bar(job, 140.0, k));
            r = r.child(text(job.fraction().map_or_else(String::new, |f| format!("{:.0}%", f * 100.0))).font(k.ty.small.features("'tnum' 1")).color(c.text_muted).w(34.0).align(rune::Align::End));
        }
        let warnings = job.log.iter().filter(|l| l.level != Level::Info).count();
        let label = if warnings > 0 { format!("Log · {warnings}") } else { "Log".to_owned() };
        r = r.child(
            rune_kit::Button::new(label)
                .variant(rune_kit::ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .expanded(open)
                .on_press(move |s: &mut App| s.log_open = if s.log_open == Some(id) { None } else { Some(id) })
                .build(k)
                .key(id.key("log-toggle")),
        );
        let r = r.child(action(job, k).key(id.key("activity-action")));
        let mut block = col().key(id.key("activity-block")).w_full().child(r);
        if open {
            block = block.child(log(job, k));
        }
        block
    });
    let header = row()
        .w_full()
        .h(32.0)
        .px(INSET)
        .center_y()
        .child(text("Activity").font(k.ty.small_medium).color(c.text_muted))
        .child(spacer())
        .child(
            rune_kit::Button::new("Clear finished")
                .variant(rune_kit::ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .disabled(!s.jobs.any_finished())
                .on_press(|s: &mut App| s.jobs.clear_finished())
                .build(k)
                .key("activity-clear"),
        )
        .child(
            rune_kit::Button::new("Open log file")
                .variant(rune_kit::ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .help("Opens every job's log in Console")
                .on_press(|_: &mut App| {
                    if let Some(p) = crate::jobs::log_path() {
                        let _ = std::process::Command::new("open").arg("-a").arg("Console").arg(p).spawn();
                    }
                })
                .build(k)
                .key("activity-logfile"),
        )
        .child(IconButton::new(&icons::CLOSE).variant(IconButtonVariant::GhostMuted).control_size(ControlSize::Small).label("Close activity").on_press(|s: &mut App| s.activity_open = false).build(k).key("activity-close"));
    Some(
        col()
            .key("activity")
            .w_full()
            // A sheet pulled up over the list: a firm top edge, a raised surface, and a shadow
            // cast up onto the rows so it never reads as more of them.
            .border_sides([1.0, 0.0, 0.0, 0.0], c.border_base)
            .fill(c.layer_01)
            .shadow(-6.0, 18.0, rune::Color::BLACK.alpha(if k.dark { 0.45 } else { 0.12 }))
            .transition(Transition::offset(0.0, 12.0))
            .child(header.border_sides([0.0, 0.0, 1.0, 0.0], c.border_muted))
            .child(col().key("activity-rows").w_full().max_h(360.0).scroll(rune::ScrollSpec { stick_bottom: false }).children(rows)),
    )
}

/// How long a running job has been going: "0:42".
fn elapsed(job: &Job) -> String {
    let secs = job.started.elapsed().as_secs() as u32;
    format!("running {}", crate::library::duration(secs))
}

/// One job's log: every line with its time and level, newest at the bottom, and a copy button.
fn log(job: &Job, k: &Kit) -> El {
    let c = &k.c;
    let id = job.id;
    let mono = rune::TextStyle { size: 11.0, ..k.ty.mono };
    let lines = job.log.iter().map(|l| {
        let (tag, ink) = match l.level {
            Level::Info => ("     ", c.text_muted),
            Level::Warn => ("warn ", c.warning.fg),
            Level::Error => ("error", c.danger.fg),
        };
        row()
            .w_full()
            .gap(8.0)
            .child(text(l.clock.clone()).font(mono.clone()).color(c.text_faint))
            .child(text(tag).font(mono.clone()).color(ink))
            .child(text(l.text.clone()).font(mono.clone()).color(if l.level == Level::Info { c.text_base } else { ink }).wrap().selectable(true).grow(1.0).min_w(0.0))
    });
    let body = col().key(id.key("log-lines")).w_full().max_h(220.0).py(8.0).px(10.0).gap(3.0).scroll(rune::ScrollSpec { stick_bottom: true }).children(lines);
    let copy = rune_kit::Button::new("Copy log")
        .variant(rune_kit::ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .on_press_tx(move |s: &mut App, tx| {
            if let Some(j) = s.jobs.get(id) {
                tx.copy(j.log_text());
            }
        })
        .build(k)
        .key(id.key("log-copy"));
    col()
        .key(id.key("log"))
        .w_full()
        .px(INSET)
        .pb(10.0)
        .child(
            col()
                .w_full()
                .radius(k.radius.md)
                .fill(c.layer_01)
                .border(1.0, c.border_muted)
                .child(body)
                .child(row().w_full().px(4.0).pb(4.0).child(spacer()).child(copy)),
        )
}

/// A thin progress bar. Indeterminate while the job is still counting its work.
fn bar(job: &Job, w: f32, k: &Kit) -> El {
    let c = &k.c;
    let ink = if job.phase == Phase::Stopping { c.icon_faint } else { c.icon_accent };
    let fill = el().h_full().radius(2.0).fill(ink);
    let fill = match job.fraction() {
        Some(f) => fill.w_frac(f),
        None => fill.w_frac(0.3).looping(rune::LoopSpec::slide(1.4, [-1.0, 0.0], [3.4, 0.0])),
    };
    row().w(w).h(4.0).radius(2.0).fill(c.layer_03).clip_soft(0.0).child(fill)
}

fn glyph(i: &'static Icon, ink: rune::Color) -> El {
    rune::icon(i, 12.0).color(ink)
}

fn chip_label(job: &Job) -> String {
    match (&job.phase, job.fraction()) {
        (Phase::Running, Some(f)) => format!("{}, {:.0} percent", job.title, f * 100.0),
        (Phase::Running, None) => job.title.clone(),
        (Phase::Stopping, _) => format!("Stopping {}", job.kind.noun()),
        (Phase::Done { outcome: Outcome::Completed(m) | Outcome::Stopped(m) | Outcome::Failed(m), .. }, _) => m.clone(),
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::jobs::{JobKind, Phase};
    use rune::{Env, Runtime};

    fn press(rt: &mut Runtime<App>, key: &str) {
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..10 {
            rt.frame(1.0 / 60.0);
        }
    }

    fn rt_with_two_jobs() -> (Runtime<App>, crate::jobs::JobId, crate::jobs::JobId) {
        let mut app = App::new();
        // The export first, as in the bug report, then the analysis.
        let export = app.jobs.stage(JobKind::Export, "Exporting to KINGSTON", "Halcyon Drift", 10, Some(100), Phase::Running);
        let analysis = app.jobs.stage(JobKind::Analysis, "Analysing 48 tracks", "Paper Orbit", 3, Some(48), Phase::Running);
        let env = Env { system_dark: true, scale: 2.0 };
        let mut rt = Runtime::new(app, crate::ui::view, (1280.0, 800.0), env);
        rt.frame(0.0);
        (rt, export, analysis)
    }

    #[test]
    fn the_stop_button_in_the_analysis_chip_stops_only_the_analysis() {
        let (mut rt, export, analysis) = rt_with_two_jobs();
        press(&mut rt, &analysis.key("action"));
        assert_eq!(rt.state.jobs.get(analysis).unwrap().phase, Phase::Stopping);
        assert_eq!(rt.state.jobs.get(export).unwrap().phase, Phase::Running);
    }

    #[test]
    fn the_stop_button_in_the_export_chip_stops_only_the_export() {
        let (mut rt, export, analysis) = rt_with_two_jobs();
        press(&mut rt, &export.key("action"));
        assert_eq!(rt.state.jobs.get(export).unwrap().phase, Phase::Stopping);
        assert_eq!(rt.state.jobs.get(analysis).unwrap().phase, Phase::Running);
    }

    #[test]
    fn each_stop_button_sits_inside_its_own_chip() {
        let (mut rt, export, analysis) = rt_with_two_jobs();
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        let footer = rt.presented("statusbar").unwrap().0;
        for id in [export, analysis] {
            let chip = rt.presented(&id.key("chip")).unwrap().0;
            assert!(chip[0] >= footer[0] && chip[0] + chip[2] <= footer[0] + footer[2], "job {id:?}'s chip spills out of the footer");
            let (bx, by) = rt.find(&id.key("action")).unwrap();
            assert!(bx >= chip[0] && bx <= chip[0] + chip[2] && by >= chip[1] && by <= chip[1] + chip[3], "job {id:?}'s stop button is outside its chip");
        }
    }

    /// With more jobs than fit, the extras fold into "+N more", and the Activity panel gives
    /// every job its own stop button there too.
    #[test]
    fn folded_jobs_stop_one_at_a_time_from_activity() {
        let mut app = App::new();
        let ids: Vec<_> = (0..4).map(|i| app.jobs.stage(JobKind::Analysis, &format!("Analysing batch {i}"), "", 1, Some(10), Phase::Running)).collect();
        let env = Env { system_dark: true, scale: 2.0 };
        let mut rt = Runtime::new(app, crate::ui::view, (1280.0, 800.0), env);
        rt.frame(0.0);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.find(&ids[0].key("chip")).is_none(), "the oldest job folds away");
        press(&mut rt, "status-more");
        assert!(rt.state.activity_open);
        press(&mut rt, &ids[0].key("activity-action"));
        assert_eq!(rt.state.jobs.get(ids[0]).unwrap().phase, Phase::Stopping);
        for id in &ids[1..] {
            assert_eq!(rt.state.jobs.get(*id).unwrap().phase, Phase::Running);
        }
    }
}
