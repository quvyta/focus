//! The data on disk as one thing: the tree, every session, the lock and what went wrong loading.
//!
//! Opening a store never fails. A missing folder is an empty store, a broken file is skipped and
//! reported, and a lock another instance holds makes this one read-only rather than stopping it.

pub mod paths;
pub mod purge;
pub mod running;
pub mod seen;
pub mod sessions;

use std::fs;
use std::io;
use std::path::PathBuf;

use qframe::diagnostics::Diagnostic;
use qframe::storage::{AppLock, atomic_write, holder_pid};

pub use paths::{Paths, broken_stamp, machine_name};
pub use purge::{PurgePlan, Purged};
pub use running::{Running, Watch};
pub use seen::Seen;
pub use sessions::{AppendError, Loaded, month_of};

use crate::clock;
use crate::id::Id;
use crate::session::Session;
use crate::session::resolve::{Resolution, newest_revision, resolve};
use crate::tree::Tree;

/// Whether this instance may write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// This instance holds the lock and writes.
    Writer,
    /// Another instance holds the lock; this one only reads until [`Store::retry_lock`] wins.
    ReadOnly {
        /// The process id the lock file names, for a message only; it may be stale.
        holder: Option<u32>,
    },
    /// This platform has no advisory lock in the framework. The instance writes, and the person
    /// is told that a second instance would not be noticed.
    NoLock,
}

impl Access {
    /// Whether writing is allowed.
    #[must_use]
    pub fn can_write(self) -> bool {
        !matches!(self, Self::ReadOnly { .. })
    }
}

/// Everything on disk, loaded.
pub struct Store {
    /// Where it all is.
    pub paths: Paths,
    /// The categories and focuses.
    pub tree: Tree,
    /// The sessions to show, one per chain of corrections.
    pub sessions: Vec<Session>,
    /// Sessions in the trash: removed, each as its last version, waiting to be brought back.
    pub voided: Vec<Session>,
    /// Corrections of records that are not on disk, kept so nothing is dropped in silence.
    pub orphans: Vec<Session>,
    /// Chains two records claim to be the newest version of, for a person to sort out.
    pub forks: Vec<Id>,
    /// Everything that went wrong loading and resolving, for the diagnostics list.
    pub diagnostics: Vec<Diagnostic>,
    /// Session lines a newer qfocus wrote. They are not broken; this version cannot read them.
    pub unknown_version: usize,
    /// Whether this instance may write.
    pub access: Access,
    /// Every record read from the month files, unresolved, so a new one can be resolved with them.
    records: Vec<Session>,
    /// What loading the files reported, kept apart so resolving again does not repeat it.
    file_diagnostics: Vec<Diagnostic>,
    /// Held for as long as this instance writes. Never read: holding it is the point.
    lock: Option<AppLock>,
}

impl Store {
    /// Opens the store at `paths` and takes the lock.
    ///
    /// Never fails: a missing folder is an empty store, broken files are reported in
    /// `diagnostics`, and a lock another instance holds makes this one [`Access::ReadOnly`]. The
    /// folder is created so the lock file has somewhere to be; if even that fails, the store is
    /// read-only and says why.
    #[must_use]
    pub fn open(paths: Paths) -> Self {
        let mut store = Self::load(paths);
        store.retry_lock();
        store
    }

    /// Opens the store at `paths` without trying the lock, for a look that never writes.
    #[must_use]
    pub fn open_read_only(paths: Paths) -> Self {
        Self::load(paths)
    }

    /// Reads the files at `paths` into a read-only store.
    fn load(paths: Paths) -> Self {
        let mut file_diagnostics = Vec::new();
        let tree_file = paths.tree_file();
        let tree = match fs::read_to_string(&tree_file) {
            Ok(text) => {
                let (tree, diagnostics) = Tree::read(&tree_file.display().to_string(), &text);
                file_diagnostics.extend(diagnostics);
                tree
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Tree::default(),
            Err(error) => {
                let message = format!("{}: cannot read the file: {error}", tree_file.display());
                file_diagnostics.push(Diagnostic::error(None, message));
                Tree::default()
            }
        };
        let loaded = sessions::load_all(&paths.sessions_dir());
        file_diagnostics.extend(loaded.diagnostics);
        let holder = holder_pid(&paths.lock_file());
        let mut store = Self {
            paths,
            tree,
            sessions: Vec::new(),
            voided: Vec::new(),
            orphans: Vec::new(),
            forks: Vec::new(),
            diagnostics: Vec::new(),
            unknown_version: loaded.unknown_version,
            access: Access::ReadOnly { holder },
            records: loaded.records,
            file_diagnostics,
            lock: None,
        };
        store.refresh();
        store
    }

    /// Resolves the records again and rebuilds the diagnostics list from the file ones.
    fn refresh(&mut self) {
        let Resolution { sessions, voided, orphans, forks, diagnostics } = resolve(self.records.clone());
        self.sessions = sessions;
        self.voided = voided;
        self.orphans = orphans;
        self.forks = forks;
        self.diagnostics = self.file_diagnostics.clone();
        self.diagnostics.extend(diagnostics);
    }

    /// Tries the lock again and answers whether this instance may write now.
    ///
    /// An instance that already writes answers `true` at once. A lock that cannot be taken
    /// because of an I/O error, the folder being unwritable among them, leaves the store
    /// read-only and adds the error to `diagnostics` once.
    pub fn retry_lock(&mut self) -> bool {
        if self.access.can_write() {
            return true;
        }
        let lock_file = self.paths.lock_file();
        let attempt = fs::create_dir_all(self.paths.root()).and_then(|()| AppLock::acquire(&lock_file));
        match attempt {
            Ok(Some(lock)) => {
                self.lock = Some(lock);
                self.access = Access::Writer;
                true
            }
            Ok(None) => {
                self.access = Access::ReadOnly { holder: holder_pid(&lock_file) };
                false
            }
            Err(error) if error.kind() == io::ErrorKind::Unsupported => {
                self.access = Access::NoLock;
                true
            }
            Err(error) => {
                let message = format!("{}: cannot take the lock: {error}", lock_file.display());
                if !self.diagnostics.iter().any(|existing| existing.message == message) {
                    self.file_diagnostics.push(Diagnostic::error(None, message));
                    self.refresh();
                }
                self.access = Access::ReadOnly { holder: holder_pid(&lock_file) };
                false
            }
        }
    }

    /// Writes the tree to disk in one piece.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::PermissionDenied`] when this instance is read-only, else the error of the
    /// step that failed; the file that was there is untouched.
    pub fn save_tree(&self) -> io::Result<()> {
        self.writable()?;
        fs::create_dir_all(self.paths.root())?;
        atomic_write(&self.paths.tree_file(), self.tree.write().as_bytes())
    }

    /// Appends `session` to its month file and resolves the sessions again.
    ///
    /// A new session goes to the month it started in. A correction or a removal goes to the month
    /// of the record at the root of its chain, so that reading one month's file alone still gives
    /// the current version of every session that started in it.
    ///
    /// The record is copied into the store only once it is on disk. On any error the store is as
    /// it was and the caller still holds the record, to keep and try again.
    ///
    /// # Errors
    ///
    /// [`AppendError::ReadOnly`] when this instance may not write, else what
    /// [`sessions::append`] reports.
    pub fn record(&mut self, session: &Session) -> Result<(), AppendError> {
        if !self.access.can_write() {
            return Err(AppendError::ReadOnly);
        }
        let (year, month) = self.month_for(session);
        sessions::append(&self.paths.month_file(year, month), session)?;
        self.records.push(session.clone());
        self.refresh();
        Ok(())
    }

    /// Moves the session `id` to the trash: a copy of its current version goes to the trash file
    /// of its month, then a line voiding it goes to the month file. Nothing is rewritten, so a
    /// crash between the two leaves at worst a copy in the trash of a session still in the lists.
    ///
    /// `now` is the wall clock, written as the removal's stamp.
    ///
    /// # Errors
    ///
    /// [`AppendError::ReadOnly`] when this instance may not write, [`AppendError::Missing`] when
    /// no current session carries `id`, else what writing either file reported.
    pub fn void(&mut self, id: Id, now: i64) -> Result<(), AppendError> {
        if !self.access.can_write() {
            return Err(AppendError::ReadOnly);
        }
        let Some(session) = self.sessions.iter().find(|session| session.id == id).cloned() else {
            return Err(AppendError::Missing(id));
        };
        let (year, month) = self.month_for(&session);
        sessions::append(&self.paths.trash_file(year, month), &session)?;
        let removal = Session {
            id: clock::new_id(),
            revision: newest_revision(&self.records, id).saturating_add(1),
            written: now,
            replaces: None,
            voids: Some(id),
            ..session
        };
        self.record(&removal)
    }

    /// Moves every current session to the trash, one at a time through [`Store::void`], so each
    /// goes as its own line under the line cap and a failure loses nothing already written.
    /// Returns the identifiers moved, in order, and the error that stopped the pass, if one did;
    /// the sessions after it are still current and a later pass takes them.
    ///
    /// `now` is the wall clock, written as the stamp of every removal.
    pub fn void_all(&mut self, now: i64) -> (Vec<Id>, Option<AppendError>) {
        self.void_matching(|_| true, now)
    }

    /// Moves every current session `matches` accepts to the trash, the same way as
    /// [`Store::void_all`]: one line each, in order, stopping at the first error with what was
    /// moved so far.
    ///
    /// `now` is the wall clock, written as the stamp of every removal.
    pub fn void_matching(&mut self, matches: impl Fn(&Session) -> bool, now: i64) -> (Vec<Id>, Option<AppendError>) {
        // The list is taken before the first write: every removal resolves the sessions again.
        let ids: Vec<Id> = self.sessions.iter().filter(|session| matches(session)).map(|session| session.id).collect();
        let mut moved = Vec::with_capacity(ids.len());
        for id in ids {
            match self.void(id, now) {
                Ok(()) => moved.push(id),
                Err(error) => return (moved, Some(error)),
            }
        }
        (moved, None)
    }

    /// Brings the session `id` back from the trash: a line replacing it with a revision above the
    /// removal's goes to the month file, which is the one thing that revives a voided chain
    /// (see [`crate::session::resolve`]). The trash file keeps its copy; the trash is data too.
    ///
    /// `now` is the wall clock, written as the revival's stamp.
    ///
    /// # Errors
    ///
    /// [`AppendError::ReadOnly`] when this instance may not write, [`AppendError::Missing`] when
    /// the trash holds no session `id`, else what writing the month file reported.
    pub fn restore(&mut self, id: Id, now: i64) -> Result<(), AppendError> {
        if !self.access.can_write() {
            return Err(AppendError::ReadOnly);
        }
        let Some(session) = self.voided.iter().find(|session| session.id == id).cloned() else {
            return Err(AppendError::Missing(id));
        };
        let revival = Session {
            id: clock::new_id(),
            revision: newest_revision(&self.records, id).saturating_add(1),
            written: now,
            replaces: Some(id),
            voids: None,
            ..session
        };
        self.record(&revival)
    }

    /// The month file `session` belongs in: the month of the root of its chain, when the chain is
    /// known here, else its own.
    fn month_for(&self, session: &Session) -> (i32, u8) {
        let mut root = session;
        // A chain is as long as the records it goes through; the bound only guards against a
        // cycle in a hand-edited file.
        for _ in 0..=self.records.len() {
            let Some(parent) = root.replaces.or(root.voids) else { break };
            match self.records.iter().find(|record| record.id == parent) {
                Some(record) => root = record,
                None => break,
            }
        }
        month_of(root)
    }

    /// The running counter this machine left on disk, if any; see [`running::load`].
    #[must_use]
    pub fn running(&self) -> Option<Result<Running, Vec<Diagnostic>>> {
        running::load(&self.paths.running_file())
    }

    /// Writes the running counter to disk; see [`running::save`].
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::PermissionDenied`] when this instance is read-only, else the error of the
    /// step that failed.
    pub fn save_running(&self, running: &Running) -> io::Result<()> {
        self.writable()?;
        fs::create_dir_all(self.paths.root())?;
        running::save(&self.paths.running_file(), running)
    }

    /// Removes the running counter from disk; see [`running::clear`].
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::PermissionDenied`] when this instance is read-only, else the error of
    /// removing the file.
    pub fn clear_running(&self) -> io::Result<()> {
        self.writable()?;
        running::clear(&self.paths.running_file())
    }

    /// Moves a running file that cannot be read out of the way, to
    /// [`Paths::broken_running_file`] with the first `attempt` whose name is free, and answers
    /// where it went; see [`running::set_aside`].
    ///
    /// Left where it is, the file would be overwritten by the next counter's first refresh, and
    /// whatever could still be read from it by hand would be gone. Moved aside, the running path is
    /// free, so the next start finds nothing and the person is told about the file only once.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::PermissionDenied`] when this instance is read-only, [`io::ErrorKind::NotFound`]
    /// when there is no running file, else the error of the rename. On any error the file is where
    /// it was.
    pub fn set_aside_running(&self, stamp: &str) -> io::Result<PathBuf> {
        self.writable()?;
        running::set_aside(&self.paths.running_file(), |attempt| self.paths.broken_running_file(stamp, attempt))
    }

    /// When a command last saw this machine's counter alive; `None` when there is no stamp or
    /// it cannot be read. See [`seen::load`].
    #[must_use]
    pub fn seen(&self) -> Option<Seen> {
        seen::load(&self.paths.seen_file())
    }

    /// Writes when a command saw the counter alive. Needs no lock: it is written by commands that
    /// only look, and two of them racing each write a true moment; see [`seen`].
    ///
    /// # Errors
    ///
    /// The error of the step that failed.
    pub fn save_seen(&self, stamp: &Seen) -> io::Result<()> {
        seen::save(&self.paths.seen_file(), stamp)
    }

    /// Whether another process holds this machine's lock right now, which for a counter that a
    /// window measures means the window is still open. The lock is taken and let go at once when
    /// it is free. A lock this process holds itself also answers `true`, so an instance that
    /// writes must not ask. Without a lock on this platform, or when the file cannot be opened,
    /// the answer is `false`: nothing can be shown to hold it.
    #[must_use]
    pub fn lock_is_held(&self) -> bool {
        matches!(AppLock::acquire(&self.paths.lock_file()), Ok(None))
    }

    /// An error when this instance is read-only, so no write path forgets to check.
    fn writable(&self) -> io::Result<()> {
        if self.access.can_write() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "another instance holds the lock; this one may not write",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::session::Source;
    use crate::span::{ClockSource, Span, SpanKind};
    use crate::tree::{Category, Focus};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qfocus-test-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn done(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    fn sample(started: i64) -> Session {
        Session {
            id: Id::new(u64::try_from(started).unwrap_or(0) * 1000, 1),
            revision: 1,
            written: started + 600,
            focus: Id::new(1_758_124_800_000, 2),
            started,
            offset_minutes: 180,
            ended: started + 600,
            spans: vec![Span::new(SpanKind::Work, 0, 600, ClockSource::Mono)],
            source: Source::Timer,
            replaces: None,
            voids: None,
            continues: None,
            flags: Vec::new(),
            note: String::new(),
        }
    }

    fn sample_running() -> Running {
        Running {
            focus: Id::new(1_758_124_800_000, 2),
            started: 1_758_124_800,
            offset_minutes: 180,
            spans: vec![Span::new(SpanKind::Work, 0, 90, ClockSource::Mono)],
            paused: false,
            refreshed: 1_758_124_890,
            flags: Vec::new(),
            target: None,
            idle_from: None,
            watch: running::Watch::Window,
        }
    }

    #[test]
    fn a_missing_folder_opens_as_an_empty_writer() {
        let dir = temp("empty");
        let store = Store::open(Paths::at(&dir, "test"));
        assert!(store.tree.categories.is_empty());
        assert!(store.sessions.is_empty());
        assert!(store.diagnostics.is_empty(), "{:?}", store.diagnostics);
        assert_eq!(store.unknown_version, 0);
        assert!(store.access.can_write());
        assert!(store.paths.lock_file().exists());
        done(&dir);
    }

    #[test]
    fn records_reach_the_month_file_and_come_back() {
        let dir = temp("record");
        let paths = Paths::at(&dir, "test");
        let mut store = Store::open(paths.clone());
        // One session in September 2025 and one in August: two month files.
        store.record(&sample(1_758_124_800)).expect("record");
        store.record(&sample(1_755_000_000)).expect("record");
        assert_eq!(store.sessions.len(), 2);
        assert!(paths.month_file(2025, 9).exists());
        assert!(paths.month_file(2025, 8).exists());
        drop(store);

        let again = Store::open_read_only(paths);
        assert_eq!(again.sessions.len(), 2);
        assert!(again.diagnostics.is_empty(), "{:?}", again.diagnostics);
        done(&dir);
    }

    #[test]
    fn a_correction_resolves_against_the_records_already_loaded() {
        let dir = temp("correct");
        let mut store = Store::open(Paths::at(&dir, "test"));
        let original = sample(1_758_124_800);
        store.record(&original).expect("record");
        let correction = Session {
            id: Id::new(1_758_200_000_000, 3),
            revision: 2,
            replaces: Some(original.id),
            note: "fixed".to_owned(),
            ..original.clone()
        };
        store.record(&correction).expect("record");
        assert_eq!(store.sessions.len(), 1);
        assert_eq!(store.sessions[0].note, "fixed");
        assert!(store.orphans.is_empty());
        done(&dir);
    }

    #[test]
    fn a_correction_goes_to_the_month_of_the_record_it_corrects() {
        let dir = temp("month");
        let paths = Paths::at(&dir, "test");
        let mut store = Store::open(paths.clone());
        // Started 2025-09-17; the correction moves the start into October and is itself
        // corrected again. Both corrections belong in September's file.
        let original = sample(1_758_124_800);
        store.record(&original).expect("record");
        let moved = Session {
            id: Id::new(1_760_000_000_000, 3),
            revision: 2,
            started: 1_759_400_000,
            ended: 1_759_400_600,
            replaces: Some(original.id),
            ..original.clone()
        };
        store.record(&moved).expect("record");
        let again =
            Session { id: Id::new(1_760_000_001_000, 4), revision: 3, replaces: Some(moved.id), ..moved.clone() };
        store.record(&again).expect("record");
        let removal = Session {
            id: Id::new(1_760_000_002_000, 5),
            revision: 4,
            replaces: None,
            voids: Some(again.id),
            ..again.clone()
        };
        store.record(&removal).expect("record");

        let september = fs::read_to_string(paths.month_file(2025, 9)).expect("september");
        assert_eq!(september.lines().count(), 4);
        assert!(!paths.month_file(2025, 10).exists());
        assert!(store.sessions.is_empty(), "voided");
        done(&dir);
    }

    #[test]
    fn voiding_copies_to_the_trash_and_restoring_brings_it_back_with_a_higher_revision() {
        let dir = temp("void");
        let paths = Paths::at(&dir, "test");
        let mut store = Store::open(paths.clone());
        let original = sample(1_758_124_800);
        store.record(&original).expect("record");
        store.void(original.id, 1_758_200_000).expect("void");
        assert!(store.sessions.is_empty());
        assert_eq!(store.voided.len(), 1);
        assert_eq!(store.voided[0].id, original.id);
        let trash = fs::read_to_string(paths.trash_file(2025, 9)).expect("trash file");
        assert_eq!(trash.lines().count(), 1);
        assert!(trash.contains(&original.id.to_string()));
        let month = fs::read_to_string(paths.month_file(2025, 9)).expect("month file");
        assert_eq!(month.lines().count(), 2, "{month}");

        store.restore(original.id, 1_758_200_100).expect("restore");
        assert!(store.voided.is_empty());
        assert_eq!(store.sessions.len(), 1);
        assert_eq!(store.sessions[0].revision, 3);
        assert_eq!(store.sessions[0].replaces, Some(original.id));
        assert_eq!(store.sessions[0].note, original.note);
        let month = fs::read_to_string(paths.month_file(2025, 9)).expect("month file");
        assert_eq!(month.lines().count(), 3, "{month}");
        // The copy stays in the trash file: nothing is ever rewritten.
        assert_eq!(fs::read_to_string(paths.trash_file(2025, 9)).expect("trash").lines().count(), 1);

        // Everything survives a fresh load.
        drop(store);
        let again = Store::open_read_only(paths);
        assert_eq!(again.sessions.len(), 1);
        assert!(again.voided.is_empty());
        assert!(again.diagnostics.is_empty(), "{:?}", again.diagnostics);
        done(&dir);
    }

    #[test]
    fn voiding_everything_writes_one_line_per_record_and_stops_where_it_fails() {
        let dir = temp("void-all");
        let paths = Paths::at(&dir, "test");
        let mut store = Store::open(paths.clone());
        let first = sample(1_758_124_800);
        let second = sample(1_758_128_400);
        let august = sample(1_755_000_000);
        for session in [&first, &second, &august] {
            store.record(session).expect("record");
        }
        let (moved, error) = store.void_all(1_758_200_000);
        assert!(error.is_none(), "{error:?}");
        assert_eq!(moved, vec![august.id, first.id, second.id]);
        assert!(store.sessions.is_empty());
        assert_eq!(store.voided.len(), 3);
        let september = fs::read_to_string(paths.month_file(2025, 9)).expect("september");
        assert_eq!(september.lines().count(), 4, "two records, two removals:\n{september}");
        assert_eq!(fs::read_to_string(paths.trash_file(2025, 9)).expect("trash").lines().count(), 2);
        assert_eq!(fs::read_to_string(paths.trash_file(2025, 8)).expect("trash").lines().count(), 1);
        // Each comes back on its own, with a revision above its removal.
        for id in &moved {
            store.restore(*id, 1_758_200_100).expect("restore");
        }
        assert_eq!(store.sessions.len(), 3);
        assert!(store.voided.is_empty());
        // A pass over nothing moves nothing.
        let mut empty = Store::open(Paths::at(temp("void-none"), "test"));
        let (moved, error) = empty.void_all(0);
        assert!(moved.is_empty() && error.is_none(), "{error:?}");
        done(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn voiding_everything_reports_the_error_that_stopped_it_and_keeps_the_rest_current() {
        let dir = temp("void-all-stop");
        let paths = Paths::at(&dir, "test");
        let mut store = Store::open(paths.clone());
        store.record(&sample(1_755_000_000)).expect("record");
        store.record(&sample(1_758_124_800)).expect("record");
        // September's trash folder is a file, so its copy cannot be written; August goes first.
        fs::create_dir_all(paths.trash_dir()).expect("trash dir");
        fs::write(paths.trash_file(2025, 9), "").expect("a file where the copy goes");
        fs::set_permissions(paths.trash_file(2025, 9), std::os::unix::fs::PermissionsExt::from_mode(0o444))
            .expect("read-only");
        let (moved, error) = store.void_all(1_758_200_000);
        assert_eq!(moved.len(), 1, "{error:?}");
        assert!(matches!(error, Some(AppendError::Io(..))), "{error:?}");
        assert_eq!(store.sessions.len(), 1, "the one that failed is still current");
        assert_eq!(store.voided.len(), 1);
        fs::set_permissions(paths.trash_file(2025, 9), std::os::unix::fs::PermissionsExt::from_mode(0o644))
            .expect("writable again");
        done(&dir);
    }

    #[test]
    fn voiding_what_matches_takes_only_the_older_days_and_keeps_the_rest_current() {
        use crate::day::day_of;
        use qframe::date::{Date, TimeOfDay};

        let dir = temp("void-matching");
        let paths = Paths::at(&dir, "test");
        let mut store = Store::open(paths.clone());
        // 2025-08-12, 2025-09-17 and 2025-09-18 in local time (three hours east of UTC).
        let august = sample(1_755_000_000);
        let before = sample(1_758_124_800);
        let after = sample(1_758_124_800 + 86_400);
        for session in [&august, &before, &after] {
            store.record(session).expect("record");
        }
        let cut = Date::new(2025, 9, 18).expect("valid date");
        let rollover = TimeOfDay::default();
        let (moved, error) = store.void_matching(|session| day_of(session, rollover) < cut, 1_758_300_000);
        assert!(error.is_none(), "{error:?}");
        assert_eq!(moved, vec![august.id, before.id]);
        assert_eq!(store.sessions.iter().map(|session| session.id).collect::<Vec<_>>(), vec![after.id]);
        assert_eq!(store.voided.len(), 2);
        // Nothing matches any more: a second pass moves nothing and writes nothing.
        let september = fs::read_to_string(paths.month_file(2025, 9)).expect("september");
        let (moved, error) = store.void_matching(|session| day_of(session, rollover) < cut, 1_758_300_000);
        assert!(moved.is_empty() && error.is_none(), "{error:?}");
        assert_eq!(fs::read_to_string(paths.month_file(2025, 9)).expect("september"), september);
        done(&dir);
    }

    #[test]
    fn voiding_and_restoring_refuse_what_is_not_there() {
        let dir = temp("void-missing");
        let mut store = Store::open(Paths::at(&dir, "test"));
        let stranger = Id::new(7, 7);
        assert!(matches!(store.void(stranger, 0), Err(AppendError::Missing(id)) if id == stranger));
        assert!(matches!(store.restore(stranger, 0), Err(AppendError::Missing(id)) if id == stranger));
        assert!(!store.paths.trash_dir().exists());
        done(&dir);
    }

    #[test]
    fn broken_files_are_reported_and_the_rest_is_kept() {
        let dir = temp("broken");
        let paths = Paths::at(&dir, "test");
        fs::create_dir_all(paths.sessions_dir()).expect("folder");
        fs::write(paths.tree_file(), "[[category]]\nid = \"not an id\"\nname = \"broken\"\n").expect("tree");
        let good = crate::session::line::write(&sample(1_758_124_800));
        fs::write(paths.month_file(2025, 9), format!("{good}\nqf9;from the future\nnot a line\n")).expect("log");
        let store = Store::open(paths);
        assert_eq!(store.sessions.len(), 1);
        assert_eq!(store.unknown_version, 1);
        assert_eq!(store.diagnostics.len(), 2, "{:?}", store.diagnostics);
        assert!(store.diagnostics.iter().all(|d| d.location.is_some()));
        done(&dir);
    }

    #[test]
    fn the_tree_is_saved_and_read_back() {
        let dir = temp("tree");
        let paths = Paths::at(&dir, "test");
        let mut store = Store::open(paths.clone());
        store.tree.categories.push(Category {
            id: Id::new(1, 1),
            name: "Work".to_owned(),
            icon: None,
            goal: None,
            archived: false,
            order: 0.0,
            focuses: vec![Focus {
                id: Id::new(2, 2),
                name: "Rust".to_owned(),
                goal: None,
                archived: false,
                order: 0.0,
            }],
        });
        store.save_tree().expect("save");
        let again = Store::open_read_only(paths);
        assert_eq!(again.tree, store.tree);
        done(&dir);
    }

    #[test]
    fn running_is_saved_read_and_cleared() {
        let dir = temp("running");
        let store = Store::open(Paths::at(&dir, "test"));
        assert!(store.running().is_none());
        store.save_running(&sample_running()).expect("save");
        assert_eq!(store.running(), Some(Ok(sample_running())));
        store.clear_running().expect("clear");
        assert!(store.running().is_none());
        done(&dir);
    }

    #[test]
    fn a_broken_running_file_is_set_aside_with_every_byte() {
        let dir = temp("aside");
        let paths = Paths::at(&dir, "test");
        let store = Store::open(paths.clone());
        let bytes = b"focus = \"x\"\n\xff\x00 half a line".to_vec();
        fs::write(paths.running_file(), &bytes).expect("broken file");
        let aside = store.set_aside_running("2026-09-18-150000").expect("set aside");
        assert_eq!(aside, paths.broken_running_file("2026-09-18-150000", 1));
        assert_eq!(fs::read(&aside).expect("the copy"), bytes);
        assert!(!paths.running_file().exists());
        assert!(store.running().is_none());
        done(&dir);
    }

    #[test]
    fn a_second_broken_file_in_the_same_second_never_overwrites_the_first() {
        let dir = temp("aside-twice");
        let paths = Paths::at(&dir, "test");
        let store = Store::open(paths.clone());
        let stamp = "2026-09-18-150000";
        fs::write(paths.running_file(), "first").expect("first");
        let first = store.set_aside_running(stamp).expect("first aside");
        fs::write(paths.running_file(), "second").expect("second");
        let second = store.set_aside_running(stamp).expect("second aside");
        fs::write(paths.running_file(), "third").expect("third");
        let third = store.set_aside_running(stamp).expect("third aside");
        assert_eq!(second, paths.broken_running_file(stamp, 2));
        assert_eq!(third, paths.broken_running_file(stamp, 3));
        assert_eq!(fs::read_to_string(&first).expect("first"), "first");
        assert_eq!(fs::read_to_string(&second).expect("second"), "second");
        assert_eq!(fs::read_to_string(&third).expect("third"), "third");
        done(&dir);
    }

    #[test]
    fn setting_aside_needs_a_file_and_the_lock() {
        let dir = temp("aside-refused");
        let paths = Paths::at(&dir, "test");
        let store = Store::open(paths.clone());
        let stamp = "2026-09-18-150000";
        assert_eq!(store.set_aside_running(stamp).map_err(|e| e.kind()), Err(io::ErrorKind::NotFound));
        fs::write(paths.running_file(), "broken").expect("broken file");
        let looker = Store::open_read_only(paths.clone());
        assert_eq!(looker.set_aside_running(stamp).map_err(|e| e.kind()), Err(io::ErrorKind::PermissionDenied));
        assert_eq!(fs::read_to_string(paths.running_file()).expect("still there"), "broken");
        assert!(!paths.broken_running_file(stamp, 1).exists());
        done(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_cannot_be_written_leaves_the_file_where_it_is() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp("aside-locked-folder");
        let paths = Paths::at(&dir, "test");
        let store = Store::open(paths.clone());
        fs::write(paths.running_file(), "broken").expect("broken file");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).expect("read-only folder");
        // A user the permissions do not bind, such as root, can still write: nothing to test.
        let probe = dir.join("probe");
        if fs::write(&probe, "").is_ok() {
            let _ = fs::remove_file(&probe);
        } else {
            assert!(store.set_aside_running("2026-09-18-150000").is_err());
            assert_eq!(fs::read_to_string(paths.running_file()).expect("still there"), "broken");
        }
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("writable again");
        done(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_second_open_is_read_only_and_drops_nothing() {
        let dir = temp("second");
        let paths = Paths::at(&dir, "test");
        let first = Store::open(paths.clone());
        assert_eq!(first.access, Access::Writer);

        let mut second = Store::open(paths.clone());
        assert_eq!(second.access, Access::ReadOnly { holder: Some(std::process::id()) });
        let session = sample(1_758_124_800);
        assert!(matches!(second.record(&session), Err(AppendError::ReadOnly)));
        assert!(matches!(second.void(session.id, 0), Err(AppendError::ReadOnly)));
        assert!(matches!(second.restore(session.id, 0), Err(AppendError::ReadOnly)));
        assert!(second.sessions.is_empty());
        assert!(!paths.month_file(2025, 9).exists());
        assert_eq!(second.save_running(&sample_running()).map_err(|e| e.kind()), Err(io::ErrorKind::PermissionDenied));
        assert_eq!(second.save_tree().map_err(|e| e.kind()), Err(io::ErrorKind::PermissionDenied));
        assert!(!second.retry_lock());

        // The second instance still sees the first one's counter.
        first.save_running(&sample_running()).expect("save");
        assert_eq!(second.running(), Some(Ok(sample_running())));

        drop(first);
        assert!(second.retry_lock());
        assert_eq!(second.access, Access::Writer);
        second.record(&session).expect("record after taking the lock");
        assert_eq!(second.sessions.len(), 1);
        done(&dir);
    }

    #[test]
    fn open_read_only_never_takes_the_lock() {
        let dir = temp("readonly");
        let paths = Paths::at(&dir, "test");
        let store = Store::open_read_only(paths.clone());
        assert_eq!(store.access, Access::ReadOnly { holder: None });
        assert!(!paths.lock_file().exists());
        done(&dir);
    }

    #[test]
    fn a_look_leaves_the_seen_stamp_without_the_lock_and_can_tell_a_window_is_open() {
        let dir = temp("seen");
        let paths = Paths::at(&dir, "test");
        let window = Store::open(paths.clone());
        assert_eq!(window.access, Access::Writer);
        let look = Store::open_read_only(paths.clone());
        assert_eq!(look.seen(), None);
        let stamp = Seen { started: 1_758_124_800, seen: 1_758_128_400 };
        look.save_seen(&stamp).expect("written without the lock");
        assert_eq!(look.seen(), Some(stamp));
        assert!(paths.seen_file().ends_with("seen-test.toml"));
        assert!(look.lock_is_held(), "the window holds it");
        drop(window);
        assert!(!look.lock_is_held(), "nobody holds it");
        assert!(Store::open(paths).access.can_write(), "the probe let it go again");
        done(&dir);
    }
}
