//! The Charts screen: the records over a day, a week, a month and a year, with a line that says
//! how the stretch compares with the one before.
//!
//! Nothing here touches the disk and nothing is drawn with lines: a bar is a block of tone, a
//! day of the year a cell of tone. Colour never carries the meaning alone. A stacked bar has a
//! legend beside it, the picked bar or day is named in text under the chart, and the counter
//! still running is not in any chart until it stops, which one line says while it runs.

use qframe::date::{Date, TimeOfDay, Weekday, days_in_month};
use qframe::prelude::*;
use qframe::widgets::{Axis, Bar, BarChart, EmptyState, Heatmap, Legend, Segmented, Series, Sparkline};

use super::{GoalRow, goal_gauges, info_line, warning_line};
use crate::duration::{Units, short};
use crate::id::Id;
use crate::prefs::Prefs;
use crate::session::Session;
use crate::stats::{self, CategoryDays, Range};
use crate::store::Store;
use crate::tree::Tree as Catalog;

/// The widget id of the scale chooser, for focusing it.
pub const SCALES: &str = "charts-scales";

/// The widget id of the chart a scale is read from, for focusing it.
pub const CHART: &str = "charts-chart";

/// The widget id of the column the charts of a scale stand in. A widget's identity comes from
/// its name and its parent's, so the chart of every scale is put under this one column and the
/// keyboard stays on it when the scale changes.
const BODY: &str = "charts-body";

/// The widget id of the column a scale's chart stands in with its axis, gap-less so the axis
/// touches the chart; named for the same reason as [`BODY`], on every scale alike.
const STAND: &str = "charts-stand";

/// Below this many columns the bars of a week lie down so their labels keep room.
const NARROW_BELOW: u16 = 56;

/// From this many columns on the twenty-four hours of a day stand side by side, each with two
/// cells for its label; below it only the hours that hold work are drawn, lying down.
const HOURS_STAND_FROM: u16 = 72;

/// Weeks the year grid holds.
const YEAR_WEEKS: i64 = 52;

/// Rows the bars of a week stand in: a value row, the bars, a label row.
const BAR_ROWS: u16 = 9;

/// Rows the bars of a day stand in: shorter, so the day's categories fit under them on a short
/// terminal.
const HOUR_ROWS: u16 = 7;

/// How many focuses the month names.
const TOP_FOCUSES: usize = 5;

/// How wide a stretch the charts cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scale {
    /// Today, hour by hour.
    #[default]
    Day,
    /// This week, day by day, stacked by category.
    Week,
    /// This month: the trend of its days and the focuses worked on most.
    Month,
    /// The last year as a grid of days.
    Year,
}

impl Scale {
    /// The scales in the order the chooser shows them.
    pub const ALL: [Self; 4] = [Self::Day, Self::Week, Self::Month, Self::Year];

    /// The scale at `index` of the chooser; out of range is the first.
    #[must_use]
    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }

    /// Where this scale sits in the chooser.
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|scale| *scale == self).unwrap_or(0)
    }

    /// The word for the scale in the locale keys.
    fn key(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Year => "year",
        }
    }

    /// The days this scale covers, ending today, with weeks starting on `week_start`.
    ///
    /// A week runs from its first day to its last, so the days still to come stand empty beside
    /// the ones lived; a month runs from its first day to today; a year is the last 52 weeks
    /// with today in the last one.
    fn range(self, today: Date, week_start: Weekday) -> Range {
        match self {
            Self::Day => Range::day(today),
            Self::Week => Range::new(today.start_of_week(week_start), 7),
            Self::Month => Range::new(today.first_of_month(), usize::from(today.day())),
            Self::Year => {
                let from = today.start_of_week(week_start).add_days(-(YEAR_WEEKS - 1) * 7);
                let days = usize::try_from(today.to_days() - from.to_days() + 1).unwrap_or(1);
                Range::new(from, days)
            }
        }
    }

    /// The stretch this one is compared with: the whole day, week, month or year before it.
    fn previous(self, today: Date, week_start: Weekday) -> Range {
        match self {
            Self::Month => {
                let first = today.first_of_month().add_months(-1);
                Range::new(first, usize::from(days_in_month(first.year(), first.month())))
            }
            Self::Day | Self::Week | Self::Year => self.range(today, week_start).previous(),
        }
    }
}

/// Something that happened on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    /// A scale was chosen.
    Scale(usize),
    /// A bar or a day was picked: an hour of the day, a day of the week, a day of the year.
    Select(usize),
    /// A day of the month is being read off the trend, or reading stopped.
    Read(Option<usize>),
}

/// The state of the screen.
#[derive(Debug, Clone, Default)]
pub struct Charts {
    scale: Scale,
    /// What is picked on the current scale: the hour, the day of the week, the day of the
    /// month, or the day of the year, counted from the start of the stretch.
    selected: Option<usize>,
}

impl Charts {
    /// The screen on the day, nothing picked.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The scale shown.
    #[must_use]
    pub fn scale(&self) -> Scale {
        self.scale
    }

    /// Whether the bars of the current scale lie down at `width` columns, so ↑ and ↓ pick among
    /// them rather than ← and →: the day below 72 columns, the week below 56.
    #[must_use]
    pub fn bars_lie_down(&self, width: u16) -> bool {
        match self.scale {
            Scale::Day => width < HOURS_STAND_FROM,
            Scale::Week => width < NARROW_BELOW,
            Scale::Month | Scale::Year => false,
        }
    }

    /// What is picked on the current scale, counted from the start of the stretch.
    #[must_use]
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }
}

/// Applies `msg` to the screen.
pub fn update<M: From<Msg> + Clone + Send + 'static>(screen: &mut Charts, msg: Msg) -> Command<M> {
    match msg {
        Msg::Scale(index) => {
            let scale = Scale::from_index(index);
            if scale != screen.scale {
                screen.scale = scale;
                screen.selected = None;
            }
            // The keys go to the chart of the new scale, whether the chooser was clicked or keyed.
            Command::focus(CHART)
        }
        Msg::Select(index) => {
            screen.selected = Some(index);
            Command::none()
        }
        Msg::Read(index) => {
            screen.selected = index;
            Command::none()
        }
    }
}

/// Draws the screen: the scale chooser, the standing remarks, the comparison line and the
/// charts of the scale. `sessions` are the current ones, on disk and waiting in memory; `today`
/// is the local day, `prefs` give the hour a day turns at and the day a week starts on; `goals`
/// are the goals measured, which the week shows under its bars; `running` says a counter is on,
/// whose work is in no chart yet.
#[allow(clippy::too_many_arguments)]
pub fn view<M: From<Msg> + Clone + Send + 'static>(
    screen: &Charts,
    sessions: &[Session],
    store: &Store,
    today: Date,
    prefs: &Prefs,
    goals: &[GoalRow],
    running: bool,
    units: &Units<'_>,
    ui: &mut View<'_, M>,
) {
    let rollover = prefs.rollover;
    let week_start = prefs.week_starts_on(ui.env().i18n().first_weekday());
    ui.column(|ui| {
        let labels = Scale::ALL.map(|scale| t!(&format!("charts.scale-{}", scale.key())));
        ui.add(Segmented::new(labels).selected(screen.scale.index()).on_select(|index| M::from(Msg::Scale(index))))
            .id(SCALES);
        let problems = u32::try_from(store.diagnostics.len()).unwrap_or(u32::MAX);
        if problems > 0 {
            warning_line(t!("charts.problems", n = problems), ui);
        }
        if running {
            info_line(t!("charts.running"), ui);
        }
        let scale = screen.scale;
        let range = scale.range(today, week_start);
        let this = stats::total(sessions, range, rollover);
        if this == 0 {
            let title = t!(&format!("charts.empty-{}", scale.key()));
            ui.add(EmptyState::new(title).message(t!("charts.empty-message"))).fill();
            return;
        }
        let previous = stats::total(sessions, scale.previous(today, week_start), rollover);
        ui.add(Text::new(comparison(scale, this, previous, units)).role("secondary")).fill_width();
        let width = ui.size().width;
        // The day packs its rows itself, so its titles sit right on their charts.
        let gap = if scale == Scale::Day { 0 } else { 1 };
        ui.column(|ui| match scale {
            Scale::Day => day(screen, sessions, &store.tree, range, rollover, width < HOURS_STAND_FROM, units, ui),
            Scale::Week => {
                week(screen, sessions, &store.tree, range, rollover, width < NARROW_BELOW, units, ui);
                goal_gauges(goals, units, ui);
            }
            Scale::Month => month(screen, sessions, &store.tree, range, rollover, units, ui),
            Scale::Year => year(screen, sessions, range, rollover, units, ui),
        })
        .id(BODY)
        .gap(gap)
        .fill();
    })
    .gap(1)
    .fill();
}

/// The line that sets the stretch against the one before: the difference with its sign, or the
/// word for no difference.
fn comparison(scale: Scale, this: u64, previous: u64, units: &Units<'_>) -> String {
    let change = stats::compare(this, previous);
    if change == 0 {
        return t!(&format!("charts.same-{}", scale.key()));
    }
    let sign = if change > 0 { "+" } else { "-" };
    let amount = short(change.unsigned_abs(), units);
    t!(&format!("charts.compare-{}", scale.key()), change = format!("{sign}{amount}"))
}

/// The name of the category `id`, or the words for sessions whose focus is in no category.
fn category_name(catalog: &Catalog, id: Option<Id>) -> String {
    id.and_then(|id| catalog.categories.iter().find(|category| category.id == id))
        .map_or_else(|| t!("charts.unknown-category"), |category| category.name.clone())
}

/// The name of the focus `id`, or the words for one that is gone.
fn focus_name(catalog: &Catalog, id: Id) -> String {
    catalog.focus(id).map_or_else(|| t!("records.unknown-focus"), |focus| focus.name.clone())
}

/// `seconds` as hours with one decimal, the number a chart writes beside a stack.
fn hours(seconds: u64) -> f32 {
    // Rounded to a tenth here so the chart's own rounding never shows a spurious decimal.
    let tenths = (seconds * 10).div_ceil(3_600).min(u64::from(u32::MAX));
    (tenths as f32) / 10.0
}

/// The categories as chart series, one value per day, each in the tone its category keeps on
/// every screen ([`stats::tone_of`]).
fn category_series(shares: &[CategoryDays], catalog: &Catalog) -> Vec<Series> {
    shares
        .iter()
        .map(|share| {
            Series::new(category_name(catalog, share.category), share.days.iter().map(|day| hours(*day)))
                .tone(stats::tone_of(catalog, share.category))
        })
        .collect()
}

/// The legend naming the categories of `shares`, each beside the tone its series was drawn in.
fn category_legend(shares: &[CategoryDays], catalog: &Catalog) -> Legend {
    let names = shares.iter().map(|share| category_name(catalog, share.category));
    let tones = shares.iter().map(|share| stats::tone_of(catalog, share.category));
    Legend::new(names).tones(tones)
}

/// The categories' shares of `day` as text, for a stack whose meaning must not rest on tone.
fn shares_text(shares: &[CategoryDays], day: usize, catalog: &Catalog, units: &Units<'_>) -> String {
    let parts: Vec<String> = shares
        .iter()
        .filter_map(|share| share.days.get(day).copied().filter(|seconds| *seconds > 0).map(|seconds| (share, seconds)))
        .map(|(share, seconds)| {
            t!("charts.share", name = category_name(catalog, share.category), duration = short(seconds, units))
        })
        .collect();
    parts.join(" · ")
}

/// A date as its day and month in words.
fn day_and_month(date: Date) -> String {
    t!("charts.day-of-month", day = u32::from(date.day()), month = t!(&format!("months.{}", date.month())))
}

/// The line under a chart that reads the picked bar, or invites picking one.
fn readout<M: 'static>(picked: Option<String>, hint: String, ui: &mut View<'_, M>) {
    let role = if picked.is_some() { "body" } else { "faint" };
    let line = super::in_glyphs(picked.unwrap_or(hint), ui.env().icons().mode());
    ui.add(Text::new(line).role(role)).fill_width();
}

/// The day: the hours as bars, then the day's categories as one stacked bar with its legend and
/// their shares in words. With `lying` the hours that hold work lie down with their labels
/// beside them, for a width the twenty-four standing bars would not fit.
#[allow(clippy::too_many_arguments)]
fn day<M: From<Msg> + Clone + Send + 'static>(
    screen: &Charts,
    sessions: &[Session],
    catalog: &Catalog,
    range: Range,
    rollover: TimeOfDay,
    lying: bool,
    units: &Units<'_>,
    ui: &mut View<'_, M>,
) {
    let by_hour = stats::hourly(sessions, range.from, rollover);
    let shares = stats::by_category(sessions, range, catalog, rollover);
    let picked = screen.selected.filter(|hour| *hour < 24).map(|hour| {
        t!(
            "charts.picked-hour",
            from = format!("{hour:02}"),
            to = format!("{:02}", (hour + 1) % 24),
            duration = short(by_hour[hour], units)
        )
    });
    ui.add(Text::new(t!("charts.hours-title")).role("secondary").no_wrap());
    ui.column(|ui| {
        if lying {
            // Lying down, only the hours that hold work fit the width and keep their labels.
            let active: Vec<usize> = (0..24).filter(|hour| by_hour[*hour] > 0).collect();
            let bars = active.iter().map(|hour| {
                let minutes = (by_hour[*hour] / 60) as f32;
                Bar::new(format!("{hour:02}:00"), minutes).value_text(short(by_hour[*hour], units))
            });
            let selected = screen.selected.and_then(|hour| active.iter().position(|active| *active == hour));
            ui.add(
                BarChart::new(bars)
                    .selected(selected)
                    .on_select(move |row| M::from(Msg::Select(active.get(row).copied().unwrap_or(0)))),
            )
            .id(CHART)
            .fill_width();
        } else {
            // Standing, every hour has two cells: room for its label and not for a value, so
            // the values stay out of the chart and read from the line under it.
            let bars = (0..24).map(|hour| Bar::new(format!("{hour:02}"), (by_hour[hour] / 60) as f32).value_text(""));
            ui.add(
                BarChart::new(bars)
                    .vertical()
                    .selected(screen.selected.filter(|hour| *hour < 24))
                    .on_select(|hour| M::from(Msg::Select(hour))),
            )
            .id(CHART)
            .height(Length::Cells(HOUR_ROWS))
            .fill_width();
        }
    })
    .id(STAND)
    .fill_width();
    readout(picked, t!("charts.pick-hint"), ui);
    ui.spacer().height(Length::Cells(1));
    ui.add(Text::new(t!("charts.shares-title")).role("secondary").no_wrap());
    let stack = BarChart::series([String::new()], category_series(&shares, catalog)).stacked().unit(units.hour);
    ui.add(stack).fill_width();
    ui.add(category_legend(&shares, catalog)).fill_width();
    ui.add(Text::new(shares_text(&shares, 0, catalog, units)).role("secondary")).fill_width();
}

/// The week: seven days side by side, each a stack of its categories, with the legend.
#[allow(clippy::too_many_arguments)]
fn week<M: From<Msg> + Clone + Send + 'static>(
    screen: &Charts,
    sessions: &[Session],
    catalog: &Catalog,
    range: Range,
    rollover: TimeOfDay,
    narrow: bool,
    units: &Units<'_>,
    ui: &mut View<'_, M>,
) {
    let shares = stats::by_category(sessions, range, catalog, rollover);
    let totals = stats::daily(sessions, range, rollover);
    let labels: Vec<String> = (0..7)
        .map(|day| {
            let date = range.from.add_days(day);
            let key = if narrow { "days" } else { "days-short" };
            t!(&format!("{key}.{}", date.weekday().number()))
        })
        .collect();
    let mut chart = BarChart::series(labels, category_series(&shares, catalog))
        .stacked()
        .unit(units.hour)
        .selected(screen.selected.filter(|day| *day < 7))
        .on_select(|day| M::from(Msg::Select(day)));
    if !narrow {
        chart = chart.vertical();
    }
    ui.column(|ui| {
        let node = ui.add(chart).id(CHART).fill_width();
        if !narrow {
            node.height(Length::Cells(BAR_ROWS));
        }
    })
    .id(STAND)
    .fill_width();
    ui.add(category_legend(&shares, catalog)).fill_width();
    let picked = screen.selected.filter(|day| *day < 7).map(|day| {
        let date = range.from.add_days(i64::try_from(day).unwrap_or(0));
        let name = t!(&format!("days.{}", date.weekday().number()));
        let total = totals.get(day).copied().unwrap_or(0);
        if total == 0 {
            t!("charts.picked-day", day = name, duration = short(0, units))
        } else {
            t!(
                "charts.picked-day-shares",
                day = name,
                duration = short(total, units),
                shares = shares_text(&shares, day, catalog, units)
            )
        }
    });
    readout(picked, t!("charts.pick-hint"), ui);
}

/// The month: the trend of its days, the focuses worked on most, the sessions and their mean.
fn month<M: From<Msg> + Clone + Send + 'static>(
    screen: &Charts,
    sessions: &[Session],
    catalog: &Catalog,
    range: Range,
    rollover: TimeOfDay,
    units: &Units<'_>,
    ui: &mut View<'_, M>,
) {
    let totals = stats::daily(sessions, range, rollover);
    ui.add(Text::new(t!("charts.trend-title")).role("secondary").no_wrap());
    let reading = screen.selected.filter(|day| *day < totals.len());
    ui.column(|ui| {
        ui.add(
            Sparkline::new(totals.iter().map(|seconds| (*seconds / 60) as f32))
                .reading(reading)
                .on_read(|day| M::from(Msg::Read(day))),
        )
        .id(CHART)
        .height(Length::Cells(3))
        .fill_width();
        // The trend draws one column per day from the left, so the days stand right under
        // their columns.
        let days = u16::try_from(totals.len()).unwrap_or(u16::MAX);
        ui.add(Axis::labels((1..=totals.len()).map(|day| day.to_string()))).width(Length::Cells(days));
    })
    .id(STAND)
    .fill_width();
    let picked = reading.map(|day| {
        let date = range.from.add_days(i64::try_from(day).unwrap_or(0));
        t!("charts.picked-day", day = day_and_month(date), duration = short(totals[day], units))
    });
    readout(picked, t!("charts.pick-hint"), ui);
    let top = stats::top_focuses(sessions, range, TOP_FOCUSES, rollover);
    ui.add(Text::new(t!("charts.top-title")).role("secondary").no_wrap());
    let bars = top.iter().map(|(id, seconds)| {
        Bar::new(focus_name(catalog, *id), (*seconds / 60) as f32).value_text(short(*seconds, units))
    });
    ui.add(BarChart::<M>::new(bars).gap(0)).fill_width();
    let (count, mean) = stats::session_count_and_mean(sessions, range, rollover);
    let count = u32::try_from(count).unwrap_or(u32::MAX);
    let line = super::in_glyphs(t!("charts.sessions", n = count, mean = short(mean, units)), ui.env().icons().mode());
    ui.add(Text::new(line).role("secondary")).fill_width();
}

/// The year: a grid of the last 52 weeks with today at the right edge, cut to the newest weeks
/// that fit, and the picked day read as text.
fn year<M: From<Msg> + Clone + Send + 'static>(
    screen: &Charts,
    sessions: &[Session],
    range: Range,
    rollover: TimeOfDay,
    units: &Units<'_>,
    ui: &mut View<'_, M>,
) {
    let totals = stats::daily(sessions, range, rollover);
    let selected = screen.selected.filter(|day| *day < totals.len());
    let grid = Heatmap::new(totals.iter().map(|seconds| (*seconds / 60) as f32))
        .rows(7)
        .selected(selected)
        .on_select(|day| M::from(Msg::Select(day)));
    let shown = grid.columns(ui.size().width);
    let weeks = if i64::from(shown) >= YEAR_WEEKS {
        t!("charts.weeks-all")
    } else {
        t!("charts.weeks-shown", shown = u32::from(shown))
    };
    ui.add(Text::new(weeks).role("secondary")).fill_width();
    // The months over the weeks shown: the grid keeps the newest weeks and draws them from the
    // left, so the axis is as wide as the grid and begins with the month of its first week.
    let first = range.from.add_days((i64::from(grid.columns(u16::MAX)) - i64::from(shown)) * 7);
    let last = range.end().add_days(-1);
    let months = (last.year() - first.year()) * 12 + i32::from(last.month()) - i32::from(first.month()) + 1;
    ui.column(|ui| {
        ui.add(Axis::months(first.month(), usize::try_from(months).unwrap_or(1))).width(Length::Cells(shown));
        ui.add(grid).id(CHART).height(Length::Cells(7)).fill_width();
    })
    .id(STAND)
    .fill_width();
    let picked = selected.map(|day| {
        let date = range.from.add_days(i64::try_from(day).unwrap_or(0));
        let name = t!(
            "today.date",
            weekday = t!(&format!("days.{}", date.weekday().number())),
            day = u32::from(date.day()),
            month = t!(&format!("months.{}", date.month()))
        );
        t!("charts.picked-day", day = name, duration = short(totals[day], units))
    });
    readout(picked, t!("charts.pick-hint-enter"), ui);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u8, day: u8) -> Date {
        Date::new(year, month, day).expect("valid date")
    }

    #[test]
    fn the_week_starts_on_monday_and_the_month_runs_to_today() {
        // 2026-09-18 is a Friday.
        let today = date(2026, 9, 18);
        let week = Scale::Week.range(today, Weekday::Monday);
        assert_eq!(week.from, date(2026, 9, 14));
        assert_eq!(week.days, 7);
        assert_eq!(Scale::Week.previous(today, Weekday::Monday).from, date(2026, 9, 7));
        assert_eq!(Scale::Week.range(today, Weekday::Sunday).from, date(2026, 9, 13));
        let month = Scale::Month.range(today, Weekday::Monday);
        assert_eq!((month.from, month.days), (date(2026, 9, 1), 18));
        let before = Scale::Month.previous(today, Weekday::Monday);
        assert_eq!((before.from, before.days), (date(2026, 8, 1), 31));
    }

    #[test]
    fn the_year_ends_today_and_begins_on_a_monday_fifty_two_weeks_back() {
        let today = date(2026, 9, 18);
        let year = Scale::Year.range(today, Weekday::Monday);
        assert_eq!(year.from.weekday(), Weekday::Monday);
        assert_eq!(year.from, date(2025, 9, 22));
        assert_eq!(year.end(), date(2026, 9, 19));
        assert_eq!(year.days, 51 * 7 + 5);
        assert_eq!(Scale::Year.previous(today, Weekday::Monday).end(), year.from);
    }

    #[test]
    fn choosing_a_scale_forgets_the_pick_and_reading_can_stop() {
        let mut screen = Charts::new();
        let _: Command<Msg> = update(&mut screen, Msg::Select(3));
        assert_eq!(screen.selected(), Some(3));
        let _: Command<Msg> = update(&mut screen, Msg::Scale(Scale::Month.index()));
        assert_eq!((screen.scale(), screen.selected()), (Scale::Month, None));
        let _: Command<Msg> = update(&mut screen, Msg::Read(Some(4)));
        let _: Command<Msg> = update(&mut screen, Msg::Read(None));
        assert_eq!(screen.selected(), None);
        let _: Command<Msg> = update(&mut screen, Msg::Scale(99));
        assert_eq!(screen.scale(), Scale::Day, "out of range is the first");
    }

    #[test]
    fn hours_round_up_to_a_tenth_so_a_minute_is_seen() {
        assert_eq!(hours(0), 0.0);
        assert_eq!(hours(60), 0.1);
        assert_eq!(hours(3_600), 1.0);
        assert_eq!(hours(3_600 + 30 * 60), 1.5);
    }
}
