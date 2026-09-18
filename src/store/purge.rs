//! Emptying the trash: the one place qfocus takes something off the disk for good.
//!
//! Everything else is an append (see [`crate::session::resolve`]). This is the exception the person asks for
//! by name, so it is as careful as an append: every month file is rewritten in one piece or not
//! at all, a line this version cannot read is kept byte for byte, and only whole chains of
//! records in the trash are taken. The tree is written last, and only when a row actually goes.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use qframe::storage::atomic_write;

use super::Store;
use crate::id::Id;
use crate::session::line;
use crate::session::resolve::roots;
use crate::tree::Tree;

/// The file name extension of a month file.
const EXTENSION: &str = "log";

/// What emptying the trash would take, counted without writing anything.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PurgePlan {
    /// Records in the trash, each going with every line of its chain.
    pub records: usize,
    /// Archived categories and focuses that nothing refers to any more.
    pub rows: usize,
    /// Archived categories and focuses that stay, because a record still refers to them.
    pub kept_rows: usize,
}

impl PurgePlan {
    /// Whether there is nothing to take. Copies in the trash folder alone do not count: they are
    /// backups, and the next purge that takes something takes them too.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records == 0 && self.rows == 0
    }
}

/// What emptying the trash took.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Purged {
    /// Records taken off the disk, each with its whole chain.
    pub records: usize,
    /// Lines taken out of the month files.
    pub lines: usize,
    /// Archived categories and focuses taken out of the tree.
    pub rows: usize,
    /// Archived categories and focuses kept, because a record still refers to them.
    pub kept_rows: usize,
    /// Files removed from the trash folder.
    pub backups: usize,
}

impl Store {
    /// Counts what [`Store::purge`] would take now, writing nothing. `keep_focuses` are focuses
    /// referred to by something the store does not know about, such as a record waiting in
    /// memory or the running counter.
    #[must_use]
    pub fn purge_plan(&self, keep_focuses: &[Id]) -> PurgePlan {
        let (_, rows, kept_rows) = prune(&self.tree, &self.referenced(keep_focuses));
        PurgePlan { records: self.voided.len(), rows, kept_rows }
    }

    /// Takes the trash off the disk for good.
    ///
    /// Every line of the chain of every record in the trash leaves its month file: the original,
    /// its corrections, its removal. A line that cannot be read, or that a newer qfocus wrote,
    /// stays byte for byte. Each changed month file is rewritten atomically, so a purge cut short
    /// leaves every file either as it was or as it should be. Then every file in the trash folder
    /// goes: they are copies, and the copies of records that came back are no longer needed. Last,
    /// archived focuses nothing refers to leave the tree, and archived categories left with no
    /// focus; `keep_focuses` are referred to from outside the store and stay.
    ///
    /// The records are read again from the disk afterwards; the lock is kept throughout.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::PermissionDenied`] when this instance is read-only, else the error of the
    /// step that failed. The files already rewritten stay rewritten, the store is read again so it
    /// shows what is on the disk, and the tree is untouched.
    pub fn purge(&mut self, keep_focuses: &[Id]) -> io::Result<Purged> {
        self.writable()?;
        let chains = roots(&self.records);
        let doomed: HashSet<Id> = self.voided.iter().filter_map(|session| chains.get(&session.id).copied()).collect();
        let mut purged = Purged { records: doomed.len(), ..Purged::default() };
        let rewritten = if doomed.is_empty() {
            Ok(0)
        } else {
            rewrite_months(&self.paths.sessions_dir(), |id| chains.get(&id).is_some_and(|root| doomed.contains(root)))
        };
        let cleared = rewritten.and_then(|lines| {
            purged.lines = lines;
            clear_folder(&self.paths.trash_dir())
        });
        self.reload();
        purged.backups = cleared?;

        let (tree, rows, kept_rows) = prune(&self.tree, &self.referenced(keep_focuses));
        purged.rows = rows;
        purged.kept_rows = kept_rows;
        if rows > 0 {
            self.tree = tree;
            self.save_tree()?;
        }
        Ok(purged)
    }

    /// The focuses a current record, a correction of a missing record or `keep` refers to.
    fn referenced(&self, keep: &[Id]) -> HashSet<Id> {
        self.sessions.iter().chain(&self.orphans).map(|session| session.focus).chain(keep.iter().copied()).collect()
    }

    /// Reads the month files again, keeping the lock, the access and the tree in memory: the
    /// tree may hold changes the disk does not have yet, and the lock must never be let go.
    fn reload(&mut self) {
        let mut fresh = Self::load(self.paths.clone());
        fresh.tree = std::mem::take(&mut self.tree);
        fresh.lock = self.lock.take();
        fresh.access = self.access;
        *self = fresh;
    }
}

/// Rewrites every month file in `dir` without the lines that read as one session `doomed`
/// accepts, and answers how many lines went. A file that loses nothing is not touched; a file
/// that loses everything is removed.
fn rewrite_months(dir: &Path, doomed: impl Fn(Id) -> bool) -> io::Result<usize> {
    let mut files: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == EXTENSION) && path.is_file())
            .collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    files.sort();
    let mut dropped = 0;
    for path in files {
        let bytes = fs::read(&path)?;
        let name = path.display().to_string();
        let mut kept = Vec::with_capacity(bytes.len());
        let mut gone = 0;
        // Each piece keeps its own line ending, so what stays is the very bytes that were there,
        // a last line without an ending included.
        for piece in bytes.split_inclusive(|byte| *byte == b'\n') {
            if goes(&name, piece, &doomed) {
                gone += 1;
            } else {
                kept.extend_from_slice(piece);
            }
        }
        if gone == 0 {
            continue;
        }
        if kept.is_empty() {
            fs::remove_file(&path)?;
        } else {
            atomic_write(&path, &kept)?;
        }
        dropped += gone;
    }
    Ok(dropped)
}

/// Whether the line `piece` of the file `name` is exactly one session whose chain `doomed`
/// accepts. Anything else, a line that is not text, not a session or not this version's, stays.
fn goes(name: &str, piece: &[u8], doomed: impl Fn(Id) -> bool) -> bool {
    let Ok(text) = std::str::from_utf8(piece) else { return false };
    let reading = line::read(name, text);
    match reading.sessions.as_slice() {
        [session] => reading.unknown_version == 0 && doomed(session.id),
        _ => false,
    }
}

/// Removes every file in `dir` and answers how many went. A missing folder holds nothing;
/// folders inside it are not the trash's and stay.
fn clear_folder(dir: &Path) -> io::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut removed = 0;
    for entry in entries {
        let path = entry?.path();
        if path.is_file() {
            fs::remove_file(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// `tree` without the archived rows nothing in `referenced` needs, with how many rows went and
/// how many archived rows stayed.
///
/// A focus counts as archived when it is, or when its category is. An archived category goes
/// only once no focus is left in it.
fn prune(tree: &Tree, referenced: &HashSet<Id>) -> (Tree, usize, usize) {
    let mut pruned = tree.clone();
    let (mut rows, mut kept) = (0, 0);
    pruned.categories.retain_mut(|category| {
        let archived = category.archived;
        category.focuses.retain(|focus| {
            if !(archived || focus.archived) {
                return true;
            }
            if referenced.contains(&focus.id) {
                kept += 1;
                true
            } else {
                rows += 1;
                false
            }
        });
        if !archived {
            return true;
        }
        if category.focuses.is_empty() {
            rows += 1;
            false
        } else {
            kept += 1;
            true
        }
    });
    (pruned, rows, kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Session, Source};
    use crate::span::{ClockSource, Span, SpanKind};
    use crate::store::Paths;
    use crate::tree::{Category, Focus};

    /// 2025-09-17 and 2025-08-12, three hours east of UTC.
    const SEPTEMBER: i64 = 1_758_124_800;
    const AUGUST: i64 = 1_755_000_000;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qfocus-test-purge-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn done(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    fn session(n: u64, focus: Id, started: i64) -> Session {
        Session {
            id: Id::new(n * 1_000, 1),
            revision: 1,
            written: started + 600,
            focus,
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

    fn focus(n: u64, archived: bool) -> Focus {
        Focus { id: Id::new(n, 2), name: format!("focus {n}"), goal: None, archived, order: 0.0 }
    }

    fn category(n: u64, archived: bool, focuses: Vec<Focus>) -> Category {
        Category {
            id: Id::new(n, 3),
            name: format!("category {n}"),
            icon: None,
            goal: None,
            archived,
            order: 0.0,
            focuses,
        }
    }

    /// Work (kept) holds Rust, Review archived with a current record, Old archived with nothing,
    /// Kept archived and asked to be kept, and Lost archived with only an orphan pointing at it.
    /// Past (archived) holds Gone, whose only record is in the trash. Held (archived) holds Busy,
    /// which a current record refers to.
    fn tree() -> Tree {
        Tree {
            categories: vec![
                category(
                    1,
                    false,
                    vec![focus(10, false), focus(11, true), focus(12, true), focus(13, true), focus(14, true)],
                ),
                category(2, true, vec![focus(20, false)]),
                category(3, true, vec![focus(30, false)]),
                category(4, true, Vec::new()),
            ],
        }
    }

    #[test]
    fn whole_chains_leave_their_month_files_and_everything_else_stays_byte_for_byte() {
        let dir = temp("chains");
        let paths = Paths::at(&dir, "test");
        let mut store = Store::open(paths.clone());
        store.tree = tree();
        store.save_tree().expect("tree");
        let rust = Id::new(10, 2);
        // Current, corrected once: stays with both lines.
        let kept = session(1, rust, SEPTEMBER);
        store.record(&kept).expect("record");
        let fixed = Session { id: Id::new(1_500, 1), revision: 2, replaces: Some(kept.id), ..kept.clone() };
        store.record(&fixed).expect("record");
        // Removed then brought back: current again, stays, but its copy in the trash goes.
        let revived = session(2, rust, SEPTEMBER + 3_600);
        store.record(&revived).expect("record");
        store.void(revived.id, SEPTEMBER + 7_200).expect("void");
        store.restore(revived.id, SEPTEMBER + 7_300).expect("restore");
        // Corrected and then removed, in August: three lines, all go, and so does the file.
        let gone = session(3, Id::new(20, 2), AUGUST);
        store.record(&gone).expect("record");
        let gone_fixed = Session { id: Id::new(3_500, 1), revision: 2, replaces: Some(gone.id), ..gone.clone() };
        store.record(&gone_fixed).expect("record");
        store.void(gone_fixed.id, AUGUST + 7_200).expect("void");
        // Removed in September: both lines go.
        let dropped = session(4, rust, SEPTEMBER + 10_800);
        store.record(&dropped).expect("record");
        store.void(dropped.id, SEPTEMBER + 14_400).expect("void");
        store.record(&session(5, Id::new(11, 2), SEPTEMBER + 18_000)).expect("record");
        store.record(&session(6, Id::new(30, 2), SEPTEMBER + 21_600)).expect("record");

        // Lines this version cannot use, in the middle and at the end without a line ending.
        let september = paths.month_file(2025, 9);
        let orphan = Session {
            id: Id::new(7_000, 1),
            revision: 2,
            replaces: Some(Id::new(404, 4)),
            ..session(7, Id::new(14, 2), SEPTEMBER)
        };
        let mut bytes = fs::read(&september).expect("september");
        bytes.extend_from_slice(b"qf9;written;by;a;newer;qfocus\n");
        bytes.extend_from_slice(b"qf1;\xff\xfe;not text\r\n");
        bytes.extend_from_slice(b"not a line at all\n");
        bytes.extend_from_slice(line::write(&orphan).as_bytes());
        bytes.extend_from_slice(b"\nhalf a lin");
        fs::write(&september, &bytes).expect("september");
        drop(store);
        let mut store = Store::open(paths.clone());
        assert_eq!(store.voided.len(), 2);
        assert_eq!(store.orphans.len(), 1);

        let expected: Vec<u8> = bytes
            .split_inclusive(|byte| *byte == b'\n')
            .filter(|piece| {
                let text = String::from_utf8_lossy(piece);
                !text.contains(&dropped.id.to_string())
            })
            .flatten()
            .copied()
            .collect();
        assert_eq!(
            store.purge_plan(&[Id::new(13, 2)]),
            PurgePlan { records: 2, rows: 4, kept_rows: 5 },
            "Old, Gone, Past and the empty archived category go; Review, Kept, Lost, Busy and Held stay"
        );
        let purged = store.purge(&[Id::new(13, 2)]).expect("purge");
        assert_eq!(purged, Purged { records: 2, lines: 5, rows: 4, kept_rows: 5, backups: 2 });

        assert_eq!(fs::read(&september).expect("september"), expected);
        assert!(!paths.month_file(2025, 8).exists(), "August held only the chain that went");
        assert_eq!(fs::read_dir(paths.trash_dir()).expect("trash folder").count(), 0);
        assert!(store.voided.is_empty());
        assert_eq!(store.sessions.len(), 4);
        assert_eq!(store.orphans.len(), 1);
        assert_eq!(store.unknown_version, 1);
        assert!(store.access.can_write(), "the lock is kept");
        assert!(
            Store::open(paths.clone()).access == crate::store::Access::ReadOnly { holder: Some(std::process::id()) }
        );

        let ids: Vec<Id> = store.tree.categories.iter().map(|category| category.id).collect();
        assert_eq!(ids, vec![Id::new(1, 3), Id::new(3, 3)], "Past and the empty archived category went");
        let focuses: Vec<Id> = store.tree.categories[0].focuses.iter().map(|focus| focus.id).collect();
        assert_eq!(focuses, vec![Id::new(10, 2), Id::new(11, 2), Id::new(13, 2), Id::new(14, 2)]);
        let again = Store::open_read_only(paths);
        assert_eq!(again.tree, store.tree, "the tree was written");
        assert_eq!(again.sessions.len(), 4);
        done(&dir);
    }

    #[test]
    fn nothing_to_take_touches_nothing() {
        let dir = temp("nothing");
        let paths = Paths::at(&dir, "test");
        let mut store = Store::open(paths.clone());
        store.tree = tree();
        store.save_tree().expect("tree");
        let revived = session(1, Id::new(10, 2), SEPTEMBER);
        store.record(&revived).expect("record");
        store.void(revived.id, SEPTEMBER + 60).expect("void");
        store.restore(revived.id, SEPTEMBER + 120).expect("restore");
        // Every archived row is needed: keep them all.
        let keep = [Id::new(11, 2), Id::new(12, 2), Id::new(13, 2), Id::new(14, 2), Id::new(20, 2), Id::new(30, 2)];
        let plan = store.purge_plan(&keep);
        // The empty archived category still has nothing to keep it.
        assert_eq!(plan, PurgePlan { records: 0, rows: 1, kept_rows: 8 });
        let mut tree = store.tree.clone();
        tree.categories.pop();
        store.tree = tree;
        store.save_tree().expect("tree");
        let plan = store.purge_plan(&keep);
        assert!(plan.is_empty(), "{plan:?}");
        let tree_before = fs::read(paths.tree_file()).expect("tree");
        let month_before = fs::read(paths.month_file(2025, 9)).expect("month");
        let purged = store.purge(&keep).expect("purge");
        assert_eq!(purged.lines, 0);
        assert_eq!(purged.rows, 0);
        assert_eq!(fs::read(paths.tree_file()).expect("tree"), tree_before);
        assert_eq!(fs::read(paths.month_file(2025, 9)).expect("month"), month_before);
        done(&dir);
    }

    #[test]
    fn a_read_only_store_cannot_purge() {
        let dir = temp("read-only");
        let paths = Paths::at(&dir, "test");
        let mut first = Store::open(paths.clone());
        let removed = session(1, Id::new(10, 2), SEPTEMBER);
        first.record(&removed).expect("record");
        first.void(removed.id, SEPTEMBER + 60).expect("void");
        let mut second = Store::open(paths.clone());
        assert!(!second.access.can_write());
        let before = fs::read(paths.month_file(2025, 9)).expect("month");
        assert_eq!(second.purge(&[]).map_err(|error| error.kind()), Err(io::ErrorKind::PermissionDenied));
        assert_eq!(fs::read(paths.month_file(2025, 9)).expect("month"), before);
        assert!(paths.trash_file(2025, 9).exists());
        assert_eq!(second.voided.len(), 1);
        drop(first);
        done(&dir);
    }
}
