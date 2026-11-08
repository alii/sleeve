//! The overview waveform: rekordbox's three-band preview (`PWV6`), drawn into pixels off the UI
//! thread.

use std::path::Path;

use rune::Pixels;

/// rekordbox's band colours: lows blue, mids amber, highs white.
const LOW: [f32; 3] = [0.0, 85.0, 225.0];
const MID: [f32; 3] = [255.0, 166.0, 0.0];
const HIGH: [f32; 3] = [255.0, 255.0, 255.0];

/// The three bands per column, 0 to 127, or None when the track has no 3-band analysis.
pub fn read(share_root: &Path, analysis_path: &str) -> Option<Vec<[u8; 3]>> {
    if analysis_path.is_empty() {
        return None;
    }
    let dat = rbl_anlz::resolve(share_root, analysis_path);
    let anlz = rbl_anlz::Anlz::read(&rbl_anlz::sibling(&dat, "2EX")).ok()?;
    let (stride, bytes) = anlz.waveform(b"PWV6")?;
    if stride < 3 {
        return None;
    }
    Some(bytes.chunks_exact(stride as usize).map(|c| [c[0], c[1], c[2]]).collect())
}

/// Draws `cols` into a `w`×`h` straight-alpha RGBA image, mirrored about the middle, each band
/// laid over the last as rekordbox layers them.
pub fn render(cols: &[[u8; 3]], w: usize, h: usize) -> Vec<u8> {
    let mut px = vec![0u8; w * h * 4];
    if cols.is_empty() || w == 0 || h == 0 {
        return px;
    }
    // Scale to the loudest column, as rekordbox's overview does, so quiet masters still fill
    // the strip.
    let peak = cols.iter().map(|c| c[0].max(c[1]).max(c[2])).max().unwrap_or(1).max(1);
    let gain = 127.0 / f32::from(peak) * 0.94;
    for x in 0..w {
        // The loudest of the columns this pixel covers, so peaks survive the squeeze.
        let a = x * cols.len() / w;
        let b = ((x + 1) * cols.len() / w).max(a + 1).min(cols.len());
        let mut band = [0u8; 3];
        for c in &cols[a..b] {
            for i in 0..3 {
                band[i] = band[i].max(c[i]);
            }
        }
        // The lows carry the body of the track; mids and highs sit inside them.
        paint(&mut px, w, h, x, band, gain);
    }
    px
}

/// The detailed waveform's resolution: rekordbox writes `PWV7` at 150 columns a second.
pub const DETAIL_PER_SEC: f64 = 150.0;
/// Columns per tile. The detailed waveform of a long track is too wide for one texture, so
/// it's cut into tiles and only the ones in view are drawn.
pub const TILE: usize = 1024;
pub const DETAIL_H: usize = 160;

/// A track's detailed waveform, in tiles of `TILE` columns that fill in from a worker.
pub struct Detail {
    pub tiles: Vec<Pixels>,
    pub cols: usize,
}

/// Reads the detailed three-band waveform (`PWV7`) and starts drawing its tiles. The read is
/// small and done here; the drawing happens on a worker.
pub fn load_detail(share_root: &Path, analysis_path: &str) -> Option<Detail> {
    if analysis_path.is_empty() {
        return None;
    }
    let dat = rbl_anlz::resolve(share_root, analysis_path);
    let anlz = rbl_anlz::Anlz::read(&rbl_anlz::sibling(&dat, "2EX")).ok()?;
    let (stride, bytes) = anlz.waveform(b"PWV7")?;
    if stride < 3 {
        return None;
    }
    let cols: Vec<[u8; 3]> = bytes.chunks_exact(stride as usize).map(|c| [c[0], c[1], c[2]]).collect();
    let tiles: Vec<Pixels> = (0..cols.len().div_ceil(TILE)).map(|_| Pixels::new()).collect();
    let out = Detail { tiles: tiles.clone(), cols: cols.len() };
    std::thread::spawn(move || {
        // One gain for the whole track, so tiles meet without a seam.
        let peak = cols.iter().map(|c| c[0].max(c[1]).max(c[2])).max().unwrap_or(1).max(1);
        let gain = 127.0 / f32::from(peak) * 0.94;
        for (i, tile) in tiles.iter().enumerate() {
            let part = &cols[i * TILE..((i + 1) * TILE).min(cols.len())];
            let mut px = vec![0u8; part.len() * DETAIL_H * 4];
            for (x, band) in part.iter().enumerate() {
                paint(&mut px, part.len(), DETAIL_H, x, *band, gain);
            }
            tile.set(part.len() as u32, DETAIL_H as u32, px);
        }
    });
    Some(out)
}

/// Draws one column's three bands, mirrored about the middle, lows under mids under highs.
fn paint(px: &mut [u8], w: usize, h: usize, x: usize, band: [u8; 3], gain: f32) {
    let mid = h as f32 / 2.0;
    let heights = [f32::from(band[0]) * gain / 127.0, f32::from(band[1]) * gain / 150.0, f32::from(band[2]) * gain / 190.0];
    for (i, colour) in [LOW, MID, HIGH].iter().enumerate() {
        let half = (heights[i].min(1.0) * mid).max(if band[i] > 0 { 0.5 } else { 0.0 });
        let (top, bottom) = ((mid - half).floor() as usize, ((mid + half).ceil() as usize).min(h));
        for y in top..bottom {
            let o = (y * w + x) * 4;
            let alpha = if i == 0 { 1.0 } else { 0.92 };
            for k in 0..3 {
                let under = f32::from(px[o + k]);
                px[o + k] = (under + (colour[k] - under) * alpha) as u8;
            }
            px[o + 3] = 255;
        }
    }
}

/// Reads and draws a track's waveform on a worker, then shows it. Returns at once.
pub fn load_into(pixels: Pixels, share_root: std::path::PathBuf, analysis_path: String, w: usize, h: usize) {
    std::thread::spawn(move || {
        let rgba = read(&share_root, &analysis_path).map_or_else(|| vec![0u8; w * h * 4], |cols| render(&cols, w, h));
        pixels.set(w as u32, h as u32, rgba);
    });
}

