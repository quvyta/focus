//! The screens: each has a state, an `update` and a `view`, and speaks a message type of its
//! own that the application's message can be made from. None of them touches the disk; they
//! ask the application for that through what `update` returns.

pub mod charts;
pub mod record_form;
pub mod records;
pub mod recover;
pub mod settings;
pub mod timer;
pub mod today;

use qframe::date::{Date, Weekday};
use qframe::prelude::*;
use qframe::widgets::{Gauge, Span};

use crate::duration::Units;
use crate::prefs::Prefs;
use crate::session::Session;
use crate::stats;
use crate::tree::{Goal, Tree as Catalog};

/// A warning above a screen: a mark and the words, so the meaning never rests on the colour
/// alone. A long one wraps rather than being cut.
pub fn warning_line<M: 'static>(text: impl Into<String>, ui: &mut View<'_, M>) {
    let mark = ui.env().icons().glyph("warning").into_owned();
    ui.add(Text::rich([Span::new(format!("{mark} ")).color("warning"), Span::new(text.into()).role("secondary")]))
        .fill_width();
}

/// A remark above a screen, marked as information; a long one wraps rather than being cut.
pub fn info_line<M: 'static>(text: impl Into<String>, ui: &mut View<'_, M>) {
    let mark = ui.env().icons().glyph("info").into_owned();
    ui.add(Text::rich([Span::new(format!("{mark} ")).color("info"), Span::new(text.into()).role("secondary")]))
        .fill_width();
}

/// How many buttons stand in each row of a strip that must not lose any of them.
///
/// A button is measured while its row is laid out, which is after the rows have to be chosen, so
/// the width is worked out here from what a button is made of: four cells of padding around the
/// label, and three more plus the key itself for the segment that carries the key. Neighbours
/// stand two cells apart. Each `(label, key)` is taken in order and opens a new row as soon as it
/// would not fit; a button without a key passes an empty one.
///
/// A row that is one cell too wide does not shrink: the framework cuts the last label off and
/// leaves its key standing alone, which says nothing.
#[must_use]
pub fn button_rows(width: u16, buttons: &[(&str, &str)]) -> Vec<usize> {
    let mut rows: Vec<usize> = Vec::new();
    let mut used = 0_u32;
    for (label, key) in buttons {
        let key_cells = if key.is_empty() { 0 } else { u32::from(qframe::text::width(key)) + 3 };
        let cells = u32::from(qframe::text::width(label)) + 4 + key_cells;
        match rows.last_mut() {
            Some(count) if used + 2 + cells <= u32::from(width) => {
                *count += 1;
                used += 2 + cells;
            }
            _ => {
                rows.push(1);
                used = cells;
            }
        }
    }
    rows
}

/// `seconds` as "how long ago", in the largest unit that fits: seconds, minutes, hours or days.
#[must_use]
pub fn ago(seconds: u64) -> String {
    if seconds < 60 {
        t!("ago.seconds", n = u32::try_from(seconds).unwrap_or(u32::MAX))
    } else if seconds < 3_600 {
        t!("ago.minutes", n = u32::try_from(seconds / 60).unwrap_or(u32::MAX))
    } else if seconds < 86_400 {
        t!("ago.hours", n = u32::try_from(seconds / 3_600).unwrap_or(u32::MAX))
    } else {
        t!("ago.days", n = u32::try_from(seconds / 86_400).unwrap_or(u32::MAX))
    }
}

/// How far one goal of the catalogue has come, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalRow {
    /// The category or focus the goal belongs to.
    pub id: crate::id::Id,
    /// Its name.
    pub name: String,
    /// Seconds of work in the goal's window.
    pub done: u64,
    /// Seconds the goal asks for.
    pub amount: u64,
}

impl GoalRow {
    /// Whether the goal is reached.
    #[must_use]
    pub fn reached(&self) -> bool {
        self.done >= self.amount
    }

    /// The text beside the meter: what is done over what is asked, in hours when the goal is
    /// at least one, else in minutes, with the success mark once it is reached.
    #[must_use]
    pub fn value_text(&self, units: &Units<'_>, mark: &str) -> String {
        let (done, amount, unit) = if self.amount >= 3_600 {
            (hours_text(self.done), hours_text(self.amount), units.hour)
        } else {
            ((self.done / 60).to_string(), (self.amount / 60).to_string(), units.minute)
        };
        let value = t!("goal.value", done = done, amount = amount, unit = unit);
        if self.reached() { format!("{mark} {value}") } else { value }
    }
}

/// `seconds` as hours to the nearest tenth, whole when they are, else with one decimal written
/// the way the active language writes it: `1.5` in English, `1,5` in Turkish.
fn hours_text(seconds: u64) -> String {
    let tenths = (seconds * 10 + 1_800) / 3_600;
    if tenths.is_multiple_of(10) {
        (tenths / 10).to_string()
    } else {
        format!("{}{}{}", tenths / 10, qframe::i18n::decimal_separator(), tenths % 10)
    }
}

/// Every goal of the catalogue's shown rows, categories first in their order and each one's
/// focuses after it, measured over `sessions` around `today` with weeks starting on
/// `week_start` and days turning at the rollover `prefs` gives. `extra` is work not recorded
/// yet, the running counter's, counted for the goals of its focus.
#[must_use]
pub fn goal_rows(
    catalog: &Catalog,
    sessions: &[Session],
    today: Date,
    prefs: &Prefs,
    week_start: Weekday,
    extra: Option<(crate::id::Id, u64)>,
) -> Vec<GoalRow> {
    let measure = |goal: Goal, id: crate::id::Id, focuses: &[crate::id::Id], name: &str| {
        let (mut done, amount) = stats::goal_progress(goal, focuses, sessions, today, week_start, prefs.rollover);
        if let Some((focus, seconds)) = extra
            && focuses.contains(&focus)
        {
            done = done.saturating_add(seconds);
        }
        GoalRow { id, name: name.to_owned(), done, amount }
    };
    let mut categories: Vec<&crate::tree::Category> =
        catalog.categories.iter().filter(|category| !category.archived).collect();
    categories.sort_by(|left, right| left.order.total_cmp(&right.order));
    let mut rows = Vec::new();
    for category in categories {
        if let Some(goal) = category.goal {
            let focuses: Vec<crate::id::Id> = category.focuses.iter().map(|focus| focus.id).collect();
            rows.push(measure(goal, category.id, &focuses, &category.name));
        }
        let mut focuses: Vec<&crate::tree::Focus> = category.focuses.iter().filter(|focus| !focus.archived).collect();
        focuses.sort_by(|left, right| left.order.total_cmp(&right.order));
        for focus in focuses {
            if let Some(goal) = focus.goal {
                rows.push(measure(goal, focus.id, &[focus.id], &focus.name));
            }
        }
    }
    rows
}

/// Draws one [`Gauge`] per goal row, labels lined up, nothing at all for no rows: a person who
/// set no goal sees no empty meter.
pub fn goal_gauges<M: 'static>(rows: &[GoalRow], units: &Units<'_>, ui: &mut View<'_, M>) {
    if rows.is_empty() {
        return;
    }
    let mark = ui.env().icons().glyph("success").into_owned();
    let widest = rows.iter().map(|row| qframe::text::width(&row.name)).max().unwrap_or(0);
    // A share of the width for the names, so long ones are cut rather than eating the meter.
    let label_width = widest.min(ui.size().width / 3).max(4);
    ui.column(|ui| {
        for row in rows {
            let value = if row.amount == 0 { 0.0 } else { (row.done.min(row.amount) as f32) / (row.amount as f32) };
            let gauge = Gauge::new(value * 100.0)
                .label(&row.name)
                .label_width(label_width)
                .value_text(row.value_text(units, &mark));
            ui.add(gauge).fill_width();
        }
    })
    .fill_width();
}

#[cfg(test)]
mod tests {
    use super::{button_rows, hours_text};

    #[test]
    fn hours_carry_the_decimal_mark_of_the_language() {
        assert_eq!(hours_text(5_400), "1.5");
        assert_eq!(hours_text(7_200), "2");
        let mut i18n = qframe::i18n::I18n::builtin();
        for &(file, text) in crate::locales() {
            i18n.add_source(file, text);
        }
        i18n.set_active("tr");
        assert_eq!(qframe::i18n::scope(std::sync::Arc::new(i18n), || hours_text(5_400)), "1,5");
    }

    #[test]
    fn a_strip_of_buttons_keeps_one_row_while_the_words_leave_room() {
        // The counter's three buttons in English, then in French, at forty columns: "Reprendre"
        // is what tips them over, and one word longer must cost a row, not the last label.
        assert_eq!(button_rows(40, &[("Stop", ""), ("Break", "p"), ("Note", "n")]), vec![3]);
        assert_eq!(button_rows(40, &[("Arrêter", ""), ("Pause", "p"), ("Note", "n")]), vec![3]);
        assert_eq!(button_rows(40, &[("Arrêter", ""), ("Reprendre", "p"), ("Note", "n")]), vec![2, 1]);
    }

    #[test]
    fn a_strip_too_wide_for_the_terminal_flows_on() {
        let records = [("Add by hand", "a"), ("Correct", "f2"), ("Spans", "s"), ("Remove", "delete"), ("Export", "e")];
        assert_eq!(button_rows(40, &records), vec![2, 2, 1]);
        assert_eq!(button_rows(64, &records), vec![3, 2]);
        assert_eq!(button_rows(89, &records), vec![5], "the whole strip needs eighty-nine cells");
        assert_eq!(button_rows(88, &records), vec![4, 1]);
    }

    #[test]
    fn a_wide_glyph_counts_for_two_cells() {
        // Japanese labels are half as many characters and just as many cells.
        assert_eq!(button_rows(22, &[("停止", ""), ("休憩", "p")]), vec![2]);
        assert_eq!(button_rows(21, &[("停止", ""), ("休憩", "p")]), vec![1, 1]);
    }
}
