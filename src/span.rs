//! The pieces a session is made of.
//!
//! A session is not one stretch of time with totals beside it: where a break or a sleep sits is
//! part of the record. A session that began at nine, took a two hour break and holds ten minutes of
//! work would otherwise draw a full block from nine to eleven ten and print "10 min" beside it.

/// What a person was doing during a span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    /// Working. Only these spans count towards the time recorded.
    Work,
    /// A break the person took on purpose.
    Pause,
    /// Nothing measured this time: the program was not running, or, in records written before
    /// a sleep counted as the kind it interrupted, the machine slept. It never counts.
    Gap,
    /// No input reached the terminal and the person has not said whether it was work.
    Idle,
}

/// The clock a span was measured with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockSource {
    /// The monotonic clock, which stops while the machine sleeps.
    Mono,
    /// The clock that keeps counting while the machine sleeps.
    Boot,
    /// The wall clock, for spans a person typed in by hand.
    Wall,
}

/// One stretch of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// What the person was doing.
    pub kind: SpanKind,
    /// Seconds between the start of the session and the start of this span.
    pub offset: u32,
    /// How long the span lasted, in seconds.
    pub seconds: u32,
    /// The clock it was measured with.
    pub clock: ClockSource,
}

impl Span {
    /// A span of `seconds` starting `offset` seconds into the session.
    #[must_use]
    pub fn new(kind: SpanKind, offset: u32, seconds: u32, clock: ClockSource) -> Self {
        Self { kind, offset, seconds, clock }
    }

    /// The second after this span ends, counted from the start of the session.
    #[must_use]
    pub fn end(&self) -> u64 {
        u64::from(self.offset) + u64::from(self.seconds)
    }
}

/// Seconds of work in `spans`. The time a session records is never stored; it is always this sum,
/// so a gap found by mistake can be corrected without touching the record.
#[must_use]
pub fn work_seconds(spans: &[Span]) -> u64 {
    spans.iter().filter(|span| span.kind == SpanKind::Work).map(|span| u64::from(span.seconds)).sum()
}

/// Everything wrong with `spans` for a session of `total` seconds; empty when they hold together.
///
/// Spans follow one another without a hole and without overlapping, and together they fill the
/// session. A sleep span may not come from the monotonic clock, because sleep is exactly the time
/// that clock does not count.
#[must_use]
pub fn check(spans: &[Span], total: u32) -> Vec<String> {
    let mut problems = Vec::new();
    let mut expected: u64 = 0;
    for (index, span) in spans.iter().enumerate() {
        let number = index + 1;
        if u64::from(span.offset) != expected {
            problems.push(format!("span {number} starts at {}, expected {expected}", span.offset));
        }
        if span.kind == SpanKind::Gap && span.clock == ClockSource::Mono {
            problems.push(format!("span {number} is a sleep measured with the monotonic clock"));
        }
        expected = span.end().max(expected);
    }
    if expected != u64::from(total) {
        problems.push(format!("spans cover {expected} seconds, the session lasts {total}"));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(offset: u32, seconds: u32) -> Span {
        Span::new(SpanKind::Work, offset, seconds, ClockSource::Mono)
    }

    #[test]
    fn net_time_counts_only_work() {
        let spans = [
            work(0, 1500),
            Span::new(SpanKind::Pause, 1500, 600, ClockSource::Mono),
            work(2100, 4500),
            Span::new(SpanKind::Gap, 6600, 7200, ClockSource::Boot),
            Span::new(SpanKind::Idle, 13_800, 900, ClockSource::Mono),
        ];
        assert_eq!(work_seconds(&spans), 6000);
    }

    #[test]
    fn contiguous_spans_are_accepted() {
        let spans = [work(0, 100), Span::new(SpanKind::Pause, 100, 50, ClockSource::Mono)];
        assert!(check(&spans, 150).is_empty());
    }

    #[test]
    fn a_hole_between_spans_is_reported() {
        let spans = [work(0, 100), work(120, 30)];
        let problems = check(&spans, 150);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("span 2 starts at 120"), "{problems:?}");
    }

    #[test]
    fn overlapping_spans_are_reported() {
        let spans = [work(0, 100), work(80, 70)];
        assert_eq!(check(&spans, 150).len(), 1);
    }

    #[test]
    fn spans_must_start_at_zero() {
        assert_eq!(check(&[work(10, 140)], 150).len(), 1);
    }

    #[test]
    fn the_total_must_match_the_session() {
        let problems = check(&[work(0, 100)], 150);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("100"), "{problems:?}");
    }

    #[test]
    fn no_spans_is_not_a_problem_for_an_empty_session() {
        assert!(check(&[], 0).is_empty());
    }

    #[test]
    fn a_sleep_span_may_not_come_from_the_monotonic_clock() {
        let spans = [Span::new(SpanKind::Gap, 0, 150, ClockSource::Mono)];
        let problems = check(&spans, 150);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("sleep"), "{problems:?}");
    }
}
