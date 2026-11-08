mod analysis;
mod app;
mod deck;
mod demo;
mod icloud;
mod import;
mod export;
mod scenes;
mod store;
mod jobs;
mod library;
mod player;
mod ui;
mod waveform;

use rune::{AppSpec, Chrome};

use app::App;

/// The theme's fonts, plus system faces for scripts Rune's own fallback doesn't reach yet
/// (Tibetan, Yi), which DJ libraries are fond of in decorative titles.
fn fonts() -> &'static [&'static [u8]] {
    let mut all: Vec<&'static [u8]> = rune_theme_opencode::FONTS.to_vec();
    for path in ["/System/Library/Fonts/Supplemental/Kailasa.ttc", "/System/Library/Fonts/Supplemental/NotoSansYi-Regular.ttf"] {
        if let Ok(bytes) = std::fs::read(path) {
            all.push(Box::leak(bytes.into_boxed_slice()));
        }
    }
    Box::leak(all.into_boxed_slice())
}

fn main() {
    // Without this, reading an iCloud-only song from a Terminal launch fails with EDEADLK.
    if let Err(e) = icloud::allow_downloads() {
        eprintln!("sleeve: couldn't turn on iCloud downloads: {e}");
    }
    rune::prewarm_gpu();
    let spec = AppSpec {
        fonts: fonts(),
        families: rune_theme_opencode::FAMILIES,
        setup: Some(|s: &mut App, w| s.setup(w)),
        poll: Some(|s: &mut App, clock, tx| s.poll(clock, tx)),
        on_key: Some(ui::on_key),
        min_size: Some((820.0, 520.0)),
        scenes: &[("selected", scenes::selected), ("busy", scenes::busy), ("playing", scenes::playing), ("glyphs", scenes::glyphs), ("activity", scenes::activity), ("collapsed", scenes::collapsed), ("log", scenes::log)],
        ..AppSpec::new("Sleeve", (1280.0, 800.0), App::new, ui::view)
    };
    let chrome = Chrome { full_size_content: true, traffic_lights: Some((26.0, 25.0)), drag_regions: true };
    rune::App::new(spec).chrome(chrome).run();
}
