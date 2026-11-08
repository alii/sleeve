//! Believable data for the demo library, so the interface can be judged on real-looking
//! tracks. Only ever run against a fixture built in a temporary folder.

use rbl_db::{LibraryLocation, OpenMode};
use rusqlite::params;

/// Anything that went wrong building the demo library. The demo is ours, so any failure is a
/// bug to read, not a state to handle: one boxed error carries every kind.
pub type DemoError = Box<dyn std::error::Error + Send + Sync>;

const ARTISTS: &[&str] = &[
    "Floating Points", "Four Tet", "Fred again..", "Overmono", "Bicep", "Peggy Gou", "Jamie xx", "Caribou",
    "Ross From Friends", "Kettama", "Joy Orbison", "Romy", "Skee Mask", "DJ Seinfeld", "Bonobo", "Mall Grab",
];
const WORDS: &[&str] = &[
    "Silhouette", "Pressure", "Afterglow", "Northern", "Tidal", "Ultraviolet", "Halcyon", "Static", "Drift",
    "Mirage", "Kinetic", "Ember", "Lucid", "Undertow", "Satellite", "Paper", "Velvet", "Signal", "Daybreak", "Orbit",
];
const SUFFIX: &[&str] = &["", "", " (Extended Mix)", " (Dub)", " (VIP)", " (Original Mix)", " (Edit)"];
const GENRES: &[&str] = &["House", "Techno", "UK Garage", "Breaks", "Electronica", "Deep House", "Drum & Bass"];
const ALBUMS: &[&str] = &["Late Night Tales", "Nocturne EP", "Weekend Sessions", "Fabric Presents", "Singles", "Remixes Vol. 2"];
const KEYS: &[&str] = &["Am", "Em", "Bm", "F#m", "C#m", "G#m", "Ebm", "Bbm", "Fm", "Cm", "Gm", "Dm", "C", "G", "D", "A", "E", "F"];

pub fn enrich(loc: &LibraryLocation, tracks: usize) -> Result<(), DemoError> {
    let db = rbl_db::Library::open(loc.clone(), OpenMode::ReadWrite)?;
    let c = db.connection();
    let now = "2026-01-01 00:00:00";
    let insert = |table: &str, col: &str, items: &[&str]| -> rusqlite::Result<()> {
        for (i, name) in items.iter().enumerate() {
            c.execute(
                &format!("INSERT INTO {table} (ID, {col}, UUID, created_at, updated_at) VALUES (?1, ?2, ?1, ?3, ?3)"),
                params![format!("9{i:03}"), name, now],
            )?;
        }
        Ok(())
    };
    let run = || -> rusqlite::Result<()> {
        insert("djmdArtist", "Name", ARTISTS)?;
        insert("djmdGenre", "Name", GENRES)?;
        insert("djmdAlbum", "Name", ALBUMS)?;
        insert("djmdKey", "ScaleName", KEYS)?;
        let ids: Vec<String> = {
            let mut q = c.prepare("SELECT ID FROM djmdContent ORDER BY ID")?;
            q.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?
        };
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        // One title from the far corners of Unicode, as real libraries have, so every check run
        // sees whether font fallback draws them.
        const ODD: &str = "✧₊⁎ ༎༽ ꒡ ⡇⣎ ◖◗◞ ❍❝ ؞ ๑ ⊁ △ ･ ̛⃛ᷠ͡";
        for (n, id) in ids.iter().take(tracks).enumerate() {
            let r = next();
            let pick = |n: usize, shift: u32| ((r >> shift) as usize) % n;
            let title = if n == 5 { format!("A {ODD}") } else { format!("{} {}{}", WORDS[pick(WORDS.len(), 0)], WORDS[pick(WORDS.len(), 7)], SUFFIX[pick(SUFFIX.len(), 13)]) };
            let bpm = 118 + pick(22, 17) as u32;
            let length = 180 + pick(300, 23) as u32;
            let rating = [0, 0, 3, 4, 5, 2, 4][pick(7, 31)];
            let color = [0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8][pick(11, 35)];
            let date = format!("2026-{:02}-{:02}", 1 + pick(10, 39), 1 + pick(28, 43));
            c.execute(
                "UPDATE djmdContent SET Title = ?2, ArtistID = ?3, GenreID = ?4, AlbumID = ?5, KeyID = ?6, BPM = ?7,
                 Length = ?8, Rating = ?9, ColorID = ?10, StockDate = ?11, BitRate = 320, FileType = 1 WHERE ID = ?1",
                params![
                    id,
                    title,
                    format!("9{:03}", pick(ARTISTS.len(), 47)),
                    format!("9{:03}", pick(GENRES.len(), 51)),
                    format!("9{:03}", pick(ALBUMS.len(), 55)),
                    format!("9{:03}", pick(KEYS.len(), 59)),
                    bpm * 100,
                    length,
                    rating,
                    color,
                    date
                ],
            )?;
        }
        Ok(())
    };
    Ok(run()?)
}

/// Writes a minute of synthetic dance music: kicks, off-beat hats and a bassline, with a
/// breakdown in the middle, so the demo has real waveforms to draw.
pub fn synth_track(path: &std::path::Path, bpm: f32, seed: u64) -> std::io::Result<()> {
    let rate = 44_100u32;
    let secs = 64.0f32;
    let frames = (rate as f32 * secs) as usize;
    let beat = 60.0 / bpm;
    let mut noise = seed | 1;
    let mut data = Vec::with_capacity(frames * 4);
    let notes = [55.0f32, 55.0, 65.4, 49.0];
    for n in 0..frames {
        let t = n as f32 / rate as f32;
        let b = t / beat;
        let in_beat = (b.fract()) * beat;
        let bar = (b / 4.0) as usize;
        let breakdown = (8..12).contains(&bar);
        let intro = bar < 2;
        noise ^= noise << 13;
        noise ^= noise >> 7;
        noise ^= noise << 17;
        let white = (noise as f32 / u64::MAX as f32) * 2.0 - 1.0;
        let kick = if breakdown { 0.0 } else { (-in_beat * 28.0).exp() * (std::f32::consts::TAU * (50.0 + 110.0 * (-in_beat * 45.0).exp()) * in_beat).sin() };
        let off = ((b + 0.5).fract()) * beat;
        let hat = if intro { 0.0 } else { (-off * 70.0).exp() * white * 0.35 };
        let f = notes[(bar / 2) % notes.len()];
        let bass = if intro { 0.0 } else { (-off * 6.0).exp() * (std::f32::consts::TAU * f * t).sin() * 0.45 };
        let pad = if breakdown { (std::f32::consts::TAU * 220.0 * t).sin() * 0.12 * ((t * 0.5).sin().abs() + 0.3) + white * 0.04 } else { 0.0 };
        let s = ((kick * 0.85 + hat + bass + pad).clamp(-1.0, 1.0) * 0.8 * f32::from(i16::MAX)) as i16;
        data.extend_from_slice(&s.to_le_bytes());
        data.extend_from_slice(&s.to_le_bytes());
    }
    let mut wav = Vec::with_capacity(44 + data.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
    wav.extend_from_slice(&data);
    std::fs::write(path, wav)
}

/// Points the first few demo tracks at synthetic audio and analyses them.
pub fn with_audio(loc: &LibraryLocation, dir: &std::path::Path, count: usize) -> Result<(), DemoError> {
    let store = crate::store::Store::new(loc.clone());
    for i in 0..count {
        let path = dir.join(format!("demo-{i}.wav"));
        synth_track(&path, 122.0 + i as f32 * 3.0, 0x1234 + i as u64)?;
        let name = path.to_string_lossy().into_owned();
        let db = rbl_db::Library::open(loc.clone(), OpenMode::ReadWrite)?;
        db.connection()
            .execute("UPDATE djmdContent SET FolderPath = ?1, FileNameL = ?2 WHERE ID = ?3", params![name, format!("demo-{i}.wav"), rbl_db::fixture::track_id(i)])
            ?;
        drop(db);
        let db = rbl_db::Library::open(loc.clone(), OpenMode::ReadOnly)?;
        let (index, _) = rbl_index::load(&db)?;
        let row = index.row_of(&rbl_db::fixture::track_id(i)).ok_or("missing demo row")?;
        let track = crate::analysis::tracks(&index, &[row]).remove(0);
        crate::analysis::analyse_now(&store, &track)?;
        // A few hot cues, so the deck's pads and markers have something to show.
        let id = rbl_db::fixture::track_id(i);
        for (slot, ms) in [(0u8, 3_900u32), (1, 15_700), (2, 31_400), (4, 47_000)] {
            store.write(|w| w.add_cue(&id, crate::deck::kind_of_slot(slot), ms).map(|_| ()))?;
        }
    }
    Ok(())
}

/// Paints a cover: a soft two-colour gradient with a few rings and bands, different for each
/// seed, so the demo looks like a real crate of records.
pub fn cover(seed: u64, size: u32) -> image::RgbImage {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x % 10_000) as f32 / 10_000.0
    };
    let hue_a = next() * 360.0;
    let hue_b = hue_a + 40.0 + next() * 140.0;
    let a = hsl(hue_a, 0.55 + next() * 0.3, 0.25 + next() * 0.25);
    let b = hsl(hue_b, 0.5 + next() * 0.4, 0.45 + next() * 0.3);
    let ink = hsl(hue_a + 180.0, 0.3, 0.9);
    let style = (next() * 3.0) as u32;
    let (cx, cy, r0) = (0.3 + next() * 0.4, 0.3 + next() * 0.4, 0.12 + next() * 0.2);
    let bands = 3.0 + (next() * 6.0).floor();
    image::RgbImage::from_fn(size, size, |px, py| {
        let (u, v) = (px as f32 / size as f32, py as f32 / size as f32);
        let t = (u * 0.35 + v * 0.65).clamp(0.0, 1.0);
        let mut c = [0.0f32; 3];
        for i in 0..3 {
            c[i] = a[i] + (b[i] - a[i]) * t;
        }
        let d = ((u - cx).powi(2) + (v - cy).powi(2)).sqrt();
        let mark = match style {
            0 => ((d / r0 * 3.0).fract() < 0.12 && d < r0 * 2.2) as u8 as f32 * 0.55,
            1 => (d < r0) as u8 as f32 * 0.85,
            _ => (((v * bands).fract() < 0.18) && u > 0.12 && u < 0.88) as u8 as f32 * 0.4,
        };
        let grain = ((px.wrapping_mul(73_856_093) ^ py.wrapping_mul(19_349_663)) % 17) as f32 / 17.0 * 0.04;
        let rgb = [0, 1, 2].map(|i| ((c[i] + (ink[i] - c[i]) * mark + grain).clamp(0.0, 1.0) * 255.0) as u8);
        image::Rgb(rgb)
    })
}

fn hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r + m, g + m, b + m]
}

/// Writes 24 covers into the share tree and gives every demo track one.
pub fn with_artwork(loc: &LibraryLocation) -> Result<(), DemoError> {
    let dir = loc.share_root.join("PIONEER/Artwork/00001");
    std::fs::create_dir_all(&dir)?;
    for i in 0..24u64 {
        cover(i + 7, 300).save(dir.join(format!("a{i}.png")))?;
    }
    let db = rbl_db::Library::open(loc.clone(), OpenMode::ReadWrite)?;
    let c = db.connection();
    let ids: Vec<String> = {
        let mut q = c.prepare("SELECT ID FROM djmdContent")?;
        q.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<_>>()?
    };
    for (n, id) in ids.iter().enumerate() {
        let art = (n * 7 + n / 5) % 24;
        c.execute("UPDATE djmdContent SET ImagePath = ?2 WHERE ID = ?1", params![id, format!("/PIONEER/Artwork/00001/a{art}.png")])?;
    }
    Ok(())
}
