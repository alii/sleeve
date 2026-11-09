//! Filtering the track list by column: tick the artists, albums, genres, keys or ratings to
//! show, or give a BPM range. A column with nothing ticked shows everything.
//!
//! It runs after the library's own view (the playlist, the search and the sort), on the rows
//! that view gives back. A few string comparisons a row is nothing next to drawing them.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rbl_index::{Library as Index, Row};
use rune::TextEdit;

use crate::app::App;

/// The columns that can be filtered, with their titles, in the order the Filter menu lists them.
pub const COLUMNS: [(&str, &str); 6] = [("artist", "Artist"), ("album", "Album"), ("genre", "Genre"), ("bpm", "BPM"), ("key", "Key"), ("rating", "Rating")];

/// The ticked values of each filtered column, and the BPM range. A column that isn't in
/// `picks` isn't filtered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    pub picks: BTreeMap<String, BTreeSet<String>>,
    pub bpm: (Option<f64>, Option<f64>),
}

impl Filters {
    pub fn is_empty(&self) -> bool {
        self.picks.is_empty() && self.bpm == (None, None)
    }

    /// How many columns are filtered.
    pub fn count(&self) -> usize {
        self.picks.len() + usize::from(self.bpm != (None, None))
    }

    pub fn is_on(&self, col: &str) -> bool {
        if col == "bpm" { self.bpm != (None, None) } else { self.picks.contains_key(col) }
    }

    /// Whether a row passes every filtered column except `skip`. Skipping one column lists the
    /// values that column could still show, as a spreadsheet's filter does.
    pub fn keeps(&self, index: &Index, row: Row, skip: Option<&str>) -> bool {
        if skip != Some("bpm") && self.bpm != (None, None) {
            let bpm = f64::from(index.bpm_x100[row as usize]) / 100.0;
            // A track with no BPM yet has no place in a range.
            if bpm <= 0.0 || self.bpm.0.is_some_and(|lo| bpm < lo) || self.bpm.1.is_some_and(|hi| bpm > hi) {
                return false;
            }
        }
        self.picks.iter().filter(|(col, _)| Some(col.as_str()) != skip).all(|(col, set)| set.contains(&value(index, row, col)))
    }
}

/// A row's value in a filterable column, as the filter compares it: names as they are, the
/// key in Camelot (as the list shows it), the rating as a star count.
pub fn value(index: &Index, row: Row, col: &str) -> String {
    match col {
        "artist" => index.artist_name(row).to_owned(),
        "album" => index.album_name(row).to_owned(),
        "genre" => index.genre_name(row).to_owned(),
        "key" => {
            let key = index.key_name(row);
            crate::ui::tracks::camelot(key, true).map_or_else(|| key.to_owned(), |(label, _)| label)
        }
        "rating" => index.rating[row as usize].to_string(),
        _ => String::new(),
    }
}

/// What a value is called in the filter's list.
pub fn label(col: &str, value: &str) -> String {
    match (col, value) {
        ("rating", "0") => "No stars".to_owned(),
        ("rating", "1") => "1 star".to_owned(),
        ("rating", n) => format!("{n} stars"),
        ("key", "") => "No key".to_owned(),
        ("album", "") => "No album".to_owned(),
        ("artist", "") => "No artist".to_owned(),
        ("genre", "") => "No genre".to_owned(),
        (_, v) => v.to_owned(),
    }
}

/// The order a column's values are listed in: Camelot keys by number then letter, ratings from
/// five stars down, names alphabetically. Blanks go last.
fn order(col: &str, value: &str) -> (bool, u32, String) {
    let blank = value.is_empty();
    match col {
        "key" => {
            let num = value.trim_end_matches(['A', 'B']).parse::<u32>().unwrap_or(99);
            (blank, num, value.to_owned())
        }
        "rating" => (false, 5 - value.parse::<u32>().unwrap_or(0).min(5), String::new()),
        _ => (blank, 0, value.to_lowercase()),
    }
}

impl App {
    /// The values a column's filter can tick, with how many tracks have each: from what's in
    /// view before filtering, narrowed by the other columns' filters.
    pub fn filter_values(&self, col: &str) -> Vec<(String, usize)> {
        let Some(l) = self.loaded() else { return Vec::new() };
        let mut counts: HashMap<String, usize> = HashMap::new();
        for &row in &self.base_rows {
            if self.filters.keeps(&l.index, row, Some(col)) {
                *counts.entry(value(&l.index, row, col)).or_default() += 1;
            }
        }
        let mut out: Vec<(String, usize)> = counts.into_iter().collect();
        out.sort_by_cached_key(|(v, _)| order(col, v));
        out
    }

    /// Ticks or unticks one value in a column's filter.
    pub fn toggle_filter(&mut self, col: &str, value: &str) {
        let set = self.filters.picks.entry(col.to_owned()).or_default();
        if !set.remove(value) {
            set.insert(value.to_owned());
        }
        if set.is_empty() {
            self.filters.picks.remove(col);
        }
        self.refresh_rows();
    }

    /// Reads the BPM boxes. A box that's empty or isn't a number sets no limit on that side.
    pub fn bpm_filter_changed(&mut self) {
        let read = |e: &TextEdit| e.text.trim().replace(',', ".").parse::<f64>().ok().filter(|v| *v > 0.0);
        self.filters.bpm = (read(&self.bpm_from), read(&self.bpm_to));
        self.refresh_rows();
    }

    pub fn clear_filter(&mut self, col: &str) {
        if col == "bpm" {
            self.filters.bpm = (None, None);
            self.bpm_from = TextEdit::new("");
            self.bpm_to = TextEdit::new("");
        } else {
            self.filters.picks.remove(col);
        }
        self.refresh_rows();
    }

    pub fn clear_filters(&mut self) {
        self.filters = Filters::default();
        self.bpm_from = TextEdit::new("");
        self.bpm_to = TextEdit::new("");
        self.filter_open = None;
        self.refresh_rows();
    }

    /// Opens a column's filter, with its search box empty and ready to type in.
    pub fn open_filter(&mut self, col: &str) {
        let mut search = TextEdit::new("");
        search.focused = col != "bpm";
        self.filter_search = search;
        self.filter_open = Some(col.to_owned());
    }
}
