//! The real clocks, read in one place.
//!
//! The timer and the records never read a clock themselves; they take readings as values so
//! tests can hand them any moment they like. This is the only module that asks the operating
//! system what time it is.

use std::time::{SystemTime, UNIX_EPOCH};

use qframe::uptime::Uptime;

use crate::id::Id;
use crate::timer::Clocks;

/// The wall clock and both monotonic clocks, right now.
///
/// A wall clock standing before 1970 gives a negative stamp rather than a panic; the timer
/// then flags the session as `suspect-clock` because the reading disagrees with the monotonic
/// clocks, which is exactly what such a machine deserves.
#[must_use]
pub fn now() -> Clocks {
    Clocks { wall: unix_seconds(), uptime: Uptime::now() }
}

/// A fresh identifier stamped with the wall clock.
#[must_use]
pub fn new_id() -> Id {
    Id::generate(unix_millis())
}

/// Seconds since 1970-01-01 00:00 UTC; negative before it.
fn unix_seconds() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_secs()).unwrap_or(i64::MAX).saturating_neg(),
    }
}

/// Milliseconds since 1970-01-01 00:00 UTC; zero before it, so an identifier still sorts.
fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reading_is_after_the_epoch_and_the_clocks_agree_with_themselves() {
        let reading = now();
        assert!(reading.wall > 1_700_000_000, "the machine's clock is not set: {}", reading.wall);
        assert!(reading.uptime.elapsed >= reading.uptime.awake);
    }

    #[test]
    fn identifiers_carry_the_current_time_and_differ() {
        let first = new_id();
        let second = new_id();
        assert_ne!(first, second);
        assert!(first.millis() > 1_700_000_000_000);
    }
}
