//! What the records add up to over a stretch of days: by the hour, by the day, by category, by
//! focus, and against the stretch before.
//!
//! Every function here is a pure calculation over sessions; the Charts screen draws what comes
//! out. A session belongs to the local day it began, as [`crate::day::day_of`] decides, and every
//! session counts whether or not its focus is still in the lists: archiving takes a focus out of
//! the lists, not out of the past.

use std::collections::HashMap;

use qframe::date::{Date, TimeOfDay, Weekday, days_in_month};
use qframe::theme::SERIES_COLORS;

use crate::day::{day_at, day_of};
use crate::id::Id;
use crate::session::Session;
use crate::span::{Span, SpanKind};
use crate::store::Running;
use crate::tree::{Goal, Period, Tree as Catalog};

/// Seconds in an hour.
const HOUR: i64 = 3_600;

/// Seconds in a day.
const DAY: i64 = 86_400;

/// A stretch of whole days: `days` days from `from` on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    /// The first day.
    pub from: Date,
    /// How many days, at least one.
    pub days: usize,
}

impl Range {
    /// The stretch of `days` days starting on `from`; at least one day.
    #[must_use]
    pub fn new(from: Date, days: usize) -> Self {
        Self { from, days: days.max(1) }
    }

    /// The single day `day`.
    #[must_use]
    pub fn day(day: Date) -> Self {
        Self::new(day, 1)
    }

    /// The stretch of the same length ending the day before this one starts.
    #[must_use]
    pub fn previous(self) -> Self {
        let days = i64::try_from(self.days).unwrap_or(i64::MAX);
        Self::new(self.from.add_days(-days), self.days)
    }

    /// The day after the last one.
    #[must_use]
    pub fn end(self) -> Date {
        self.from.add_days(i64::try_from(self.days).unwrap_or(i64::MAX))
    }

    /// Where `day` sits in the stretch, or `None` outside it.
    #[must_use]
    pub fn index_of(self, day: Date) -> Option<usize> {
        let offset = day.to_days().checked_sub(self.from.to_days())?;
        let index = usize::try_from(offset).ok()?;
        (index < self.days).then_some(index)
    }
}

/// The sessions of `range`, each with the day it fell on, counted from the start of the stretch.
fn within(sessions: &[Session], range: Range, rollover: TimeOfDay) -> impl Iterator<Item = (usize, &Session)> {
    sessions.iter().filter_map(move |session| range.index_of(day_of(session, rollover)).map(|index| (index, session)))
}

/// Seconds of work in each hour of `day`, by the clock the sessions were lived in.
///
/// A work span is shared out over the hours it crosses, so an hour and a half from 13:30 puts
/// thirty minutes in the 13 slot and an hour in the 14 slot. A session counted under `day`
/// because it began before the rollover puts its work in the small hours of the clock, which is
/// where it was lived.
#[must_use]
pub fn hourly(sessions: &[Session], day: Date, rollover: TimeOfDay) -> [u64; 24] {
    let mut hours = [0_u64; 24];
    for (_, session) in within(sessions, Range::day(day), rollover) {
        let local_start = session.started.saturating_add(i64::from(session.offset_minutes) * 60);
        for span in session.spans.iter().filter(|span| span.kind == crate::span::SpanKind::Work) {
            let mut at = local_start.saturating_add(i64::from(span.offset));
            let mut left = i64::from(span.seconds);
            while left > 0 {
                let hour = usize::try_from(at.rem_euclid(DAY) / HOUR).unwrap_or(0).min(23);
                let until_next = HOUR - at.rem_euclid(HOUR);
                let taken = left.min(until_next);
                hours[hour] = hours[hour].saturating_add(u64::try_from(taken).unwrap_or(0));
                at = at.saturating_add(taken);
                left -= taken;
            }
        }
    }
    hours
}

/// Seconds of work on each day of `range`, in order.
#[must_use]
pub fn daily(sessions: &[Session], range: Range, rollover: TimeOfDay) -> Vec<u64> {
    let mut days = vec![0_u64; range.days];
    for (index, session) in within(sessions, range, rollover) {
        days[index] = days[index].saturating_add(session.work_seconds());
    }
    days
}

/// Seconds of work over the whole of `range`.
#[must_use]
pub fn total(sessions: &[Session], range: Range, rollover: TimeOfDay) -> u64 {
    daily(sessions, range, rollover).iter().fold(0, |sum, day| sum.saturating_add(*day))
}

/// The work of one category over a stretch of days.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryDays {
    /// The category, or `None` for sessions whose focus is in no category any more.
    pub category: Option<Id>,
    /// Seconds of work on each day of the stretch.
    pub days: Vec<u64>,
}

impl CategoryDays {
    /// Seconds of work over the whole stretch.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.days.iter().fold(0, |sum, day| sum.saturating_add(*day))
    }
}

/// The work of every category that had any over `range`, day by day, in the tone order of the
/// categories ([`tone_of`]); sessions of a focus that is in no category come last under `None`.
///
/// Archived categories and focuses count: their sessions were lived.
#[must_use]
pub fn by_category(sessions: &[Session], range: Range, catalog: &Catalog, rollover: TimeOfDay) -> Vec<CategoryDays> {
    let owner: HashMap<Id, Id> = catalog
        .categories
        .iter()
        .flat_map(|category| category.focuses.iter().map(move |focus| (focus.id, category.id)))
        .collect();
    let mut totals: HashMap<Option<Id>, Vec<u64>> = HashMap::new();
    for (index, session) in within(sessions, range, rollover) {
        let days = totals.entry(owner.get(&session.focus).copied()).or_insert_with(|| vec![0; range.days]);
        days[index] = days[index].saturating_add(session.work_seconds());
    }
    let mut shares: Vec<CategoryDays> =
        totals.into_iter().map(|(category, days)| CategoryDays { category, days }).collect();
    shares.sort_by_key(|share| share.category.map_or((1, 0, None), |id| (0, tone_of(catalog, Some(id)), Some(id))));
    shares
}

/// Where a block of a day strip begins and ends, beyond the plain case of a block that lies
/// wholly inside its day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The block begins and ends inside the day.
    Whole,
    /// The block runs past the end of the day: the session went on into the next one, and the
    /// strip of that day carries its rest as a [`Continued`](Self::Continued) block.
    PastEnd,
    /// The block carries on a session that began the day before.
    Continued,
    /// The block is the counter still running: it ends where the count has reached so far.
    Running,
}

/// One block of a day strip: a stretch of work under one focus, placed in seconds from the
/// start of the day. Breaks, sleep and idle time are not blocks; they are the holes between
/// them, and so is time not recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    /// Seconds from the start of the day to the start of the block.
    pub from: u32,
    /// How long the block lasts inside the day, in seconds; a block that runs past the end of
    /// the day is cut there.
    pub seconds: u32,
    /// The focus the work was under.
    pub focus: Id,
    /// How the block sits in the day.
    pub edge: Edge,
}

/// The work of `day` as blocks for its strip, in time order: one block per work span, so two
/// focuses in one hour are two blocks and no dominant one is chosen. `running` is the counter
/// still on, whose open work span is a [`Running`](Edge::Running) block.
///
/// A day runs 24 hours from `rollover`. A session counted under `day` that goes on past its
/// end keeps its block to the end, marked [`PastEnd`](Edge::PastEnd); the strip of the next day
/// carries the rest as [`Continued`](Edge::Continued) blocks from its start.
#[must_use]
pub fn strip(sessions: &[Session], running: Option<&Running>, day: Date, rollover: TimeOfDay) -> Vec<Block> {
    let day_start = day.to_days().saturating_mul(DAY).saturating_add(i64::from(rollover.seconds_since_midnight()));
    let yesterday = day.add_days(-1);
    let mut blocks = Vec::new();
    let stretches = sessions
        .iter()
        .map(|session| Stretch {
            focus: session.focus,
            started: session.started,
            offset_minutes: session.offset_minutes,
            spans: &session.spans,
            running: false,
        })
        .chain(running.map(|counter| Stretch {
            focus: counter.focus,
            started: counter.started,
            offset_minutes: counter.offset_minutes,
            spans: &counter.spans,
            running: true,
        }));
    for stretch in stretches {
        let on = day_at(stretch.started, stretch.offset_minutes, rollover);
        if on == day {
            place(&mut blocks, &stretch, day_start, false);
        } else if on == yesterday {
            place(&mut blocks, &stretch, day_start, true);
        }
    }
    blocks.sort_by_key(|block| (block.from, block.seconds, block.focus));
    blocks
}

/// A session or the running counter, as far as the strip is concerned.
struct Stretch<'a> {
    focus: Id,
    started: i64,
    offset_minutes: i16,
    spans: &'a [Span],
    /// Whether the last span is still open, so its block is the counter's.
    running: bool,
}

/// Adds the work spans of `stretch` that fall inside the day starting at `day_start` (local
/// seconds) to `blocks`, cut to the day. `continued` marks every block as carrying on from the
/// day before.
fn place(blocks: &mut Vec<Block>, stretch: &Stretch<'_>, day_start: i64, continued: bool) {
    let local_start = stretch.started.saturating_add(i64::from(stretch.offset_minutes) * 60);
    let day_end = day_start.saturating_add(DAY);
    let last = stretch.spans.len().saturating_sub(1);
    for (index, span) in stretch.spans.iter().enumerate().filter(|(_, span)| span.kind == SpanKind::Work) {
        let begin = local_start.saturating_add(i64::from(span.offset));
        let end = begin.saturating_add(i64::from(span.seconds));
        if end <= day_start || begin >= day_end {
            continue;
        }
        let from = u32::try_from(begin.max(day_start) - day_start).unwrap_or(0);
        let seconds = u32::try_from(end.min(day_end) - begin.max(day_start)).unwrap_or(0);
        let edge = if continued {
            Edge::Continued
        } else if end > day_end {
            Edge::PastEnd
        } else if stretch.running && index == last {
            Edge::Running
        } else {
            Edge::Whole
        };
        blocks.push(Block { from, seconds, focus: stretch.focus, edge });
    }
}

/// The stretch of days a goal of `period` runs over that holds `today`: the day itself, its
/// week from `week_start`, or its calendar month.
#[must_use]
pub fn goal_range(period: Period, today: Date, week_start: Weekday) -> Range {
    match period {
        Period::Day => Range::day(today),
        Period::Week => Range::new(today.start_of_week(week_start), 7),
        Period::Month => Range::new(today.first_of_month(), usize::from(days_in_month(today.year(), today.month()))),
    }
}

/// How far `goal` has come: the seconds of work under any of `focuses` in the goal's window
/// around `today`, and the seconds the goal asks for. A category's goal is measured over all of
/// its focuses, archived ones included, since their work was lived too.
#[must_use]
pub fn goal_progress(
    goal: Goal,
    focuses: &[Id],
    sessions: &[Session],
    today: Date,
    week_start: Weekday,
    rollover: TimeOfDay,
) -> (u64, u64) {
    let range = goal_range(goal.period, today, week_start);
    let done = within(sessions, range, rollover)
        .filter(|(_, session)| focuses.contains(&session.focus))
        .fold(0_u64, |sum, (_, session)| sum.saturating_add(session.work_seconds()));
    (done, u64::from(goal.amount))
}

/// The `n` focuses worked on most over `range`, most first, with their seconds.
#[must_use]
pub fn top_focuses(sessions: &[Session], range: Range, n: usize, rollover: TimeOfDay) -> Vec<(Id, u64)> {
    let mut totals: HashMap<Id, u64> = HashMap::new();
    for (_, session) in within(sessions, range, rollover) {
        let slot = totals.entry(session.focus).or_insert(0);
        *slot = slot.saturating_add(session.work_seconds());
    }
    let mut ranked: Vec<(Id, u64)> = totals.into_iter().collect();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    ranked.truncate(n);
    ranked
}

/// How many sessions fell in `range` and the mean seconds of work in one; zero for none.
#[must_use]
pub fn session_count_and_mean(sessions: &[Session], range: Range, rollover: TimeOfDay) -> (usize, u64) {
    let (count, seconds) = within(sessions, range, rollover)
        .fold((0_usize, 0_u64), |(count, sum), (_, session)| (count + 1, sum.saturating_add(session.work_seconds())));
    let mean = u64::try_from(count).ok().filter(|count| *count > 0).map_or(0, |count| seconds / count);
    (count, mean)
}

/// How `this` stands against `previous`: positive for more, negative for less, zero for the same.
#[must_use]
pub fn compare(this: u64, previous: u64) -> i64 {
    let this = i64::try_from(this).unwrap_or(i64::MAX);
    let previous = i64::try_from(previous).unwrap_or(i64::MAX);
    this.saturating_sub(previous)
}

/// The series tone of a category, the same on every screen and every day: its place among the
/// catalogue's categories in the order they were made, wrapped over the theme's tones.
///
/// Categories are only ever added, never taken out (archiving keeps them in the catalogue), so
/// a new one takes the next tone and no existing one changes. Up to [`SERIES_COLORS`]
/// categories each have a tone of their own; from the sixth on two share one and the legend's
/// names tell them apart. Sessions of a focus in no category (`None`) take the tone after the
/// last category's.
#[must_use]
pub fn tone_of(catalog: &Catalog, category: Option<Id>) -> usize {
    let mut ids: Vec<Id> = catalog.categories.iter().map(|category| category.id).collect();
    ids.sort_unstable();
    let place = category.and_then(|id| ids.iter().position(|known| *known == id)).unwrap_or(ids.len());
    place % SERIES_COLORS
}

#[cfg(test)]
mod tests {
    use qframe::date::DateTime;

    use super::*;
    use crate::session::Source;
    use crate::span::{ClockSource, Span, SpanKind};
    use crate::tree::{Category, Focus};

    /// Istanbul, three hours ahead of UTC.
    const OFFSET: i16 = 180;

    fn date(day: u8) -> Date {
        Date::new(2026, 9, day).expect("valid date")
    }

    /// A session of `spans` starting at local `hour:minute` on 2026-09-`day`.
    fn session_on(day: u8, focus: Id, hour: u8, minute: u8, spans: Vec<Span>) -> Session {
        let local = DateTime { date: date(day), time: TimeOfDay::new(hour, minute, 0), offset_minutes: OFFSET };
        let started = local.to_unix();
        let ended = started + i64::try_from(spans.last().map_or(0, Span::end)).unwrap_or(0);
        Session {
            id: Id::new(u64::from(day) * 100 + u64::from(hour), 1),
            revision: 1,
            written: ended,
            focus,
            started,
            offset_minutes: OFFSET,
            ended,
            spans,
            source: Source::Timer,
            replaces: None,
            voids: None,
            continues: None,
            flags: Vec::new(),
            note: String::new(),
        }
    }

    fn work(day: u8, focus: Id, hour: u8, minute: u8, seconds: u32) -> Session {
        session_on(day, focus, hour, minute, vec![Span::new(SpanKind::Work, 0, seconds, ClockSource::Mono)])
    }

    fn catalog() -> Catalog {
        let focus = |id: Id, archived: bool| Focus { id, name: String::new(), goal: None, archived, order: 0.0 };
        Catalog {
            categories: vec![
                Category {
                    id: Id::new(1, 1),
                    name: "Work".to_owned(),
                    icon: None,
                    goal: None,
                    archived: false,
                    order: 0.0,
                    focuses: vec![focus(Id::new(11, 1), false), focus(Id::new(12, 1), true)],
                },
                Category {
                    id: Id::new(2, 2),
                    name: "Study".to_owned(),
                    icon: None,
                    goal: None,
                    archived: true,
                    order: 1.0,
                    focuses: vec![focus(Id::new(21, 2), false)],
                },
            ],
        }
    }

    #[test]
    fn a_span_crossing_an_hour_is_shared_between_the_hours() {
        let session = work(18, Id::new(11, 1), 13, 30, 5_400);
        let hours = hourly(&[session], date(18), TimeOfDay::default());
        assert_eq!(hours[13], 1_800);
        assert_eq!(hours[14], 3_600);
        assert_eq!(hours.iter().sum::<u64>(), 5_400);
    }

    #[test]
    fn breaks_are_not_in_the_hours_and_the_work_after_them_lands_where_it_was() {
        let spans = vec![
            Span::new(SpanKind::Work, 0, 1_800, ClockSource::Mono),
            Span::new(SpanKind::Pause, 1_800, 1_800, ClockSource::Mono),
            Span::new(SpanKind::Work, 3_600, 600, ClockSource::Mono),
        ];
        let session = session_on(18, Id::new(11, 1), 9, 0, spans);
        let hours = hourly(&[session], date(18), TimeOfDay::default());
        assert_eq!(hours[9], 1_800);
        assert_eq!(hours[10], 600);
    }

    #[test]
    fn a_session_past_midnight_stays_on_the_day_it_began() {
        let session = work(18, Id::new(11, 1), 23, 30, 3_600);
        let rollover = TimeOfDay::default();
        assert_eq!(daily(std::slice::from_ref(&session), Range::new(date(18), 2), rollover), vec![3_600, 0]);
        let hours = hourly(&[session], date(18), rollover);
        assert_eq!(hours[23], 1_800);
        assert_eq!(hours[0], 1_800, "the small hours are drawn where they were lived");
    }

    #[test]
    fn daily_totals_follow_the_range_and_ignore_what_is_outside_it() {
        let sessions = [
            work(17, Id::new(11, 1), 9, 0, 100),
            work(18, Id::new(11, 1), 9, 0, 200),
            work(18, Id::new(21, 2), 15, 0, 300),
            work(20, Id::new(11, 1), 9, 0, 400),
        ];
        let range = Range::new(date(18), 2);
        assert_eq!(daily(&sessions, range, TimeOfDay::default()), vec![500, 0]);
        assert_eq!(total(&sessions, range, TimeOfDay::default()), 500);
        assert_eq!(total(&sessions, range.previous(), TimeOfDay::default()), 100);
        assert_eq!(range.previous().from, date(16));
        assert_eq!(range.end(), date(20));
    }

    #[test]
    fn categories_include_archived_focuses_and_archived_categories() {
        let sessions = [
            work(18, Id::new(11, 1), 9, 0, 100),
            work(18, Id::new(12, 1), 10, 0, 50),
            work(19, Id::new(21, 2), 9, 0, 70),
            work(19, Id::new(99, 9), 9, 0, 5),
        ];
        let shares = by_category(&sessions, Range::new(date(18), 2), &catalog(), TimeOfDay::default());
        let work_share = shares.iter().find(|share| share.category == Some(Id::new(1, 1))).expect("Work");
        assert_eq!(work_share.days, vec![150, 0], "the archived focus counts");
        let study = shares.iter().find(|share| share.category == Some(Id::new(2, 2))).expect("Study");
        assert_eq!(study.total(), 70, "the archived category counts");
        assert_eq!(shares.last().map(|share| (share.category, share.total())), Some((None, 5)), "unknown last");
        assert_eq!(shares.len(), 3);
    }

    #[test]
    fn a_goal_is_measured_over_its_day_week_or_month() {
        let rust = Id::new(11, 1);
        let sessions = [
            work(18, rust, 9, 0, 3_600),
            work(17, rust, 9, 0, 1_800),
            work(13, rust, 9, 0, 600),
            work(1, rust, 9, 0, 60),
        ];
        let progress = |period: Period| {
            goal_progress(
                Goal { amount: 7_200, period },
                &[rust],
                &sessions,
                date(18),
                Weekday::Monday,
                TimeOfDay::default(),
            )
        };
        assert_eq!(progress(Period::Day), (3_600, 7_200));
        assert_eq!(progress(Period::Week), (5_400, 7_200), "the 13th is the Sunday before this week");
        assert_eq!(progress(Period::Month), (6_060, 7_200));
        let sunday_first = goal_progress(
            Goal { amount: 7_200, period: Period::Week },
            &[rust],
            &sessions,
            date(18),
            Weekday::Sunday,
            TimeOfDay::default(),
        );
        assert_eq!(sunday_first.0, 6_000, "with the week from Sunday the 13th is in it");
        assert_eq!(goal_range(Period::Month, date(18), Weekday::Monday).days, 30);
    }

    #[test]
    fn a_category_goal_sums_its_focuses_archived_ones_included() {
        let sessions = [
            work(18, Id::new(11, 1), 9, 0, 100),
            work(18, Id::new(12, 1), 10, 0, 50),
            work(18, Id::new(21, 2), 11, 0, 70),
        ];
        let catalog = catalog();
        let focuses: Vec<Id> = catalog.categories[0].focuses.iter().map(|focus| focus.id).collect();
        let goal = Goal { amount: 600, period: Period::Day };
        assert_eq!(
            goal_progress(goal, &focuses, &sessions, date(18), Weekday::Monday, TimeOfDay::default()),
            (150, 600)
        );
        assert_eq!(goal_progress(goal, &[], &sessions, date(18), Weekday::Monday, TimeOfDay::default()), (0, 600));
    }

    #[test]
    fn the_top_focuses_are_ranked_and_cut() {
        let sessions = [
            work(18, Id::new(11, 1), 9, 0, 100),
            work(18, Id::new(11, 1), 11, 0, 100),
            work(18, Id::new(12, 1), 10, 0, 150),
            work(18, Id::new(21, 2), 12, 0, 20),
        ];
        let top = top_focuses(&sessions, Range::day(date(18)), 2, TimeOfDay::default());
        assert_eq!(top, vec![(Id::new(11, 1), 200), (Id::new(12, 1), 150)]);
    }

    #[test]
    fn the_session_count_and_the_mean_are_over_the_range_only() {
        let sessions = [
            work(18, Id::new(11, 1), 9, 0, 100),
            work(18, Id::new(11, 1), 11, 0, 300),
            work(25, Id::new(11, 1), 11, 0, 900),
        ];
        assert_eq!(session_count_and_mean(&sessions, Range::new(date(14), 7), TimeOfDay::default()), (2, 200));
        assert_eq!(session_count_and_mean(&sessions, Range::day(date(1)), TimeOfDay::default()), (0, 0));
    }

    #[test]
    fn comparison_carries_the_sign() {
        assert_eq!(compare(3_600, 1_800), 1_800);
        assert_eq!(compare(1_800, 3_600), -1_800);
        assert_eq!(compare(0, 0), 0);
    }

    #[test]
    fn tones_follow_the_order_categories_were_made_in_and_wrap() {
        let mut catalog = catalog();
        // Study was made after Work, however the catalogue orders them.
        catalog.categories.swap(0, 1);
        assert_eq!(tone_of(&catalog, Some(Id::new(1, 1))), 0);
        assert_eq!(tone_of(&catalog, Some(Id::new(2, 2))), 1, "the archived category keeps its tone");
        assert_eq!(tone_of(&catalog, None), 2, "no category comes after the last one");
        for stamp in 3..=SERIES_COLORS as u64 {
            catalog.categories.push(Category {
                id: Id::new(stamp, stamp.into()),
                name: String::new(),
                icon: None,
                goal: None,
                archived: false,
                order: 0.0,
                focuses: Vec::new(),
            });
        }
        assert_eq!(tone_of(&catalog, Some(Id::new(1, 1))), 0, "adding categories changes no tone");
        assert_eq!(tone_of(&catalog, None), 0, "the sixth wraps to the first tone");
    }

    #[test]
    fn two_focuses_in_one_hour_are_two_blocks_and_a_break_is_a_hole() {
        let spans = vec![
            Span::new(SpanKind::Work, 0, 1_200, ClockSource::Mono),
            Span::new(SpanKind::Pause, 1_200, 600, ClockSource::Mono),
            Span::new(SpanKind::Work, 1_800, 600, ClockSource::Mono),
        ];
        let sessions = [session_on(18, Id::new(11, 1), 9, 0, spans), work(18, Id::new(21, 2), 9, 45, 900)];
        let blocks = strip(&sessions, None, date(18), TimeOfDay::default());
        let placed: Vec<(u32, u32, Id)> = blocks.iter().map(|block| (block.from, block.seconds, block.focus)).collect();
        assert_eq!(
            placed,
            vec![
                (9 * 3_600, 1_200, Id::new(11, 1)),
                (9 * 3_600 + 1_800, 600, Id::new(11, 1)),
                (9 * 3_600 + 2_700, 900, Id::new(21, 2)),
            ]
        );
        assert!(blocks.iter().all(|block| block.edge == Edge::Whole));
        assert!(strip(&sessions, None, date(19), TimeOfDay::default()).is_empty(), "another day holds nothing");
    }

    #[test]
    fn a_session_past_the_end_of_the_day_is_open_ended_and_continues_on_the_next() {
        let session = work(18, Id::new(11, 1), 23, 30, 3_600);
        let rollover = TimeOfDay::default();
        let today = strip(std::slice::from_ref(&session), None, date(18), rollover);
        assert_eq!(
            today,
            vec![Block { from: 23 * 3_600 + 1_800, seconds: 1_800, focus: Id::new(11, 1), edge: Edge::PastEnd }]
        );
        let tomorrow = strip(std::slice::from_ref(&session), None, date(19), rollover);
        assert_eq!(tomorrow, vec![Block { from: 0, seconds: 1_800, focus: Id::new(11, 1), edge: Edge::Continued }]);
        // With the day turning at four, the same session lies whole in the day of the 18th.
        let late = TimeOfDay::new(4, 0, 0);
        let whole = strip(std::slice::from_ref(&session), None, date(18), late);
        assert_eq!(
            whole,
            vec![Block { from: 19 * 3_600 + 1_800, seconds: 3_600, focus: Id::new(11, 1), edge: Edge::Whole }]
        );
        assert!(strip(std::slice::from_ref(&session), None, date(19), late).is_empty());
    }

    #[test]
    fn the_running_counter_is_an_open_block_unless_it_is_on_a_break() {
        let started = DateTime { date: date(18), time: TimeOfDay::new(14, 0, 0), offset_minutes: OFFSET }.to_unix();
        let mut counter = Running {
            focus: Id::new(11, 1),
            started,
            offset_minutes: OFFSET,
            spans: vec![Span::new(SpanKind::Work, 0, 900, ClockSource::Mono)],
            paused: false,
            refreshed: started + 900,
            flags: Vec::new(),
            target: None,
            idle_from: None,
            watch: crate::store::Watch::Window,
        };
        let blocks = strip(&[], Some(&counter), date(18), TimeOfDay::default());
        assert_eq!(blocks, vec![Block { from: 14 * 3_600, seconds: 900, focus: Id::new(11, 1), edge: Edge::Running }]);
        counter.spans.push(Span::new(SpanKind::Pause, 900, 300, ClockSource::Mono));
        counter.paused = true;
        let blocks = strip(&[], Some(&counter), date(18), TimeOfDay::default());
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].edge, Edge::Whole, "on a break the work so far is closed and the break is a hole");
    }
}
