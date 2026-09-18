//! Where the data lives: the folder the framework gives and the files inside it.
//!
//! The running file and the lock file carry the machine name, so a data folder shared between two
//! machines through a file synchroniser keeps their running counters apart.

use std::path::{Path, PathBuf};

use qframe::date::DateTime;
use qframe::storage::data_dir;

/// The application's folder under the platform's data directory.
const APP: &str = "quvyta/focus";

/// The name used when neither the system nor the environment says what the machine is called.
const FALLBACK_MACHINE: &str = "machine";

/// The folders and files of one data store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// The data folder; everything else sits under it.
    root: PathBuf,
    /// The machine name that goes into per-machine file names.
    machine: String,
}

impl Paths {
    /// The paths under the platform's data directory, for this machine.
    ///
    /// `None` when the platform's variables name no data directory. The application then works
    /// in memory and tells the person so; it never invents a folder.
    #[must_use]
    pub fn detect() -> Option<Self> {
        data_dir(APP).map(|root| Self::at(root, machine_name()))
    }

    /// The paths under a given folder for a given machine name, for tests and the command line.
    ///
    /// The machine name is cleaned the same way [`machine_name`] cleans it, so it is always safe
    /// in a file name.
    pub fn at(root: impl Into<PathBuf>, machine: impl Into<String>) -> Self {
        Self { root: root.into(), machine: clean_machine_name(&machine.into()) }
    }

    /// The data folder.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The categories and focuses.
    #[must_use]
    pub fn tree_file(&self) -> PathBuf {
        self.root.join("tree.toml")
    }

    /// The folder of the month files.
    #[must_use]
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    /// The file sessions started in a given month are appended to, such as `sessions/2026-09.log`.
    #[must_use]
    pub fn month_file(&self, year: i32, month: u8) -> PathBuf {
        self.sessions_dir().join(format!("{year:04}-{month:02}.log"))
    }

    /// The folder removed sessions are copied to before they are voided.
    #[must_use]
    pub fn trash_dir(&self) -> PathBuf {
        self.root.join("trash")
    }

    /// The trash file of a month, named like the month file whose sessions it keeps copies of.
    #[must_use]
    pub fn trash_file(&self, year: i32, month: u8) -> PathBuf {
        self.trash_dir().join(format!("{year:04}-{month:02}.log"))
    }

    /// The folder exported files are written to.
    #[must_use]
    pub fn export_dir(&self) -> PathBuf {
        self.root.join("export")
    }

    /// The state of this machine's running counter.
    #[must_use]
    pub fn running_file(&self) -> PathBuf {
        self.root.join(format!("running-{}.toml", self.machine))
    }

    /// Where this machine's running file goes when it cannot be read, such as
    /// `running-laptop-broken-2026-09-18-150000.toml` for a `stamp` from [`broken_stamp`].
    ///
    /// It sits beside the running file, so moving it there is a rename within one folder: every
    /// byte is kept and nothing is copied. An `attempt` above 1 adds `-2`, `-3`, ... before the
    /// extension, so a second broken file in the same second does not take the first one's name.
    #[must_use]
    pub fn broken_running_file(&self, stamp: &str, attempt: u32) -> PathBuf {
        let suffix = if attempt > 1 { format!("-{attempt}") } else { String::new() };
        self.root.join(format!("running-{}-broken-{stamp}{suffix}.toml", self.machine))
    }

    /// When a command last saw this machine's counter alive, for a counter nobody measures; see
    /// [`crate::store::seen`].
    #[must_use]
    pub fn seen_file(&self) -> PathBuf {
        self.root.join(format!("seen-{}.toml", self.machine))
    }

    /// The file this machine's instances take the lock on.
    #[must_use]
    pub fn lock_file(&self) -> PathBuf {
        self.root.join(format!("qfocus-{}.lock", self.machine))
    }
}

/// A moment as `YYYY-MM-DD-HHMMSS` in local time, for [`Paths::broken_running_file`].
///
/// Local time, because the person looks for the file by when they opened the app; no `:`, because
/// some file systems refuse it in a name.
#[must_use]
pub fn broken_stamp(seconds: i64, offset_minutes: i16) -> String {
    let moment = DateTime::from_unix(seconds, offset_minutes);
    format!(
        "{:04}-{:02}-{:02}-{:02}{:02}{:02}",
        moment.date.year(),
        moment.date.month(),
        moment.date.day(),
        moment.time.hour,
        moment.time.minute,
        moment.time.second
    )
}

/// The name of this machine, safe for a file name.
///
/// The framework asks the system ([`qframe::storage::machine_name`]); a platform that gives
/// no name falls back to the `HOSTNAME` or `COMPUTERNAME` environment variable, whichever is
/// set first. Either way characters outside `A-Z a-z 0-9 _ -` become `-`, so the name is the
/// same on every file system, and an empty result is `machine`.
#[must_use]
pub fn machine_name() -> String {
    let raw = qframe::storage::machine_name()
        .or_else(|| ["HOSTNAME", "COMPUTERNAME"].iter().find_map(|name| std::env::var(name).ok()))
        .unwrap_or_default();
    clean_machine_name(&raw)
}

/// `raw` with everything a file name should not carry replaced by `-`, or `machine` when nothing
/// is left.
fn clean_machine_name(raw: &str) -> String {
    let cleaned: String =
        raw.trim()
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' || character == '-' { character } else { '-' }
            })
            .collect();
    if cleaned.is_empty() { FALLBACK_MACHINE.to_owned() } else { cleaned }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_sit_under_the_root() {
        let paths = Paths::at("/data/focus", "laptop");
        assert_eq!(paths.root(), Path::new("/data/focus"));
        assert_eq!(paths.tree_file(), PathBuf::from("/data/focus/tree.toml"));
        assert_eq!(paths.sessions_dir(), PathBuf::from("/data/focus/sessions"));
        assert_eq!(paths.month_file(2026, 9), PathBuf::from("/data/focus/sessions/2026-09.log"));
        assert_eq!(paths.trash_dir(), PathBuf::from("/data/focus/trash"));
        assert_eq!(paths.trash_file(2026, 9), PathBuf::from("/data/focus/trash/2026-09.log"));
        assert_eq!(paths.export_dir(), PathBuf::from("/data/focus/export"));
        assert_eq!(paths.running_file(), PathBuf::from("/data/focus/running-laptop.toml"));
        assert_eq!(paths.lock_file(), PathBuf::from("/data/focus/qfocus-laptop.lock"));
    }

    #[test]
    fn a_broken_running_file_is_set_aside_under_a_stamped_name_beside_it() {
        let paths = Paths::at("/data/focus", "laptop");
        // Noon UTC on 2026-09-18, three hours ahead: 15:00:00 local.
        let stamp = broken_stamp(1_789_732_800, 180);
        assert_eq!(stamp, "2026-09-18-150000");
        assert_eq!(
            paths.broken_running_file(&stamp, 1),
            PathBuf::from("/data/focus/running-laptop-broken-2026-09-18-150000.toml")
        );
        assert_eq!(
            paths.broken_running_file(&stamp, 2),
            PathBuf::from("/data/focus/running-laptop-broken-2026-09-18-150000-2.toml")
        );
        assert_eq!(
            paths.broken_running_file(&stamp, 13),
            PathBuf::from("/data/focus/running-laptop-broken-2026-09-18-150000-13.toml")
        );
        assert_eq!(broken_stamp(1_789_732_800 + 9 * 3_600 + 61, 0), "2026-09-18-210101");
    }

    #[test]
    fn the_machine_name_is_cleaned_for_a_file_name() {
        assert_eq!(clean_machine_name("my laptop.local"), "my-laptop-local");
        assert_eq!(clean_machine_name("  "), "machine");
        assert_eq!(clean_machine_name("ok_name-1"), "ok_name-1");
        assert_eq!(clean_machine_name("çay/ev"), "-ay-ev");
        let paths = Paths::at("/data", "a b");
        assert_eq!(paths.running_file(), PathBuf::from("/data/running-a-b.toml"));
    }

    #[test]
    fn machine_name_never_comes_back_empty() {
        let name = machine_name();
        assert!(!name.is_empty());
        assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'), "{name}");
    }

    #[test]
    fn the_system_name_comes_first_and_is_cleaned_the_same_way() {
        if let Some(system) = qframe::storage::machine_name() {
            assert_eq!(machine_name(), clean_machine_name(&system));
            let paths = Paths::at("/data", machine_name());
            assert_eq!(paths.running_file(), PathBuf::from(format!("/data/running-{}.toml", machine_name())));
            assert_eq!(paths.lock_file(), PathBuf::from(format!("/data/qfocus-{}.lock", machine_name())));
        }
    }
}
