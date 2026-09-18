//! What a person can set about qfocus itself, and what applies until they do.
//!
//! The values here drive the calculations, not the records: the day a session is grouped under,
//! the week a goal runs over, when silence counts as being away, when a session is flagged as
//! forgotten, and what the goal field suggests. Changing one regroups what is shown and touches
//! no file of records.
//!
//! They live in the settings file the framework keeps, under keys of qfocus's own beside the
//! framework's (theme, language, icons, motion, pillar). A [`Schema`] says what each key may
//! hold, so a file edited by hand is checked and, with self-healing, repaired with a backup;
//! a value that is the default is never written, so the file holds only what was chosen.

use std::time::Duration;

use qframe::date::{TimeOfDay, Weekday};
use qframe::storage::{Schema, SettingKind, Settings};

/// The key of the day a week starts on: a weekday name in English. Without it the week starts
/// where the language's calendar starts it.
pub const WEEK_START: &str = "week-start";
/// The key of the hour the day turns at: `hh:mm`, midnight by default.
pub const DAY_ROLLOVER: &str = "day-rollover";
/// The key of the idle threshold in seconds; fifteen minutes by default.
pub const IDLE_AFTER: &str = "idle-after";
/// The key of the session ceiling in seconds; twelve hours by default.
pub const CEILING: &str = "ceiling";
/// The key of the stop-at-goal flag; off by default.
pub const STOP_AT_GOAL: &str = "stop-at-goal";
/// The key of the goal the goal field suggests, in seconds; one hour by default.
pub const DEFAULT_GOAL: &str = "default-goal";

/// The weekdays in the order a chooser lists them, with the names written in the file.
pub const WEEKDAYS: [(Weekday, &str); 7] = [
    (Weekday::Monday, "monday"),
    (Weekday::Tuesday, "tuesday"),
    (Weekday::Wednesday, "wednesday"),
    (Weekday::Thursday, "thursday"),
    (Weekday::Friday, "friday"),
    (Weekday::Saturday, "saturday"),
    (Weekday::Sunday, "sunday"),
];

/// The shortest idle threshold, in seconds: one minute.
const IDLE_LEAST: i64 = 60;
/// The longest idle threshold, in seconds: a day.
const IDLE_MOST: i64 = 86_400;
/// The lowest ceiling, in seconds: an hour.
const CEILING_LEAST: i64 = 3_600;
/// The highest ceiling, in seconds: a week.
const CEILING_MOST: i64 = 7 * 86_400;
/// The shortest suggested goal, in seconds: a minute.
const GOAL_LEAST: i64 = 60;
/// The longest suggested goal, in seconds: what a goal can hold at all.
const GOAL_MOST: i64 = u32::MAX as i64;

/// The preferences of the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefs {
    /// The day a week starts on, for the week windows of goals and charts, when one was chosen;
    /// `None` follows the active language, as the framework's calendar does.
    pub week_start: Option<Weekday>,
    /// The hour the day turns at; sessions before it belong to the day before.
    pub rollover: TimeOfDay,
    /// How long the terminal may go without input before the work is idle.
    pub idle_after: Duration,
    /// Seconds of work past which a session is flagged as forgotten.
    pub ceiling: u32,
    /// Whether the counter stops by itself when a goal or a countdown is reached.
    pub stop_at_goal: bool,
    /// Seconds the goal field suggests for a row that has no goal yet.
    pub default_goal: u32,
}

impl Default for Prefs {
    /// The language's week, midnight, fifteen minutes, twelve hours, no stopping, one hour.
    fn default() -> Self {
        Self {
            week_start: None,
            rollover: TimeOfDay::default(),
            idle_after: Duration::from_secs(15 * 60),
            ceiling: 12 * 3_600,
            stop_at_goal: false,
            default_goal: 3_600,
        }
    }
}

impl Prefs {
    /// The day the week starts on: the chosen one, or the active language's.
    #[must_use]
    pub fn week_starts_on(&self) -> Weekday {
        self.week_start.unwrap_or_else(language_week_start)
    }

    /// What the settings file may hold: the framework's keys and qfocus's own, each with its
    /// default and its bounds.
    #[must_use]
    pub fn schema() -> Schema {
        let names = WEEKDAYS.map(|(_, name)| name);
        Schema::builtin()
            // No default of its own: a value that cannot be read is taken out, and the week
            // follows the language again.
            .optional(WEEK_START, SettingKind::choice(names))
            .check(DAY_ROLLOVER, "00:00".to_owned(), |text: &String| TimeOfDay::parse(text).is_some())
            .check(IDLE_AFTER, 15 * 60_i64, |seconds| (IDLE_LEAST..=IDLE_MOST).contains(seconds))
            .check(CEILING, 12 * 3_600_i64, |seconds| (CEILING_LEAST..=CEILING_MOST).contains(seconds))
            .flag(STOP_AT_GOAL, false)
            .check(DEFAULT_GOAL, 3_600_i64, |seconds| (GOAL_LEAST..=GOAL_MOST).contains(seconds))
    }

    /// The preferences `settings` hold; a key that is missing, or holds what the schema would
    /// not accept, gives its default.
    #[must_use]
    pub fn from_settings(settings: &Settings) -> Self {
        let defaults = Self::default();
        let seconds = |key: &str, least: i64, most: i64, default: u64| {
            settings
                .get::<i64>(key)
                .filter(|seconds| (least..=most).contains(seconds))
                .and_then(|seconds| u64::try_from(seconds).ok())
                .unwrap_or(default)
        };
        Self {
            week_start: settings
                .get::<String>(WEEK_START)
                .and_then(|name| WEEKDAYS.iter().find(|(_, known)| *known == name).map(|(day, _)| *day))
                .or(defaults.week_start),
            rollover: settings
                .get::<String>(DAY_ROLLOVER)
                .and_then(|text| TimeOfDay::parse(&text))
                .unwrap_or(defaults.rollover),
            idle_after: Duration::from_secs(seconds(IDLE_AFTER, IDLE_LEAST, IDLE_MOST, defaults.idle_after.as_secs())),
            ceiling: u32::try_from(seconds(CEILING, CEILING_LEAST, CEILING_MOST, u64::from(defaults.ceiling)))
                .unwrap_or(defaults.ceiling),
            stop_at_goal: settings.get_or(STOP_AT_GOAL, defaults.stop_at_goal),
            default_goal: u32::try_from(seconds(DEFAULT_GOAL, GOAL_LEAST, GOAL_MOST, u64::from(defaults.default_goal)))
                .unwrap_or(defaults.default_goal),
        }
    }

    /// Writes these preferences into `settings`: a value that is the default is taken out of the
    /// file rather than written, so the file holds only what was chosen.
    pub fn write(&self, settings: &mut Settings) {
        let defaults = Self::default();
        let name = WEEKDAYS.iter().find(|(day, _)| Some(*day) == self.week_start).map_or("monday", |(_, name)| *name);
        store(settings, WEEK_START, name.to_owned(), self.week_start == defaults.week_start);
        let rollover = format!("{:02}:{:02}", self.rollover.hour, self.rollover.minute);
        store(settings, DAY_ROLLOVER, rollover, self.rollover == defaults.rollover);
        let idle = i64::try_from(self.idle_after.as_secs()).unwrap_or(i64::MAX);
        store(settings, IDLE_AFTER, idle, self.idle_after == defaults.idle_after);
        store(settings, CEILING, i64::from(self.ceiling), self.ceiling == defaults.ceiling);
        store(settings, STOP_AT_GOAL, self.stop_at_goal, self.stop_at_goal == defaults.stop_at_goal);
        store(settings, DEFAULT_GOAL, i64::from(self.default_goal), self.default_goal == defaults.default_goal);
    }
}

/// The first day of the week in the active language, as the framework's calendar reads it;
/// Monday where no language is in force, as in the ISO week.
#[must_use]
pub fn language_week_start() -> Weekday {
    qframe::t!("quvyta.date.first-weekday")
        .trim()
        .parse::<u8>()
        .ok()
        .and_then(Weekday::from_number)
        .unwrap_or(Weekday::Monday)
}

/// Stores `value` under `key`, or removes the key when the value `is_default`.
fn store<T: qframe::storage::Setting>(settings: &mut Settings, key: &str, value: T, is_default: bool) {
    if is_default {
        settings.remove(key);
    } else {
        settings.set(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_documented_ones() {
        let prefs = Prefs::default();
        assert_eq!(prefs.week_start, None, "the week follows the language");
        assert_eq!(prefs.week_starts_on(), Weekday::Monday, "which is the ISO week where there is none");
        assert_eq!(prefs.rollover, TimeOfDay::new(0, 0, 0));
        assert_eq!(prefs.idle_after, Duration::from_secs(900));
        assert_eq!(prefs.ceiling, 43_200);
        assert!(!prefs.stop_at_goal);
        assert_eq!(prefs.default_goal, 3_600);
        assert_eq!(Prefs::from_settings(&Settings::in_memory()), prefs);
    }

    #[test]
    fn chosen_values_round_trip_through_the_file_and_defaults_are_not_written() {
        let prefs = Prefs {
            week_start: Some(Weekday::Sunday),
            rollover: TimeOfDay::new(4, 30, 0),
            idle_after: Duration::from_secs(600),
            ceiling: 8 * 3_600,
            stop_at_goal: true,
            default_goal: 1_800,
        };
        let mut settings = Settings::in_memory();
        prefs.write(&mut settings);
        let text = settings.to_toml();
        assert!(text.contains("week-start = \"sunday\""), "{text}");
        assert!(text.contains("day-rollover = \"04:30\""), "{text}");
        let back = Settings::parse_str("settings.toml", &text).schema(Prefs::schema()).self_heal(true);
        assert!(back.diagnostics().is_empty(), "{:?}", back.diagnostics());
        assert_eq!(Prefs::from_settings(&back), prefs);
        Prefs::default().write(&mut settings);
        assert_eq!(settings.to_toml(), "", "defaults leave the file empty");
    }

    #[test]
    fn the_schema_repairs_what_it_cannot_accept_and_reading_falls_back_to_the_defaults() {
        let text =
            "week-start = \"someday\"\nday-rollover = \"25:00\"\nidle-after = 5\nceiling = true\ncolor = \"red\"\n";
        let healed = Settings::parse_str("settings.toml", text).schema(Prefs::schema()).self_heal(true);
        assert_eq!(healed.diagnostics().len(), 5, "{:?}", healed.diagnostics());
        assert_eq!(Prefs::from_settings(&healed), Prefs::default());
        assert!(healed.value("color").is_none());
        // Without healing the values are ignored just the same.
        let checked = Settings::parse_str("settings.toml", text).schema(Prefs::schema());
        assert_eq!(Prefs::from_settings(&checked), Prefs::default());
    }
}
