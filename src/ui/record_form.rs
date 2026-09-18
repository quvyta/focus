//! The record form: a session typed in by hand, a correction of one on disk, a lost focus
//! attached to a real one, or a duration over the ceiling fixed before the record is written.
//!
//! The form owns what is typed and checks it; nothing is saved until every check passes, and a
//! failed check is written beside its field rather than shown as a colour. The application
//! turns what the form hands back into a record with [`crate::session::edit`].

use std::time::Duration;

use qframe::date::{Date, DateTime, TimeOfDay};
use qframe::icons::GlyphMode;
use qframe::prelude::*;
use qframe::widgets::{
    DatePicker, DurationError, DurationInput, Field, Form, FormErrors, Modal, Select, TextInput, TimeInput,
};

use crate::id::Id;
use crate::session::edit::{Changes, manual};
use crate::session::{Session, line};
use crate::store::sessions::MAX_LINE_BYTES;
use crate::tree::Tree as Catalog;

/// The widget id of the focus chooser, for focusing it.
pub const FOCUS_SELECT: &str = "record-focus";

/// The widget id of the date field.
pub const DATE_PICKER: &str = "record-date";

/// The widget id of the start time field.
pub const START_INPUT: &str = "record-start";

/// The widget id of the duration field, for focusing it.
pub const DURATION_INPUT: &str = "record-duration";

/// The widget id of the note field.
pub const NOTE_INPUT: &str = "record-note";

/// The widget id of the save button.
pub const SAVE: &str = "record-save";

/// The most bytes a note may take in a record line.
pub const MAX_NOTE_BYTES: usize = 1024;

/// Cells the labels take beside the controls while the form is wide enough.
const LABEL_WIDTH: u16 = 10;

/// What the form is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    /// A session typed in after the fact.
    Add,
    /// A correction of the session with this identifier.
    Correct(Id),
    /// The session with this identifier belongs to a focus that is gone; it is attached to one
    /// that exists.
    Attach(Id),
    /// The running counter went over the ceiling; its duration is fixed before it is recorded.
    Finish,
}

/// Something that happened on the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// An option of the focus chooser was chosen.
    Focus(usize),
    /// The date changed.
    Date(Date),
    /// The start time changed.
    Start(TimeOfDay),
    /// The duration changed.
    Duration(Duration),
    /// A paste into the duration field could not be read.
    Rejected(DurationError),
    /// The note changed.
    Note(String),
    /// Save was asked for.
    Submit,
    /// The form was closed without saving.
    Cancel,
}

/// What the form hands back to the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Every check passed; this is what was typed.
    Save(Changes),
    /// The form was closed without saving.
    Cancel,
}

/// The state of the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordForm {
    purpose: Purpose,
    focus: Option<Id>,
    date: Date,
    start: TimeOfDay,
    duration: Duration,
    note: String,
    offset_minutes: i16,
    /// Why the last paste into the duration field was not read, until the field changes.
    rejected: Option<DurationError>,
    /// Whether saving was tried; the checks are written beside the fields from then on.
    tried: bool,
}

impl RecordForm {
    /// An empty form for a session typed in by hand, dated `now`: today, this hour and minute,
    /// no duration, no focus.
    #[must_use]
    pub fn add(now: DateTime) -> Self {
        Self {
            purpose: Purpose::Add,
            focus: None,
            date: now.date,
            start: TimeOfDay::new(now.time.hour, now.time.minute, 0),
            duration: Duration::ZERO,
            note: String::new(),
            offset_minutes: now.offset_minutes,
            rejected: None,
            tried: false,
        }
    }

    /// A form filled from `session`, for `purpose`.
    #[must_use]
    pub fn of(purpose: Purpose, session: &Session) -> Self {
        let started = DateTime::from_unix(session.started, session.offset_minutes);
        let changes = Changes::of(session);
        Self {
            purpose,
            focus: Some(session.focus),
            date: started.date,
            start: started.time,
            duration: Duration::from_secs(u64::from(changes.seconds)),
            note: changes.note,
            offset_minutes: session.offset_minutes,
            rejected: None,
            tried: false,
        }
    }

    /// What the form is for.
    #[must_use]
    pub fn purpose(&self) -> &Purpose {
        &self.purpose
    }

    /// The widget that takes the keyboard when the form opens: the duration when that is what
    /// is being fixed, else the focus chooser.
    #[must_use]
    pub fn first_field(&self) -> &'static str {
        match self.purpose {
            Purpose::Finish => DURATION_INPUT,
            Purpose::Add | Purpose::Correct(_) | Purpose::Attach(_) => FOCUS_SELECT,
        }
    }

    /// The focuses the chooser offers, in the order of the catalogue: every focus that is not
    /// archived, and the one the record already has even when it is, so it can be kept.
    fn options(&self, catalog: &Catalog) -> Vec<Id> {
        let mut ids: Vec<Id> = catalog
            .categories
            .iter()
            .filter(|category| !category.archived)
            .flat_map(|category| category.focuses.iter().filter(|focus| !focus.archived).map(|focus| focus.id))
            .collect();
        if let Some(own) = self.focus
            && !ids.contains(&own)
            && catalog.focus(own).is_some()
        {
            ids.push(own);
        }
        ids
    }

    /// When the session starts, in seconds since the Unix epoch.
    fn started(&self) -> i64 {
        DateTime { date: self.date, time: self.start, offset_minutes: self.offset_minutes }.to_unix()
    }

    /// Seconds of work, as far as a span reaches.
    fn seconds(&self) -> u32 {
        u32::try_from(self.duration.as_secs()).unwrap_or(u32::MAX)
    }

    /// When the session ends, in the form's offset.
    fn ends(&self) -> DateTime {
        DateTime::from_unix(self.started().saturating_add(i64::from(self.seconds())), self.offset_minutes)
    }

    /// What was typed, with `focus` standing in when none is chosen, so the record line can be
    /// measured before the focus is.
    fn changes_with(&self, focus: Id) -> Changes {
        Changes {
            focus,
            started: self.started(),
            offset_minutes: self.offset_minutes,
            seconds: self.seconds(),
            note: self.note.trim().to_owned(),
        }
    }

    /// Everything wrong with what was typed, by field, against the catalogue and the wall clock
    /// `now`.
    fn errors(&self, catalog: &Catalog, now: i64) -> FormErrors {
        let mut errors = FormErrors::new();
        let focus = self.focus.filter(|id| catalog.focus(*id).is_some());
        errors.check(FOCUS_SELECT, focus.is_some(), t!("form.focus-empty"));
        let started = self.started();
        if started > now {
            errors.set(START_INPUT, t!("form.start-future"));
        }
        if self.seconds() == 0 {
            errors.set(DURATION_INPUT, t!("form.duration-zero"));
        } else if started <= now && started.saturating_add(i64::from(self.seconds())) > now {
            errors.set(DURATION_INPUT, t!("form.ends-future"));
        }
        let note = self.note.trim();
        if note.len() > MAX_NOTE_BYTES {
            errors.set(NOTE_INPUT, t!("form.note-long", max = u32::try_from(MAX_NOTE_BYTES).unwrap_or(u32::MAX)));
        } else {
            // The note is the only field that can grow, so the line's cap is written beside it.
            let line = line::write(&manual(&self.changes_with(focus.unwrap_or(Id::new(0, 0))), Id::new(0, 0), now));
            if line.len() + 1 > MAX_LINE_BYTES {
                errors.set(NOTE_INPUT, t!("form.line-long", max = u32::try_from(MAX_LINE_BYTES).unwrap_or(u32::MAX)));
            }
        }
        errors
    }
}

/// Applies `msg` against the catalogue and the wall clock `now`, and says what the application
/// should do with the form.
pub fn update<M: From<Msg> + Clone + Send + 'static>(
    form: &mut RecordForm,
    catalog: &Catalog,
    now: i64,
    msg: Msg,
) -> (Command<M>, Option<Outcome>) {
    match msg {
        Msg::Focus(index) => {
            form.focus = form.options(catalog).get(index).copied();
            (Command::none(), None)
        }
        Msg::Date(date) => {
            form.date = date;
            (Command::none(), None)
        }
        Msg::Start(time) => {
            form.start = time;
            (Command::none(), None)
        }
        Msg::Duration(duration) => {
            form.duration = duration;
            form.rejected = None;
            (Command::none(), None)
        }
        Msg::Rejected(error) => {
            form.rejected = Some(error);
            (Command::none(), None)
        }
        Msg::Note(text) => {
            form.note = text;
            (Command::none(), None)
        }
        Msg::Submit => {
            form.tried = true;
            let errors = form.errors(catalog, now);
            match form.focus.filter(|_| errors.is_empty()) {
                Some(focus) => (Command::none(), Some(Outcome::Save(form.changes_with(focus)))),
                None => (errors.focus_first(), None),
            }
        }
        Msg::Cancel => (Command::none(), Some(Outcome::Cancel)),
    }
}

/// The title of the form for `purpose`.
fn title(purpose: &Purpose) -> String {
    match purpose {
        Purpose::Add => t!("form.title-add"),
        Purpose::Correct(_) => t!("form.title-correct"),
        Purpose::Attach(_) => t!("form.title-attach"),
        Purpose::Finish => t!("form.title-fix"),
    }
}

/// The line under the duration that says when the session ends: the time alone on the same
/// day, the date as well when the session crosses into another.
fn ends_line(form: &RecordForm) -> String {
    let ends = form.ends();
    let time = format!("{:02}:{:02}", ends.time.hour, ends.time.minute);
    if ends.date == form.date {
        t!("form.ends", time = time)
    } else {
        let date = format!("{:04}-{:02}-{:02}", ends.date.year(), ends.date.month(), ends.date.day());
        t!("form.ends-on", date = date, time = time)
    }
}

/// Draws the form as a dialog over the page: the focus, the date, the start, the duration with
/// the end it works out to, the note, and the reason beside every field that fails a check
/// once saving was tried. `today` marks the calendar; `now` is the wall clock the checks run
/// against.
pub fn view<M: From<Msg> + Clone + Send + 'static>(
    form: &RecordForm,
    catalog: &Catalog,
    today: Date,
    now: i64,
    ui: &mut View<'_, M>,
) {
    let separator = if ui.env().icons().mode() == GlyphMode::Ascii { " > " } else { " › " };
    let options = form.options(catalog);
    let labels: Vec<String> = options
        .iter()
        .map(|id| {
            catalog
                .categories
                .iter()
                .find_map(|category| {
                    category
                        .focuses
                        .iter()
                        .find(|focus| focus.id == *id)
                        .map(|focus| format!("{}{separator}{}", category.name, focus.name))
                })
                .unwrap_or_default()
        })
        .collect();
    let selected = form.focus.and_then(|focus| options.iter().position(|id| *id == focus));
    let errors = if form.tried { form.errors(catalog, now) } else { FormErrors::new() };
    let rejected = form.rejected.as_ref().map(|error| error.message(ui.env().i18n()));
    let duration_error = errors.get(DURATION_INPUT).map(str::to_owned).or(rejected);
    let dialog = Modal::new()
        .title(title(&form.purpose))
        .on_close(M::from(Msg::Cancel))
        .action(Button::new(t!("form.cancel")).on_press(M::from(Msg::Cancel)))
        .action(Button::new(t!("form.save")).variant("primary").on_press(M::from(Msg::Submit)));
    ui.add_with(dialog, |ui| {
        Form::new().label_width(LABEL_WIDTH).show(ui, |fields| {
            fields.field(Field::new(t!("form.focus")).required(true).error(errors.get(FOCUS_SELECT)), |ui| {
                ui.add(
                    Select::new(labels.clone())
                        .selected(selected)
                        .placeholder(t!("form.focus-placeholder"))
                        .on_select(|index| M::from(Msg::Focus(index))),
                )
                .id(FOCUS_SELECT)
                .fill_width();
            });
            fields.field(Field::new(t!("form.date")).error(errors.get(DATE_PICKER)), |ui| {
                ui.add(DatePicker::new(Some(form.date)).today(today).on_change(|date| M::from(Msg::Date(date))))
                    .id(DATE_PICKER);
            });
            fields.field(Field::new(t!("form.start")).error(errors.get(START_INPUT)), |ui| {
                ui.add(
                    TimeInput::new(form.start)
                        .invalid(errors.has(START_INPUT))
                        .on_change(|time| M::from(Msg::Start(time))),
                )
                .id(START_INPUT);
            });
            fields.field(
                Field::new(t!("form.duration")).required(true).hint(ends_line(form)).error(duration_error.clone()),
                |ui| {
                    ui.add(
                        DurationInput::new(form.duration)
                            .invalid(duration_error.is_some())
                            .on_change(|duration| M::from(Msg::Duration(duration)))
                            .on_reject(|error| M::from(Msg::Rejected(error))),
                    )
                    .id(DURATION_INPUT);
                },
            );
            fields.field(Field::new(t!("form.note")).error(errors.get(NOTE_INPUT)), |ui| {
                ui.add(
                    TextInput::new(&form.note)
                        .placeholder(t!("form.note-placeholder"))
                        .invalid(errors.has(NOTE_INPUT))
                        .on_change(|text| M::from(Msg::Note(text)))
                        .on_submit(|_| M::from(Msg::Submit)),
                )
                .id(NOTE_INPUT)
                .fill_width();
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Source;
    use crate::span::{ClockSource, Span, SpanKind};
    use crate::tree::{Category, Focus};

    /// Noon UTC on 2026-09-18.
    const NOON: i64 = 1_789_732_800;

    fn catalog() -> Catalog {
        Catalog {
            categories: vec![Category {
                id: Id::new(1, 1),
                name: "Work".to_owned(),
                icon: None,
                goal: None,
                archived: false,
                order: 0.0,
                focuses: vec![
                    Focus { id: Id::new(2, 2), name: "Rust".to_owned(), goal: None, archived: false, order: 0.0 },
                    Focus { id: Id::new(3, 3), name: "Review".to_owned(), goal: None, archived: true, order: 1.0 },
                ],
            }],
        }
    }

    fn apply(form: &mut RecordForm, catalog: &Catalog, msg: Msg) -> Option<Outcome> {
        let (_, outcome): (Command<Msg>, _) = update(form, catalog, NOON, msg);
        outcome
    }

    fn session() -> Session {
        Session {
            id: Id::new(10, 1),
            revision: 1,
            written: NOON - 3_000,
            focus: Id::new(3, 3),
            started: NOON - 3_600,
            offset_minutes: 180,
            ended: NOON - 600,
            spans: vec![Span::new(SpanKind::Work, 0, 3_000, ClockSource::Mono)],
            source: Source::Timer,
            replaces: None,
            voids: None,
            continues: None,
            flags: Vec::new(),
            note: "notes".to_owned(),
        }
    }

    #[test]
    fn an_empty_form_refuses_to_save_and_names_every_reason() {
        let catalog = catalog();
        let mut form = RecordForm::add(DateTime::from_unix(NOON, 180));
        assert_eq!(apply(&mut form, &catalog, Msg::Submit), None);
        let errors = form.errors(&catalog, NOON);
        assert!(errors.has(FOCUS_SELECT));
        assert!(errors.has(DURATION_INPUT));
        assert!(!errors.has(START_INPUT), "this minute is not the future");
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn a_filled_form_saves_what_was_typed() {
        let catalog = catalog();
        let mut form = RecordForm::add(DateTime::from_unix(NOON, 180));
        assert_eq!(form.options(&catalog), vec![Id::new(2, 2)], "the archived focus is not offered");
        apply(&mut form, &catalog, Msg::Focus(0));
        apply(&mut form, &catalog, Msg::Start(TimeOfDay::new(13, 0, 0)));
        apply(&mut form, &catalog, Msg::Duration(Duration::from_secs(5_400)));
        apply(&mut form, &catalog, Msg::Note("  read  ".to_owned()));
        let outcome = apply(&mut form, &catalog, Msg::Submit);
        // Noon UTC is 15:00 in the form's offset; 13:00 to 14:30 lies before it.
        let started = DateTime { date: form.date, time: TimeOfDay::new(13, 0, 0), offset_minutes: 180 }.to_unix();
        assert_eq!(
            outcome,
            Some(Outcome::Save(Changes {
                focus: Id::new(2, 2),
                started,
                offset_minutes: 180,
                seconds: 5_400,
                note: "read".to_owned(),
            }))
        );
    }

    #[test]
    fn the_future_is_refused_at_the_start_or_at_the_end() {
        let catalog = catalog();
        let mut form = RecordForm::add(DateTime::from_unix(NOON, 180));
        apply(&mut form, &catalog, Msg::Focus(0));
        apply(&mut form, &catalog, Msg::Start(TimeOfDay::new(16, 0, 0)));
        apply(&mut form, &catalog, Msg::Duration(Duration::from_secs(600)));
        assert_eq!(apply(&mut form, &catalog, Msg::Submit), None);
        let errors = form.errors(&catalog, NOON);
        assert!(errors.has(START_INPUT));
        assert!(!errors.has(DURATION_INPUT));
        apply(&mut form, &catalog, Msg::Start(TimeOfDay::new(14, 55, 0)));
        assert_eq!(apply(&mut form, &catalog, Msg::Submit), None);
        let errors = form.errors(&catalog, NOON);
        assert!(!errors.has(START_INPUT));
        assert!(errors.has(DURATION_INPUT), "ten minutes from five to three ends after noon UTC");
        apply(&mut form, &catalog, Msg::Duration(Duration::from_secs(300)));
        assert!(matches!(apply(&mut form, &catalog, Msg::Submit), Some(Outcome::Save(_))));
    }

    #[test]
    fn a_long_note_is_refused_with_the_cap() {
        let catalog = catalog();
        let mut form = RecordForm::add(DateTime::from_unix(NOON, 180));
        apply(&mut form, &catalog, Msg::Focus(0));
        apply(&mut form, &catalog, Msg::Start(TimeOfDay::new(13, 0, 0)));
        apply(&mut form, &catalog, Msg::Duration(Duration::from_secs(60)));
        apply(&mut form, &catalog, Msg::Note("ş".repeat(600)));
        assert_eq!(apply(&mut form, &catalog, Msg::Submit), None, "600 two-byte letters pass a kibibyte");
        assert!(form.errors(&catalog, NOON).has(NOTE_INPUT));
        apply(&mut form, &catalog, Msg::Note("x".repeat(1_024)));
        assert!(matches!(apply(&mut form, &catalog, Msg::Submit), Some(Outcome::Save(_))));
    }

    #[test]
    fn a_form_over_a_session_starts_from_it_and_keeps_its_archived_focus_on_offer() {
        let catalog = catalog();
        let session = session();
        let mut form = RecordForm::of(Purpose::Correct(session.id), &session);
        assert_eq!(form.options(&catalog), vec![Id::new(2, 2), Id::new(3, 3)]);
        assert_eq!(form.start, TimeOfDay::new(14, 0, 0));
        assert_eq!(form.duration, Duration::from_secs(3_000));
        assert_eq!(form.first_field(), FOCUS_SELECT);
        let outcome = apply(&mut form, &catalog, Msg::Submit);
        assert_eq!(outcome, Some(Outcome::Save(Changes::of(&session))), "unchanged, it saves as it was");
        let mut fixing = RecordForm::of(Purpose::Finish, &session);
        assert_eq!(fixing.first_field(), DURATION_INPUT);
        assert_eq!(apply(&mut fixing, &catalog, Msg::Cancel), Some(Outcome::Cancel));
    }

    #[test]
    fn a_rejected_paste_stays_until_the_duration_changes() {
        let catalog = catalog();
        let mut form = RecordForm::add(DateTime::from_unix(NOON, 180));
        apply(&mut form, &catalog, Msg::Rejected(DurationError::TooLarge));
        assert_eq!(form.rejected, Some(DurationError::TooLarge));
        apply(&mut form, &catalog, Msg::Duration(Duration::from_secs(60)));
        assert_eq!(form.rejected, None);
    }

    #[test]
    fn the_end_is_worked_out_across_midnight() {
        let session = session();
        let mut form = RecordForm::of(Purpose::Add, &session);
        form.start = TimeOfDay::new(23, 30, 0);
        form.duration = Duration::from_secs(3_600);
        assert_eq!(form.ends().date, form.date.add_days(1));
        assert_eq!(form.ends().time, TimeOfDay::new(0, 30, 0));
    }
}
