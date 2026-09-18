//! The running counter on disk, so a crash or a power cut does not lose the session in progress.
//!
//! While a counter runs its state is written to `running-<machine>.toml` every few seconds and at
//! every change of span. It holds the focus, the start, the spans, the break and the flags, and
//! when there are ones the countdown target and where a silence nobody answered for began. On the
//! next start the file is offered back to the person: save it, carry on from it, or drop it. A
//! broken file is never rewritten: an instance that may write moves it aside under a time-stamped
//! name in the same folder, so the next counter cannot overwrite it, and an instance that only
//! looks leaves it where it is.
//!
//! `watch` says who measures the counter: a window that refreshes the file every few seconds, or
//! nobody, for a counter the command line started or a window left running. How far a counter
//! reaches when the program comes back depends on it; see [`crate::liveness`].
//!
//! The first line, `format = 2`, says the file comes from a writer that records every silence
//! still waiting for an answer under `idle-from`. Files without it came before that and are read
//! as they were written, with the one guess [`Running::read`] documents.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use qframe::diagnostics::{Diagnostic, Location};
use qframe::storage::atomic_write;
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use crate::id::Id;
use crate::session::Flag;
use crate::span::{ClockSource, Span, SpanKind};

/// Who measures a running counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Watch {
    /// A window measures it and refreshes the file every few seconds; the counter is alive while
    /// that window is.
    Window,
    /// Nothing measures it: the command line started it, or a window was left with the counter
    /// running. It goes on counting until the machine restarts.
    None,
}

impl Watch {
    /// The word it is written as.
    fn word(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::None => "none",
        }
    }

    /// The watch a word names.
    fn parse(word: &str) -> Option<Self> {
        match word {
            "window" => Some(Self::Window),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

/// The state of a counter, as much of it as the last refresh saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    /// The focus being worked on.
    pub focus: Id,
    /// When the counter started, in seconds since the Unix epoch.
    pub started: i64,
    /// Minutes the local time was ahead of UTC when it started.
    pub offset_minutes: i16,
    /// The spans so far, the open one closed at the last refresh.
    pub spans: Vec<Span>,
    /// Whether the person was on a break at the last refresh.
    pub paused: bool,
    /// The wall clock at the last refresh, in seconds since the Unix epoch.
    pub refreshed: i64,
    /// Anything worth saying about the session so far.
    pub flags: Vec<Flag>,
    /// Seconds of work the counter counts down to, when it was started with a countdown; kept
    /// here so a counter carried on after a crash still counts down.
    pub target: Option<u32>,
    /// The session offset where the latest silence began, while nobody has said what it was;
    /// kept here so a counter carried on after a crash asks about it again.
    pub idle_from: Option<u32>,
    /// Who measures the counter. Files without the key come from windows, which were the only
    /// writers that kept a counter until the command line learned to.
    pub watch: Watch,
}

/// The format the writer emits and the only marked one the reader knows.
const FORMAT: i64 = 2;

impl Running {
    /// The state as TOML.
    #[must_use]
    pub fn write(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("format = {FORMAT}\n"));
        out.push_str(&format!("focus = \"{}\"\n", self.focus));
        out.push_str(&format!("started = {}\n", self.started));
        out.push_str(&format!("offset = {}\n", self.offset_minutes));
        out.push_str(&format!("paused = {}\n", self.paused));
        out.push_str(&format!("refreshed = {}\n", self.refreshed));
        let flags = self.flags.iter().map(|flag| format!("\"{}\"", flag.as_str())).collect::<Vec<_>>().join(", ");
        out.push_str(&format!("flags = [{flags}]\n"));
        if let Some(target) = self.target {
            out.push_str(&format!("target = {target}\n"));
        }
        if let Some(from) = self.idle_from {
            out.push_str(&format!("idle-from = {from}\n"));
        }
        out.push_str(&format!("watch = \"{}\"\n", self.watch.word()));
        for span in &self.spans {
            out.push_str("\n[[span]]\n");
            out.push_str(&format!("kind = \"{}\"\n", span_kind_word(span.kind)));
            out.push_str(&format!("offset = {}\n", span.offset));
            out.push_str(&format!("seconds = {}\n", span.seconds));
            out.push_str(&format!("clock = \"{}\"\n", clock_word(span.clock)));
        }
        out
    }

    /// The spans grown to `until`, the wall second the counter is known to reach: the ones on
    /// disk plus the time since the last refresh as the kind that was open, measured by the wall
    /// clock. Nothing is added for an `until` at or before the refresh.
    #[must_use]
    pub fn spans_until(&self, until: i64) -> Vec<Span> {
        let mut spans = self.spans.clone();
        let end = spans.last().map_or(0, |span| u32::try_from(span.end()).unwrap_or(u32::MAX));
        let unseen = u32::try_from(until.saturating_sub(self.refreshed)).unwrap_or(0);
        if unseen > 0 {
            let kind = if self.paused { SpanKind::Pause } else { SpanKind::Work };
            spans.push(Span::new(kind, end, unseen, ClockSource::Wall));
        }
        spans
    }

    /// Seconds of work the counter holds when it reaches `until`; see [`Running::spans_until`].
    #[must_use]
    pub fn work_until(&self, until: i64) -> u64 {
        crate::span::work_seconds(&self.spans_until(until))
    }

    /// Reads the state back from `text`, naming `file` in every diagnostic.
    ///
    /// A file without the `format` marker that carries the unanswered-idle flag and idle spans is
    /// taken to be waiting for an answer about its latest silence, since such a file cannot say
    /// which one it meant.
    ///
    /// # Errors
    ///
    /// Everything wrong with the file, each pointing at a line and column where it can. A counter
    /// with a field missing or unreadable is not partly restored: a wrong time would be worse than
    /// asking the person.
    pub fn read(file: &str, text: &str) -> Result<Self, Vec<Diagnostic>> {
        let mut diagnostics = Vec::new();
        let (root, errors) = DeTable::parse_recoverable(text);
        for error in &errors {
            let location = error.span().map(|span| Location::from_offset(file, text, span.start));
            diagnostics.push(Diagnostic::error(location, error.message().to_owned()));
        }
        let table = root.get_ref();
        let mut reader = Reader { file, text, table, diagnostics: &mut diagnostics };

        let focus = reader.string("focus").and_then(|(value, location)| match Id::parse(&value) {
            Some(id) => Some(id),
            None => {
                reader.diagnostics.push(Diagnostic::error(Some(location), "focus is not an identifier"));
                None
            }
        });
        let started = reader.integer("started");
        let offset_minutes = reader.integer("offset").and_then(|(value, location)| match i16::try_from(value) {
            Ok(offset) => Some(offset),
            Err(_) => {
                reader.diagnostics.push(Diagnostic::error(Some(location), "offset does not fit in minutes"));
                None
            }
        });
        let refreshed = reader.integer("refreshed");
        let marked = reader.format();
        let paused = reader.boolean("paused").unwrap_or(false);
        let flags = reader.flags();
        // Files written before countdowns were kept have no target; they count up as before.
        let target = reader.optional_seconds("target");
        let idle_from = reader.optional_seconds("idle-from");
        let watch =
            if reader.entry("watch").is_some() { reader.word("watch", Watch::parse) } else { Some(Watch::Window) };
        let spans = reader.spans();
        // A marked file says every unanswered silence under `idle-from`, so its absence there
        // means none; only an unmarked, older file has to be guessed about.
        let idle_from = if marked { idle_from } else { idle_from.or_else(|| unanswered_silence(&flags, &spans)) };

        match (focus, started, offset_minutes, refreshed, watch) {
            (Some(focus), Some((started, _)), Some(offset_minutes), Some((refreshed, _)), Some(watch))
                if diagnostics.is_empty() =>
            {
                Ok(Self { focus, started, offset_minutes, spans, paused, refreshed, flags, target, idle_from, watch })
            }
            _ => Err(diagnostics),
        }
    }
}

/// Where the latest silence of an unmarked file began, when the file says a silence went
/// unanswered: the start of the last run of idle spans, a sleep inside it included, as the timer
/// measured it. Files written before the unanswered stretch was kept do not say which silence the
/// flag is about, and the latest is the one the question was left open on; not asking would
/// leave that time out for good. A marked file is never guessed about: its flag may stand for a
/// silence already answered "don't count", which must not be asked again.
fn unanswered_silence(flags: &[Flag], spans: &[Span]) -> Option<u32> {
    if !flags.contains(&Flag::UnclaimedIdle) {
        return None;
    }
    let last = spans.iter().rposition(|span| span.kind == SpanKind::Idle)?;
    let mut from = spans[last].offset;
    for span in spans[..last].iter().rev() {
        match span.kind {
            SpanKind::Idle => from = span.offset,
            SpanKind::Gap => {}
            SpanKind::Work | SpanKind::Pause => break,
        }
    }
    Some(from)
}

/// Writes `running` to `path` so that a crash leaves either the old file or the new one.
///
/// # Errors
///
/// The I/O error of the step that failed; the file that was there is untouched.
pub fn save(path: &Path, running: &Running) -> io::Result<()> {
    atomic_write(path, running.write().as_bytes())
}

/// Reads the running file at `path`: `None` when there is none, `Some(Err)` when it is there but
/// cannot be read or understood. Reading never changes the file; moving a broken one aside is
/// [`set_aside`]'s job.
#[must_use]
pub fn load(path: &Path) -> Option<Result<Running, Vec<Diagnostic>>> {
    let name = path.display().to_string();
    match fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Some(Running::read(&name, &text)),
            Err(error) => {
                let offset = error.utf8_error().valid_up_to();
                let location = Location { file: name, line: 1, column: 1 };
                let message = format!("not valid UTF-8 at byte {}", offset + 1);
                Some(Err(vec![Diagnostic::error(Some(location), message)]))
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            let location = Location { file: name, line: 1, column: 1 };
            Some(Err(vec![Diagnostic::error(Some(location), format!("cannot read the file: {error}"))]))
        }
    }
}

/// Renames the running file at `path` to the first name `aside` gives for attempts 1, 2, 3, ...
/// that is not taken, and answers that name.
///
/// A rename within one folder keeps every byte and is atomic: a crash leaves the file under one
/// name or the other, never half of it. A name that is taken is skipped rather than overwritten.
/// The check and the rename are two steps, which is enough here: only the instance holding this
/// machine's lock sets files aside, and the names carry the machine, so nobody else makes them.
///
/// # Errors
///
/// [`io::ErrorKind::NotFound`] when there is no file at `path`, else the error of looking at a
/// name or of the rename. The file stays where it was.
pub fn set_aside(path: &Path, aside: impl Fn(u32) -> PathBuf) -> io::Result<PathBuf> {
    fs::symlink_metadata(path)?;
    let mut attempt = 1;
    loop {
        let target = aside(attempt);
        if !target.try_exists()? {
            fs::rename(path, &target)?;
            return Ok(target);
        }
        attempt += 1;
    }
}

/// Removes the running file at `path`. A file that is already gone is not an error.
///
/// # Errors
///
/// The I/O error when the file is there and cannot be removed.
pub fn clear(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// The word a span kind is written as.
fn span_kind_word(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::Work => "work",
        SpanKind::Pause => "pause",
        SpanKind::Gap => "gap",
        SpanKind::Idle => "idle",
    }
}

/// The span kind a word names.
fn span_kind(word: &str) -> Option<SpanKind> {
    match word {
        "work" => Some(SpanKind::Work),
        "pause" => Some(SpanKind::Pause),
        "gap" => Some(SpanKind::Gap),
        "idle" => Some(SpanKind::Idle),
        _ => None,
    }
}

/// The word a clock source is written as.
fn clock_word(clock: ClockSource) -> &'static str {
    match clock {
        ClockSource::Mono => "mono",
        ClockSource::Boot => "boot",
        ClockSource::Wall => "wall",
    }
}

/// The clock source a word names.
fn clock_source(word: &str) -> Option<ClockSource> {
    match word {
        "mono" => Some(ClockSource::Mono),
        "boot" => Some(ClockSource::Boot),
        "wall" => Some(ClockSource::Wall),
        _ => None,
    }
}

/// Reads the values of one table, turning every missing or mistyped one into a diagnostic that
/// points at the value, or at the start of the table when the value is not there at all.
struct Reader<'a, 'i> {
    file: &'a str,
    text: &'a str,
    table: &'a DeTable<'i>,
    diagnostics: &'a mut Vec<Diagnostic>,
}

impl<'a, 'i> Reader<'a, 'i> {
    /// The value under `key`, if there is one. The reference lives as long as the table, not as
    /// long as this borrow of the reader, so a diagnostic can be pushed while it is held.
    fn entry(&self, key: &str) -> Option<&'a Spanned<DeValue<'i>>> {
        self.table.iter().find(|(name, _)| name.get_ref().as_ref() == key).map(|(_, value)| value)
    }

    /// Where a value sits in the file.
    fn locate(&self, value: &Spanned<DeValue<'i>>) -> Location {
        Location::from_offset(self.file, self.text, value.span().start)
    }

    /// Where the table itself sits, for a value that is missing.
    fn table_location(&self) -> Location {
        let start = self.table.iter().next().map_or(0, |(name, _)| name.span().start);
        Location::from_offset(self.file, self.text, start)
    }

    /// A required text value.
    fn string(&mut self, key: &str) -> Option<(String, Location)> {
        match self.entry(key) {
            Some(value) => match value.get_ref() {
                DeValue::String(text) => Some((text.to_string(), self.locate(value))),
                _ => self.wrong(value, key, "text"),
            },
            None => self.missing(key),
        }
    }

    /// A required whole number.
    fn integer(&mut self, key: &str) -> Option<(i64, Location)> {
        match self.entry(key) {
            Some(value) => match value.get_ref() {
                DeValue::Integer(number) => match number.as_str().replace('_', "").parse() {
                    Ok(parsed) => Some((parsed, self.locate(value))),
                    Err(_) => self.wrong(value, key, "a whole number"),
                },
                _ => self.wrong(value, key, "a whole number"),
            },
            None => self.missing(key),
        }
    }

    /// An optional true or false value; a wrong type is still a mistake.
    fn boolean(&mut self, key: &str) -> Option<bool> {
        let value = self.entry(key)?;
        match value.get_ref() {
            DeValue::Boolean(flag) => Some(*flag),
            _ => self.wrong(value, key, "true or false"),
        }
    }

    /// The `flags` list; missing means none.
    fn flags(&mut self) -> Vec<Flag> {
        let Some(value) = self.entry("flags") else { return Vec::new() };
        let DeValue::Array(items) = value.get_ref() else {
            self.wrong::<()>(value, "flags", "a list");
            return Vec::new();
        };
        let mut flags = Vec::new();
        for item in items {
            match item.get_ref() {
                DeValue::String(word) => match Flag::parse(word) {
                    Some(flag) => flags.push(flag),
                    None => {
                        let location = self.locate(item);
                        self.diagnostics.push(Diagnostic::error(Some(location), format!("unknown flag {word:?}")));
                    }
                },
                _ => {
                    let location = self.locate(item);
                    self.diagnostics.push(Diagnostic::error(Some(location), "a flag is a word in quotes"));
                }
            }
        }
        flags
    }

    /// The `[[span]]` tables; missing means none.
    fn spans(&mut self) -> Vec<Span> {
        let Some(value) = self.entry("span") else { return Vec::new() };
        let DeValue::Array(items) = value.get_ref() else {
            self.wrong::<()>(value, "span", "a list of tables");
            return Vec::new();
        };
        let mut spans = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let DeValue::Table(table) = item.get_ref() else {
                let location = self.locate(item);
                self.diagnostics.push(Diagnostic::error(Some(location), format!("span {} is not a table", index + 1)));
                continue;
            };
            let mut inner = Reader { file: self.file, text: self.text, table, diagnostics: &mut *self.diagnostics };
            let kind = inner.word("kind", span_kind);
            let offset = inner.seconds("offset");
            let seconds = inner.seconds("seconds");
            let clock = inner.word("clock", clock_source);
            if let (Some(kind), Some(offset), Some(seconds), Some(clock)) = (kind, offset, seconds, clock) {
                spans.push(Span::new(kind, offset, seconds, clock));
            }
        }
        spans
    }

    /// A required word that `parse` knows.
    fn word<T>(&mut self, key: &str, parse: fn(&str) -> Option<T>) -> Option<T> {
        let (text, location) = self.string(key)?;
        match parse(&text) {
            Some(value) => Some(value),
            None => {
                self.diagnostics.push(Diagnostic::error(Some(location), format!("unknown {key} {text:?}")));
                None
            }
        }
    }

    /// A required count of seconds.
    fn seconds(&mut self, key: &str) -> Option<u32> {
        let (number, location) = self.integer(key)?;
        match u32::try_from(number) {
            Ok(seconds) => Some(seconds),
            Err(_) => {
                self.diagnostics.push(Diagnostic::error(Some(location), format!("{key} is not a count of seconds")));
                None
            }
        }
    }

    /// The `format` marker: `true` when it is there and known, `false` when the file has none.
    /// A marker of another value or type is a mistake, since the file cannot be read as its
    /// writer meant.
    fn format(&mut self) -> bool {
        if self.entry("format").is_none() {
            return false;
        }
        match self.integer("format") {
            Some((FORMAT, _)) => true,
            Some((other, location)) => {
                self.diagnostics.push(Diagnostic::error(Some(location), format!("unknown format {other}")));
                true
            }
            None => true,
        }
    }

    /// An optional count of seconds; a wrong type or a negative number is still a mistake.
    fn optional_seconds(&mut self, key: &str) -> Option<u32> {
        self.entry(key)?;
        self.seconds(key)
    }

    /// Reports a value of the wrong type and answers nothing.
    fn wrong<T>(&mut self, value: &Spanned<DeValue<'i>>, key: &str, expected: &str) -> Option<T> {
        let location = self.locate(value);
        self.diagnostics.push(Diagnostic::error(Some(location), format!("{key} is not {expected}")));
        None
    }

    /// Reports a value that is not there and answers nothing.
    fn missing<T>(&mut self, key: &str) -> Option<T> {
        let location = self.table_location();
        self.diagnostics.push(Diagnostic::error(Some(location), format!("{key} is missing")));
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qfocus-test-running-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("test folder");
        dir
    }

    fn sample() -> Running {
        Running {
            focus: Id::new(1_758_124_800_000, 2),
            started: 1_758_124_800,
            offset_minutes: 180,
            spans: vec![
                Span::new(SpanKind::Work, 0, 90, ClockSource::Mono),
                Span::new(SpanKind::Gap, 90, 30, ClockSource::Boot),
                Span::new(SpanKind::Pause, 120, 10, ClockSource::Mono),
            ],
            paused: true,
            refreshed: 1_758_124_930,
            flags: vec![Flag::SuspectClock, Flag::OverCeiling],
            target: None,
            idle_from: None,
            watch: Watch::Window,
        }
    }

    #[test]
    fn who_watches_the_counter_is_written_and_read_back() {
        let window = sample().write();
        assert!(window.contains("watch = \"window\"\n"), "{window}");
        let alone = Running { watch: Watch::None, ..sample() };
        let text = alone.write();
        assert!(text.contains("watch = \"none\"\n"), "{text}");
        assert!(text.starts_with("format = 2\n"), "the format stays");
        assert_eq!(Running::read("running.toml", &text), Ok(alone));
    }

    #[test]
    fn a_file_without_watch_reads_as_watched_by_a_window() {
        let text = sample().write().replace("watch = \"window\"\n", "");
        assert_eq!(Running::read("running.toml", &text).map(|running| running.watch), Ok(Watch::Window));
    }

    #[test]
    fn an_unknown_watch_gives_a_located_diagnostic() {
        let text = sample().write().replace("watch = \"window\"", "watch = \"daemon\"");
        let Err(diagnostics) = Running::read("running.toml", &text) else { panic!("read an unknown watch") };
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("unknown watch"), "{diagnostics:?}");
        assert_eq!(diagnostics[0].location.as_ref().map(|at| at.line), Some(8));
    }

    #[test]
    fn the_time_since_the_refresh_is_the_open_kind_up_to_where_the_counter_reaches() {
        let paused = sample();
        assert_eq!(paused.work_until(paused.refreshed + 600), 90, "a break stays a break");
        let working = Running { paused: false, ..sample() };
        assert_eq!(working.work_until(working.refreshed + 600), 690);
        assert_eq!(
            working.spans_until(working.refreshed + 600).last(),
            Some(&Span::new(SpanKind::Work, 130, 600, ClockSource::Wall))
        );
        assert_eq!(working.spans_until(working.refreshed - 50), working.spans, "nothing before the refresh");
    }

    #[test]
    fn writes_and_reads_back_the_same_state() {
        let running = sample();
        let text = running.write();
        assert!(text.contains("[[span]]\nkind = \"work\"\noffset = 0\nseconds = 90\nclock = \"mono\"\n"), "{text}");
        assert_eq!(Running::read("running.toml", &text), Ok(running));
    }

    #[test]
    fn a_counter_without_spans_or_flags_reads_back() {
        let running = Running { spans: Vec::new(), flags: Vec::new(), paused: false, ..sample() };
        assert_eq!(Running::read("running.toml", &running.write()), Ok(running));
    }

    #[test]
    fn a_countdown_target_is_written_and_read_back() {
        let running = Running { target: Some(1_500), ..sample() };
        let text = running.write();
        assert!(text.contains("target = 1500\n"), "{text}");
        assert_eq!(Running::read("running.toml", &text), Ok(running));
        assert!(!sample().write().contains("target"), "no countdown, no key");
    }

    #[test]
    fn a_file_written_before_countdowns_were_kept_reads_without_one() {
        let focus = sample().focus;
        let text = format!(
            "focus = \"{focus}\"\nstarted = 1758124800\noffset = 180\npaused = false\nrefreshed = 1758124930\nflags = []\n\n[[span]]\nkind = \"work\"\noffset = 0\nseconds = 130\nclock = \"mono\"\n"
        );
        let running = Running::read("running.toml", &text).expect("an old file reads");
        assert_eq!(running.target, None);
        assert_eq!(running.spans, vec![Span::new(SpanKind::Work, 0, 130, ClockSource::Mono)]);
    }

    #[test]
    fn a_broken_target_gives_a_located_diagnostic() {
        let base = sample().write();
        for (value, message) in
            [("\"soon\"", "target is not a whole number"), ("-5", "target is not a count of seconds")]
        {
            let text = base.replacen("flags = ", &format!("target = {value}\nflags = "), 1);
            let Err(diagnostics) = Running::read("running.toml", &text) else { panic!("read a broken target") };
            assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
            assert!(diagnostics[0].message.contains(message), "{}", diagnostics[0]);
            assert!(diagnostics[0].to_string().contains("running.toml:7:10"), "{}", diagnostics[0]);
        }
    }

    fn idle(offset: u32, seconds: u32) -> Span {
        Span::new(SpanKind::Idle, offset, seconds, ClockSource::Mono)
    }

    #[test]
    fn an_unanswered_silence_is_written_and_read_back() {
        let running = Running {
            spans: vec![Span::new(SpanKind::Work, 0, 600, ClockSource::Mono), idle(600, 900)],
            paused: false,
            flags: vec![Flag::UnclaimedIdle],
            idle_from: Some(600),
            ..sample()
        };
        let text = running.write();
        assert!(text.contains("idle-from = 600\n"), "{text}");
        assert_eq!(Running::read("running.toml", &text), Ok(running));
        assert!(!sample().write().contains("idle-from"), "nothing unanswered, no key");
    }

    #[test]
    fn a_file_without_idle_from_asks_about_its_latest_silence() {
        let spans = vec![
            Span::new(SpanKind::Work, 0, 300, ClockSource::Mono),
            idle(300, 200),
            Span::new(SpanKind::Work, 500, 100, ClockSource::Mono),
            idle(600, 100),
            Span::new(SpanKind::Gap, 700, 400, ClockSource::Boot),
            idle(1_100, 50),
            Span::new(SpanKind::Work, 1_150, 50, ClockSource::Mono),
        ];
        let flagged = Running { spans: spans.clone(), flags: vec![Flag::UnclaimedIdle], ..sample() };
        let unmarked = |running: &Running| running.write().replacen("format = 2\n", "", 1);
        let read = Running::read("running.toml", &unmarked(&flagged)).expect("reads");
        assert_eq!(read.idle_from, Some(600), "the last run of idle spans, the sleep inside it included");
        let unflagged = Running { spans, flags: Vec::new(), ..sample() };
        assert_eq!(Running::read("running.toml", &unmarked(&unflagged)).map(|found| found.idle_from), Ok(None));
        let no_idle = Running { flags: vec![Flag::UnclaimedIdle], ..sample() };
        assert_eq!(Running::read("running.toml", &unmarked(&no_idle)).map(|found| found.idle_from), Ok(None));
    }

    #[test]
    fn a_marked_file_without_idle_from_asks_about_nothing() {
        // The silence was answered "don't count": the flag and the idle span stay, the question
        // is gone, and a crash must not bring it back.
        let answered = Running {
            spans: vec![Span::new(SpanKind::Work, 0, 300, ClockSource::Mono), idle(300, 200)],
            flags: vec![Flag::UnclaimedIdle],
            ..sample()
        };
        let text = answered.write();
        assert!(text.starts_with("format = 2\n"), "{text}");
        assert_eq!(Running::read("running.toml", &text), Ok(answered));
    }

    #[test]
    fn an_unknown_format_gives_a_located_diagnostic() {
        let base = sample().write();
        for (value, message) in [("3", "unknown format 3"), ("\"two\"", "format is not a whole number")] {
            let text = base.replacen("format = 2", &format!("format = {value}"), 1);
            let Err(diagnostics) = Running::read("running.toml", &text) else { panic!("read an unknown format") };
            assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
            assert!(diagnostics[0].to_string().contains("running.toml:1:10"), "{}", diagnostics[0]);
            assert!(diagnostics[0].message.contains(message), "{}", diagnostics[0]);
        }
    }

    #[test]
    fn a_broken_idle_from_gives_a_located_diagnostic() {
        let text = sample().write().replacen("flags = ", "idle-from = -1\nflags = ", 1);
        let Err(diagnostics) = Running::read("running.toml", &text) else { panic!("read a broken idle-from") };
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert!(diagnostics[0].to_string().contains("running.toml:7:13"), "{}", diagnostics[0]);
        assert!(diagnostics[0].message.contains("idle-from is not a count of seconds"), "{}", diagnostics[0]);
    }

    #[test]
    fn a_broken_file_gives_located_diagnostics() {
        let text = "focus = \"not an id\"\nstarted = \"soon\"\noffset = 180\nrefreshed = 5\n\n[[span]]\nkind = \"nap\"\noffset = 0\nseconds = 1\nclock = \"mono\"\n";
        let Err(diagnostics) = Running::read("running.toml", text) else { panic!("read a broken file") };
        let messages: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
        assert_eq!(diagnostics.len(), 3, "{messages:?}");
        assert!(messages[0].contains("running.toml:1:9"), "{messages:?}");
        assert!(messages[1].contains("running.toml:2:11"), "{messages:?}");
        assert!(messages[2].contains("running.toml:7:8"), "{messages:?}");
    }

    #[test]
    fn a_missing_field_is_reported() {
        let Err(diagnostics) = Running::read("running.toml", "started = 1\noffset = 0\nrefreshed = 2\n") else {
            panic!("read a file without a focus")
        };
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("focus is missing"), "{}", diagnostics[0]);
    }

    #[test]
    fn syntax_errors_are_reported_where_they_are() {
        let Err(diagnostics) = Running::read("running.toml", "focus = \n") else { panic!("read broken toml") };
        assert!(!diagnostics.is_empty());
        assert!(diagnostics[0].location.as_ref().is_some_and(|l| l.file == "running.toml" && l.line == 1));
    }

    #[test]
    fn save_load_clear_round_trip() {
        let dir = temp("roundtrip");
        let path = dir.join("running-test.toml");
        assert!(load(&path).is_none());
        save(&path, &sample()).expect("save");
        assert_eq!(load(&path), Some(Ok(sample())));
        clear(&path).expect("clear");
        assert!(load(&path).is_none());
        clear(&path).expect("clearing twice is fine");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_on_disk_is_left_in_place() {
        let dir = temp("broken");
        let path = dir.join("running-test.toml");
        fs::write(&path, b"focus = \"x\"\n\xff").expect("file");
        let Some(Err(diagnostics)) = load(&path) else { panic!("read a broken file") };
        assert!(diagnostics[0].location.as_ref().is_some_and(|l| l.file == path.display().to_string()));
        assert!(path.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
