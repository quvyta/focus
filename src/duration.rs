//! How a length of time is written.
//!
//! Two shapes are needed. The timer screen shows a clock that widens on its own as the session
//! grows, so a short session is not padded with zeros it does not need. Lists and totals show at
//! most two units with the words the language file gives, because "2 h 20 min" is read at a
//! glance and "2 h 20 min 12 s" is not.

/// Seconds in a minute.
const MINUTE: u64 = 60;

/// Seconds in an hour.
const HOUR: u64 = 60 * MINUTE;

/// Seconds in a day.
const DAY: u64 = 24 * HOUR;

/// The words a language uses for hours, minutes and seconds.
///
/// They come from the language file; this module holds no text of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Units<'a> {
    /// The word after a number of hours.
    pub hour: &'a str,
    /// The word after a number of minutes.
    pub minute: &'a str,
    /// The word after a number of seconds.
    pub second: &'a str,
}

/// `seconds` as a clock that widens as it grows: `MM:SS`, then `H:MM:SS` past an hour, then
/// `D:HH:MM:SS` past a day.
///
/// ```
/// use qfocus::duration::clock;
///
/// assert_eq!(clock(47 * 60 + 12), "47:12");
/// assert_eq!(clock(3_600), "1:00:00");
/// assert_eq!(clock(90_000), "1:01:00:00");
/// ```
#[must_use]
pub fn clock(seconds: u64) -> String {
    let days = seconds / DAY;
    let hours = seconds % DAY / HOUR;
    let minutes = seconds % HOUR / MINUTE;
    let secs = seconds % MINUTE;
    if days > 0 {
        format!("{days}:{hours:02}:{minutes:02}:{secs:02}")
    } else if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes:02}:{secs:02}")
    }
}

/// `seconds` in words, with at most two units and no unit that is zero: `2 sa 20 dk`, `40 dk`,
/// `12 sn`. Seconds are dropped once there is an hour to show. Zero is written with the smallest
/// unit, so an empty total still reads as a time.
///
/// ```
/// use qfocus::duration::{Units, short};
///
/// let turkish = Units { hour: "sa", minute: "dk", second: "sn" };
/// assert_eq!(short(2 * 3_600 + 20 * 60, &turkish), "2 sa 20 dk");
/// assert_eq!(short(40 * 60, &turkish), "40 dk");
/// assert_eq!(short(0, &turkish), "0 sn");
/// ```
#[must_use]
pub fn short(seconds: u64, units: &Units<'_>) -> String {
    let hours = seconds / HOUR;
    let minutes = seconds % HOUR / MINUTE;
    let secs = seconds % MINUTE;
    let parts: Vec<(u64, &str)> = if hours > 0 {
        vec![(hours, units.hour), (minutes, units.minute)]
    } else if minutes > 0 {
        vec![(minutes, units.minute), (secs, units.second)]
    } else {
        vec![(secs, units.second)]
    };
    let words: Vec<String> = parts
        .iter()
        .enumerate()
        .filter(|(index, (amount, _))| *amount > 0 || *index == 0)
        .map(|(_, (amount, unit))| format!("{amount} {unit}"))
        .collect();
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TR: Units<'static> = Units { hour: "sa", minute: "dk", second: "sn" };

    #[test]
    fn clock_starts_as_minutes_and_seconds() {
        assert_eq!(clock(0), "00:00");
        assert_eq!(clock(5), "00:05");
        assert_eq!(clock(47 * 60 + 12), "47:12");
        assert_eq!(clock(59 * 60 + 59), "59:59");
    }

    #[test]
    fn clock_widens_past_an_hour() {
        assert_eq!(clock(3_600), "1:00:00");
        assert_eq!(clock(10 * 3_600 + 7 * 60 + 3), "10:07:03");
        assert_eq!(clock(23 * 3_600 + 59 * 60 + 59), "23:59:59");
    }

    #[test]
    fn clock_widens_past_a_day() {
        assert_eq!(clock(90_000), "1:01:00:00");
        assert_eq!(clock(86_400), "1:00:00:00");
        assert_eq!(clock(3 * 86_400 + 5), "3:00:00:05");
    }

    #[test]
    fn short_writes_at_most_two_units() {
        assert_eq!(short(2 * 3_600 + 20 * 60, &TR), "2 sa 20 dk");
        assert_eq!(short(2 * 3_600 + 20 * 60 + 45, &TR), "2 sa 20 dk");
        assert_eq!(short(40 * 60 + 12, &TR), "40 dk 12 sn");
    }

    #[test]
    fn short_skips_zero_units() {
        assert_eq!(short(40 * 60, &TR), "40 dk");
        assert_eq!(short(3_600, &TR), "1 sa");
        assert_eq!(short(3_661, &TR), "1 sa 1 dk");
        assert_eq!(short(12, &TR), "12 sn");
    }

    #[test]
    fn short_writes_zero_with_the_smallest_unit() {
        assert_eq!(short(0, &TR), "0 sn");
    }

    #[test]
    fn short_uses_whatever_words_it_is_given() {
        let english = Units { hour: "h", minute: "min", second: "s" };
        assert_eq!(short(3 * 3_600 + 5 * 60, &english), "3 h 5 min");
    }
}
