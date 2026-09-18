//! Which day a session belongs to, and what a day adds up to.
//!
//! A session is written to the day it began on, in the local time of the place it began. The day
//! does not have to turn at midnight: someone who works past it wants the small hours counted
//! with the evening before, so the rollover hour is a setting. It only changes how sessions are
//! grouped, never what was recorded.

use std::collections::HashMap;

use qframe::date::{Date, DateTime, TimeOfDay};

use crate::id::Id;
use crate::session::Session;
use crate::tree::Category;

/// The local day `session` is counted under: the day it began, or the day before when it began
/// earlier than `rollover`.
///
/// The session's own offset from UTC is used, so a record made on a trip stays on the day it
/// was lived, not the day the machine now thinks it was.
#[must_use]
pub fn day_of(session: &Session, rollover: TimeOfDay) -> Date {
    day_at(session.started, session.offset_minutes, rollover)
}

/// The local day a stretch that began at `started` (seconds since the Unix epoch, lived
/// `offset_minutes` ahead of UTC) is counted under: the same rule as [`day_of`], for a counter
/// that is not a session yet.
#[must_use]
pub fn day_at(started: i64, offset_minutes: i16, rollover: TimeOfDay) -> Date {
    let local = DateTime::from_unix(started, offset_minutes);
    if local.time < rollover { local.date.add_days(-1) } else { local.date }
}

/// Seconds of work in one day.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DayTotals {
    /// Work across every focus.
    pub total: u64,
    /// Work under each focus that had any.
    pub by_focus: HashMap<Id, u64>,
}

/// The work recorded on `day` among `sessions`, grouped with the given `rollover`.
#[must_use]
pub fn totals(sessions: &[Session], day: Date, rollover: TimeOfDay) -> DayTotals {
    let mut totals = DayTotals::default();
    for session in sessions.iter().filter(|session| day_of(session, rollover) == day) {
        let work = session.work_seconds();
        totals.total = totals.total.saturating_add(work);
        let slot = totals.by_focus.entry(session.focus).or_insert(0);
        *slot = slot.saturating_add(work);
    }
    totals
}

/// Work under `category`: the sum over its focuses. Archived focuses count, because archiving
/// takes a focus out of the lists, not out of the past.
#[must_use]
pub fn category_total(totals: &DayTotals, category: &Category) -> u64 {
    category
        .focuses
        .iter()
        .filter_map(|focus| totals.by_focus.get(&focus.id))
        .fold(0, |sum, work| sum.saturating_add(*work))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Source;
    use crate::span::{ClockSource, Span, SpanKind};
    use crate::tree::Focus;

    /// Istanbul, three hours ahead of UTC.
    const OFFSET: i16 = 180;

    /// A session of `work` seconds of work starting at local `hour:minute` on 2026-09-18.
    fn session(focus: Id, hour: u8, minute: u8, work: u32) -> Session {
        let local = DateTime {
            date: Date::new(2026, 9, 18).expect("valid date"),
            time: TimeOfDay::new(hour, minute, 0),
            offset_minutes: OFFSET,
        };
        let started = local.to_unix();
        Session {
            id: Id::new(1, 1),
            revision: 1,
            written: started,
            focus,
            started,
            offset_minutes: OFFSET,
            ended: started + i64::from(work),
            spans: vec![Span::new(SpanKind::Work, 0, work, ClockSource::Mono)],
            source: Source::Timer,
            replaces: None,
            voids: None,
            continues: None,
            flags: Vec::new(),
            note: String::new(),
        }
    }

    fn focus(id: Id, archived: bool) -> Focus {
        Focus { id, name: String::new(), goal: None, archived, order: 1.0 }
    }

    #[test]
    fn a_session_belongs_to_the_local_day_it_began() {
        let session = session(Id::new(1, 1), 9, 0, 60);
        assert_eq!(day_of(&session, TimeOfDay::default()), Date::new(2026, 9, 18).expect("valid date"));
    }

    #[test]
    fn the_small_hours_before_the_rollover_belong_to_the_day_before() {
        let session = session(Id::new(1, 1), 0, 30, 60);
        let rollover = TimeOfDay::new(4, 0, 0);
        assert_eq!(day_of(&session, rollover), Date::new(2026, 9, 17).expect("valid date"));
        assert_eq!(day_of(&session, TimeOfDay::default()), Date::new(2026, 9, 18).expect("valid date"));
    }

    #[test]
    fn the_rollover_moment_itself_starts_the_new_day() {
        let session = session(Id::new(1, 1), 4, 0, 60);
        assert_eq!(day_of(&session, TimeOfDay::new(4, 0, 0)), Date::new(2026, 9, 18).expect("valid date"));
    }

    #[test]
    fn the_sessions_own_offset_decides_the_day() {
        // 23:30 in Istanbul is 20:30 UTC; the day is still the 18th where the session was lived.
        let mut session = session(Id::new(1, 1), 23, 30, 60);
        assert_eq!(day_of(&session, TimeOfDay::default()), Date::new(2026, 9, 18).expect("valid date"));
        // The same instant written from a place nine hours further east is already the 19th.
        session.offset_minutes = OFFSET + 9 * 60;
        assert_eq!(day_of(&session, TimeOfDay::default()), Date::new(2026, 9, 19).expect("valid date"));
    }

    #[test]
    fn totals_sum_work_of_the_day_by_focus() {
        let reading = Id::new(1, 1);
        let writing = Id::new(2, 2);
        let mut yesterday = session(reading, 10, 0, 900);
        yesterday.started -= 86_400;
        let sessions =
            [session(reading, 9, 0, 1_500), session(reading, 14, 0, 600), session(writing, 16, 0, 300), yesterday];
        let day = Date::new(2026, 9, 18).expect("valid date");
        let totals = totals(&sessions, day, TimeOfDay::default());
        assert_eq!(totals.total, 2_400);
        assert_eq!(totals.by_focus.get(&reading), Some(&2_100));
        assert_eq!(totals.by_focus.get(&writing), Some(&300));
        assert_eq!(totals.by_focus.len(), 2);
    }

    #[test]
    fn totals_of_a_day_with_nothing_are_empty() {
        let sessions = [session(Id::new(1, 1), 9, 0, 1_500)];
        let totals = totals(&sessions, Date::new(2026, 9, 20).expect("valid date"), TimeOfDay::default());
        assert_eq!(totals, DayTotals::default());
    }

    #[test]
    fn totals_only_count_work_spans() {
        let mut session = session(Id::new(1, 1), 9, 0, 600);
        session.spans.push(Span::new(SpanKind::Pause, 600, 300, ClockSource::Mono));
        session.spans.push(Span::new(SpanKind::Work, 900, 100, ClockSource::Mono));
        session.ended = session.started + 1_000;
        let totals = totals(&[session], Date::new(2026, 9, 18).expect("valid date"), TimeOfDay::default());
        assert_eq!(totals.total, 700);
    }

    #[test]
    fn a_category_sums_its_focuses_including_archived_ones() {
        let reading = Id::new(1, 1);
        let old = Id::new(2, 2);
        let elsewhere = Id::new(3, 3);
        let sessions = [session(reading, 9, 0, 1_000), session(old, 10, 0, 500), session(elsewhere, 11, 0, 200)];
        let totals = totals(&sessions, Date::new(2026, 9, 18).expect("valid date"), TimeOfDay::default());
        let category = Category {
            id: Id::new(9, 9),
            name: String::new(),
            icon: None,
            goal: None,
            archived: false,
            order: 1.0,
            focuses: vec![focus(reading, false), focus(old, true), focus(Id::new(4, 4), false)],
        };
        assert_eq!(category_total(&totals, &category), 1_500);
    }
}
