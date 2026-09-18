//! Permanent identifiers for categories, focuses and sessions.
//!
//! An identifier is 128 bits: a 48 bit millisecond stamp followed by 80 bits of randomness,
//! written as 26 Crockford base32 characters. Sorting identifiers sorts them by the moment they
//! were made. A counting identifier is not used: restoring a file from a backup would move the
//! counter back and a new focus would take the identifier of an archived one, which would attach
//! old sessions to the wrong name.

use std::fmt;
use std::hash::{BuildHasher, Hasher, RandomState};

/// Characters of Crockford base32, in value order.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Characters an identifier is written with.
const LENGTH: usize = 26;

/// Bits the randomness takes.
const RANDOM_BITS: u32 = 80;

/// A permanent identifier, sortable by the moment it was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(u128);

impl Id {
    /// An identifier for `millis` since the Unix epoch with the given `random` bits.
    ///
    /// Only the lowest 48 bits of `millis` and the lowest 80 bits of `random` are kept.
    #[must_use]
    pub fn new(millis: u64, random: u128) -> Self {
        let stamp = u128::from(millis) & ((1 << 48) - 1);
        let noise = random & ((1 << RANDOM_BITS) - 1);
        Self((stamp << RANDOM_BITS) | noise)
    }

    /// An identifier for `millis` with randomness from the operating system.
    ///
    /// The randomness comes from the seeds [`RandomState`] takes from the operating system. It is
    /// not meant to resist an attacker; it only has to keep two identifiers made in the same
    /// millisecond, on two machines, apart.
    #[must_use]
    pub fn generate(millis: u64) -> Self {
        let random = (u128::from(random_bits()) << 64) | u128::from(random_bits());
        Self::new(millis, random)
    }

    /// Milliseconds since the Unix epoch the identifier carries.
    #[must_use]
    pub fn millis(self) -> u64 {
        u64::try_from(self.0 >> RANDOM_BITS).unwrap_or(u64::MAX)
    }

    /// Reads an identifier, or `None` when `text` is not one.
    ///
    /// Letters may be lower case, and the Crockford aliases `O`, `I` and `L` are read as `0` and
    /// `1`, so an identifier copied by hand still works.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != LENGTH {
            return None;
        }
        let mut value: u128 = 0;
        for (index, byte) in text.bytes().enumerate() {
            let digit = digit_of(byte)?;
            // Twenty six digits carry 130 bits; the first digit may only fill the top two.
            if index == 0 && digit > 3 {
                return None;
            }
            value = (value << 5) | u128::from(digit);
        }
        Some(Self(value))
    }
}

impl fmt::Display for Id {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = [b'0'; LENGTH];
        let mut value = self.0;
        for slot in out.iter_mut().rev() {
            let index = usize::try_from(value & 0x1f).unwrap_or(0);
            *slot = ALPHABET[index];
            value >>= 5;
        }
        match std::str::from_utf8(&out) {
            Ok(text) => formatter.write_str(text),
            Err(_) => Err(fmt::Error),
        }
    }
}

/// The value of one Crockford base32 character, or `None` when it is not one.
fn digit_of(byte: u8) -> Option<u8> {
    match byte.to_ascii_uppercase() {
        b'O' => Some(0),
        b'I' | b'L' => Some(1),
        upper => ALPHABET.iter().position(|candidate| *candidate == upper).and_then(|index| u8::try_from(index).ok()),
    }
}

/// Sixty four bits from the operating system's seed for hashers.
fn random_bits() -> u64 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u8(0);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_twenty_six_characters_and_reads_them_back() {
        let id = Id::new(1_758_124_800_000, 0x0123_4567_89AB_CDEF_0123);
        let text = id.to_string();
        assert_eq!(text.len(), 26);
        assert_eq!(Id::parse(&text), Some(id));
    }

    #[test]
    fn keeps_the_millisecond_stamp() {
        let id = Id::new(1_758_124_800_000, 7);
        assert_eq!(id.millis(), 1_758_124_800_000);
    }

    #[test]
    fn sorts_by_time_then_randomness() {
        let earlier = Id::new(1_000, u128::MAX);
        let later = Id::new(1_001, 0);
        assert!(earlier < later);
    }

    #[test]
    fn rejects_wrong_length_and_unknown_letters() {
        assert_eq!(Id::parse("TOOSHORT"), None);
        assert_eq!(Id::parse("UUUUUUUUUUUUUUUUUUUUUUUUUU"), None);
    }

    #[test]
    fn accepts_lowercase_and_crockford_aliases() {
        let id = Id::new(1_758_124_800_000, 42);
        let text = id.to_string().to_lowercase();
        assert_eq!(Id::parse(&text), Some(id));
        assert_eq!(Id::parse("0000000000000000000000000i"), Id::parse("00000000000000000000000001"));
    }

    #[test]
    fn generated_ids_differ() {
        let first = Id::generate(1_758_124_800_000);
        let second = Id::generate(1_758_124_800_000);
        assert_ne!(first, second);
    }
}
