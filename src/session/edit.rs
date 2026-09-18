//! Building the records a person writes by hand: a session typed in after the fact, and a
//! correction of one already on disk.
//!
//! Nothing here touches a file. A correction is a new session that replaces the old one with a
//! higher revision (the rule is in [`super::resolve`]); the store puts it in the right month. Time typed by a
//! person was not measured by any clock, so it is written as one work span from the wall clock:
//! the file says exactly what is known and nothing more.

use super::{Flag, Session, Source};
use crate::id::Id;
use crate::span::{ClockSource, Span, SpanKind};

/// What a person typed into the record form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changes {
    /// The focus the session belongs to.
    pub focus: Id,
    /// When it began, in seconds since the Unix epoch.
    pub started: i64,
    /// Minutes the local time was ahead of UTC when it began.
    pub offset_minutes: i16,
    /// Seconds of work.
    pub seconds: u32,
    /// The note, already trimmed.
    pub note: String,
}

impl Changes {
    /// The form's starting values for `session`: its focus, start, offset, work and note.
    #[must_use]
    pub fn of(session: &Session) -> Self {
        Self {
            focus: session.focus,
            started: session.started,
            offset_minutes: session.offset_minutes,
            seconds: u32::try_from(session.work_seconds()).unwrap_or(u32::MAX),
            note: session.note.clone(),
        }
    }

    /// Whether the time differs from the session's: its start or its work.
    #[must_use]
    pub fn moves_time(&self, session: &Session) -> bool {
        self.started != session.started || u64::from(self.seconds) != session.work_seconds()
    }
}

/// A session typed in by hand: one work span measured by nothing but the person, with `id`,
/// written at `written`.
#[must_use]
pub fn manual(changes: &Changes, id: Id, written: i64) -> Session {
    Session {
        id,
        revision: 1,
        written,
        focus: changes.focus,
        started: changes.started,
        offset_minutes: changes.offset_minutes,
        ended: changes.started.saturating_add(i64::from(changes.seconds)),
        spans: vec![Span::new(SpanKind::Work, 0, changes.seconds, ClockSource::Wall)],
        source: Source::Manual,
        replaces: None,
        voids: None,
        continues: None,
        flags: Vec::new(),
        note: changes.note.clone(),
    }
}

/// `session` with `changes` applied, keeping its identity: what the timer stops with once the
/// person has fixed a duration over the ceiling.
///
/// A change to the time rebuilds the spans as one work span from the wall clock: the measured
/// breaks and sleeps do not fit a length the person set. The `over-ceiling` flag goes with them,
/// since the person has now said how long the session was. A change to the focus or the note
/// alone keeps the spans, the end and every flag as they were measured.
#[must_use]
pub fn adjusted(session: &Session, changes: &Changes) -> Session {
    let mut adjusted = session.clone();
    adjusted.focus = changes.focus;
    adjusted.note = changes.note.clone();
    if changes.moves_time(session) {
        adjusted.started = changes.started;
        adjusted.offset_minutes = changes.offset_minutes;
        adjusted.ended = changes.started.saturating_add(i64::from(changes.seconds));
        adjusted.spans = vec![Span::new(SpanKind::Work, 0, changes.seconds, ClockSource::Wall)];
        adjusted.flags.retain(|flag| *flag != Flag::OverCeiling);
    }
    adjusted
}

/// A correction of `original` with `changes`: a new record `id`, written at `written`, one
/// revision above the original and replacing it. The spans and the flags follow [`adjusted`].
#[must_use]
pub fn correction(original: &Session, changes: &Changes, id: Id, written: i64) -> Session {
    Session {
        id,
        revision: original.revision.saturating_add(1),
        written,
        source: Source::Edited,
        replaces: Some(original.id),
        voids: None,
        ..adjusted(original, changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::check;

    fn original() -> Session {
        Session {
            id: Id::new(10, 1),
            revision: 1,
            written: 1_758_131_400,
            focus: Id::new(2, 2),
            started: 1_758_124_800,
            offset_minutes: 180,
            ended: 1_758_131_400,
            spans: vec![
                Span::new(SpanKind::Work, 0, 1_500, ClockSource::Mono),
                Span::new(SpanKind::Pause, 1_500, 600, ClockSource::Mono),
                Span::new(SpanKind::Work, 2_100, 4_500, ClockSource::Mono),
            ],
            source: Source::Timer,
            replaces: None,
            voids: None,
            continues: Some(Id::new(9, 9)),
            flags: vec![Flag::SuspectClock, Flag::OverCeiling],
            note: "before".to_owned(),
        }
    }

    #[test]
    fn a_manual_session_is_one_wall_clock_work_span() {
        let changes = Changes {
            focus: Id::new(2, 2),
            started: 1_758_124_800,
            offset_minutes: 180,
            seconds: 5_400,
            note: "typed".to_owned(),
        };
        let session = manual(&changes, Id::new(11, 1), 1_758_200_000);
        assert_eq!(session.revision, 1);
        assert_eq!(session.source, Source::Manual);
        assert_eq!(session.replaces, None);
        assert_eq!(session.ended, 1_758_124_800 + 5_400);
        assert_eq!(session.work_seconds(), 5_400);
        assert_eq!(session.spans, vec![Span::new(SpanKind::Work, 0, 5_400, ClockSource::Wall)]);
        assert!(check(&session.spans, 5_400).is_empty());
        assert!(session.flags.is_empty());
        assert_eq!(session.note, "typed");
    }

    #[test]
    fn a_correction_of_the_time_rebuilds_the_spans_and_drops_the_ceiling_flag() {
        let original = original();
        let changes = Changes { seconds: 3_600, ..Changes::of(&original) };
        let correction = correction(&original, &changes, Id::new(12, 1), 1_758_300_000);
        assert_eq!(correction.revision, 2);
        assert_eq!(correction.replaces, Some(original.id));
        assert_eq!(correction.source, Source::Edited);
        assert_eq!(correction.written, 1_758_300_000);
        assert_eq!(correction.continues, original.continues, "the link to the earlier part stays");
        assert_eq!(correction.spans, vec![Span::new(SpanKind::Work, 0, 3_600, ClockSource::Wall)]);
        assert_eq!(correction.ended, original.started + 3_600);
        assert!(check(&correction.spans, 3_600).is_empty());
        assert_eq!(correction.flags, vec![Flag::SuspectClock], "the clock flag is kept, the ceiling one goes");
    }

    #[test]
    fn a_correction_of_the_note_or_the_focus_keeps_the_measured_spans() {
        let original = original();
        let changes = Changes { focus: Id::new(3, 3), note: "after".to_owned(), ..Changes::of(&original) };
        let correction = correction(&original, &changes, Id::new(12, 1), 1_758_300_000);
        assert_eq!(correction.spans, original.spans);
        assert_eq!(correction.ended, original.ended);
        assert_eq!(correction.flags, original.flags, "nothing about the time changed");
        assert_eq!(correction.focus, Id::new(3, 3));
        assert_eq!(correction.note, "after");
        assert_eq!(correction.revision, 2);
    }

    #[test]
    fn a_moved_start_counts_as_a_change_of_time() {
        let original = original();
        let changes = Changes { started: original.started + 600, ..Changes::of(&original) };
        assert!(changes.moves_time(&original));
        let fixed = adjusted(&original, &changes);
        assert_eq!(fixed.id, original.id, "adjusting keeps the identity");
        assert_eq!(fixed.started, original.started + 600);
        assert_eq!(fixed.work_seconds(), original.work_seconds());
        assert!(!fixed.flags.contains(&Flag::OverCeiling));
    }
}
