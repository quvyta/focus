//! How far a running counter reaches: up to now, or up to the last moment it is known to have
//! been alive.
//!
//! Sleep counts; time the machine was off, or the program was not running, does not. A counter a
//! window measures is alive while that window is. A counter nobody measures, started from the
//! command line or left running by a window, is alive as long as the machine has not restarted
//! since the last proof that it was on: the file's `refreshed` stamp, or the moment a command last
//! looked at it ([`Seen`]).
//!
//! Nothing here reads a clock, a file or a lock. The running file, the stamp, the clocks and the
//! answer to "does a window hold the lock" all come in as values, so every case can be replayed.

use qframe::date::DateTime;
use qframe::t;

use crate::store::{Running, Seen, Watch};
use crate::timer::Clocks;

/// Seconds a window's refresh may be old before the window is asked whether it is still there.
/// A window refreshes every few seconds, so an older stamp means it is gone or the machine just
/// woke up and it has not ticked yet.
pub const WINDOW_STALE: i64 = 30;

/// Seconds the machine's boot may lie after the last proof before it counts as a restart. Wall
/// clock corrections move the computed boot moment a little; a machine that restarts within this
/// margin of the proof has at most this much of its off time counted.
pub const RESTART_MARGIN: i64 = 60;

/// Seconds between two stamps a command writes for a counter nobody measures, so a status bar
/// asking every second writes at most once a minute.
pub const SEEN_EVERY: i64 = 60;

/// Why a counter stopped counting before now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The machine restarted since the counter was last known alive.
    Restart,
    /// The window that measured the counter is gone.
    WindowGone,
}

impl Reason {
    /// The word used for it in JSON answers.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::WindowGone => "window-gone",
        }
    }
}

/// Where a counter stopped counting, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cut {
    /// The last moment it is known to have been alive, in seconds since the Unix epoch.
    pub at: i64,
    /// Why it counts no further.
    pub reason: Reason,
}

/// How far a counter reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reach {
    /// The wall second the counter counts to; never before its `refreshed` stamp.
    pub until: i64,
    /// Where and why it was cut, when it does not reach now.
    pub cut: Option<Cut>,
}

impl Reach {
    /// A counter alive up to `now`.
    fn alive(running: &Running, now: i64) -> Self {
        Self { until: now.max(running.refreshed), cut: None }
    }

    /// A counter that stopped at `at` because of `reason`.
    fn cut(at: i64, reason: Reason) -> Self {
        Self { until: at, cut: Some(Cut { at, reason }) }
    }
}

/// How far `running` reaches at `now`.
///
/// `seen` is the stamp a command left, used only when it belongs to this counter. `knows_boot`
/// says whether `now.uptime.elapsed` counts from the machine's boot, sleep included, which is
/// what makes a restart visible ([`qframe::uptime::Uptime::detects_suspend`]); where it does not,
/// a counter nobody measures counts up to now, as other command line trackers do. `window_alive`
/// answers whether a window still holds the lock; it is asked only for a counter a window
/// measures whose last refresh is more than [`WINDOW_STALE`] seconds old.
pub fn reach(
    running: &Running,
    seen: Option<&Seen>,
    now: Clocks,
    knows_boot: bool,
    window_alive: impl FnOnce() -> bool,
) -> Reach {
    match running.watch {
        Watch::Window => {
            if now.wall.saturating_sub(running.refreshed) <= WINDOW_STALE || window_alive() {
                return Reach::alive(running, now.wall);
            }
            window_gone(running, now, knows_boot)
        }
        Watch::None => {
            let proof = last_proof(running, seen);
            if restarted(proof, now, knows_boot) {
                Reach::cut(proof, Reason::Restart)
            } else {
                Reach::alive(running, now.wall)
            }
        }
    }
}

/// How far `running` reaches at `now` for an instance that holds the lock, so knows no window
/// measures the counter: one that did ended at its last refresh, however recent. A counter
/// nobody measures reaches as far as [`reach`] says.
#[must_use]
pub fn reach_without_window(running: &Running, seen: Option<&Seen>, now: Clocks, knows_boot: bool) -> Reach {
    match running.watch {
        Watch::Window => window_gone(running, now, knows_boot),
        Watch::None => reach(running, seen, now, knows_boot, || false),
    }
}

/// A counter whose window is gone ends at its last refresh; the restart is named when there was
/// one, as it says more than a window that went away.
fn window_gone(running: &Running, now: Clocks, knows_boot: bool) -> Reach {
    let reason = if restarted(running.refreshed, now, knows_boot) { Reason::Restart } else { Reason::WindowGone };
    Reach::cut(running.refreshed, reason)
}

/// The stamp a command looking at `running` at `now` should leave, when it should leave one: the
/// counter is one nobody measures, it reaches now on a machine that can tell a restart, and the
/// last proof is at least [`SEEN_EVERY`] seconds old.
#[must_use]
pub fn stamp_to_leave(
    running: &Running,
    seen: Option<&Seen>,
    reach: &Reach,
    now: i64,
    knows_boot: bool,
) -> Option<Seen> {
    let due = now.saturating_sub(last_proof(running, seen)) >= SEEN_EVERY;
    (running.watch == Watch::None && knows_boot && reach.cut.is_none() && due)
        .then_some(Seen { started: running.started, seen: now })
}

/// A cut moment in the words the person reads: the local time as `HH:MM`, with the day and
/// month in front when it lies on another local day than `now`. Both are read in the offset
/// given, the one in force where the person is now.
#[must_use]
pub fn moment_words(at: i64, now: i64, offset_minutes: i16) -> String {
    let moment = DateTime::from_unix(at, offset_minutes);
    let time = format!("{:02}:{:02}", moment.time.hour, moment.time.minute);
    if moment.date == DateTime::from_unix(now, offset_minutes).date {
        return time;
    }
    let date = moment.date;
    let day = t!("charts.day-of-month", day = u32::from(date.day()), month = t!(&format!("months.{}", date.month())));
    format!("{day} {time}")
}

/// The last moment `running` is known to have been alive: its refresh, or a later stamp that
/// belongs to it.
fn last_proof(running: &Running, seen: Option<&Seen>) -> i64 {
    seen.filter(|seen| seen.started == running.started && seen.seen >= running.refreshed)
        .map_or(running.refreshed, |seen| seen.seen)
}

/// Whether the machine booted more than [`RESTART_MARGIN`] seconds after `proof`. The boot moment
/// is the wall clock minus the time since boot; without that clock nothing can be said.
fn restarted(proof: i64, now: Clocks, knows_boot: bool) -> bool {
    if !knows_boot {
        return false;
    }
    let since_boot = i64::try_from(now.uptime.elapsed.as_secs()).unwrap_or(i64::MAX);
    let boot = now.wall.saturating_sub(since_boot);
    boot > proof.saturating_add(RESTART_MARGIN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::time::Duration;

    use qframe::uptime::Uptime;

    use crate::id::Id;
    use crate::span::{ClockSource, Span, SpanKind};

    /// 2026-09-18 10:00:00 UTC, when the counter started.
    const STARTED: i64 = 1_789_725_600;

    fn counter(watch: Watch, refreshed: i64) -> Running {
        Running {
            focus: Id::new(1, 1),
            started: STARTED,
            offset_minutes: 180,
            spans: vec![Span::new(SpanKind::Work, 0, 60, ClockSource::Mono)],
            paused: false,
            refreshed,
            flags: Vec::new(),
            target: None,
            idle_from: None,
            watch,
        }
    }

    /// The wall clock at `wall` on a machine that booted at `boot`.
    fn clocks(wall: i64, boot: i64) -> Clocks {
        let since = Duration::from_secs(u64::try_from(wall - boot).expect("after boot"));
        Clocks { wall, uptime: Uptime { awake: since, elapsed: since } }
    }

    /// A window check that must not be asked.
    fn never() -> bool {
        panic!("the window was asked about")
    }

    const HOUR: i64 = 3_600;

    #[test]
    fn a_counter_nobody_measures_counts_to_now_on_the_same_boot() {
        let running = counter(Watch::None, STARTED);
        let now = STARTED + 5 * HOUR;
        let reach = reach(&running, None, clocks(now, STARTED - HOUR), true, never);
        assert_eq!(reach, Reach { until: now, cut: None });
    }

    #[test]
    fn a_counter_nobody_measures_stops_at_its_refresh_when_the_machine_restarted() {
        let running = counter(Watch::None, STARTED);
        let now = STARTED + 20 * HOUR;
        let reach = reach(&running, None, clocks(now, STARTED + 10 * HOUR), true, never);
        assert_eq!(reach, Reach { until: STARTED, cut: Some(Cut { at: STARTED, reason: Reason::Restart }) });
    }

    #[test]
    fn a_later_stamp_of_the_same_counter_moves_the_cut() {
        let running = counter(Watch::None, STARTED);
        let seen = Seen { started: STARTED, seen: STARTED + 2 * HOUR };
        let reach = reach(&running, Some(&seen), clocks(STARTED + 20 * HOUR, STARTED + 10 * HOUR), true, never);
        assert_eq!(reach.until, STARTED + 2 * HOUR);
        assert_eq!(reach.cut.map(|cut| cut.reason), Some(Reason::Restart));
    }

    #[test]
    fn a_stamp_of_another_counter_or_older_than_the_refresh_is_ignored() {
        let running = counter(Watch::None, STARTED + HOUR);
        let now = clocks(STARTED + 20 * HOUR, STARTED + 10 * HOUR);
        let foreign = Seen { started: STARTED - HOUR, seen: STARTED + 5 * HOUR };
        assert_eq!(reach(&running, Some(&foreign), now, true, never).until, STARTED + HOUR);
        let stale = Seen { started: STARTED, seen: STARTED + 30 };
        assert_eq!(reach(&running, Some(&stale), now, true, never).until, STARTED + HOUR);
    }

    #[test]
    fn a_boot_within_the_margin_of_the_proof_is_not_a_restart() {
        let running = counter(Watch::None, STARTED);
        let now = STARTED + HOUR;
        assert_eq!(reach(&running, None, clocks(now, STARTED + RESTART_MARGIN), true, never).cut, None);
        assert!(reach(&running, None, clocks(now, STARTED + RESTART_MARGIN + 1), true, never).cut.is_some());
    }

    #[test]
    fn without_knowing_the_boot_a_counter_nobody_measures_counts_to_now() {
        let running = counter(Watch::None, STARTED);
        let now = STARTED + 20 * HOUR;
        let reach = reach(&running, None, clocks(now, STARTED + 10 * HOUR), false, never);
        assert_eq!(reach, Reach { until: now, cut: None });
    }

    #[test]
    fn a_window_refreshed_lately_is_alive_without_asking() {
        let running = counter(Watch::Window, STARTED);
        let now = STARTED + WINDOW_STALE;
        assert_eq!(reach(&running, None, clocks(now, STARTED - HOUR), true, never), Reach { until: now, cut: None });
    }

    #[test]
    fn a_stale_window_is_asked_and_stops_at_its_refresh_when_gone() {
        let running = counter(Watch::Window, STARTED);
        let now = STARTED + HOUR;
        let asked = Cell::new(false);
        let reach = reach(&running, None, clocks(now, STARTED - HOUR), true, || {
            asked.set(true);
            false
        });
        assert!(asked.get());
        assert_eq!(reach, Reach { until: STARTED, cut: Some(Cut { at: STARTED, reason: Reason::WindowGone }) });
    }

    #[test]
    fn a_stale_window_that_still_holds_the_lock_counts_to_now() {
        // The machine slept an hour and the window has not ticked since waking.
        let running = counter(Watch::Window, STARTED);
        let now = STARTED + HOUR;
        assert_eq!(reach(&running, None, clocks(now, STARTED - HOUR), true, || true), Reach { until: now, cut: None });
    }

    #[test]
    fn an_instance_with_the_lock_ends_a_window_counter_at_its_refresh_however_fresh() {
        let window = counter(Watch::Window, STARTED);
        let now = clocks(STARTED + 5, STARTED - HOUR);
        let cut = Some(Cut { at: STARTED, reason: Reason::WindowGone });
        assert_eq!(reach_without_window(&window, None, now, true), Reach { until: STARTED, cut });
        let alone = counter(Watch::None, STARTED);
        assert_eq!(reach_without_window(&alone, None, now, true), Reach { until: STARTED + 5, cut: None });
    }

    #[test]
    fn a_window_gone_with_a_restart_says_the_restart() {
        let running = counter(Watch::Window, STARTED);
        let reach = reach(&running, None, clocks(STARTED + 20 * HOUR, STARTED + 10 * HOUR), true, || false);
        assert_eq!(reach.cut, Some(Cut { at: STARTED, reason: Reason::Restart }));
    }

    #[test]
    fn a_wall_clock_behind_the_refresh_reaches_the_refresh() {
        let running = counter(Watch::None, STARTED);
        let reach = reach(&running, None, clocks(STARTED - 100, STARTED - HOUR), true, never);
        assert_eq!(reach, Reach { until: STARTED, cut: None });
    }

    #[test]
    fn a_cut_on_the_same_day_is_a_time_and_on_another_day_carries_the_date() {
        // 11:30 at +03:00 on the 18th; asked at 23:00 the same day, then at noon the next.
        let at = STARTED - 5_400;
        assert_eq!(moment_words(at, STARTED + 10 * HOUR, 180), "11:30");
        let mut i18n = qframe::i18n::I18n::builtin();
        for (file, text) in crate::locales() {
            i18n.add_source(file, text);
        }
        i18n.set_active("tr");
        let words = qframe::i18n::scope(std::sync::Arc::new(i18n), || moment_words(at, STARTED + 23 * HOUR, 180));
        assert_eq!(words, "18 Eylül 11:30");
    }

    #[test]
    fn a_stamp_is_left_at_most_once_a_minute_and_only_for_a_live_counter_nobody_measures() {
        let running = counter(Watch::None, STARTED);
        let boot = STARTED - HOUR;
        let alive = |now: i64| reach(&running, None, clocks(now, boot), true, never);
        let now = STARTED + SEEN_EVERY - 1;
        assert_eq!(stamp_to_leave(&running, None, &alive(now), now, true), None, "the start is proof enough");
        let now = STARTED + SEEN_EVERY;
        let stamp = stamp_to_leave(&running, None, &alive(now), now, true);
        assert_eq!(stamp, Some(Seen { started: STARTED, seen: now }));
        let fresh = Seen { started: STARTED, seen: now };
        let later = now + 30;
        assert_eq!(stamp_to_leave(&running, Some(&fresh), &alive(later), later, true), None);
        let later = now + SEEN_EVERY;
        assert!(stamp_to_leave(&running, Some(&fresh), &alive(later), later, true).is_some());
        assert_eq!(stamp_to_leave(&running, None, &alive(later), later, false), None, "no boot, no use");
        let window = counter(Watch::Window, STARTED);
        assert_eq!(stamp_to_leave(&window, None, &alive(later), later, true), None);
        let cut = Reach::cut(STARTED, Reason::Restart);
        assert_eq!(stamp_to_leave(&running, None, &cut, later, true), None);
    }
}
