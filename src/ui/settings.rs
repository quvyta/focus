//! The Settings screen: what the person can change about qfocus, and the two sweeping actions
//! at the bottom.
//!
//! What the framework keeps for every Quvyta application (language, theme, icons, motion, the
//! pillar) is read from the environment the screen draws in, which is the one place that knows
//! what is in force: a saved value the shell overrides would show a choice nobody has. qfocus's
//! own preferences come from [`Prefs`], which the application owns; a change is handed back as a
//! [`Request`] and applied at once, so it can be seen, and written by the application.
//!
//! Nothing here is destructive on its own. Deleting the records, the older records or everything
//! is held to confirm and hands back a request; the application does them as appends, so `ctrl+z`
//! brings everything back. Emptying the trash, the one action that cannot be undone, stands apart
//! under its own mark, is held too, and the application asks once more before doing it.

use std::time::Duration;

use qframe::date::{Date, TimeOfDay, Weekday};
use qframe::diagnostics::{Diagnostic, Severity};
use qframe::prelude::*;
use qframe::widgets::{
    Appearance, AppearanceChange, DatePicker, DurationInput, HoldToConfirm, ScrollView, Select, SettingRow,
    SettingsList, Switch, TimeInput,
};

use super::warning_line;
use crate::duration::{Units, short};
use crate::prefs::{Prefs, WEEKDAYS};

/// The widget id of the settings list, which takes the keyboard when the screen opens.
pub const LIST: &str = "settings-list";

/// The widget id of the control that deletes every record.
pub const RESET: &str = "settings-reset";

/// The widget id of the day the older records are deleted before.
pub const OLDER_DATE: &str = "settings-older-date";

/// The widget id of the control that deletes the records before that day.
pub const OLDER: &str = "settings-older";

/// The widget id of the control that deletes everything.
pub const DELETE: &str = "settings-delete";

/// The widget id of the control that empties the trash for good.
pub const PURGE: &str = "settings-purge";

/// Width of the drop-downs: enough for the longest theme, language and weekday name.
const CONTROL_WIDTH: u16 = 18;

/// The shortest idle threshold: a minute.
const IDLE_LEAST: Duration = Duration::from_secs(60);

/// The lowest ceiling: an hour.
const CEILING_LEAST: Duration = Duration::from_secs(3_600);

/// The shortest suggested goal: a minute.
const GOAL_LEAST: Duration = Duration::from_secs(60);

/// A preference set from a length of time, for a value typed under its floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timed {
    /// The idle threshold.
    IdleAfter,
    /// The session ceiling.
    Ceiling,
    /// The suggested goal.
    DefaultGoal,
}

impl Timed {
    /// The least the preference accepts.
    fn least(self) -> Duration {
        match self {
            Self::IdleAfter => IDLE_LEAST,
            Self::Ceiling => CEILING_LEAST,
            Self::DefaultGoal => GOAL_LEAST,
        }
    }
}

/// Something that happened on the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// An appearance row of the family's own section was changed.
    Appearance(AppearanceChange),
    /// The day the week starts on was chosen, by position among the weekdays.
    WeekStart(usize),
    /// The hour the day turns at was set.
    Rollover(TimeOfDay),
    /// A length was typed for a preference.
    Length(Timed, Duration),
    /// The stop-at-goal switch was moved.
    StopAtGoal(bool),
    /// The hold to delete every record completed.
    ResetStats,
    /// The day the older records are deleted before was chosen.
    OlderDate(Date),
    /// The hold to delete the records before this day completed.
    DeleteOlder(Date),
    /// The hold to delete everything completed.
    DeleteAll,
    /// The hold to empty the trash completed; the application asks before it does anything.
    EmptyTrash,
    /// Emptying the trash was confirmed.
    Purge,
    /// Emptying the trash was cancelled.
    PurgeCancelled,
    /// The repair report was read and can go.
    ReadRepairs,
    /// The report of the old files that were not moved was read and can go.
    ReadLeftBehind,
    /// The application wrote the settings, or could not.
    Stored(Result<(), String>),
}

/// What the screen asks the application for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Apply and save an appearance change; only the application holds the rows' state.
    Appearance(AppearanceChange),
    /// Use and store these preferences.
    Prefs(Prefs),
    /// Move every session to the trash.
    ResetStats,
    /// Move every session whose day is before this one to the trash.
    DeleteOlder(Date),
    /// Move every session to the trash and archive every category and focus.
    DeleteAll,
    /// Say what emptying the trash would take and ask before doing it.
    EmptyTrash,
    /// Empty the trash for good.
    Purge,
    /// Say that the trash was left as it is.
    PurgeCancelled,
}

/// The state of the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// What reading the settings file had to put right, until it is read.
    repairs: Vec<Diagnostic>,
    repairs_read: bool,
    /// Files from an earlier version's settings folder that were not moved, until read.
    left_behind: Vec<Diagnostic>,
    left_behind_read: bool,
    /// Why the last write failed, until one succeeds.
    failure: Option<String>,
    /// A length typed under its floor, kept in the field with the reason until it is fixed.
    short: Option<(Timed, Duration)>,
    /// The day the older records are deleted before, once chosen; until then a year before today.
    older: Option<Date>,
    /// Whether the family's update notice is offered, right after the appearance section: only
    /// where qfocus asks for its updates, so the page offers no switch that would do nothing.
    updates: bool,
}

impl Settings {
    /// The screen over a settings file that needed `repairs`.
    #[must_use]
    pub fn new(repairs: Vec<Diagnostic>) -> Self {
        Self {
            repairs,
            repairs_read: false,
            left_behind: Vec::new(),
            left_behind_read: false,
            failure: None,
            short: None,
            older: None,
            updates: false,
        }
    }

    /// The same screen offering the family's update notice (`true`) right after the appearance
    /// section, or not.
    #[must_use]
    pub fn with_updates(mut self, offered: bool) -> Self {
        self.updates = offered;
        self
    }

    /// The screen that also says which `files` from an earlier version's settings folder were
    /// left where they were.
    #[must_use]
    pub fn with_left_behind(mut self, files: Vec<Diagnostic>) -> Self {
        self.left_behind = files;
        self
    }

    /// The day the older records are deleted before: the one chosen, or a year before `today`.
    #[must_use]
    pub fn older_than(&self, today: Date) -> Date {
        self.older.unwrap_or_else(|| today.add_months(-12))
    }

    /// Why the last write failed, if it did.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
}

/// Applies `msg` over the preferences `prefs` and says what the application should do.
pub fn update<M: From<Msg> + Clone + Send + 'static>(
    screen: &mut Settings,
    prefs: &Prefs,
    msg: Msg,
) -> (Command<M>, Option<Request>) {
    match msg {
        // The rows themselves live in the framework's `Appearance`, which the application holds:
        // it applies the change and saves it, so the screen only passes it on.
        Msg::Appearance(change) => (Command::none(), Some(Request::Appearance(change))),
        Msg::WeekStart(index) => match WEEKDAYS.get(index) {
            // The language's and region's own first day is not pinned, so the week keeps
            // following them when they change.
            Some((day, _)) => {
                let week_start = (*day != qframe::i18n::first_weekday()).then_some(*day);
                (Command::none(), Some(Request::Prefs(Prefs { week_start, ..prefs.clone() })))
            }
            None => (Command::none(), None),
        },
        Msg::Rollover(time) => {
            let rollover = TimeOfDay::new(time.hour, time.minute, 0);
            (Command::none(), Some(Request::Prefs(Prefs { rollover, ..prefs.clone() })))
        }
        Msg::Length(which, length) => {
            if length < which.least() {
                screen.short = Some((which, length));
                return (Command::none(), None);
            }
            screen.short = None;
            let seconds = u32::try_from(length.as_secs()).unwrap_or(u32::MAX);
            let next = match which {
                Timed::IdleAfter => Prefs { idle_after: length, ..prefs.clone() },
                Timed::Ceiling => Prefs { ceiling: seconds, ..prefs.clone() },
                Timed::DefaultGoal => Prefs { default_goal: seconds, ..prefs.clone() },
            };
            (Command::none(), Some(Request::Prefs(next)))
        }
        Msg::StopAtGoal(on) => (Command::none(), Some(Request::Prefs(Prefs { stop_at_goal: on, ..prefs.clone() }))),
        Msg::ResetStats => (Command::none(), Some(Request::ResetStats)),
        Msg::OlderDate(date) => {
            screen.older = Some(date);
            (Command::none(), None)
        }
        Msg::DeleteOlder(date) => (Command::none(), Some(Request::DeleteOlder(date))),
        Msg::DeleteAll => (Command::none(), Some(Request::DeleteAll)),
        Msg::EmptyTrash => (Command::none(), Some(Request::EmptyTrash)),
        Msg::Purge => (Command::none(), Some(Request::Purge)),
        Msg::PurgeCancelled => (Command::none(), Some(Request::PurgeCancelled)),
        Msg::ReadRepairs => {
            screen.repairs_read = true;
            (Command::none(), None)
        }
        Msg::ReadLeftBehind => {
            screen.left_behind_read = true;
            (Command::none(), None)
        }
        Msg::Stored(result) => {
            screen.failure = result.err();
            (Command::none(), None)
        }
    }
}

/// Draws the screen: the repairs until read, the last failure, the list of settings and, under
/// it, the danger actions with their marks. `today` is the local day, where the older records'
/// date starts from. `appearance` draws the rows the whole family shares. `can_edit` false mutes
/// the actions, for an instance that may not write.
pub fn view<M: From<Msg> + Clone + Send + 'static>(
    screen: &Settings,
    prefs: &Prefs,
    appearance: &Appearance,
    today: Date,
    units: &Units<'_>,
    can_edit: bool,
    ui: &mut View<'_, M>,
) {
    let week_start = prefs.week_starts_on(ui.env().i18n().first_weekday());
    let warning = ui.env().icons().glyph("warning").into_owned();

    ui.add_with(ScrollView::new(), |ui| {
        ui.column(|ui| {
            if !screen.left_behind_read {
                report(&Report::LEFT_BEHIND, &screen.left_behind, ui);
            }
            if !screen.repairs_read {
                report(&Report::REPAIRS, &screen.repairs, ui);
            }
            if let Some(reason) = &screen.failure {
                warning_line(t!("settings.store-failed", reason = reason.clone()), ui);
            }
            let list = SettingsList::show(ui, |list| {
                // Language, theme, icons, motion and the pillar are the family's own section:
                // every Quvyta application shows the same rows, and each shared row carries the
                // choice of changing it everywhere or here only.
                appearance.section(list, |change| M::from(Msg::Appearance(change)));
                // The family's own switch and words, the same in every application that asks.
                if screen.updates {
                    appearance.updates(list, |change| M::from(Msg::Appearance(change)));
                }

                list.heading(t!("settings.time"));
                time_rows(list, screen, prefs, week_start, units);

                list.heading(t!("settings.goals"));
                let row = SettingRow::new(t!("settings.stop-at-goal")).description(t!("settings.stop-at-goal-text"));
                list.row(row, |ui| {
                    ui.add(Switch::new(prefs.stop_at_goal).on_toggle(|on| M::from(Msg::StopAtGoal(on))));
                });
                length_row(list, screen, Timed::DefaultGoal, Duration::from_secs(u64::from(prefs.default_goal)), units);
            });
            list.id(LIST);

            // The sweeping actions stand apart from the list, under a mark, so nothing that is
            // walked with the arrow keys ends in one of them by accident; the one that cannot be
            // undone stands apart again, under a mark of its own.
            let older = screen.older_than(today);
            ui.column(|ui| {
                ui.column(|ui| {
                    group_heading(&warning, t!("settings.danger"), t!("settings.danger-text"), ui);
                    held(t!("settings.reset"), Msg::ResetStats, RESET, can_edit, ui);
                    warning_note(&warning, t!("settings.reset-text"), ui);
                    ui.add(
                        DatePicker::new(Some(older))
                            .today(today)
                            .disabled(!can_edit)
                            .on_change(|date| M::from(Msg::OlderDate(date))),
                    )
                    .id(OLDER_DATE);
                    held(t!("settings.older"), Msg::DeleteOlder(older), OLDER, can_edit, ui);
                    warning_note(&warning, t!("settings.older-text"), ui);
                    held(t!("settings.delete"), Msg::DeleteAll, DELETE, can_edit, ui);
                    warning_note(&warning, t!("settings.delete-text"), ui);
                })
                .gap(0)
                .fill_width();
                ui.column(|ui| {
                    group_heading(&warning, t!("settings.purge-group"), t!("settings.purge-group-text"), ui);
                    // Not held: the button only opens the question, and the question itself is
                    // the lock, a word typed on purpose. Holding first as well would set two
                    // locks on one act and teach the hand to pass both without reading.
                    ui.add(
                        Button::new(t!("settings.purge"))
                            .variant("danger")
                            .disabled(!can_edit)
                            .on_press(M::from(Msg::EmptyTrash)),
                    )
                    .id(PURGE);
                })
                .gap(0)
                .fill_width();
            })
            .gap(1)
            .padding(Padding { top: 1, left: 2, ..Padding::default() })
            .fill_width();
        })
        .fill_width()
        .gap(1);
    })
    .fill();
}

/// The title of a group of danger actions, with the danger sign, and the line that says what they
/// have in common.
fn group_heading<M: From<Msg> + Clone + Send + 'static>(
    warning: &str,
    title: String,
    text: String,
    ui: &mut View<'_, M>,
) {
    ui.add(Text::rich([Span::new(format!("{warning} ")).color("danger"), Span::new(title).bold()]).no_wrap());
    ui.add(Text::new(text).role("secondary")).fill_width();
}

/// A held action in the danger tone, muted for an instance that may not write.
fn held<M: From<Msg> + Clone + Send + 'static>(
    label: String,
    message: Msg,
    id: &'static str,
    can_edit: bool,
    ui: &mut View<'_, M>,
) {
    ui.add(HoldToConfirm::new(label).color("$danger").disabled(!can_edit).on_confirm(M::from(message))).id(id);
}

/// The faint line under a held action saying what it takes, with the danger sign.
fn warning_note<M: From<Msg> + Clone + Send + 'static>(warning: &str, text: String, ui: &mut View<'_, M>) {
    ui.add(Text::rich([Span::new(format!("{warning} ")).color("danger"), Span::new(text)]).role("faint")).fill_width();
}

/// The four rows of the times that shape the day: the day the week starts on, the hour the day
/// turns at, the idle threshold and the session ceiling. The Settings page and the first-run
/// wizard ask for the same four with the same controls and the same floors. `week_start` is the
/// day in force, the chosen one or the active language's, and `screen` carries a length typed
/// under its floor so the field keeps it with the reason.
pub fn time_rows<M: From<Msg> + Clone + Send + 'static>(
    list: &mut qframe::widgets::SettingsRows<'_, M>,
    screen: &Settings,
    prefs: &Prefs,
    week_start: Weekday,
    units: &Units<'_>,
) {
    let days = WEEKDAYS.map(|(day, _)| t!(&format!("days.{}", day.number())));
    let chosen = WEEKDAYS.iter().position(|(day, _)| *day == week_start);
    list.row(SettingRow::new(t!("settings.week-start")), |ui| {
        ui.add(Select::new(days).selected(chosen).on_select(|index| M::from(Msg::WeekStart(index))))
            .width(Length::Cells(CONTROL_WIDTH));
    });
    let row = SettingRow::new(t!("settings.day-rollover")).description(t!("settings.day-rollover-text"));
    list.row(row, |ui| {
        ui.add(TimeInput::new(prefs.rollover).on_change(|time| M::from(Msg::Rollover(time))));
    });
    length_row(list, screen, Timed::IdleAfter, prefs.idle_after, units);
    length_row(list, screen, Timed::Ceiling, Duration::from_secs(u64::from(prefs.ceiling)), units);
}

/// A row with a length of time: the value in force, or the too-short one being typed, marked
/// invalid with the floor named under the label.
fn length_row<M: From<Msg> + Clone + Send + 'static>(
    list: &mut qframe::widgets::SettingsRows<'_, M>,
    screen: &Settings,
    which: Timed,
    value: Duration,
    units: &Units<'_>,
) {
    let key = match which {
        Timed::IdleAfter => "idle-after",
        Timed::Ceiling => "ceiling",
        Timed::DefaultGoal => "default-goal",
    };
    let typed = screen.short.filter(|(short, _)| *short == which).map(|(_, length)| length);
    let description = match typed {
        Some(_) => t!("settings.too-short", least = short(which.least().as_secs(), units)),
        None => t!(&format!("settings.{key}-text")),
    };
    let row = SettingRow::new(t!(&format!("settings.{key}"))).description(description);
    list.row(row, |ui| {
        ui.add(
            DurationInput::new(typed.unwrap_or(value))
                .invalid(typed.is_some())
                .on_change(move |length| M::from(Msg::Length(which, length))),
        );
    });
}

/// The words and the button of one report over the settings.
struct Report {
    title: &'static str,
    text: &'static str,
    dismiss: Msg,
    id: &'static str,
}

impl Report {
    /// What reading the settings file had to put right.
    const REPAIRS: Self = Self {
        title: "settings.repaired",
        text: "settings.repaired-text",
        dismiss: Msg::ReadRepairs,
        id: "settings-repairs",
    };
    /// The files from an earlier version's settings folder that stayed where they were.
    const LEFT_BEHIND: Self = Self {
        title: "settings.left-behind",
        text: "settings.left-behind-text",
        dismiss: Msg::ReadLeftBehind,
        id: "settings-left-behind",
    };
}

/// A report over the settings with one line per diagnostic. It stands until it is read: a repair
/// or a file left behind is news, and news that disappears on its own is news nobody got.
fn report<M: From<Msg> + Clone + Send + 'static>(which: &Report, lines: &[Diagnostic], ui: &mut View<'_, M>) {
    if lines.is_empty() {
        return;
    }
    ui.add_with(Panel::new().title(t!(which.title)), |ui| {
        ui.add(Text::new(t!(which.text)).role("secondary")).fill_width();
        for line in lines {
            let text = match line.location.as_ref() {
                Some(place) => format!("{place}  {}", line.message),
                None => line.message.clone(),
            };
            let colour = if line.severity == Severity::Error { "danger" } else { "warning" };
            ui.add(Text::new(text).color(colour)).fill_width();
        }
        ui.add(Button::new(t!("settings.repaired-dismiss")).on_press(M::from(which.dismiss.clone()))).id(which.id);
    })
    .fill_width();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preference_change_hands_back_the_whole_set_with_one_field_changed() {
        let mut screen = Settings::new(Vec::new());
        let prefs = Prefs::default();
        // Outside the runtime the week starts on Monday, so Sunday is pinned.
        let (_, request): (Command<Msg>, _) = update(&mut screen, &prefs, Msg::WeekStart(6));
        assert_eq!(request, Some(Request::Prefs(Prefs { week_start: Some(Weekday::Sunday), ..Prefs::default() })));
        // Where the language and region start the week on Sunday, Sunday is not pinned.
        let mut american = qframe::i18n::I18n::builtin();
        american.set_region(Some("US"));
        assert_eq!(american.first_weekday(), Weekday::Sunday);
        let (_, request): (Command<Msg>, _) =
            qframe::i18n::scope(std::sync::Arc::new(american), || update(&mut screen, &prefs, Msg::WeekStart(6)));
        assert_eq!(request, Some(Request::Prefs(Prefs::default())));
        let (_, request): (Command<Msg>, _) = update(&mut screen, &prefs, Msg::Rollover(TimeOfDay::new(4, 15, 30)));
        assert_eq!(request, Some(Request::Prefs(Prefs { rollover: TimeOfDay::new(4, 15, 0), ..Prefs::default() })));
        let (_, request): (Command<Msg>, _) = update(&mut screen, &prefs, Msg::StopAtGoal(true));
        assert_eq!(request, Some(Request::Prefs(Prefs { stop_at_goal: true, ..Prefs::default() })));
        let (_, request): (Command<Msg>, _) = update(&mut screen, &prefs, Msg::WeekStart(9));
        assert_eq!(request, None);
    }

    #[test]
    fn a_length_under_its_floor_is_kept_in_the_field_and_not_applied() {
        let mut screen = Settings::new(Vec::new());
        let prefs = Prefs::default();
        let (_, request): (Command<Msg>, _) =
            update(&mut screen, &prefs, Msg::Length(Timed::IdleAfter, Duration::from_secs(30)));
        assert_eq!(request, None);
        assert_eq!(screen.short, Some((Timed::IdleAfter, Duration::from_secs(30))));
        let (_, request): (Command<Msg>, _) =
            update(&mut screen, &prefs, Msg::Length(Timed::IdleAfter, Duration::from_secs(120)));
        assert_eq!(request, Some(Request::Prefs(Prefs { idle_after: Duration::from_secs(120), ..Prefs::default() })));
        assert_eq!(screen.short, None);
        let (_, request): (Command<Msg>, _) =
            update(&mut screen, &prefs, Msg::Length(Timed::Ceiling, Duration::from_secs(7_200)));
        assert_eq!(request, Some(Request::Prefs(Prefs { ceiling: 7_200, ..Prefs::default() })));
        let (_, request): (Command<Msg>, _) =
            update(&mut screen, &prefs, Msg::Length(Timed::DefaultGoal, Duration::from_secs(1_800)));
        assert_eq!(request, Some(Request::Prefs(Prefs { default_goal: 1_800, ..Prefs::default() })));
    }

    #[test]
    fn an_appearance_change_is_passed_on_and_a_failed_write_is_shown() {
        let mut screen = Settings::new(Vec::new());
        let prefs = Prefs::default();
        let change = AppearanceChange::Theme("nordic".to_owned());
        let (_, request): (Command<Msg>, _) = update(&mut screen, &prefs, Msg::Appearance(change.clone()));
        assert_eq!(request, Some(Request::Appearance(change)));
        let (_, request): (Command<Msg>, _) = update(&mut screen, &prefs, Msg::Stored(Err("disk full".to_owned())));
        assert_eq!(request, None);
        assert_eq!(screen.failure(), Some("disk full"));
        let _: (Command<Msg>, _) = update(&mut screen, &prefs, Msg::Stored(Ok(())));
        assert_eq!(screen.failure(), None);
    }
}
