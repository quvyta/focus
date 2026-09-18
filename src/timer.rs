//! The running timer: a state machine fed clock readings from outside.
//!
//! Nothing in here reads a clock. Every reading arrives as a [`Clocks`] value, so a sleep, a
//! clock jump or a starved process can be replayed in a test exactly as it would happen on a
//! machine. Three clocks do three jobs: the clock that stops in sleep measures work, the clock
//! that keeps going in sleep measures the session and the sleep inside it, and the wall clock
//! only stamps the record. The wall clock never shortens a session: a time daemon stepping the
//! clock back ninety seconds would otherwise erase ninety seconds someone really worked.

use std::time::Duration;

use qframe::uptime::Uptime;

use crate::id::Id;
use crate::session::{Flag, Session, Source};
use crate::span::{ClockSource, Span, SpanKind, work_seconds};

/// The tick interval when none is set, in seconds.
const DEFAULT_TICK: u32 = 1;

/// The most the wall clock may drift from the session clock between two ticks before the record
/// is flagged, in seconds.
const CLOCK_DRIFT: i64 = 5;

/// The least a tick may be late before the lateness is treated as sleep, in seconds. The threshold
/// is `max(LAG_FLOOR, 3 × tick)`, so a slow tick does not trip it.
const LAG_FLOOR: u32 = 5;

/// What the person said about a stretch of silence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleDecision {
    /// It was work after all.
    Count,
    /// It was not work; it stays out of the net time.
    DontCount,
    /// It was a break.
    Break,
}

impl IdleDecision {
    /// The kind the idle spans become.
    fn kind(self) -> SpanKind {
        match self {
            Self::Count => SpanKind::Work,
            Self::DontCount => SpanKind::Idle,
            Self::Break => SpanKind::Pause,
        }
    }
}

/// The three clocks read at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clocks {
    /// Seconds since the Unix epoch on the wall clock.
    pub wall: i64,
    /// Both monotonic clocks.
    pub uptime: Uptime,
}

/// Something a tick found out that the screen may want to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// The machine slept for this many seconds since the last tick; the sleep was recorded as
    /// the kind that was being measured, which is given.
    Suspended(u32, SpanKind),
    /// The wall clock moved by this many seconds more (or, negative, less) than the session
    /// clock since the last tick.
    ClockJump(i64),
    /// The work in this session has passed the ceiling.
    OverCeiling,
    /// The tick came this many seconds later than expected on a platform that cannot tell sleep
    /// apart; the lateness was recorded as sleep, of the kind given, as [`Notice::Suspended`] is.
    Lagged(u32, SpanKind),
}

/// A session being measured.
///
/// Spans are kept as a closed list plus one open span that grows until the next reading. The
/// session offset of every reading is counted from one origin reading, not by adding up tick
/// deltas, so rounding never opens a hole between spans.
#[derive(Debug, Clone)]
pub struct Timer {
    focus: Id,
    started: i64,
    offset_minutes: i16,
    /// Spans that are finished, contiguous from zero, adjacent same-kind spans merged.
    closed: Vec<Span>,
    /// What is being measured now and the session offset it began at.
    open: (SpanKind, u32),
    /// The reading the session offset is counted from.
    origin: Uptime,
    /// The session offset at `origin`; zero unless the timer was restored.
    base: u32,
    /// The last reading seen and its session offset.
    last: Clocks,
    last_offset: u32,
    paused: bool,
    /// Where the latest stretch of silence began, while it is being measured or waits for the
    /// person to say what it was; every idle span from there on belongs to it.
    idle_from: Option<u32>,
    /// Whether the silence is still going on, so the open span is idle.
    away: bool,
    flags: Vec<Flag>,
    ceiling: u32,
    tick: u32,
    detects_suspend: bool,
    source: Source,
}

impl Timer {
    /// The ceiling a session is flagged over when none is set: twelve hours.
    pub const DEFAULT_CEILING: u32 = 12 * 3_600;

    /// Starts measuring `focus` at the reading `now`, stamping it in an offset of
    /// `offset_minutes` from UTC.
    #[must_use]
    pub fn start(focus: Id, now: Clocks, offset_minutes: i16) -> Self {
        Self {
            focus,
            started: now.wall,
            offset_minutes,
            closed: Vec::new(),
            open: (SpanKind::Work, 0),
            origin: now.uptime,
            base: 0,
            last: now,
            last_offset: 0,
            paused: false,
            idle_from: None,
            away: false,
            flags: Vec::new(),
            ceiling: Self::DEFAULT_CEILING,
            tick: DEFAULT_TICK,
            detects_suspend: Uptime::detects_suspend(),
            source: Source::Timer,
        }
    }

    /// Carries on a timer that was running when the program last stopped.
    ///
    /// `spans` are the ones written up to `refreshed`, the wall stamp of the last save, and
    /// `until` is how far the counter is known to have been alive; see [`crate::liveness`]. No
    /// monotonic clock of this process saw any of the time since `refreshed`, so the wall clock
    /// stands in: from `refreshed` to `until` the kind that was open (work, or a break when
    /// `paused`) goes on and counts, and from `until` to `now.wall` the program was not alive, so
    /// that becomes a gap that does not count. The timer then continues in the state it was in: a
    /// paused timer stays paused. An `until` before `refreshed` counts as `refreshed`, and one
    /// after `now.wall` as `now.wall`. The session it eventually stops with is marked recovered unless
    /// [`Timer::source`] says otherwise, and carries no flags until [`Timer::with_flags`] hands them over.
    /// Idle spans the file holds stay as they are; a silence left unanswered is handed over with
    /// [`Timer::with_unclaimed_idle`] so the question about it can be asked again.
    #[must_use]
    #[expect(clippy::too_many_arguments, reason = "each is one field of the running file, read by name at the call")]
    pub fn restore(
        focus: Id,
        started: i64,
        offset_minutes: i16,
        spans: Vec<Span>,
        paused: bool,
        refreshed: i64,
        until: i64,
        now: Clocks,
    ) -> Self {
        let mut closed = Vec::with_capacity(spans.len() + 2);
        for span in spans {
            push_merged(&mut closed, span);
        }
        let kind = if paused { SpanKind::Pause } else { SpanKind::Work };
        let end = closed.last().map_or(0, |span| u32::try_from(span.end()).unwrap_or(u32::MAX));
        let until = until.max(refreshed);
        let alive = u32::try_from(until.min(now.wall).saturating_sub(refreshed)).unwrap_or(0);
        push_merged(&mut closed, Span::new(kind, end, alive, ClockSource::Wall));
        let end = end.saturating_add(alive);
        let unseen = u32::try_from(now.wall.saturating_sub(until)).unwrap_or(0);
        push_merged(&mut closed, Span::new(SpanKind::Gap, end, unseen, ClockSource::Wall));
        let offset = end.saturating_add(unseen);
        Self {
            focus,
            started,
            offset_minutes,
            closed,
            open: (kind, offset),
            origin: now.uptime,
            base: offset,
            last: now,
            last_offset: offset,
            paused,
            idle_from: None,
            away: false,
            flags: Vec::new(),
            ceiling: Self::DEFAULT_CEILING,
            tick: DEFAULT_TICK,
            detects_suspend: Uptime::detects_suspend(),
            source: Source::Recovered,
        }
    }

    /// Sets the work time past which the session is flagged; twelve hours unless set.
    #[must_use]
    pub fn ceiling(mut self, seconds: u32) -> Self {
        self.ceiling = seconds;
        self
    }

    /// The work time past which the session is flagged.
    #[must_use]
    pub fn ceiling_seconds(&self) -> u32 {
        self.ceiling
    }

    /// Sets how often a tick is expected, which decides how late a tick may be before the
    /// lateness counts as sleep; one second unless set. Zero is read as one.
    #[must_use]
    pub fn tick_interval(mut self, seconds: u32) -> Self {
        self.tick = seconds.max(1);
        self
    }

    /// Sets whether the readings tell sleep apart, which is [`Uptime::detects_suspend`] unless
    /// set. When they do not, a late tick is the only sign of sleep.
    #[must_use]
    pub fn detects_suspend(mut self, detects: bool) -> Self {
        self.detects_suspend = detects;
        self
    }

    /// Adds the flags a running file carried, each once, so a restored timer keeps what the run
    /// before it found out.
    #[must_use]
    pub fn with_flags(mut self, flags: Vec<Flag>) -> Self {
        for flag in flags {
            self.flag(flag);
        }
        self
    }

    /// Takes over the silence a running file says nobody answered for, which began at the session
    /// offset `from`. The run that was measuring it is gone, so the silence is over and waits for
    /// the person to say what it was. Nothing is taken over when no idle span lies at or after
    /// `from`: there would be nothing for the answer to change.
    #[must_use]
    pub fn with_unclaimed_idle(mut self, from: Option<u32>) -> Self {
        self.away = false;
        self.idle_from = from
            .filter(|from| self.closed.iter().any(|span| span.kind == SpanKind::Idle && span.end() > u64::from(*from)));
        self
    }

    /// Sets where the session it stops with is said to come from: recovered after a restore,
    /// measured by the timer after a start. A counter the command line stopped in the ordinary
    /// way is not a recovery, however it was rebuilt.
    #[must_use]
    pub fn source(mut self, source: Source) -> Self {
        self.source = source;
        self
    }

    /// Takes the reading `now` into account and reports what changed.
    pub fn tick(&mut self, now: Clocks) -> Vec<Notice> {
        let mut notices = Vec::new();
        let offset = self.offset_at(now);
        let step = offset.saturating_sub(self.last_offset);

        let (kind, from) = self.open;
        let asleep = if self.detects_suspend {
            let slept = whole_seconds(now.uptime.suspended_since(&self.last.uptime));
            if slept > 0 {
                notices.push(Notice::Suspended(slept, kind));
            }
            slept
        } else {
            let awake = whole_seconds(now.uptime.awake.saturating_sub(self.last.uptime.awake));
            let threshold = LAG_FLOOR.max(self.tick.saturating_mul(3));
            if awake > threshold {
                let late = awake.saturating_sub(self.tick);
                notices.push(Notice::Lagged(late, kind));
                late
            } else {
                0
            }
        };
        let asleep = asleep.min(step);
        if asleep > 0 {
            // A sleep counts as whatever was being measured when it began: work stays work, a
            // break a break, a silence a silence the person is asked about. It is its own span,
            // measured by the clock that runs through sleep, so the record keeps where it was.
            self.close(kind, from, self.last_offset);
            let after = self.last_offset.saturating_add(asleep);
            push_merged(&mut self.closed, Span::new(kind, self.last_offset, asleep, ClockSource::Boot));
            self.open = (kind, after);
        }

        let wall_step = now.wall.saturating_sub(self.last.wall);
        let drift = wall_step.saturating_sub(i64::from(step));
        if drift.abs() > CLOCK_DRIFT {
            notices.push(Notice::ClockJump(drift));
            self.flag(Flag::SuspectClock);
        }

        self.last = now;
        self.last_offset = offset;

        if self.work_seconds(now) > u64::from(self.ceiling) && !self.flags.contains(&Flag::OverCeiling) {
            notices.push(Notice::OverCeiling);
            self.flag(Flag::OverCeiling);
        }
        notices
    }

    /// Starts a break at `now`. Does nothing while already paused. The reading is ticked first,
    /// so anything it reveals is returned as from [`Timer::tick`].
    pub fn pause(&mut self, now: Clocks) -> Vec<Notice> {
        let notices = self.tick(now);
        if !self.paused {
            self.paused = true;
            self.switch(SpanKind::Pause);
        }
        notices
    }

    /// Ends a break at `now`. Does nothing while not paused. The reading is ticked first, so
    /// anything it reveals is returned as from [`Timer::tick`].
    pub fn resume(&mut self, now: Clocks) -> Vec<Notice> {
        let notices = self.tick(now);
        if self.paused {
            self.paused = false;
            self.switch(SpanKind::Work);
        }
        notices
    }

    /// Whether a break is on.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// No input has reached the terminal for `seconds_ago` seconds, as of `now`: from that
    /// moment on the work is idle until the input comes back, and the record is flagged until
    /// the person says what the silence was. The time is not cut; it is measured under another
    /// kind. Work spans in that tail are re-kinded, a break inside it stays what it was, and so
    /// does work counted through a sleep: the silence is measured on the awake clock, which says
    /// nothing about the time the machine slept. Nothing changes during a break or while a silence is already being measured. The
    /// reading is ticked first, so anything it reveals is returned as from [`Timer::tick`].
    pub fn idle_since(&mut self, now: Clocks, seconds_ago: u32) -> Vec<Notice> {
        let notices = self.tick(now);
        if self.paused || self.away {
            return notices;
        }
        let from = self.last_offset.saturating_sub(seconds_ago);
        self.rekind_from(from, SpanKind::Work, SpanKind::Idle, false);
        // The open span is work: the part before the silence stays so, the rest is idle.
        let (_, open_from) = self.open;
        if from > open_from {
            self.close(SpanKind::Work, open_from, from);
        }
        self.open = (SpanKind::Idle, open_from.max(from));
        self.idle_from = Some(from);
        self.away = true;
        self.flag(Flag::UnclaimedIdle);
        notices
    }

    /// Input came back at `now`: the idle span ends there and work goes on, while the stretch
    /// waits for [`Timer::claim_idle`]. Nothing changes when no silence was being measured. The
    /// reading is ticked first, so anything it reveals is returned as from [`Timer::tick`].
    pub fn idle_over(&mut self, now: Clocks) -> Vec<Notice> {
        let notices = self.tick(now);
        if self.away {
            self.away = false;
            self.switch(SpanKind::Work);
        }
        notices
    }

    /// The person said what the latest silence was: its idle spans take the kind of the
    /// decision. Counting it or making it a break clears the flag, unless an earlier silence was
    /// left out and still holds idle spans; leaving it out keeps the flag. A silence still being
    /// measured is ended first. `false` when there is nothing to claim. The reading is ticked
    /// first.
    pub fn claim_idle(&mut self, now: Clocks, decision: IdleDecision) -> bool {
        self.idle_over(now);
        let Some(from) = self.idle_from.take() else { return false };
        self.rekind_from(from, SpanKind::Idle, decision.kind(), true);
        if !self.closed.iter().any(|span| span.kind == SpanKind::Idle) {
            self.flags.retain(|flag| *flag != Flag::UnclaimedIdle);
        }
        true
    }

    /// Whether a silence is being measured right now.
    #[must_use]
    pub fn is_away(&self) -> bool {
        self.away
    }

    /// Whether a silence waits for the person to say what it was.
    #[must_use]
    pub fn has_unclaimed_idle(&self) -> bool {
        self.idle_from.is_some() && !self.away
    }

    /// Where the latest silence began, while it is being measured or waits for the person to say
    /// what it was; the running file keeps it so a restored counter can ask again.
    #[must_use]
    pub fn unclaimed_idle_from(&self) -> Option<u32> {
        self.idle_from
    }

    /// Seconds of the latest silence at `now`: the idle spans since it began, whether it is
    /// still going on or waits to be claimed; zero when there is none.
    #[must_use]
    pub fn idle_seconds(&self, now: Clocks) -> u32 {
        let Some(from) = self.idle_from else { return 0 };
        self.spans(now)
            .iter()
            .filter(|span| span.kind == SpanKind::Idle && span.offset >= from)
            .fold(0u32, |sum, span| sum.saturating_add(span.seconds))
    }

    /// The focus being measured.
    #[must_use]
    pub fn focus(&self) -> Id {
        self.focus
    }

    /// When the session began, in seconds since the Unix epoch.
    #[must_use]
    pub fn started(&self) -> i64 {
        self.started
    }

    /// Minutes the local time was ahead of UTC when the session began.
    #[must_use]
    pub fn offset_minutes(&self) -> i16 {
        self.offset_minutes
    }

    /// The flags gathered so far.
    #[must_use]
    pub fn flags(&self) -> &[Flag] {
        &self.flags
    }

    /// Every span up to `now`: the closed ones and the open one grown to `now`, with adjacent
    /// spans of the same kind merged. A sleep since the last tick is not visible until the
    /// next [`Timer::tick`].
    #[must_use]
    pub fn spans(&self, now: Clocks) -> Vec<Span> {
        let mut spans = self.closed.clone();
        let (kind, from) = self.open;
        let seconds = self.offset_at(now).saturating_sub(from);
        if seconds > 0 {
            push_merged(&mut spans, Span::new(kind, from, seconds, ClockSource::Mono));
        }
        spans
    }

    /// Seconds the session has lasted at `now`, sleep included, measured by the clock that keeps
    /// going through sleep.
    #[must_use]
    pub fn elapsed(&self, now: Clocks) -> u32 {
        self.offset_at(now)
    }

    /// Seconds of work at `now`: the sum of the work spans.
    #[must_use]
    pub fn work_seconds(&self, now: Clocks) -> u64 {
        work_seconds(&self.spans(now))
    }

    /// Ends the session at `now` as a record with `id`, written at `written`. Its end is the
    /// start plus the elapsed time, so the spans always fill it exactly.
    #[must_use]
    pub fn stop(mut self, now: Clocks, id: Id, written: i64) -> Session {
        self.tick(now);
        let elapsed = self.elapsed(now);
        Session {
            id,
            revision: 1,
            written,
            focus: self.focus,
            started: self.started,
            offset_minutes: self.offset_minutes,
            ended: self.started.saturating_add(i64::from(elapsed)),
            spans: self.spans(now),
            source: self.source,
            replaces: None,
            voids: None,
            continues: None,
            flags: self.flags,
            note: String::new(),
        }
    }

    /// The session offset of `now`, counted from the origin reading. A reading before the
    /// origin counts as the origin, and a reading behind the last one counts as the last one, so
    /// the offset never moves back.
    fn offset_at(&self, now: Clocks) -> u32 {
        let since = whole_seconds(now.uptime.elapsed.saturating_sub(self.origin.elapsed));
        self.base.saturating_add(since).max(self.last_offset)
    }

    /// Closes the open span of `kind` from `from` to `to` and starts a new one of `kind` there.
    fn close(&mut self, kind: SpanKind, from: u32, to: u32) {
        let seconds = to.saturating_sub(from);
        if seconds > 0 {
            push_merged(&mut self.closed, Span::new(kind, from, seconds, ClockSource::Mono));
        }
        self.open = (kind, to);
    }

    /// Closes the open span at the last reading and opens one of `kind` there.
    fn switch(&mut self, kind: SpanKind) {
        let (current, from) = self.open;
        self.close(current, from, self.last_offset);
        self.open = (kind, self.last_offset);
    }

    /// Gives every part of a closed span of kind `was` that lies at or after `from` the kind
    /// `becomes`, splitting a span the offset falls inside, and merges what comes out. Spans
    /// measured through a sleep are left alone unless `through_sleep` says otherwise. The open
    /// span is not touched.
    fn rekind_from(&mut self, from: u32, was: SpanKind, becomes: SpanKind, through_sleep: bool) {
        let mut rebuilt = Vec::with_capacity(self.closed.len() + 1);
        for span in self.closed.drain(..) {
            let untouched = span.clock == ClockSource::Boot && !through_sleep;
            if span.kind != was || untouched || span.end() <= u64::from(from) {
                push_merged(&mut rebuilt, span);
            } else if span.offset >= from {
                push_merged(&mut rebuilt, Span::new(becomes, span.offset, span.seconds, span.clock));
            } else {
                let before = from.saturating_sub(span.offset);
                push_merged(&mut rebuilt, Span::new(was, span.offset, before, span.clock));
                push_merged(&mut rebuilt, Span::new(becomes, from, span.seconds.saturating_sub(before), span.clock));
            }
        }
        self.closed = rebuilt;
    }

    /// Adds `flag` unless it is already there.
    fn flag(&mut self, flag: Flag) {
        if !self.flags.contains(&flag) {
            self.flags.push(flag);
        }
    }
}

/// Whole seconds of `duration`; a value too large for a span counts as the largest span.
fn whole_seconds(duration: Duration) -> u32 {
    u32::try_from(duration.as_secs()).unwrap_or(u32::MAX)
}

/// Appends `span` to `spans`, extending the last one instead when it is of the same kind and
/// measured by the same clock. Spans of different clocks stay apart, so work counted through a
/// sleep keeps its place beside the work measured awake.
fn push_merged(spans: &mut Vec<Span>, span: Span) {
    if span.seconds == 0 {
        return;
    }
    match spans.last_mut() {
        Some(last) if last.kind == span.kind && last.clock == span.clock && last.end() == u64::from(span.offset) => {
            last.seconds = last.seconds.saturating_add(span.seconds);
        }
        _ => spans.push(span),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::check;

    fn focus_id() -> Id {
        Id::new(1, 1)
    }
    const OFFSET: i16 = 180;
    const WALL: i64 = 1_758_186_000;

    /// A reading `awake` seconds of awake time and `asleep` seconds of sleep after boot, with a
    /// wall clock that has moved along with the session clock plus `skew` seconds.
    fn clocks(awake: u64, asleep: u64, skew: i64) -> Clocks {
        let awake = Duration::from_secs(awake);
        Clocks {
            wall: WALL + i64::try_from(awake.as_secs() + asleep).expect("fits") + skew,
            uptime: Uptime { awake, elapsed: awake + Duration::from_secs(asleep) },
        }
    }

    /// A reading `seconds` in, with nothing slept and the wall clock in step.
    fn at(seconds: u64) -> Clocks {
        clocks(seconds, 0, 0)
    }

    fn timer() -> Timer {
        Timer::start(focus_id(), at(0), OFFSET).detects_suspend(true)
    }

    fn assert_consistent(timer: &Timer, now: Clocks) {
        let spans = timer.spans(now);
        let problems = check(&spans, timer.elapsed(now));
        assert!(problems.is_empty(), "{problems:?} in {spans:?}");
    }

    fn work(offset: u32, seconds: u32) -> Span {
        Span::new(SpanKind::Work, offset, seconds, ClockSource::Mono)
    }

    fn pause(offset: u32, seconds: u32) -> Span {
        Span::new(SpanKind::Pause, offset, seconds, ClockSource::Mono)
    }

    /// A span of `kind` counted through a sleep, measured by the clock that keeps going in it.
    fn asleep(kind: SpanKind, offset: u32, seconds: u32) -> Span {
        Span::new(kind, offset, seconds, ClockSource::Boot)
    }

    fn slept_work(offset: u32, seconds: u32) -> Span {
        asleep(SpanKind::Work, offset, seconds)
    }

    #[test]
    fn a_fresh_timer_holds_no_time() {
        let timer = timer();
        assert_eq!(timer.elapsed(at(0)), 0);
        assert_eq!(timer.work_seconds(at(0)), 0);
        assert!(timer.spans(at(0)).is_empty());
        assert!(!timer.is_paused());
        assert_eq!(timer.focus(), focus_id());
        assert_eq!(timer.started(), WALL);
        assert_eq!(timer.offset_minutes(), OFFSET);
        assert!(timer.flags().is_empty());
        assert_consistent(&timer, at(0));
    }

    #[test]
    fn work_grows_with_the_awake_clock() {
        let mut timer = timer();
        for second in 1..=90 {
            assert!(timer.tick(at(second)).is_empty());
        }
        assert_eq!(timer.elapsed(at(90)), 90);
        assert_eq!(timer.work_seconds(at(90)), 90);
        assert_eq!(timer.spans(at(90)), vec![work(0, 90)]);
        assert_consistent(&timer, at(90));
    }

    #[test]
    fn readings_between_ticks_are_counted_from_the_origin_not_added_up() {
        let mut timer = timer();
        // Ticks that land just short of each second must not lose a second each.
        for tick in 1..=10u64 {
            let now = Clocks {
                wall: WALL + i64::try_from(tick).expect("fits"),
                uptime: Uptime {
                    awake: Duration::from_millis(tick * 1_000 - 1),
                    elapsed: Duration::from_millis(tick * 1_000 - 1),
                },
            };
            timer.tick(now);
        }
        assert_eq!(timer.elapsed(at(10)), 10);
        assert_consistent(&timer, at(10));
    }

    #[test]
    fn a_pause_is_its_own_span_and_does_not_count_as_work() {
        let mut timer = timer();
        timer.tick(at(60));
        timer.pause(at(60));
        assert!(timer.is_paused());
        timer.tick(at(90));
        timer.resume(at(120));
        assert!(!timer.is_paused());
        timer.tick(at(150));
        assert_eq!(timer.spans(at(150)), vec![work(0, 60), pause(60, 60), work(120, 30)]);
        assert_eq!(timer.work_seconds(at(150)), 90);
        assert_eq!(timer.elapsed(at(150)), 150);
        assert_consistent(&timer, at(150));
    }

    #[test]
    fn pausing_twice_or_resuming_while_running_changes_nothing() {
        let mut timer = timer();
        timer.resume(at(10));
        timer.pause(at(20));
        timer.pause(at(30));
        timer.resume(at(40));
        assert_eq!(timer.spans(at(50)), vec![work(0, 20), pause(20, 20), work(40, 10)]);
        assert_consistent(&timer, at(50));
    }

    #[test]
    fn sleep_while_working_counts_as_work_measured_through_the_sleep() {
        let mut timer = timer();
        timer.tick(at(300));
        // Five minutes of work, two hours of sleep, and the tick fires on waking.
        let now = clocks(300, 7_200, 0);
        assert_eq!(timer.tick(now), vec![Notice::Suspended(7_200, SpanKind::Work)]);
        assert_eq!(timer.spans(now), vec![work(0, 300), slept_work(300, 7_200)]);
        assert_eq!(timer.work_seconds(now), 7_500, "two hours and five minutes");
        assert!(timer.flags().is_empty(), "sleep is not a clock jump");
        assert_consistent(&timer, now);
        let later = clocks(360, 7_200, 0);
        timer.tick(later);
        assert_eq!(timer.spans(later), vec![work(0, 300), slept_work(300, 7_200), work(7_500, 60)]);
        assert_eq!(timer.elapsed(later), 7_560);
        assert_eq!(timer.work_seconds(later), 7_560);
        assert_consistent(&timer, later);
    }

    #[test]
    fn sleep_during_a_break_is_added_to_the_break() {
        let mut timer = timer();
        timer.tick(at(30));
        timer.pause(at(30));
        timer.tick(at(40));
        assert_eq!(timer.tick(clocks(41, 500, 0)), vec![Notice::Suspended(500, SpanKind::Pause)]);
        timer.tick(clocks(50, 500, 0));
        assert!(timer.is_paused());
        let now = clocks(50, 500, 0);
        let slept = asleep(SpanKind::Pause, 40, 500);
        assert_eq!(timer.spans(now), vec![work(0, 30), pause(30, 10), slept, pause(540, 10)]);
        assert_eq!(timer.work_seconds(now), 30);
        assert_consistent(&timer, now);
        timer.resume(now);
        timer.tick(clocks(60, 500, 0));
        let now = clocks(60, 500, 0);
        assert_eq!(timer.spans(now), vec![work(0, 30), pause(30, 10), slept, pause(540, 10), work(550, 10)]);
        assert_consistent(&timer, now);
    }

    #[test]
    fn two_sleeps_in_a_row_merge_into_one_span_counted_through_sleep() {
        let mut timer = timer();
        timer.tick(at(10));
        timer.tick(clocks(10, 100, 0));
        timer.tick(clocks(10, 250, 0));
        let now = clocks(20, 250, 0);
        assert_eq!(timer.spans(now), vec![work(0, 10), slept_work(10, 250), work(260, 10)]);
        assert_consistent(&timer, now);
    }

    #[test]
    fn without_suspend_detection_a_late_tick_is_counted_as_sleep_of_the_open_kind() {
        let mut timer = Timer::start(focus_id(), at(0), OFFSET).detects_suspend(false);
        timer.tick(at(1));
        timer.tick(at(2));
        // On such a platform both clocks read the same; the tick arrives thirty seconds late.
        let notices = timer.tick(at(32));
        assert_eq!(notices, vec![Notice::Lagged(29, SpanKind::Work)]);
        timer.tick(at(33));
        assert_eq!(timer.spans(at(33)), vec![work(0, 2), slept_work(2, 29), work(31, 2)]);
        assert_eq!(timer.elapsed(at(33)), 33);
        assert_eq!(timer.work_seconds(at(33)), 33, "the lateness is counted");
        assert_consistent(&timer, at(33));
    }

    #[test]
    fn without_suspend_detection_a_slightly_late_tick_is_still_work() {
        let mut timer = Timer::start(focus_id(), at(0), OFFSET).detects_suspend(false);
        timer.tick(at(1));
        assert!(timer.tick(at(6)).is_empty(), "five seconds is within the floor");
        assert_eq!(timer.spans(at(6)), vec![work(0, 6)]);
        assert_consistent(&timer, at(6));
    }

    #[test]
    fn the_lag_threshold_follows_the_tick_interval() {
        let mut timer = Timer::start(focus_id(), at(0), OFFSET).detects_suspend(false).tick_interval(10);
        assert!(timer.tick(at(30)).is_empty(), "three ticks late is within the threshold");
        let notices = timer.tick(at(61));
        assert_eq!(notices, vec![Notice::Lagged(21, SpanKind::Work)]);
        assert_eq!(timer.spans(at(61)), vec![work(0, 30), slept_work(30, 21), work(51, 10)]);
        assert_consistent(&timer, at(61));
    }

    #[test]
    fn with_suspend_detection_a_late_tick_is_only_work() {
        let mut timer = timer();
        timer.tick(at(1));
        assert!(timer.tick(at(60)).is_empty());
        assert_eq!(timer.spans(at(60)), vec![work(0, 60)]);
    }

    #[test]
    fn a_clock_jump_flags_the_session_once_and_removes_no_time() {
        let mut timer = timer();
        timer.tick(at(10));
        // The wall clock is stepped back ninety seconds by a time daemon.
        let notices = timer.tick(clocks(11, 0, -90));
        assert_eq!(notices, vec![Notice::ClockJump(-90)]);
        assert_eq!(timer.flags(), &[Flag::SuspectClock]);
        // It stays ninety seconds behind: no new jump, and still one flag.
        assert!(timer.tick(clocks(12, 0, -90)).is_empty());
        // Then it is stepped forward again.
        let notices = timer.tick(clocks(13, 0, 30));
        assert_eq!(notices, vec![Notice::ClockJump(120)]);
        assert_eq!(timer.flags(), &[Flag::SuspectClock]);
        assert_eq!(timer.elapsed(clocks(13, 0, 30)), 13);
        assert_eq!(timer.spans(clocks(13, 0, 30)), vec![work(0, 13)]);
        assert_consistent(&timer, clocks(13, 0, 30));
    }

    #[test]
    fn small_wall_drift_is_tolerated() {
        let mut timer = timer();
        assert!(timer.tick(clocks(1, 0, 5)).is_empty());
        assert!(timer.flags().is_empty());
        assert_eq!(timer.tick(clocks(2, 0, 11)), vec![Notice::ClockJump(6)]);
    }

    #[test]
    fn sleep_keeps_the_wall_clock_in_step() {
        let mut timer = timer();
        timer.tick(at(5));
        // The wall clock moved 600 seconds, and so did the elapsed clock: no jump.
        assert_eq!(timer.tick(clocks(6, 599, 0)), vec![Notice::Suspended(599, SpanKind::Work)]);
        assert!(timer.flags().is_empty());
    }

    #[test]
    fn passing_the_ceiling_flags_the_session_once() {
        let mut timer = timer().ceiling(100);
        assert!(timer.tick(at(100)).is_empty(), "the ceiling itself is not over it");
        assert_eq!(timer.tick(at(101)), vec![Notice::OverCeiling]);
        assert_eq!(timer.flags(), &[Flag::OverCeiling]);
        assert!(timer.tick(at(200)).is_empty());
        assert_eq!(timer.flags(), &[Flag::OverCeiling]);
    }

    #[test]
    fn only_work_counts_towards_the_ceiling() {
        let mut timer = timer().ceiling(100);
        timer.tick(at(50));
        timer.pause(at(50));
        assert!(timer.tick(at(300)).is_empty());
        timer.resume(at(300));
        assert!(timer.tick(at(350)).is_empty());
        assert_eq!(timer.tick(at(351)), vec![Notice::OverCeiling]);
    }

    #[test]
    fn readings_that_go_backwards_count_as_nothing() {
        let mut timer = timer();
        timer.tick(at(10));
        let earlier = Clocks {
            wall: WALL + 10,
            uptime: Uptime { awake: Duration::from_secs(3), elapsed: Duration::from_secs(3) },
        };
        assert!(timer.tick(earlier).is_empty());
        assert_eq!(timer.elapsed(earlier), 10);
        assert_eq!(timer.spans(earlier), vec![work(0, 10)]);
        timer.tick(at(12));
        assert_eq!(timer.spans(at(12)), vec![work(0, 12)]);
        assert_consistent(&timer, at(12));
    }

    #[test]
    fn a_reading_before_the_origin_counts_as_the_origin() {
        let timer = Timer::start(focus_id(), at(100), OFFSET);
        assert_eq!(timer.elapsed(at(40)), 0);
        assert!(timer.spans(at(40)).is_empty());
    }

    #[test]
    fn stop_writes_a_timer_session_that_holds_together() {
        let mut timer = timer();
        timer.tick(at(100));
        timer.pause(at(100));
        timer.resume(at(160));
        timer.tick(clocks(200, 300, 0));
        let now = clocks(260, 300, 0);
        let id = Id::new(5, 5);
        let session = timer.stop(now, id, WALL + 1_000);
        assert_eq!(session.id, id);
        assert_eq!(session.revision, 1);
        assert_eq!(session.written, WALL + 1_000);
        assert_eq!(session.focus, focus_id());
        assert_eq!(session.started, WALL);
        assert_eq!(session.offset_minutes, OFFSET);
        assert_eq!(session.ended, WALL + 560);
        assert_eq!(session.spans, vec![work(0, 100), pause(100, 60), slept_work(160, 300), work(460, 100)]);
        assert_eq!(session.source, Source::Timer);
        assert_eq!(session.replaces, None);
        assert_eq!(session.voids, None);
        assert_eq!(session.continues, None);
        assert!(session.flags.is_empty());
        assert!(session.note.is_empty());
        assert_eq!(session.work_seconds(), 500);
        assert!(check(&session.spans, 560).is_empty());
    }

    #[test]
    fn stop_takes_the_last_reading_into_account() {
        let mut timer = timer().ceiling(30);
        timer.tick(at(10));
        // Sleep and the ceiling are both found by the final reading, not a tick before it.
        let session = timer.stop(clocks(40, 100, 0), Id::new(5, 5), WALL + 140);
        assert_eq!(session.spans, vec![work(0, 10), slept_work(10, 100), work(110, 30)]);
        assert_eq!(session.ended, WALL + 140);
        assert_eq!(session.flags, vec![Flag::OverCeiling]);
    }

    #[test]
    fn restore_fills_the_unseen_time_with_a_wall_gap_and_carries_on() {
        let spans = vec![work(0, 300), pause(300, 60), work(360, 240)];
        // The program died 600 seconds in and comes back 900 seconds after its last save.
        let now = at(0);
        let mut timer = Timer::restore(focus_id(), WALL - 1_500, OFFSET, spans, false, WALL - 900, WALL - 900, now)
            .detects_suspend(true);
        assert_eq!(timer.elapsed(now), 1_500);
        assert_consistent(&timer, now);
        timer.tick(at(30));
        let expected = vec![
            work(0, 300),
            pause(300, 60),
            work(360, 240),
            Span::new(SpanKind::Gap, 600, 900, ClockSource::Wall),
            work(1_500, 30),
        ];
        assert_eq!(timer.spans(at(30)), expected);
        assert_consistent(&timer, at(30));
        let session = timer.stop(at(30), Id::new(5, 5), WALL + 30);
        assert_eq!(session.source, Source::Recovered);
        assert_eq!(session.started, WALL - 1_500);
        assert_eq!(session.ended, WALL + 30);
        assert!(check(&session.spans, 1_530).is_empty());
    }

    #[test]
    fn restore_counts_to_where_the_counter_was_alive_and_leaves_the_rest_out() {
        let spans = vec![work(0, 300)];
        // Saved 300 seconds in, alive for 600 more, then the machine was off for 900.
        let now = at(0);
        let mut timer = Timer::restore(focus_id(), WALL - 1_800, OFFSET, spans, false, WALL - 1_500, WALL - 900, now)
            .detects_suspend(true);
        assert_eq!(timer.elapsed(now), 1_800);
        timer.tick(at(30));
        let expected = vec![
            work(0, 300),
            Span::new(SpanKind::Work, 300, 600, ClockSource::Wall),
            Span::new(SpanKind::Gap, 900, 900, ClockSource::Wall),
            work(1_800, 30),
        ];
        assert_eq!(timer.spans(at(30)), expected);
        assert_eq!(timer.work_seconds(at(30)), 930);
        assert_consistent(&timer, at(30));
    }

    #[test]
    fn restore_of_a_paused_counter_alive_until_now_is_one_longer_break() {
        let spans = vec![work(0, 100), pause(100, 20)];
        let timer = Timer::restore(focus_id(), WALL - 200, OFFSET, spans, true, WALL - 80, WALL, at(0));
        let expected = vec![work(0, 100), pause(100, 20), Span::new(SpanKind::Pause, 120, 80, ClockSource::Wall)];
        assert_eq!(timer.spans(at(0)), expected, "no gap when it was alive all along");
        assert_eq!(timer.work_seconds(at(0)), 100);
    }

    #[test]
    fn restore_keeps_until_between_the_last_save_and_now() {
        let spans = vec![work(0, 100)];
        let early = Timer::restore(focus_id(), WALL - 200, OFFSET, spans.clone(), false, WALL - 100, WALL - 500, at(0));
        assert_eq!(
            early.spans(at(0)),
            vec![work(0, 100), Span::new(SpanKind::Gap, 100, 100, ClockSource::Wall)],
            "an until before the save is the save"
        );
        let late = Timer::restore(focus_id(), WALL - 200, OFFSET, spans, false, WALL - 100, WALL + 500, at(0));
        assert_eq!(late.spans(at(0)), vec![work(0, 100), Span::new(SpanKind::Work, 100, 100, ClockSource::Wall)]);
        assert_consistent(&late, at(0));
    }

    #[test]
    fn restore_keeps_the_flags_it_is_handed_once_and_the_source_it_is_told() {
        let now = at(0);
        let timer = Timer::restore(focus_id(), WALL - 100, OFFSET, vec![work(0, 100)], false, WALL, WALL, now)
            .with_flags(vec![Flag::SuspectClock, Flag::SuspectClock])
            .source(Source::Timer);
        let session = timer.stop(at(10), Id::new(5, 5), WALL + 10);
        assert_eq!(session.flags, vec![Flag::SuspectClock]);
        assert_eq!(session.source, Source::Timer);
        let plain = Timer::restore(focus_id(), WALL - 100, OFFSET, vec![work(0, 100)], false, WALL, WALL, now);
        assert_eq!(plain.stop(at(10), Id::new(6, 6), WALL + 10).source, Source::Recovered);
    }

    #[test]
    fn restore_of_a_paused_timer_stays_paused() {
        let spans = vec![work(0, 100), pause(100, 20)];
        let mut timer = Timer::restore(focus_id(), WALL - 200, OFFSET, spans, true, WALL - 80, WALL - 80, at(0))
            .detects_suspend(true);
        assert!(timer.is_paused());
        timer.tick(at(10));
        assert_eq!(
            timer.spans(at(10)),
            vec![work(0, 100), pause(100, 20), Span::new(SpanKind::Gap, 120, 80, ClockSource::Wall), pause(200, 10)]
        );
        assert_consistent(&timer, at(10));
        timer.resume(at(10));
        timer.tick(at(15));
        assert_eq!(timer.work_seconds(at(15)), 105);
        assert_consistent(&timer, at(15));
    }

    #[test]
    fn restore_right_after_the_last_save_opens_no_gap() {
        let spans = vec![work(0, 100)];
        let timer = Timer::restore(focus_id(), WALL - 100, OFFSET, spans, false, WALL, WALL, at(0));
        assert_eq!(timer.spans(at(5)), vec![work(0, 105)]);
        assert_consistent(&timer, at(5));
    }

    #[test]
    fn restore_with_a_wall_clock_behind_the_last_save_opens_no_gap() {
        let spans = vec![work(0, 100)];
        let timer = Timer::restore(focus_id(), WALL - 100, OFFSET, spans, false, WALL + 500, WALL + 500, at(0));
        assert_eq!(timer.spans(at(1)), vec![work(0, 101)]);
        assert_consistent(&timer, at(1));
    }

    #[test]
    fn restore_merges_adjacent_spans_of_the_same_kind() {
        let spans = vec![work(0, 40), work(40, 60)];
        let timer = Timer::restore(focus_id(), WALL - 100, OFFSET, spans, false, WALL, WALL, at(0));
        assert_eq!(timer.spans(at(0)), vec![work(0, 100)]);
    }

    #[test]
    fn restore_with_no_spans_starts_from_zero() {
        let mut timer = Timer::restore(focus_id(), WALL - 30, OFFSET, Vec::new(), false, WALL - 30, WALL - 30, at(0));
        timer.tick(at(5));
        assert_eq!(timer.spans(at(5)), vec![Span::new(SpanKind::Gap, 0, 30, ClockSource::Wall), work(30, 5)]);
        assert_consistent(&timer, at(5));
    }

    fn idle(offset: u32, seconds: u32) -> Span {
        Span::new(SpanKind::Idle, offset, seconds, ClockSource::Mono)
    }

    /// Fifteen minutes of work, then the silence is noticed at the twenty-fifth minute: the
    /// last fifteen are re-kinded, and the silence goes on to the thirtieth.
    fn away_timer() -> Timer {
        let mut timer = timer();
        timer.tick(at(600));
        timer.idle_since(at(1_500), 900);
        timer.tick(at(1_800));
        timer
    }

    #[test]
    fn silence_re_kinds_the_tail_as_idle_and_is_not_counted() {
        let timer = away_timer();
        assert!(timer.is_away());
        assert!(!timer.has_unclaimed_idle());
        assert_eq!(timer.spans(at(1_800)), vec![work(0, 600), idle(600, 1_200)]);
        assert_eq!(timer.work_seconds(at(1_800)), 600);
        assert_eq!(timer.elapsed(at(1_800)), 1_800, "the time is not cut");
        assert_eq!(timer.idle_seconds(at(1_800)), 1_200);
        assert_eq!(timer.flags(), &[Flag::UnclaimedIdle]);
        assert_consistent(&timer, at(1_800));
    }

    #[test]
    fn input_coming_back_ends_the_idle_span_and_leaves_the_question_open() {
        let mut timer = away_timer();
        timer.idle_over(at(2_100));
        timer.tick(at(2_400));
        assert!(!timer.is_away());
        assert!(timer.has_unclaimed_idle());
        assert_eq!(timer.spans(at(2_400)), vec![work(0, 600), idle(600, 1_500), work(2_100, 300)]);
        assert_eq!(timer.work_seconds(at(2_400)), 900);
        assert_eq!(timer.idle_seconds(at(2_400)), 1_500);
        assert_eq!(timer.flags(), &[Flag::UnclaimedIdle]);
        assert_consistent(&timer, at(2_400));
    }

    #[test]
    fn counting_the_silence_makes_it_work_and_clears_the_flag() {
        let mut timer = away_timer();
        timer.idle_over(at(2_100));
        assert!(timer.claim_idle(at(2_400), IdleDecision::Count));
        assert_eq!(timer.spans(at(2_400)), vec![work(0, 2_400)], "merged into one span");
        assert_eq!(timer.work_seconds(at(2_400)), 2_400);
        assert!(timer.flags().is_empty());
        assert!(!timer.has_unclaimed_idle());
        assert_eq!(timer.idle_seconds(at(2_400)), 0);
        assert_consistent(&timer, at(2_400));
        assert!(!timer.claim_idle(at(2_500), IdleDecision::Count), "nothing is left to claim");
    }

    #[test]
    fn leaving_the_silence_out_keeps_it_idle_and_flagged() {
        let mut timer = away_timer();
        timer.idle_over(at(2_100));
        assert!(timer.claim_idle(at(2_400), IdleDecision::DontCount));
        assert_eq!(timer.spans(at(2_400)), vec![work(0, 600), idle(600, 1_500), work(2_100, 300)]);
        assert_eq!(timer.flags(), &[Flag::UnclaimedIdle]);
        assert!(!timer.has_unclaimed_idle(), "answered, even if left out");
        assert_consistent(&timer, at(2_400));
    }

    #[test]
    fn turning_the_silence_into_a_break_makes_it_a_pause() {
        let mut timer = away_timer();
        timer.idle_over(at(2_100));
        assert!(timer.claim_idle(at(2_400), IdleDecision::Break));
        assert_eq!(timer.spans(at(2_400)), vec![work(0, 600), pause(600, 1_500), work(2_100, 300)]);
        assert!(timer.flags().is_empty());
        assert!(!timer.is_paused(), "history changed, not the state");
        assert_consistent(&timer, at(2_400));
    }

    #[test]
    fn a_break_taken_just_before_the_silence_stays_a_break() {
        let mut timer = timer();
        timer.tick(at(600));
        timer.pause(at(600));
        timer.resume(at(1_200));
        // Silence for fifteen minutes since the resume; the span before the break is untouched.
        timer.idle_since(at(2_100), 900);
        assert_eq!(timer.spans(at(2_100)), vec![work(0, 600), pause(600, 600), idle(1_200, 900)]);
        assert_consistent(&timer, at(2_100));
        // Reaching further back than the break: only the work parts turn idle.
        let mut timer = timer.clone();
        timer.idle_over(at(2_100));
        timer.claim_idle(at(2_100), IdleDecision::Count);
        timer.idle_since(at(2_400), 2_000);
        assert_eq!(timer.spans(at(2_400)), vec![work(0, 400), idle(400, 200), pause(600, 600), idle(1_200, 1_200)]);
        assert_consistent(&timer, at(2_400));
        timer.idle_over(at(2_400));
        assert!(timer.claim_idle(at(2_400), IdleDecision::Break));
        assert_eq!(timer.spans(at(2_400)), vec![work(0, 400), pause(400, 2_000)], "breaks merge");
        assert_consistent(&timer, at(2_400));
    }

    #[test]
    fn sleep_inside_the_silence_joins_it_and_the_claim_covers_the_sleep() {
        let mut timer = away_timer();
        assert_eq!(timer.tick(clocks(1_801, 3_000, 0)), vec![Notice::Suspended(3_000, SpanKind::Idle)]);
        timer.tick(clocks(2_100, 3_000, 0));
        let now = clocks(2_100, 3_000, 0);
        let slept = asleep(SpanKind::Idle, 1_800, 3_000);
        assert_eq!(timer.spans(now), vec![work(0, 600), idle(600, 1_200), slept, idle(4_800, 300)]);
        assert_eq!(timer.idle_seconds(now), 4_500, "the sleep is part of the silence");
        assert_eq!(timer.work_seconds(now), 600);
        assert_consistent(&timer, now);
        assert!(timer.claim_idle(now, IdleDecision::Count));
        assert_eq!(timer.spans(now), vec![work(0, 1_800), slept_work(1_800, 3_000), work(4_800, 300)]);
        assert_eq!(timer.work_seconds(now), 5_100);
        assert!(timer.flags().is_empty());
        assert_consistent(&timer, now);
    }

    #[test]
    fn a_silence_noticed_after_a_sleep_leaves_the_work_counted_through_it() {
        let mut timer = timer();
        timer.tick(at(300));
        timer.tick(clocks(300, 7_200, 0));
        let now = clocks(600, 7_200, 0);
        // The silence reaches back past the sleep, to the hundredth second.
        timer.idle_since(now, 7_700);
        assert_eq!(timer.spans(now), vec![work(0, 100), idle(100, 200), slept_work(300, 7_200), idle(7_500, 300)]);
        assert_eq!(timer.work_seconds(now), 7_300);
        assert_consistent(&timer, now);
        timer.idle_over(now);
        assert!(timer.claim_idle(now, IdleDecision::Break));
        assert_eq!(timer.spans(now), vec![work(0, 100), pause(100, 200), slept_work(300, 7_200), pause(7_500, 300)]);
        assert_consistent(&timer, now);
    }

    #[test]
    fn silence_during_a_break_changes_nothing() {
        let mut timer = timer();
        timer.tick(at(600));
        timer.pause(at(600));
        timer.idle_since(at(1_800), 900);
        assert!(!timer.is_away());
        assert!(timer.flags().is_empty());
        timer.idle_over(at(1_900));
        assert!(!timer.has_unclaimed_idle());
        assert_eq!(timer.spans(at(1_900)), vec![work(0, 600), pause(600, 1_300)]);
        assert_consistent(&timer, at(1_900));
    }

    #[test]
    fn a_second_silence_while_the_first_is_measured_changes_nothing() {
        let mut timer = away_timer();
        timer.idle_since(at(2_000), 900);
        assert_eq!(timer.spans(at(2_000)), vec![work(0, 600), idle(600, 1_400)]);
        assert_consistent(&timer, at(2_000));
    }

    #[test]
    fn an_earlier_silence_left_out_keeps_the_flag_when_a_later_one_is_counted() {
        let mut timer = away_timer();
        timer.idle_over(at(2_100));
        timer.claim_idle(at(2_100), IdleDecision::DontCount);
        timer.idle_since(at(3_300), 900);
        timer.idle_over(at(3_600));
        assert_eq!(timer.idle_seconds(at(3_600)), 1_200, "only the latest silence");
        assert!(timer.claim_idle(at(3_600), IdleDecision::Count));
        assert_eq!(timer.spans(at(3_600)), vec![work(0, 600), idle(600, 1_500), work(2_100, 1_500)]);
        assert_eq!(timer.flags(), &[Flag::UnclaimedIdle]);
        assert_consistent(&timer, at(3_600));
    }

    #[test]
    fn a_silence_longer_than_the_session_reaches_back_to_its_start() {
        let mut timer = timer();
        timer.idle_since(at(300), 900);
        assert_eq!(timer.spans(at(400)), vec![idle(0, 400)]);
        assert_eq!(timer.work_seconds(at(400)), 0);
        assert_consistent(&timer, at(400));
    }

    #[test]
    fn stopping_while_away_or_unanswered_records_the_idle_spans_and_the_flag() {
        let away = away_timer();
        let session = away.stop(at(2_000), Id::new(5, 5), WALL + 2_000);
        assert_eq!(session.spans, vec![work(0, 600), idle(600, 1_400)]);
        assert_eq!(session.flags, vec![Flag::UnclaimedIdle]);
        assert_eq!(session.work_seconds(), 600);
        assert!(check(&session.spans, 2_000).is_empty());
        let mut unanswered = away_timer();
        unanswered.idle_over(at(2_100));
        let session = unanswered.stop(at(2_400), Id::new(6, 6), WALL + 2_400);
        assert_eq!(session.spans, vec![work(0, 600), idle(600, 1_500), work(2_100, 300)]);
        assert_eq!(session.flags, vec![Flag::UnclaimedIdle]);
        assert!(check(&session.spans, 2_400).is_empty());
    }

    /// A counter restored from a file that says the silence from ten minutes in, fifteen minutes
    /// long, was never answered; the program died five minutes after input came back.
    fn restored_with_pending_idle() -> Timer {
        let spans = vec![work(0, 600), idle(600, 900), work(1_500, 300)];
        Timer::restore(focus_id(), WALL - 1_800, OFFSET, spans, false, WALL, WALL, at(0))
            .detects_suspend(true)
            .with_flags(vec![Flag::UnclaimedIdle])
            .with_unclaimed_idle(Some(600))
    }

    #[test]
    fn a_restored_unanswered_silence_is_asked_about_again() {
        let timer = restored_with_pending_idle();
        assert!(timer.has_unclaimed_idle());
        assert!(!timer.is_away());
        assert_eq!(timer.unclaimed_idle_from(), Some(600));
        assert_eq!(timer.idle_seconds(at(60)), 900);
        assert_eq!(timer.work_seconds(at(60)), 960);
        assert_consistent(&timer, at(60));
    }

    #[test]
    fn counting_a_restored_silence_makes_it_work_and_clears_the_flag() {
        let mut timer = restored_with_pending_idle();
        timer.tick(at(60));
        assert!(timer.claim_idle(at(60), IdleDecision::Count));
        assert_eq!(timer.spans(at(60)), vec![work(0, 1_860)]);
        assert!(timer.flags().is_empty());
        assert!(!timer.has_unclaimed_idle());
        assert_eq!(timer.unclaimed_idle_from(), None);
        assert_consistent(&timer, at(60));
        let session = timer.stop(at(60), Id::new(5, 5), WALL + 60);
        assert_eq!(session.work_seconds(), 1_860);
        assert!(session.flags.is_empty());
    }

    #[test]
    fn a_counter_saved_while_away_comes_back_with_the_silence_over() {
        let spans = vec![work(0, 600), idle(600, 1_200)];
        let mut timer = Timer::restore(focus_id(), WALL - 1_800, OFFSET, spans, false, WALL - 100, WALL - 100, at(0))
            .detects_suspend(true)
            .with_flags(vec![Flag::UnclaimedIdle])
            .with_unclaimed_idle(Some(600));
        assert!(!timer.is_away(), "the silence is over; nobody measures it any more");
        assert!(timer.has_unclaimed_idle());
        timer.tick(at(30));
        assert_eq!(
            timer.spans(at(30)),
            vec![
                work(0, 600),
                idle(600, 1_200),
                Span::new(SpanKind::Gap, 1_800, 100, ClockSource::Wall),
                work(1_900, 30)
            ]
        );
        assert_eq!(timer.idle_seconds(at(30)), 1_200, "the unseen time is not part of the silence");
        assert!(timer.claim_idle(at(30), IdleDecision::Break));
        assert_eq!(
            timer.spans(at(30)),
            vec![
                work(0, 600),
                pause(600, 1_200),
                Span::new(SpanKind::Gap, 1_800, 100, ClockSource::Wall),
                work(1_900, 30)
            ]
        );
        assert!(timer.flags().is_empty());
        assert_consistent(&timer, at(30));
    }

    #[test]
    fn nothing_is_asked_about_when_no_idle_span_lies_where_the_file_says() {
        let spans = vec![work(0, 600), idle(600, 300), work(900, 300)];
        let restore = || Timer::restore(focus_id(), WALL - 1_200, OFFSET, spans.clone(), false, WALL, WALL, at(0));
        assert!(!restore().with_unclaimed_idle(Some(900)).has_unclaimed_idle());
        assert!(!restore().with_unclaimed_idle(None).has_unclaimed_idle());
        assert!(restore().with_unclaimed_idle(Some(700)).has_unclaimed_idle(), "inside the silence still counts");
    }

    #[test]
    fn a_running_timer_says_where_the_unanswered_silence_began() {
        let mut timer = away_timer();
        assert_eq!(timer.unclaimed_idle_from(), Some(600), "while away");
        timer.idle_over(at(2_100));
        assert_eq!(timer.unclaimed_idle_from(), Some(600), "while waiting for the answer");
        timer.claim_idle(at(2_100), IdleDecision::DontCount);
        assert_eq!(timer.unclaimed_idle_from(), None);
    }

    #[test]
    fn a_session_far_past_the_largest_span_saturates_instead_of_wrapping() {
        let mut timer = timer();
        let far = Clocks { wall: WALL, uptime: Uptime { awake: Duration::MAX, elapsed: Duration::MAX } };
        timer.tick(far);
        assert_eq!(timer.elapsed(far), u32::MAX);
        assert_consistent(&timer, far);
    }
}
