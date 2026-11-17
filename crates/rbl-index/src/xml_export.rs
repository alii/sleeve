//! The collection as rekordbox's XML, for `File › Export Collection in xml
//! format`: what another rekordbox, or anything that reads the format,
//! takes in.
//!
//! A `DJ_PLAYLISTS` document: every track of the library as a `TRACK` with
//! its tags as attributes and its cues as `POSITION_MARK`s, and the playlist
//! tree as `NODE`s. An intelligent playlist is written as the tracks its
//! rule admits now, which is what rekordbox writes for one too.
//!
//! The grids live in the analysis files, not the library, so `TEMPO` entries
//! are written only when the caller reads them: [`export_xml_with_grids`].
//! A `TEMPO` says "from here, beats go at this tempo, and this one is beat
//! N of its bar", and holds until the next one. That's how rekordbox
//! writes a dynamic grid, a tempo change included: a steady stretch is one
//! entry, and a ramp, where every beat has its own tempo, is one per beat.
//! See [`tempo_marks`].

use std::fmt::Write as _;

use rbl_core::xml::escape;

use rbl_anlz::Beat;

use crate::{Cue, Library, Row, TrackSource};

/// rekordbox's `Colour` values for `ColorID` 1 to 8, in the palette's own
/// order (see `COLOR_NAMES`) [DOC: pyrekordbox's account of the format].
const COLOURS: [&str; 8] =
    ["0xFF007F", "0xFF0000", "0xFFA500", "0xFFFF00", "0x00FF00", "0x25FDE9", "0x0000FF", "0x660099"];

/// The whole library as a document, without grids.
#[must_use]
pub fn export_xml(lib: &Library) -> String {
    export_xml_with_grids(lib, |_| None)
}

/// The whole library as a document, with each track's grid as `TEMPO`
/// entries: `grid` reads a row's beats, or gives `None` for a track without.
/// It's called once per row, in order, so it can report progress.
#[must_use]
pub fn export_xml_with_grids(lib: &Library, mut grid: impl FnMut(Row) -> Option<Vec<Beat>>) -> String {
    let mut out = String::with_capacity(lib.len() * 400);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<DJ_PLAYLISTS Version=\"1.0.0\">\n");
    out.push_str("  <PRODUCT Name=\"rbxport\" Version=\"");
    out.push_str(env!("CARGO_PKG_VERSION"));
    out.push_str("\" Company=\"rbxport\"/>\n");
    let _ = writeln!(out, "  <COLLECTION Entries=\"{}\">", lib.len());
    for row in 0..u32::try_from(lib.len()).unwrap_or(u32::MAX) {
        push_track(&mut out, lib, row, &grid(row).unwrap_or_default());
    }
    out.push_str("  </COLLECTION>\n  <PLAYLISTS>\n");
    push_tree(&mut out, lib);
    out.push_str("  </PLAYLISTS>\n</DJ_PLAYLISTS>\n");
    out
}

fn push_track(out: &mut String, lib: &Library, row: Row, beats: &[Beat]) {
    let i = row as usize;
    let id = lib.ids.get(i).copied().unwrap_or(0);
    let bpm = f64::from(lib.bpm_x100.get(i).copied().unwrap_or(0)) / 100.0;
    let colour = lib.color.get(i).copied().unwrap_or(0) as usize;
    let _ = write!(
        out,
        "    <TRACK TrackID=\"{id}\" Name=\"{}\" Artist=\"{}\" Album=\"{}\" Genre=\"{}\" Label=\"{}\" \
         TotalTime=\"{}\" Year=\"{}\" AverageBpm=\"{bpm:.2}\" DateAdded=\"{}\" Comments=\"{}\" \
         PlayCount=\"{}\" Rating=\"{}\" Tonality=\"{}\" Location=\"{}\"",
        escape(lib.title.get(i)),
        escape(lib.artist_name(row)),
        escape(lib.album_name(row)),
        escape(lib.genre_name(row)),
        escape(lib.label_name(row)),
        lib.length_sec.get(i).copied().unwrap_or(0),
        lib.year.get(i).copied().unwrap_or(0),
        escape(lib.date_added.get(i)),
        escape(lib.comment.get(i)),
        lib.play_count.get(i).copied().unwrap_or(0),
        u32::from(lib.rating.get(i).copied().unwrap_or(0)) * 51,
        escape(lib.key_name(row)),
        escape(&location(lib.folder_path.get(i))),
    );
    if let Some(hex) = colour.checked_sub(1).and_then(|c| COLOURS.get(c)) {
        let _ = write!(out, " Colour=\"{hex}\"");
    }
    let cues = lib.cues_of(row);
    let tempos = tempo_marks(beats);
    if cues.is_empty() && tempos.is_empty() {
        out.push_str("/>\n");
        return;
    }
    out.push_str(">\n");
    // rekordbox writes the grid before the cues.
    for mark in &tempos {
        let _ = writeln!(
            out,
            "      <TEMPO Inizio=\"{:.3}\" Bpm=\"{:.2}\" Metro=\"4/4\" Battito=\"{}\"/>",
            f64::from(mark.at_ms) / 1000.0,
            f64::from(mark.bpm_x100) / 100.0,
            mark.beat,
        );
    }
    for cue in &cues {
        push_cue(out, cue);
    }
    out.push_str("    </TRACK>\n");
}

/// One `TEMPO` entry: from `at_ms`, beats at `bpm_x100`, the first of them
/// beat `beat` (1 to 4) of its bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TempoMark {
    pub at_ms: u32,
    pub bpm_x100: u16,
    pub beat: u16,
}

/// A grid as `TEMPO` entries: a new one wherever the beats stop following
/// the last. That's where the tempo changes, which in a ramp is every beat;
/// where the bar count jumps; and where the beats have drifted more than a
/// millisecond from where the entry's tempo puts them (more than the
/// grid's own rounding), so a reader that counts beats out from an entry
/// lands on the grid's beats, not near them.
#[must_use]
pub fn tempo_marks(beats: &[Beat]) -> Vec<TempoMark> {
    let mut marks: Vec<TempoMark> = Vec::new();
    let mut from = 0_usize;
    for (i, b) in beats.iter().enumerate() {
        if b.tempo_x100 == 0 {
            continue;
        }
        let follows = marks.last().is_some_and(|m| {
            let n = i - from;
            let at = f64::from(m.at_ms) + f64::from(u32::try_from(n).unwrap_or(u32::MAX)) * 6_000_000.0 / f64::from(m.bpm_x100);
            let beat = (usize::from(m.beat.max(1)) - 1 + n) % 4 + 1;
            m.bpm_x100 == b.tempo_x100 && (at - f64::from(b.time_ms)).abs() <= 1.0 && beat == usize::from(b.beat_number)
        });
        if !follows {
            marks.push(TempoMark { at_ms: b.time_ms, bpm_x100: b.tempo_x100, beat: b.beat_number });
            from = i;
        }
    }
    marks
}

fn push_cue(out: &mut String, cue: &Cue) {
    // `Num` -1 is a memory cue; a hot cue is its slot from 0. `Type` 0 is a
    // cue and 4 a loop, which also carries its `End`.
    let num = cue.hot_letter().map_or(-1, |letter| i32::from(letter as u8) - i32::from(b'A'));
    let start = f64::from(cue.position_ms) / 1000.0;
    if cue.out_ms > cue.position_ms {
        let end = f64::from(cue.out_ms) / 1000.0;
        let _ = writeln!(out, "      <POSITION_MARK Name=\"\" Type=\"4\" Start=\"{start:.3}\" End=\"{end:.3}\" Num=\"{num}\"/>");
    } else {
        let _ = writeln!(out, "      <POSITION_MARK Name=\"\" Type=\"0\" Start=\"{start:.3}\" Num=\"{num}\"/>");
    }
}

/// The tree, depth-first in `Seq` order, folders as `Type="0"` and
/// playlists as `Type="1"` with their tracks.
fn push_tree(out: &mut String, lib: &Library) {
    let playlists = lib.playlists();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); playlists.len()];
    let mut roots: Vec<usize> = Vec::new();
    for index in 0..playlists.len() {
        match playlists.parent.get(index).copied() {
            Some(parent) if parent != crate::NO_ID && (parent as usize) < playlists.len() => {
                if let Some(bucket) = children.get_mut(parent as usize) {
                    bucket.push(index);
                }
            }
            _ => roots.push(index),
        }
    }
    let _ = writeln!(out, "    <NODE Type=\"0\" Name=\"ROOT\" Count=\"{}\">", roots.len());
    // Iterative with a visited set, as the tree command walks it: a corrupt
    // parent cycle must not recurse for ever.
    let mut visited = vec![false; playlists.len()];
    let mut stack: Vec<(usize, usize, bool)> = roots.iter().rev().map(|&i| (i, 3, false)).collect();
    while let Some((index, depth, closing)) = stack.pop() {
        let indent = "  ".repeat(depth);
        if closing {
            let _ = writeln!(out, "{indent}</NODE>");
            continue;
        }
        if visited.get(index).copied().unwrap_or(true) {
            continue;
        }
        if let Some(slot) = visited.get_mut(index) {
            *slot = true;
        }
        let name = escape(playlists.name(index));
        if playlists.is_folder(index) {
            let below = children.get(index).cloned().unwrap_or_default();
            let _ = writeln!(out, "{indent}<NODE Name=\"{name}\" Type=\"0\" Count=\"{}\">", below.len());
            stack.push((index, depth, true));
            for &child in below.iter().rev() {
                stack.push((child, depth + 1, false));
            }
        } else {
            let source = if playlists.is_smart(index) {
                TrackSource::SmartPlaylist(index)
            } else {
                TrackSource::Playlist(index)
            };
            // Read after the guard: `source_rows` takes the playlists lock.
            let rows = lib.source_rows_unlocked(&playlists, &source);
            let _ = writeln!(out, "{indent}<NODE Name=\"{name}\" Type=\"1\" KeyType=\"0\" Entries=\"{}\">", rows.len());
            for row in rows {
                let _ = writeln!(out, "{indent}  <TRACK Key=\"{}\"/>", lib.ids.get(row as usize).copied().unwrap_or(0));
            }
            let _ = writeln!(out, "{indent}</NODE>");
        }
    }
    out.push_str("    </NODE>\n");
}

/// A path as rekordbox writes `Location`: `file://localhost` and the path
/// percent-encoded, `/` and the unreserved characters left as they are.
fn location(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 16);
    out.push_str("file://localhost");
    let path = path.replace('\\', "/");
    if !path.starts_with('/') {
        out.push('/');
    }
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(char::from(byte)),
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
// The helpers' casts are of beat counts and lengths a test picks, all small and positive.
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
mod tests {
    use super::*;

    /// A grid from beat lengths in milliseconds, each beat carrying the
    /// tempo of its own length, as Sleeve and rekordbox write them.
    fn grid(lengths: &[f64]) -> Vec<Beat> {
        let mut at = 25.0_f64;
        lengths
            .iter()
            .enumerate()
            .map(|(i, len)| {
                let b = Beat { beat_number: (i % 4 + 1) as u16, tempo_x100: (6_000_000.0 / len).round() as u16, time_ms: at.round() as u32 };
                at += len;
                b
            })
            .collect()
    }

    /// Where a reader of the entries puts each beat: counted out from the
    /// entry before it at that entry's tempo.
    fn beats_from(marks: &[TempoMark], count: usize) -> Vec<u32> {
        let mut out = Vec::new();
        for (k, m) in marks.iter().enumerate() {
            // Up to half a beat before the next entry: a tempo rounded to
            // hundredths can put the beat it would count next a hair early.
            let len = 6_000_000.0 / f64::from(m.bpm_x100);
            let until = marks.get(k + 1).map_or(f64::INFINITY, |n| f64::from(n.at_ms) - len / 2.0);
            let mut n = 0.0;
            loop {
                let at = f64::from(m.at_ms) + n * len;
                if at >= until || out.len() == count {
                    break;
                }
                out.push(at.round() as u32);
                n += 1.0;
            }
        }
        out
    }

    #[test]
    fn a_steady_grid_is_one_tempo_and_a_ramp_is_one_per_beat() {
        let steady = grid(&[468.75; 64]);
        assert_eq!(tempo_marks(&steady), [TempoMark { at_ms: 25, bpm_x100: 12800, beat: 1 }]);

        // 128 for 4 bars, a 16-beat ramp to 140, then 140 for 4 bars.
        let mut lengths = vec![468.75; 16];
        lengths.extend((1..=16).map(|i| 468.75 + (60_000.0 / 140.0 - 468.75) * f64::from(i) / 16.0));
        lengths.extend([60_000.0 / 140.0; 16]);
        let ramp = grid(&lengths);
        let marks = tempo_marks(&ramp);
        assert_eq!(marks.first().map(|m| m.bpm_x100), Some(12800));
        assert_eq!(marks.last().map(|m| m.bpm_x100), Some(14000));
        assert!((16..=24).contains(&marks.len()), "{} entries", marks.len());
        // Whatever reads them puts every beat back where the grid has it.
        let back = beats_from(&marks, ramp.len());
        assert_eq!(back.len(), ramp.len());
        for (b, at) in ramp.iter().zip(&back) {
            assert!(b.time_ms.abs_diff(*at) <= 1, "{} against {at}", b.time_ms);
        }
    }

    #[test]
    fn an_instant_change_and_a_bar_jump_each_start_an_entry() {
        let mut lengths = vec![500.0; 8];
        lengths.extend([400.0; 8]);
        let mut beats = grid(&lengths);
        let marks = tempo_marks(&beats);
        assert_eq!(marks.iter().map(|m| m.bpm_x100).collect::<Vec<_>>(), [12000, 15000]);
        // The grid starts the bar again on beat 5.
        for (i, b) in beats.iter_mut().enumerate().skip(5) {
            b.beat_number = ((i - 5) % 4 + 1) as u16;
        }
        assert_eq!(tempo_marks(&beats).len(), 3);
    }

    #[test]
    fn a_location_is_a_file_url_with_the_path_percent_encoded() {
        assert_eq!(location("/Users/me/Music/All U Need.mp3"), "file://localhost/Users/me/Music/All%20U%20Need.mp3");
        assert_eq!(location("C:\\Music\\x&y.mp3"), "file://localhost/C%3A/Music/x%26y.mp3");
        assert_eq!(location("/Müsic/♪.mp3"), "file://localhost/M%C3%BCsic/%E2%99%AA.mp3");
    }
}
