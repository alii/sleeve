//! Icons: OpenCode's catalog where it has one, our own 16 point line icons where it doesn't.

use rune::Icon;

pub use rune_theme_opencode::icons::{CHECK, CLOSE, MAGNIFYING_GLASS as SEARCH, STOP, WARNING};

pub static COLLECTION: Icon = Icon {
    name: "sleeve-collection",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="5.75" stroke="currentColor" stroke-width="1.2" fill="none"/><circle cx="8" cy="8" r="1.6" fill="currentColor"/>"#,
};

pub static PLAYLIST: Icon = Icon {
    name: "sleeve-playlist",
    view: 16.0,
    body: r#"<path d="M3 4.5H10M3 8H10M3 11.5H7" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/><path d="M12 6.5V12.2" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/><circle cx="11" cy="12.2" r="1.3" fill="currentColor"/>"#,
};

pub static FOLDER: Icon = Icon {
    name: "sleeve-folder",
    view: 16.0,
    body: r#"<path d="M2.75 4.75C2.75 4.2 3.2 3.75 3.75 3.75H6.4L7.6 5H12.25C12.8 5 13.25 5.45 13.25 6V11.25C13.25 11.8 12.8 12.25 12.25 12.25H3.75C3.2 12.25 2.75 11.8 2.75 11.25Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" fill="none"/>"#,
};

pub static SMART: Icon = Icon {
    name: "sleeve-smart",
    view: 16.0,
    body: r#"<path d="M8 2.75L9.2 6.8L13.25 8L9.2 9.2L8 13.25L6.8 9.2L2.75 8L6.8 6.8Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" fill="none"/>"#,
};

pub static USB: Icon = Icon {
    name: "sleeve-usb",
    view: 16.0,
    body: r#"<rect x="4.75" y="6.25" width="6.5" height="7" rx="1.25" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M6 6.25V2.75H10V6.25" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" fill="none"/><path d="M7.25 4.25V4.75M8.75 4.25V4.75" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};

pub static WAVE: Icon = Icon {
    name: "sleeve-wave",
    view: 16.0,
    body: r#"<path d="M2.5 8H3.5M5 5.5V10.5M7 3.5V12.5M9 5V11M11 6.5V9.5M12.5 8H13.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};

pub static PLAY: Icon = Icon {
    name: "sleeve-play",
    view: 16.0,
    body: r#"<path d="M5 3.6V12.4C5 12.8 5.45 13.05 5.8 12.83L12.7 8.43C13.02 8.23 13.02 7.77 12.7 7.57L5.8 3.17C5.45 2.95 5 3.2 5 3.6Z" fill="currentColor"/>"#,
};

pub static PAUSE: Icon = Icon {
    name: "sleeve-pause",
    view: 16.0,
    body: r#"<rect x="4.25" y="3.25" width="2.6" height="9.5" rx="0.8" fill="currentColor"/><rect x="9.15" y="3.25" width="2.6" height="9.5" rx="0.8" fill="currentColor"/>"#,
};

pub static LOCATE: Icon = Icon {
    name: "sleeve-locate",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="4.25" stroke="currentColor" stroke-width="1.2" fill="none"/><circle cx="8" cy="8" r="1.25" fill="currentColor"/><path d="M8 1.75V3.5M8 12.5V14.25M1.75 8H3.5M12.5 8H14.25" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};

pub static CLOUD: Icon = Icon {
    name: "sleeve-cloud",
    view: 16.0,
    body: r#"<path d="M4.75 12.25H11.5C12.88 12.25 14 11.13 14 9.75C14 8.45 13.01 7.38 11.74 7.26C11.4 5.4 9.78 4 7.83 4C6.03 4 4.5 5.2 4.03 6.85C2.86 7.17 2 8.24 2 9.5C2 11.02 3.23 12.25 4.75 12.25Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" fill="none"/>"#,
};
