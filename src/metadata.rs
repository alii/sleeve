//! Editing a track's details in place: its title, artist, album, genre, label and comment.
//!
//! In the details panel a double click starts editing. In the track list it's a click on a row
//! that was already selected, after a moment, as in Finder and rekordbox: a double click there
//! loads the track instead, so the pause tells the two apart. Return or clicking away saves;
//! Escape doesn't.

use rbl_db::write::TrackField;
use rbl_index::Row;
use rune::{TextEdit, Tx};

use crate::app::App;
use crate::jobs::JobId;

/// How long after the click a track list cell turns into a field. Short, so editing feels
/// immediate; a double click usually lands its second click in the new field, which
/// `SECOND_CLICK_SECS` catches.
pub const EDIT_DELAY_SECS: f64 = 0.08;

/// A click inside a field this soon after it opened is the second half of a double click, not
/// someone placing the caret: the field closes again and the track loads, as a double click
/// does anywhere else on the row.
const SECOND_CLICK_SECS: f64 = 0.45;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Title,
    Artist,
    Album,
    Genre,
    Label,
    Comment,
}

impl Field {
    /// The track list's editable columns, by their column id.
    pub fn of_column(col: &str) -> Option<Field> {
        Some(match col {
            "title" => Field::Title,
            "artist" => Field::Artist,
            "album" => Field::Album,
            "genre" => Field::Genre,
            _ => return None,
        })
    }

    fn noun(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Artist => "artist",
            Field::Album => "album",
            Field::Genre => "genre",
            Field::Label => "label",
            Field::Comment => "comment",
        }
    }
}

/// The field being typed in, and whether it's in the details panel or the track list: the
/// same track's title can show in both, and only one of them is a text box.
#[derive(Clone, Debug, PartialEq)]
pub struct Editing {
    pub id: String,
    pub field: Field,
    pub in_panel: bool,
    /// When it opened, and the text it opened with, to tell a double click's second click.
    opened_at: f64,
    original: String,
}

/// A value just saved, shown in place of the library's until the reload after its job brings
/// it in, so the old value doesn't flash back for a moment.
pub struct Shown {
    job: JobId,
    id: String,
    field: Field,
    value: String,
}

impl App {
    /// A track's value for a field, as the library has it.
    pub fn stored_value(&self, row: Row, field: Field) -> String {
        let Some(l) = self.loaded() else { return String::new() };
        let i = row as usize;
        match field {
            Field::Title => l.index.title.get(i),
            Field::Artist => l.index.artist_name(row),
            Field::Album => l.index.album_name(row),
            Field::Genre => l.index.genre_name(row),
            Field::Label => l.index.label_name(row),
            Field::Comment => l.index.comment.get(i),
        }
        .to_owned()
    }

    /// A value just saved for this track and field, while the library hasn't caught up yet.
    pub fn shown(&self, row: Row, field: Field) -> Option<&str> {
        if self.shown_edits.is_empty() {
            return None;
        }
        let id = self.loaded()?.index.ids.get(row as usize)?.to_string();
        self.shown_edits.iter().rev().find(|e| e.id == id && e.field == field).map(|e| e.value.as_str())
    }

    pub fn is_editing(&self, id: &str, field: Field, in_panel: bool) -> bool {
        self.editing.as_ref().is_some_and(|e| e.id == id && e.field == field && e.in_panel == in_panel)
    }

    /// Turns a field into a text box, with all of it selected so typing replaces it.
    pub fn start_edit(&mut self, id: &str, field: Field, in_panel: bool) {
        self.edit_armed = None;
        if self.editing.is_some() {
            self.commit_edit();
        }
        let Some(row) = self.loaded().and_then(|l| l.index.row_of(id)) else { return };
        let value = self.shown(row, field).map_or_else(|| self.stored_value(row, field), str::to_owned);
        let mut edit = TextEdit::new(value.clone());
        edit.anchor = 0;
        edit.focus = value.len();
        edit.focused = true;
        self.edit = edit;
        self.editing = Some(Editing { id: id.to_owned(), field, in_panel, opened_at: self.clock, original: value });
    }

    pub fn cancel_edit(&mut self) {
        self.editing = None;
        self.edit.focused = false;
    }

    /// Saves what was typed, as a job. Nothing changed, nothing is written. A track has to keep
    /// a title, so an empty one is ignored.
    pub fn commit_edit(&mut self) {
        let Some(Editing { id, field, .. }) = self.editing.take() else { return };
        self.edit.focused = false;
        let value = self.edit.text.trim().to_owned();
        let Some(row) = self.loaded().and_then(|l| l.index.row_of(&id)) else { return };
        let before = self.shown(row, field).map_or_else(|| self.stored_value(row, field), str::to_owned);
        if value == before || (field == Field::Title && value.is_empty()) {
            return;
        }
        let title = self.stored_value(row, Field::Title);
        let noun = field.noun();
        let doing = format!("Changing the {noun} of \"{title}\" to \"{value}\"");
        let done = format!("Changed the {noun} of \"{title}\" to \"{value}\"");
        let (content, typed) = (id.clone(), value.clone());
        let job = self.change(doing, done, move |w| {
            let track_field = match field {
                Field::Title => TrackField::Title,
                Field::Artist => TrackField::Artist,
                Field::Album => TrackField::Album,
                Field::Genre => TrackField::Genre,
                Field::Label => TrackField::Label,
                // Not a `TrackField`: the comment has its own call.
                Field::Comment => return w.set_comment(&content, &typed).map(|_| ()),
            };
            w.set_field(&content, track_field, &typed).map(|_| ())
        });
        if let Some((job, _)) = job {
            self.shown_edits.push(Shown { job, id, field, value });
        }
    }

    /// After a reload: values whose job has finished are in the library now (or failed, and the
    /// library's value is the right one to show again).
    pub(crate) fn settle_shown_edits(&mut self) {
        let jobs = &self.jobs;
        self.shown_edits.retain(|e| jobs.get(e.job).is_some_and(crate::jobs::Job::is_active));
    }

    /// A click on a track list row. A second click on the row that was already the only one
    /// selected, over an editable column, edits that cell once a double click is ruled out.
    pub fn track_clicked(&mut self, id: &str, tx: &mut Tx) {
        let again = self.last_clicked.as_deref() == Some(id);
        self.last_clicked = Some(id.to_owned());
        self.edit_armed = None;
        if self.editing.as_ref().is_some_and(|e| e.id != id) {
            self.commit_edit();
        }
        let m = tx.mods();
        let plain = !m.shift && !m.cmd && !m.ctrl && !m.alt;
        if let (true, true, Some(field)) = (again && plain, self.selected_rows().len() == 1, self.hover_field) {
            self.edit_armed = Some((id.to_owned(), field, self.clock));
            tx.wake_after(std::time::Duration::from_secs_f64(EDIT_DELAY_SECS));
        }
    }

    /// Runs each turn: starts an armed edit once its moment has passed, and saves a field that
    /// lost focus because something else was clicked.
    pub(crate) fn tick_edit(&mut self, tx: &mut Tx) -> bool {
        let mut changed = false;
        // A track list field that just opened and got clicked, its text untouched but its
        // selection moved: that click was a double click's second. Load, don't edit.
        if let Some(e) = &self.editing
            && !e.in_panel
            && self.clock - e.opened_at < SECOND_CLICK_SECS
            && self.edit.text == e.original
            && (self.edit.anchor, self.edit.focus) != (0, self.edit.text.len())
        {
            let id = e.id.clone();
            self.cancel_edit();
            self.load_track(&id, tx);
            return true;
        }
        if let Some((id, field, at)) = self.edit_armed.clone()
            && self.clock - at >= EDIT_DELAY_SECS
        {
            self.start_edit(&id, field, false);
            changed = true;
        }
        if self.editing.is_some() && !self.edit.focused {
            self.commit_edit();
            changed = true;
        }
        changed
    }
}
