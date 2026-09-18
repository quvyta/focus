//! The last moment something looked at a counter nobody measures.
//!
//! A counter the command line started has no process behind it, so nothing refreshes its running
//! file. The only proof that the machine was still on with the counter running is that `status`
//! or `today` looked at it; they write that moment here, in `seen-<machine>.toml`, beside the
//! running file. They do it without the lock, because a status bar asks every second and a `stop`
//! given at the same moment must not be refused, so two writers may race: each writes a moment
//! that is true, and the atomic write never leaves half a file. The counter's start is kept too,
//! so a stamp left by an earlier counter is never taken for this one's.
//!
//! The file is only a hint. One that is missing, broken or about another counter is no proof,
//! never an error anybody is told about.

use std::fs;
use std::io;
use std::path::Path;

use qframe::storage::atomic_write;
use toml::de::{DeTable, DeValue};

/// A moment a counter was seen alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    /// When the counter started, in seconds since the Unix epoch; says which counter was seen.
    pub started: i64,
    /// When it was seen, in seconds since the Unix epoch.
    pub seen: i64,
}

impl Seen {
    /// The stamp as TOML.
    #[must_use]
    pub fn write(&self) -> String {
        format!("started = {}\nseen = {}\n", self.started, self.seen)
    }

    /// Reads a stamp from `text`; `None` when either value is missing or is not a whole number.
    #[must_use]
    pub fn read(text: &str) -> Option<Self> {
        let root = DeTable::parse(text).ok()?;
        let table = root.get_ref();
        let integer = |key: &str| {
            let (_, value) = table.iter().find(|(name, _)| name.get_ref().as_ref() == key)?;
            match value.get_ref() {
                DeValue::Integer(number) => number.as_str().replace('_', "").parse::<i64>().ok(),
                _ => None,
            }
        };
        Some(Self { started: integer("started")?, seen: integer("seen")? })
    }
}

/// The stamp at `path`, or `None` when there is none or it cannot be read.
#[must_use]
pub fn load(path: &Path) -> Option<Seen> {
    Seen::read(&fs::read_to_string(path).ok()?)
}

/// Writes `seen` to `path` so that a crash leaves either the old stamp or the new one.
///
/// # Errors
///
/// The I/O error of the step that failed.
pub fn save(path: &Path, seen: &Seen) -> io::Result<()> {
    atomic_write(path, seen.write().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qfocus-test-seen-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("test folder");
        dir
    }

    #[test]
    fn a_stamp_is_written_and_read_back() {
        let dir = temp("round-trip");
        let path = dir.join("seen-test.toml");
        assert_eq!(load(&path), None, "no file, no proof");
        let seen = Seen { started: 1_789_725_600, seen: 1_789_729_200 };
        save(&path, &seen).expect("saves");
        assert_eq!(fs::read_to_string(&path).expect("reads"), "started = 1789725600\nseen = 1789729200\n");
        assert_eq!(load(&path), Some(seen));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_stamp_is_no_proof() {
        for text in ["", "started = 1\n", "seen = 2\n", "started = \"soon\"\nseen = 2\n", "started = 1\nseen = [\n"] {
            assert_eq!(Seen::read(text), None, "{text:?}");
        }
        assert_eq!(Seen::read("started = 1_000\nseen = 2_000\n"), Some(Seen { started: 1_000, seen: 2_000 }));
    }
}
