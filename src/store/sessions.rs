//! The month files: appending one session and reading all of them back.
//!
//! A month file only ever grows. One session is one line, written with a single `write` call so
//! that a crash leaves either the whole line or nothing of it; a line that would not fit in one
//! call is refused before anything touches the disk.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use qframe::date::DateTime;
use qframe::diagnostics::{Diagnostic, Location};

use crate::id::Id;
use crate::session::Session;
use crate::session::line;

/// The most bytes one line may take, its line ending included. Larger writes may be cut short by
/// the operating system, which would leave half a record in the file.
pub const MAX_LINE_BYTES: usize = 4096;

/// The file name extension of a month file.
const EXTENSION: &str = "log";

/// The local year and month a session started in: the month file it belongs to.
///
/// A correction is written to the same month as the record it corrects,
/// so callers pass the original session here, never the correction.
#[must_use]
pub fn month_of(session: &Session) -> (i32, u8) {
    let date = DateTime::from_unix(session.started, session.offset_minutes).date;
    (date.year(), date.month())
}

/// Why a session could not be appended. In every case the record stays with the caller; nothing
/// is thrown away.
#[derive(Debug)]
pub enum AppendError {
    /// The line would take more than [`MAX_LINE_BYTES`], so it was not written at all.
    TooLong {
        /// How many bytes the line takes.
        bytes: usize,
    },
    /// The operating system wrote only part of the line. The file now ends in a broken line that
    /// the reader will skip and report; the record itself has to be written again.
    Short {
        /// How many bytes reached the file.
        written: usize,
    },
    /// The file or its folder could not be opened or written.
    Io(PathBuf, io::Error),
    /// This store may not write: another instance holds the lock.
    ReadOnly,
    /// No record the operation could apply to carries this identifier.
    Missing(Id),
}

impl fmt::Display for AppendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { bytes } => write!(formatter, "the record takes {bytes} bytes, more than {MAX_LINE_BYTES}"),
            Self::Short { written } => write!(formatter, "only {written} bytes of the record reached the file"),
            Self::Io(path, error) => write!(formatter, "{}: {error}", path.display()),
            Self::ReadOnly => formatter.write_str("another instance holds the lock; this one may not write"),
            Self::Missing(id) => write!(formatter, "no record carries the identifier {id}"),
        }
    }
}

impl std::error::Error for AppendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(_, error) => Some(error),
            Self::TooLong { .. } | Self::Short { .. } | Self::ReadOnly | Self::Missing(_) => None,
        }
    }
}

/// Appends `session` as one line to the file at `path`, creating the folder and the file when
/// they are not there.
///
/// The line goes to the disk in one `write` and is flushed before this returns. A line over
/// [`MAX_LINE_BYTES`] is refused without writing.
///
/// # Errors
///
/// [`AppendError`] says what went wrong; the record is not written in any of those cases except
/// [`AppendError::Short`], where part of it is, and it is the caller's job to keep the record and
/// try again.
pub fn append(path: &Path, session: &Session) -> Result<(), AppendError> {
    let mut text = line::write(session);
    text.push('\n');
    if text.len() > MAX_LINE_BYTES {
        return Err(AppendError::TooLong { bytes: text.len() });
    }
    let io_error = |error| AppendError::Io(path.to_path_buf(), error);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    let mut file = OpenOptions::new().append(true).create(true).open(path).map_err(io_error)?;
    let written = file.write(text.as_bytes()).map_err(io_error)?;
    if written != text.len() {
        return Err(AppendError::Short { written });
    }
    file.sync_all().map_err(io_error)
}

/// Everything the month files hold.
#[derive(Debug, Clone, Default)]
pub struct Loaded {
    /// Every record that could be read, file by file in name order, unresolved: corrections and
    /// the records they correct are all here.
    pub records: Vec<Session>,
    /// Lines a newer qfocus wrote. They are not broken and are not reported as errors.
    pub unknown_version: usize,
    /// Everything that went wrong, each pointing at a file and, where it can, a line.
    pub diagnostics: Vec<Diagnostic>,
    /// How many month files were read.
    pub files: usize,
}

/// Reads every month file in `dir`, in name order.
///
/// A missing folder is an empty store, not an error. A file that cannot be read becomes a
/// diagnostic and the other files are still loaded. Inside a file, a line that is not valid UTF-8
/// and a line that is not a session are each skipped with a diagnostic, and the rest of the file
/// is kept.
#[must_use]
pub fn load_all(dir: &Path) -> Loaded {
    let mut loaded = Loaded::default();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return loaded,
        Err(error) => {
            let message = format!("{}: cannot list the folder: {error}", dir.display());
            loaded.diagnostics.push(Diagnostic::error(None, message));
            return loaded;
        }
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == EXTENSION) && path.is_file())
        .collect();
    files.sort();
    for path in files {
        let name = path.display().to_string();
        match fs::read(&path) {
            Ok(bytes) => {
                loaded.files += 1;
                let text = valid_lines(&name, &bytes, &mut loaded.diagnostics);
                let reading = line::read(&name, &text);
                loaded.records.extend(reading.sessions);
                loaded.unknown_version += reading.unknown_version;
                loaded.diagnostics.extend(reading.diagnostics);
            }
            Err(error) => {
                let location = Location { file: name.clone(), line: 1, column: 1 };
                loaded.diagnostics.push(Diagnostic::error(Some(location), format!("cannot read the file: {error}")));
            }
        }
    }
    loaded
}

/// `bytes` as text, with every line that is not valid UTF-8 replaced by an empty one.
///
/// The line numbers of the text are those of the file, so a diagnostic from the session reader
/// still points at the right line. The reader skips empty lines.
fn valid_lines(file: &str, bytes: &[u8], diagnostics: &mut Vec<Diagnostic>) -> String {
    let mut text = String::with_capacity(bytes.len());
    for (index, raw) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if index > 0 {
            text.push('\n');
        }
        match std::str::from_utf8(raw) {
            Ok(line) => text.push_str(line),
            Err(error) => {
                let location = Location { file: file.to_owned(), line: index + 1, column: 1 };
                let message = format!("not valid UTF-8 at byte {}; the line is skipped", error.valid_up_to() + 1);
                diagnostics.push(Diagnostic::error(Some(location), message));
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Source;
    use crate::span::{ClockSource, Span, SpanKind};

    /// An empty folder of this test's own, removed by [`done`].
    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qfocus-test-sessions-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("test folder");
        dir
    }

    fn done(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    fn sample(started: i64, offset_minutes: i16) -> Session {
        Session {
            id: Id::new(u64::try_from(started).unwrap_or(0) * 1000, 1),
            revision: 1,
            written: started + 600,
            focus: Id::new(1_758_124_800_000, 2),
            started,
            offset_minutes,
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

    #[test]
    fn month_is_the_local_one() {
        // 2026-09-30 23:30 UTC is already October three hours east of Greenwich.
        let late = 1_790_811_000;
        assert_eq!(month_of(&sample(late, 0)), (2026, 9));
        assert_eq!(month_of(&sample(late, 180)), (2026, 10));
        // And still September west of it.
        assert_eq!(month_of(&sample(late, -300)), (2026, 9));
    }

    #[test]
    fn a_missing_folder_is_an_empty_store() {
        let dir = temp("missing");
        let loaded = load_all(&dir.join("sessions"));
        assert!(loaded.records.is_empty());
        assert!(loaded.diagnostics.is_empty());
        assert_eq!(loaded.files, 0);
        done(&dir);
    }

    #[test]
    fn append_writes_a_line_that_load_all_reads_back() {
        let dir = temp("roundtrip");
        let path = dir.join("sessions").join("2026-09.log");
        let session = sample(1_758_124_800, 180);
        append(&path, &session).expect("append");
        append(&path, &sample(1_758_200_000, 180)).expect("second append");
        let text = fs::read_to_string(&path).expect("file");
        assert_eq!(text.lines().count(), 2);
        assert!(text.ends_with('\n'));
        let loaded = load_all(&dir.join("sessions"));
        assert_eq!(loaded.records.len(), 2);
        assert_eq!(loaded.records[0], session);
        assert_eq!(loaded.files, 1);
        assert!(loaded.diagnostics.is_empty());
        done(&dir);
    }

    #[test]
    fn two_month_files_load_in_name_order() {
        let dir = temp("months");
        let sessions = dir.join("sessions");
        append(&sessions.join("2026-09.log"), &sample(1_758_124_800, 0)).expect("september");
        append(&sessions.join("2026-08.log"), &sample(1_755_000_000, 0)).expect("august");
        fs::write(sessions.join("notes.txt"), "not a month file").expect("stray file");
        let loaded = load_all(&sessions);
        assert_eq!(loaded.files, 2);
        assert_eq!(loaded.records.len(), 2);
        assert_eq!(loaded.records[0].started, 1_755_000_000);
        assert_eq!(loaded.records[1].started, 1_758_124_800);
        done(&dir);
    }

    #[test]
    fn broken_and_invalid_utf8_lines_are_skipped_with_locations() {
        let dir = temp("broken");
        let sessions = dir.join("sessions");
        let path = sessions.join("2026-09.log");
        let good = line::write(&sample(1_758_124_800, 0));
        let mut bytes = Vec::new();
        bytes.extend_from_slice(good.as_bytes());
        bytes.extend_from_slice(b"\nqf1;this;is;broken\n");
        bytes.extend_from_slice(b"qf1;\xff\xfe;bad bytes\n");
        bytes.extend_from_slice(good.as_bytes());
        bytes.push(b'\n');
        fs::create_dir_all(&sessions).expect("folder");
        fs::write(&path, bytes).expect("file");

        let loaded = load_all(&sessions);
        assert_eq!(loaded.records.len(), 2);
        assert_eq!(loaded.diagnostics.len(), 2, "{:?}", loaded.diagnostics);
        let lines: Vec<(usize, usize)> = loaded
            .diagnostics
            .iter()
            .map(|d| d.location.as_ref().map(|l| (l.line, l.column)).expect("located"))
            .collect();
        assert!(lines.contains(&(2, 1)), "{lines:?}");
        assert!(lines.contains(&(3, 1)), "{lines:?}");
        assert!(
            loaded
                .diagnostics
                .iter()
                .all(|d| d.location.as_ref().is_some_and(|l| l.file == path.display().to_string()))
        );
        done(&dir);
    }

    #[test]
    fn unknown_versions_are_counted_not_reported() {
        let dir = temp("versions");
        let sessions = dir.join("sessions");
        fs::create_dir_all(&sessions).expect("folder");
        let good = line::write(&sample(1_758_124_800, 0));
        fs::write(sessions.join("2026-09.log"), format!("qf2;something;new\n{good}\nqf3;newer\n")).expect("file");
        let loaded = load_all(&sessions);
        assert_eq!(loaded.records.len(), 1);
        assert_eq!(loaded.unknown_version, 2);
        assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
        done(&dir);
    }

    #[test]
    fn an_unreadable_file_does_not_stop_the_others() {
        let dir = temp("unreadable");
        let sessions = dir.join("sessions");
        append(&sessions.join("2026-09.log"), &sample(1_758_124_800, 0)).expect("september");
        // A folder with the extension of a month file: it is listed but is not a file.
        fs::create_dir_all(sessions.join("2026-08.log")).expect("folder");
        let loaded = load_all(&sessions);
        assert_eq!(loaded.records.len(), 1);
        assert_eq!(loaded.files, 1);
        done(&dir);
    }

    #[test]
    fn too_long_is_refused_before_writing() {
        let dir = temp("toolong");
        let path = dir.join("sessions").join("2026-09.log");
        let mut session = sample(1_758_124_800, 0);
        session.note = "x".repeat(MAX_LINE_BYTES);
        match append(&path, &session) {
            Err(AppendError::TooLong { bytes }) => assert!(bytes > MAX_LINE_BYTES),
            other => panic!("expected TooLong, got {other:?}"),
        }
        assert!(!path.exists());
        done(&dir);
    }

    #[test]
    fn io_errors_name_the_path() {
        let dir = temp("io");
        let blocker = dir.join("sessions");
        fs::write(&blocker, "a file where the folder should be").expect("blocker");
        let path = blocker.join("2026-09.log");
        match append(&path, &sample(1_758_124_800, 0)) {
            Err(AppendError::Io(reported, _)) => assert_eq!(reported, path),
            other => panic!("expected Io, got {other:?}"),
        }
        done(&dir);
    }
}
