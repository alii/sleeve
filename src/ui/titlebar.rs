//! The title bar on Windows. There Sleeve draws its own in the window's colours, in place of
//! the white system one. The Sleeve menu sits on the left and holds what the menu bar does on a
//! Mac. Rune draws Windows' own minimise, maximise and close buttons on the right.
//!
//! On a Mac the menu bar and the traffic lights do this job, so there's no bar.

use rune::{Cx, El};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, Menu, MenuItem, Placement, WindowTitlebar};

use crate::app::App;
use crate::ui::icons;

/// The bar's height in points. The caption buttons take it too, as Windows 11's do.
pub const HEIGHT: f32 = 32.0;
/// What the three caption buttons take of the right edge, for the snapshot scene that draws the
/// bar on a Mac, where Rune draws no buttons.
const CAPTION_WIDTH: f32 = rune::Titlebar::CAPTION_BUTTON_WIDTH * 3.0;

const ISSUES: &str = "https://github.com/alii/sleeve/issues";

/// The bar, where the window has one: on Windows, or in the `windows-titlebar` scene.
pub fn build(s: &App, cx: &mut Cx<App>, k: &Kit, layer: &mut Vec<El>) -> Option<El> {
    let insets = cx.chrome_insets();
    if insets.height <= 0.0 && !s.preview_titlebar {
        return None;
    }
    let c = &k.c;
    cx.caption_color(c.text_base);
    let mut bar = WindowTitlebar::from_insets(insets, 1.0).key("win-titlebar").height(HEIGHT).fill(c.bg_deep).leading_padding(6.0).gap(2.0);
    if insets.height <= 0.0 {
        bar = bar.trailing_padding(12.0 + CAPTION_WIDTH);
    }
    Some(bar.leading(menu(s, k, layer)).build(k))
}

/// The Sleeve menu: the build, updates, help and Exit.
fn menu(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let trigger = Button::new("Sleeve")
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .help("Updates, help and Exit")
        .on_press(|s: &mut App| s.app_menu ^= true)
        .build(k)
        .key("app-menu-button");
    let build = crate::update::this_build().map_or_else(|| "Built by hand".to_owned(), |b| format!("Sleeve build {b}"));
    let notes = crate::update::this_build().map_or_else(|| crate::update::RELEASE_PAGE.to_owned(), |b| format!("https://github.com/alii/sleeve/releases/tag/build-{b}"));
    Menu::new("app-menu")
        .trigger(trigger)
        .placement(Placement::BottomStart)
        .min_width(220.0)
        .open(s.app_menu)
        .on_close(|s: &mut App| s.app_menu = false)
        .item(MenuItem::new(build).disabled(true))
        .separator()
        .item(MenuItem::new("Check for Updates…").icon(&icons::UPDATE).on_select(|s: &mut App| {
            s.app_menu = false;
            s.update.check_now();
        }))
        .item(MenuItem::new("What's New").on_select_tx(move |s: &mut App, tx| {
            s.app_menu = false;
            let _ = tx.open_url(&notes);
        }))
        .item(MenuItem::new("Report a Problem…").on_select_tx(|s: &mut App, tx| {
            s.app_menu = false;
            let _ = tx.open_url(ISSUES);
        }))
        .separator()
        .item(MenuItem::new("Exit").shortcut("Alt+F4").on_select_tx(|s: &mut App, tx| {
            s.app_menu = false;
            tx.quit();
        }))
        .build(k, layer)
}
