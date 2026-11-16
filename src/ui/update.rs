//! The update card at the foot of the sidebar. It shows only when there's something to say: a
//! newer build downloading, ready to go in, or a problem getting it.

use rune::{El, col, row, spacer, text};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, Spinner};

use crate::app::App;
use crate::ui::icons;
use crate::update::{Manual, Status};

pub fn card(s: &App, k: &Kit) -> Option<El> {
    if !s.update.visible() {
        return None;
    }
    let c = &k.c;
    let u = &s.update;
    if let (Status::Idle, Some(m)) = (&u.status, &u.manual) {
        return Some(manual_card(m, k));
    }
    let build = u.status.build().unwrap_or(0);
    let (title, lead, ink) = match &u.status {
        Status::Idle => return None,
        Status::Downloading { .. } => ("Downloading an update", rune::icon(&icons::UPDATE, 14.0).color(c.icon_muted), c.text_base),
        Status::Ready { .. } => ("Update ready", rune::icon(&icons::UPDATE, 14.0).color(c.icon_accent), c.text_base),
        Status::Installing { .. } => ("Updating…", Spinner::new().size(14.0).build(k), c.text_base),
        Status::Failed { .. } => ("Couldn't update", rune::icon(&icons::WARNING, 14.0).color(c.warning.icon), c.text_base),
        Status::Manual { .. } => ("Update available", rune::icon(&icons::UPDATE, 14.0).color(c.icon_accent), c.text_base),
    };
    let mut head = row().w_full().gap(8.0).center_y().child(lead.key("update-lead")).child(
        col()
            .min_w(0.0)
            .shrink()
            .gap(1.0)
            .child(text(title).key("update-title").font(k.ty.small_medium).color(ink).truncate())
            .child(text(format!("Sleeve build {build}")).key("update-build").font(k.ty.small).color(c.text_faint).truncate()),
    );
    head = head.child(spacer());
    // "Later" hides it until a newer build comes out. Not while it's going in.
    if !matches!(u.status, Status::Installing { .. } | Status::Downloading { .. }) {
        head = head.child(
            IconButton::new(&icons::CLOSE)
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .label("Later")
                .help("Hide until the next update")
                .on_press(move |s: &mut App| {
                    s.update.hidden = Some(build);
                    s.update.restart_waiting = false;
                })
                .build(k)
                .key("update-later"),
        );
    }
    let note = |t: String| text(t).font(k.ty.small.lh(16.0)).color(c.text_muted).w_full().wrap();
    let mut body = col().w_full().gap(8.0);
    match &u.status {
        Status::Idle | Status::Installing { .. } => {}
        Status::Downloading { done, total, .. } => {
            let f = if *total > 0 { (*done as f32 / *total as f32).clamp(0.0, 1.0) } else { 0.0 };
            body = body
                .child(row().key("update-bar").w_full().h(4.0).radius(2.0).fill(c.layer_03).clip_soft(0.0).child(rune::el().h_full().radius(2.0).fill(c.icon_accent).w_frac(f)))
                .child(text(format!("{} of {}", crate::icloud::size(*done), crate::icloud::size(*total))).key("update-progress").font(k.ty.small.features("'tnum' 1")).color(c.text_faint));
        }
        Status::Ready { .. } => {
            let busy = s.jobs.list().iter().any(crate::jobs::Job::is_active);
            if u.restart_waiting {
                body = body.child(note("Sleeve will restart when your jobs finish.".into())).child(
                    Button::new("Don't restart")
                        .variant(ButtonVariant::Outline)
                        .control_size(ControlSize::Small)
                        .on_press(|s: &mut App| s.update.restart_waiting = false)
                        .build(k)
                        .key("update-restart")
                        .w_full(),
                );
            } else {
                if busy {
                    body = body.child(note("A job is running. Restart waits for it to finish.".into()));
                } else if s.player.playing {
                    body = body.child(note("Restarting stops the music.".into()));
                }
                let busy_now = busy;
                body = body.child(
                    row()
                        .w_full()
                        .gap(6.0)
                        .child(
                            Button::new("What's new")
                                .variant(ButtonVariant::Ghost)
                                .control_size(ControlSize::Small)
                                .on_press_tx(move |_: &mut App, tx| {
                                    let _ = tx.open_url(&format!("https://github.com/alii/sleeve/releases/tag/build-{build}"));
                                })
                                .build(k)
                                .key("update-notes"),
                        )
                        .child(spacer())
                        .child(
                            Button::new("Restart")
                                .variant(ButtonVariant::Submit)
                                .control_size(ControlSize::Small)
                                .on_press(move |s: &mut App| {
                                    let busy = busy_now || s.jobs.list().iter().any(crate::jobs::Job::is_active);
                                    s.update.restart(busy);
                                })
                                .build(k)
                                .key("update-restart"),
                        ),
                );
            }
        }
        Status::Failed { why, .. } => {
            body = body.child(note(capitalise(why))).child(
                Button::new("Try again")
                    .variant(ButtonVariant::Outline)
                    .control_size(ControlSize::Small)
                    .on_press(|s: &mut App| {
                        s.update.retry();
                    })
                    .build(k)
                    .key("update-retry")
                    .w_full(),
            );
        }
        Status::Manual { why, .. } => {
            body = body.child(note(why.clone())).child(
                Button::new("Download")
                    .variant(ButtonVariant::Outline)
                    .control_size(ControlSize::Small)
                    .on_press_tx(|_: &mut App, tx| {
                        let _ = tx.open_url(crate::update::RELEASE_PAGE);
                    })
                    .build(k)
                    .key("update-download")
                    .w_full(),
            );
        }
    }
    Some(
        col()
            .key("update-card")
            .role("status")
            .label(format!("{title}, Sleeve build {build}"))
            .w_full()
            .px(10.0)
            .py(10.0)
            .gap(8.0)
            .radius(k.radius.md)
            .fill(c.layer_01)
            .border(1.0, c.border_muted)
            .transition(rune::Transition::offset(0.0, 8.0))
            .child(head)
            .child(body),
    )
}

/// What a check from the menu found, when there's nothing to download.
fn manual_card(m: &Manual, k: &Kit) -> El {
    let c = &k.c;
    let (title, lead) = match m {
        Manual::Checking => ("Checking for updates…", Spinner::new().size(14.0).build(k)),
        Manual::UpToDate => ("Sleeve is up to date", rune::icon(&icons::UPDATE, 14.0).color(c.icon_accent)),
        Manual::Failed(_) => ("Couldn't check for updates", rune::icon(&icons::WARNING, 14.0).color(c.warning.icon)),
    };
    let sub = crate::update::this_build().map_or_else(|| "Built by hand".to_owned(), |b| format!("Sleeve build {b}"));
    let close = IconButton::new(&icons::CLOSE)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .label("Close")
        .on_press(|s: &mut App| s.update.manual = None)
        .build(k)
        .key("update-later");
    let head = row()
        .w_full()
        .gap(8.0)
        .center_y()
        .child(lead.key("update-lead"))
        .child(
            col()
                .min_w(0.0)
                .shrink()
                .gap(1.0)
                .child(text(title).key("update-title").font(k.ty.small_medium).color(c.text_base).truncate())
                .child(text(sub).key("update-build").font(k.ty.small).color(c.text_faint).truncate()),
        )
        .child(spacer())
        .child(close);
    let mut card = col()
        .key("update-card")
        .role("status")
        .label(title)
        .w_full()
        .px(10.0)
        .py(10.0)
        .gap(8.0)
        .radius(k.radius.md)
        .fill(c.layer_01)
        .border(1.0, c.border_muted)
        .transition(rune::Transition::offset(0.0, 8.0))
        .child(head);
    if let Manual::Failed(why) = m {
        card = card.child(text(capitalise(why)).font(k.ty.small.lh(16.0)).color(c.text_muted).w_full().wrap()).child(
            row()
                .w_full()
                .gap(6.0)
                .child(
                    Button::new("Download")
                        .variant(ButtonVariant::Ghost)
                        .control_size(ControlSize::Small)
                        .on_press_tx(|_: &mut App, tx| {
                            let _ = tx.open_url(crate::update::RELEASE_PAGE);
                        })
                        .build(k)
                        .key("update-download"),
                )
                .child(spacer())
                .child(Button::new("Try again").variant(ButtonVariant::Outline).control_size(ControlSize::Small).on_press(|s: &mut App| s.update.check_now()).build(k).key("update-retry")),
        );
    }
    card
}

/// The reason with a capital letter, as it starts a sentence on the card.
fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    let mut out: String = chars.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    out.push_str(chars.as_str());
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}
