//! The window's own controls on Windows. There Rune takes away the white system title bar and
//! draws Windows' minimise, maximise and close buttons over the window's top-right corner, so
//! the window looks like the Mac one: the split view runs to the top edge.
//!
//! The Sleeve menu, which holds what the menu bar does on a Mac, sits where the traffic lights
//! do on a Mac: at the leading end of the sidebar's title bar row, or of the toolbar with the
//! sidebar closed. The split view clears room for it the way it clears the traffic lights.
//! What sits in the top-right corner keeps clear of the buttons ([`Chrome::caption`]).

use rune::{Cx, El, row};
use rune_kit::shell::TOOLBAR_HEIGHT;
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, Menu, MenuItem, Placement};

use crate::app::App;
use crate::ui::icons;

/// The caption buttons' height in points: Windows 11's.
pub const HEIGHT: f32 = 32.0;
/// What the three caption buttons take of the right edge, for the snapshot scenes on a Mac,
/// where Rune draws no buttons.
const CAPTION_WIDTH: f32 = rune::Titlebar::CAPTION_BUTTON_WIDTH * 3.0;
/// The Sleeve button's distance from the window's leading edge.
const MENU_X: f32 = 10.0;
/// The Sleeve button's width before it's first laid out.
const MENU_WIDTH: f32 = 56.0;

const ISSUES: &str = "https://github.com/alii/sleeve/issues";

/// Sleeve's own window chrome on Windows.
pub struct Chrome {
    /// The Sleeve menu's button, placed over the split view's title bar row.
    pub menu: El,
    /// What to pass the split view as its titlebar inset, so it clears the menu.
    pub inset: f32,
    /// The caption buttons' width and height in the window's top-right corner.
    pub caption: (f32, f32),
}

/// The chrome, where the window has it: on Windows, or in the `windows-titlebar` scenes.
/// `frame` is the split view's frame inset, which pushes its title bar row down.
pub fn build(s: &App, cx: &mut Cx<App>, k: &Kit, frame: f32, layer: &mut Vec<El>) -> Option<Chrome> {
    let insets = cx.chrome_insets();
    if insets.height <= 0.0 && !s.preview_titlebar {
        return None;
    }
    cx.caption_color(k.c.text_base);
    let caption = if insets.height > 0.0 { (insets.trailing, insets.height) } else { (CAPTION_WIDTH, HEIGHT) };
    let width = cx.layout_size("app-menu-button").map_or(MENU_WIDTH, |z| z[0]);
    // Centred in the title bar row, as the sidebar toggle beside it is.
    let menu = row().key("app-menu-slot").absolute(MENU_X, frame).h(TOOLBAR_HEIGHT).center_y().child(menu(s, k, layer));
    Some(Chrome { menu, inset: MENU_X + width, caption })
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
