//! The command line: `start`, `stop`, `status` and `today` without opening the interface.
//!
//! Every command reads and writes the same data folder the interface uses, through the same
//! store, so a counter started here is the one the interface finds when it opens. `status` and
//! `today` never take the lock, because they are meant for a status bar that asks every few
//! seconds while a window may be open and a `stop` given at the same moment must not be refused.
//! What they write is only the moment they saw a counter nobody measures, in a file of its own;
//! that is the proof of how far the counter reaches if the machine restarts
//! ([`crate::liveness`]).
//!
//! [`run`] is the thin wrapper the binaries call: it reads the real clocks, the real data folder
//! and the system language. [`execute`] underneath takes all of those as values, so a test can
//! hand it a folder of its own and a moment of its choosing.

use std::fmt::Write as _;
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::Arc;

use qframe::date::{DateTime, TimeOfDay, local_offset};
use qframe::i18n::{I18n, scope};
use qframe::t;
use qframe::uptime::Uptime;

use crate::day;
use crate::duration::{self, Units};
use crate::id::Id;
use crate::liveness::{self, Cut, Reach, Reason};
use crate::session::Source;
use crate::store::{Access, Paths, Running, Store, Watch};
use crate::timer::{Clocks, Timer};
use crate::tree::Tree;
use crate::{clock, locales};

/// The command did what was asked.
pub const EXIT_OK: u8 = 0;
/// The focus was not found, the name was ambiguous, the arguments made no sense, or a file could
/// not be written.
pub const EXIT_NOT_FOUND: u8 = 1;
/// Another instance holds the lock, so nothing may be written.
pub const EXIT_LOCKED: u8 = 2;
/// A counter is already running and `--switch` was not given.
pub const EXIT_ALREADY_RUNNING: u8 = 3;
/// No counter is running.
pub const EXIT_NOTHING_RUNNING: u8 = 4;

/// What the arguments ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start a counter on the focus called `name`, stopping a running one first when `switch`.
    Start {
        /// A focus name, or `category/focus` when the name alone is not unique.
        name: String,
        /// Whether a running counter is stopped and recorded instead of refusing.
        switch: bool,
    },
    /// Stop the running counter and record it.
    Stop,
    /// Say what is running and for how long.
    Status,
    /// Say how much work today holds.
    Today,
    /// Print the usage text.
    Help,
}

/// A command and how to print its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// What to do.
    pub command: Command,
    /// Whether the answer is one JSON object instead of text.
    pub json: bool,
}

/// Why the arguments could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The first word is not a command.
    UnknownCommand(String),
    /// `start` came without a focus name.
    MissingName,
}

/// The clocks and the time zone at the moment a command runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moment {
    /// The wall clock and the monotonic clocks.
    pub clocks: Clocks,
    /// Minutes local time is ahead of UTC, or `None` when the system does not say; a record
    /// made then carries offset 0 and the person is told in the interface.
    pub offset_minutes: Option<i16>,
    /// Whether the clock that runs through sleep counts from the machine's boot, so a restart
    /// since a counter was last seen can be told; see [`liveness::reach`].
    pub knows_boot: bool,
}

impl Moment {
    /// The real clocks and the real time zone, right now.
    #[must_use]
    pub fn now() -> Self {
        Self { clocks: clock::now(), offset_minutes: local_offset(), knows_boot: Uptime::detects_suspend() }
    }

    /// The offset to stamp records with: the known one, else zero.
    fn offset(self) -> i16 {
        self.offset_minutes.unwrap_or(0)
    }
}

/// Reads the arguments after the program name.
///
/// `Ok(None)` means no command was given and the interface should open. Options may stand
/// anywhere; the words after `start` are joined with single spaces, so a focus called
/// `Deep work` can be named without quotes.
///
/// # Errors
///
/// [`ParseError`] when the first word is not a command or `start` has no name.
pub fn parse(args: &[String]) -> Result<Option<Request>, ParseError> {
    let mut json = false;
    let mut switch = false;
    let mut words: Vec<&str> = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            "--switch" => switch = true,
            "--help" | "-h" => return Ok(Some(Request { command: Command::Help, json })),
            word => words.push(word),
        }
    }
    let Some((first, rest)) = words.split_first() else {
        return Ok(None);
    };
    let command = match *first {
        "start" => {
            let name = rest.join(" ").trim().to_owned();
            if name.is_empty() {
                return Err(ParseError::MissingName);
            }
            Command::Start { name, switch }
        }
        "stop" => Command::Stop,
        "status" => Command::Status,
        "today" => Command::Today,
        other => return Err(ParseError::UnknownCommand(other.to_owned())),
    };
    Ok(Some(Request { command, json }))
}

/// Runs the command the arguments ask for against the real data folder, at the real moment, in
/// the system's language.
///
/// `args` are the arguments after the program name. `None` means no command was given and the
/// caller should open the interface. Everything else is answered on `out` (the one line or JSON
/// object the command produces) or `err` (what went wrong and the usage text), and the exit code
/// is one of the `EXIT_` constants. When `out` itself cannot be written the code is
/// [`EXIT_NOT_FOUND`], because nothing else can be said.
pub fn run(args: &[String], out: &mut impl Write, err: &mut impl Write) -> Option<ExitCode> {
    let request = match parse(args) {
        Ok(Some(request)) => request,
        Ok(None) => return None,
        Err(error) => {
            let code = scope(translator(None), || report_parse_error(&error, err));
            return Some(ExitCode::from(code.unwrap_or(EXIT_NOT_FOUND)));
        }
    };
    let code = scope(translator(None), || {
        if request.command == Command::Help {
            return writeln!(out, "{}", t!("cli.usage")).map(|()| EXIT_OK);
        }
        let Some(paths) = Paths::detect() else {
            writeln!(err, "{}", t!("cli.no-data-dir"))?;
            return Ok(EXIT_NOT_FOUND);
        };
        execute(&request, paths, Moment::now(), clock::new_id, out, err)
    });
    Some(ExitCode::from(code.unwrap_or(EXIT_NOT_FOUND)))
}

/// Says what was wrong with the arguments and prints the usage, both on `err`.
fn report_parse_error(error: &ParseError, err: &mut impl Write) -> io::Result<u8> {
    match error {
        ParseError::UnknownCommand(name) => writeln!(err, "{}", t!("cli.unknown-command", name = name.as_str()))?,
        ParseError::MissingName => writeln!(err, "{}", t!("cli.missing-name"))?,
    }
    writeln!(err, "{}", t!("cli.usage"))?;
    Ok(EXIT_NOT_FOUND)
}

/// The application's text with the system language active, or `code` when one is given;
/// English when neither is known.
fn translator(code: Option<&str>) -> Arc<I18n> {
    let mut i18n = I18n::builtin();
    for &(file, text) in locales() {
        i18n.add_source(file, text);
    }
    let detected = code.map(str::to_owned).or_else(|| i18n.detect(|name| std::env::var(name).ok()));
    if let Some(code) = detected {
        i18n.set_active(&code);
    }
    Arc::new(i18n)
}

/// Runs `request` against the store at `paths` as of `moment`, taking fresh identifiers from
/// `new_id`. Must run inside an [`scope`] so the text has a language.
///
/// The answer goes to `out`, problems to `err`, and the exit code is one of the `EXIT_`
/// constants. [`Command::Help`] prints the usage on `out`.
///
/// # Errors
///
/// The I/O error of writing to `out` or `err`; nothing else is an error here, everything else is
/// an exit code.
pub fn execute(
    request: &Request,
    paths: Paths,
    moment: Moment,
    new_id: impl Fn() -> Id,
    out: &mut impl Write,
    err: &mut impl Write,
) -> io::Result<u8> {
    let mut answer = Answer { json: request.json, out, err };
    match &request.command {
        Command::Help => {
            writeln!(answer.out, "{}", t!("cli.usage"))?;
            Ok(EXIT_OK)
        }
        Command::Start { name, switch } => start(name, *switch, paths, moment, &new_id, &mut answer),
        Command::Stop => stop(paths, moment, &new_id, &mut answer),
        Command::Status => status(paths, moment, &mut answer),
        Command::Today => today(paths, moment, &mut answer),
    }
}

/// Where a command's answer goes and in which shape.
struct Answer<'a, O: Write, E: Write> {
    json: bool,
    out: &'a mut O,
    err: &'a mut E,
}

impl<O: Write, E: Write> Answer<'_, O, E> {
    /// Prints `text` as the answer, or the JSON `object` (already serialised) when JSON was
    /// asked for.
    fn line(&mut self, text: &str, object: &str) -> io::Result<()> {
        if self.json { writeln!(self.out, "{object}") } else { writeln!(self.out, "{text}") }
    }

    /// Prints a problem: on `err` as text, or as the JSON `object` on `out` when JSON was asked
    /// for, so a machine reader always gets one object.
    fn problem(&mut self, text: &str, object: &str) -> io::Result<()> {
        if self.json { writeln!(self.out, "{object}") } else { writeln!(self.err, "{text}") }
    }

    /// Prints the diagnostics of a file that could not be read, always on `err`.
    fn diagnostics(&mut self, diagnostics: &[qframe::diagnostics::Diagnostic]) -> io::Result<()> {
        for diagnostic in diagnostics {
            writeln!(self.err, "{}", diagnostic.message)?;
        }
        Ok(())
    }
}

/// The name of a focus and of its category, for the answers.
struct Named {
    focus: String,
    category: Option<String>,
}

impl Named {
    /// The names of the focus `id` in `tree`, or the "unknown focus" text when the tree no
    /// longer has it.
    fn of(tree: &Tree, id: Id) -> Self {
        tree.categories
            .iter()
            .find_map(|category| {
                category
                    .focuses
                    .iter()
                    .find(|focus| focus.id == id)
                    .map(|focus| Self { focus: focus.name.clone(), category: Some(category.name.clone()) })
            })
            .unwrap_or_else(|| Self { focus: t!("cli.unknown-focus"), category: None })
    }

    /// `"focus":"…","category":"…"` for a JSON object, the category `null` when unknown.
    fn json_fields(&self) -> String {
        let category = self.category.as_deref().map_or_else(|| "null".to_owned(), json_string);
        format!("\"focus\":{},\"category\":{category}", json_string(&self.focus))
    }
}

/// The counter on disk, read for a command: nothing, a counter, or a file that cannot be read.
enum OnDisk {
    Nothing,
    Counter(Running),
    Broken,
}

/// Reads the running file, reporting a broken one on `err` once.
fn on_disk<O: Write, E: Write>(store: &Store, answer: &mut Answer<'_, O, E>) -> io::Result<OnDisk> {
    match store.running() {
        None => Ok(OnDisk::Nothing),
        Some(Ok(running)) => Ok(OnDisk::Counter(running)),
        Some(Err(diagnostics)) => {
            let path = store.paths.running_file().display().to_string();
            answer.problem(
                &t!("cli.running-broken", path = path.as_str()),
                &format!("{{\"error\":\"running-broken\",\"path\":{}}}", json_string(&path)),
            )?;
            answer.diagnostics(&diagnostics)?;
            Ok(OnDisk::Broken)
        }
    }
}

/// Opens the store for writing, or says why it cannot be written to and gives the exit code.
fn open_writer<O: Write, E: Write>(paths: Paths, answer: &mut Answer<'_, O, E>) -> io::Result<Result<Store, u8>> {
    let store = Store::open(paths);
    if let Access::ReadOnly { holder } = store.access {
        let holder = holder.map_or_else(|| "null".to_owned(), |pid| pid.to_string());
        answer.problem(&t!("cli.locked"), &format!("{{\"error\":\"locked\",\"holder\":{holder}}}"))?;
        answer.diagnostics(&store.diagnostics)?;
        return Ok(Err(EXIT_LOCKED));
    }
    Ok(Ok(store))
}

/// `start`: finds the focus, checks nothing else runs, writes the running file.
fn start<O: Write, E: Write>(
    name: &str,
    switch: bool,
    paths: Paths,
    moment: Moment,
    new_id: &impl Fn() -> Id,
    answer: &mut Answer<'_, O, E>,
) -> io::Result<u8> {
    let mut store = match open_writer(paths, answer)? {
        Ok(store) => store,
        Err(code) => return Ok(code),
    };
    let matches = find_focus(&store.tree, name);
    let focus = match matches.as_slice() {
        [] => {
            answer.problem(
                &t!("cli.not-found", name = name),
                &format!("{{\"error\":\"not-found\",\"name\":{}}}", json_string(name)),
            )?;
            return Ok(EXIT_NOT_FOUND);
        }
        [(id, _)] => *id,
        several => {
            if answer.json {
                let list: Vec<String> =
                    several.iter().map(|(_, named)| format!("{{{}}}", named.json_fields())).collect();
                writeln!(
                    answer.out,
                    "{{\"error\":\"ambiguous\",\"name\":{},\"matches\":[{}]}}",
                    json_string(name),
                    list.join(",")
                )?;
            } else {
                writeln!(answer.err, "{}", t!("cli.ambiguous", name = name))?;
                for (_, named) in several {
                    writeln!(answer.err, "{}", full_name(named))?;
                }
            }
            return Ok(EXIT_NOT_FOUND);
        }
    };
    let mut stopped = None;
    match on_disk(&store, answer)? {
        OnDisk::Nothing => {}
        OnDisk::Broken => return Ok(EXIT_NOT_FOUND),
        OnDisk::Counter(running) if switch => match record_running(&mut store, &running, moment, new_id, answer)? {
            Ok(finished) => stopped = Some(finished),
            Err(code) => return Ok(code),
        },
        OnDisk::Counter(running) => {
            let named = Named::of(&store.tree, running.focus);
            let seconds = running.work_until(reach_for_writer(&store, &running, moment).until);
            answer.problem(
                &t!(
                    "cli.already-running",
                    focus = named.focus.as_str(),
                    duration = short(seconds).as_str(),
                    name = name
                ),
                &format!("{{\"error\":\"already-running\",{},\"seconds\":{seconds}}}", named.json_fields()),
            )?;
            return Ok(EXIT_ALREADY_RUNNING);
        }
    }
    let timer = Timer::start(focus, moment.clocks, moment.offset());
    let running = Running {
        focus,
        started: timer.started(),
        offset_minutes: timer.offset_minutes(),
        spans: timer.spans(moment.clocks),
        paused: false,
        refreshed: moment.clocks.wall,
        flags: Vec::new(),
        target: None,
        idle_from: None,
        watch: Watch::None,
    };
    if let Err(error) = store.save_running(&running) {
        let path = store.paths.running_file().display().to_string();
        let text = format!("{path}: {error}");
        answer.problem(&text, &format!("{{\"error\":\"write-failed\",\"path\":{}}}", json_string(&path)))?;
        return Ok(EXIT_NOT_FOUND);
    }
    let named = Named::of(&store.tree, focus);
    let mut object = String::new();
    let _ = write!(object, "{{{},\"started\":{}", named.json_fields(), running.started);
    if let Some(Recorded { named: previous, seconds, cut }) = &stopped {
        let _ = write!(
            object,
            ",\"stopped\":{{{},\"seconds\":{seconds}{}}}",
            previous.json_fields(),
            cut_json(cut.as_ref())
        );
        if !answer.json {
            writeln!(answer.out, "{}", stopped_line(previous, *seconds, cut.as_ref(), moment))?;
        }
    }
    object.push('}');
    answer.line(&t!("cli.started", focus = named.focus.as_str()), &object)?;
    Ok(EXIT_OK)
}

/// `stop`: records the running counter and clears the file.
fn stop<O: Write, E: Write>(
    paths: Paths,
    moment: Moment,
    new_id: &impl Fn() -> Id,
    answer: &mut Answer<'_, O, E>,
) -> io::Result<u8> {
    let mut store = match open_writer(paths, answer)? {
        Ok(store) => store,
        Err(code) => return Ok(code),
    };
    let running = match on_disk(&store, answer)? {
        OnDisk::Counter(running) => running,
        OnDisk::Broken => return Ok(EXIT_NOT_FOUND),
        OnDisk::Nothing => {
            answer.problem(&t!("cli.nothing-running"), "{\"error\":\"nothing-running\"}")?;
            return Ok(EXIT_NOTHING_RUNNING);
        }
    };
    match record_running(&mut store, &running, moment, new_id, answer)? {
        Ok(Recorded { named, seconds, cut }) => {
            answer.line(
                &stopped_line(&named, seconds, cut.as_ref(), moment),
                &format!("{{{},\"seconds\":{seconds}{}}}", named.json_fields(), cut_json(cut.as_ref())),
            )?;
            Ok(EXIT_OK)
        }
        Err(code) => Ok(code),
    }
}

/// Closes `running` where it reaches as of `moment`, appends it to its month file and removes the
/// running file. Answers the names, the work recorded and where the counter was cut, if it was.
///
/// The time since the last refresh was watched by no monotonic clock, so as far as the counter
/// was alive it is added as a span of the kind that was open, measured by the wall clock; the
/// record says so in its clock source. A counter cut by a restart or a window that went away ends
/// at the cut: the time after it is not part of the session at all.
///
/// When the record cannot be written, the running file is left where it is so nothing is lost,
/// and the answer names the path.
fn record_running<O: Write, E: Write>(
    store: &mut Store,
    running: &Running,
    moment: Moment,
    new_id: &impl Fn() -> Id,
    answer: &mut Answer<'_, O, E>,
) -> io::Result<Result<Recorded, u8>> {
    let reach = reach_for_writer(store, running, moment);
    let then = Clocks { wall: reach.until, uptime: moment.clocks.uptime };
    let timer = Timer::restore(
        running.focus,
        running.started,
        running.offset_minutes,
        running.spans.clone(),
        running.paused,
        running.refreshed,
        reach.until,
        then,
    )
    .with_flags(running.flags.clone())
    .source(Source::Timer);
    let session = timer.stop(then, new_id(), moment.clocks.wall);
    let seconds = session.work_seconds();
    let named = Named::of(&store.tree, running.focus);
    if let Err(error) = store.record(&session) {
        let path = store.paths.running_file().display().to_string();
        let text = error.to_string();
        answer.problem(
            &t!("cli.record-failed", error = text.as_str(), path = path.as_str()),
            &format!(
                "{{\"error\":\"write-failed\",\"path\":{},\"detail\":{}}}",
                json_string(&path),
                json_string(&text)
            ),
        )?;
        return Ok(Err(EXIT_NOT_FOUND));
    }
    if let Err(error) = store.clear_running() {
        let path = store.paths.running_file().display().to_string();
        writeln!(answer.err, "{path}: {error}")?;
    }
    Ok(Ok(Recorded { named, seconds, cut: reach.cut }))
}

/// A counter the command line stopped and recorded.
struct Recorded {
    /// Its focus.
    named: Named,
    /// The work recorded.
    seconds: u64,
    /// Where it was cut, when it did not reach the moment it was stopped.
    cut: Option<Cut>,
}

/// `status`: one line about the running counter, nothing when there is none.
fn status<O: Write, E: Write>(paths: Paths, moment: Moment, answer: &mut Answer<'_, O, E>) -> io::Result<u8> {
    let store = Store::open_read_only(paths);
    let running = match on_disk(&store, answer)? {
        OnDisk::Counter(running) => running,
        OnDisk::Broken => return Ok(EXIT_NOT_FOUND),
        OnDisk::Nothing => {
            if answer.json {
                writeln!(answer.out, "{{\"running\":false}}")?;
            }
            return Ok(EXIT_NOTHING_RUNNING);
        }
    };
    let named = Named::of(&store.tree, running.focus);
    let reach = look(&store, &running, moment);
    let seconds = running.work_until(reach.until);
    let duration = short(seconds);
    let text = match reach.cut {
        Some(cut) => cut_line(&named, &duration, cut, moment),
        None if running.paused => t!("cli.status-paused", focus = named.focus.as_str(), duration = duration.as_str()),
        None => t!("cli.status", focus = named.focus.as_str(), duration = duration.as_str()),
    };
    answer.line(
        &text,
        &format!(
            "{{\"running\":true,{},\"seconds\":{seconds},\"paused\":{},\"started\":{}{}}}",
            named.json_fields(),
            running.paused,
            running.started,
            cut_json(reach.cut.as_ref())
        ),
    )?;
    Ok(EXIT_OK)
}

/// `today`: one line with the day's total, the running counter's work included.
fn today<O: Write, E: Write>(paths: Paths, moment: Moment, answer: &mut Answer<'_, O, E>) -> io::Result<u8> {
    let store = Store::open_read_only(paths);
    let date = DateTime::from_unix(moment.clocks.wall, moment.offset()).date;
    let recorded = day::totals(&store.sessions, date, TimeOfDay::new(0, 0, 0)).total;
    let running = match on_disk(&store, answer)? {
        OnDisk::Counter(running) => running.work_until(look(&store, &running, moment).until),
        OnDisk::Nothing | OnDisk::Broken => 0,
    };
    let seconds = recorded.saturating_add(running);
    answer.line(
        &t!("cli.today", duration = short(seconds).as_str()),
        &format!(
            "{{\"date\":\"{:04}-{:02}-{:02}\",\"seconds\":{seconds},\"recorded\":{recorded},\"running\":{running}}}",
            date.year(),
            date.month(),
            date.day()
        ),
    )?;
    Ok(EXIT_OK)
}

/// The focuses `query` names: every focus whose name is `query`, or whose `category/focus` is,
/// compared without case on trimmed names. Archived focuses and the focuses of archived
/// categories are not offered, as they are not in the lists either.
fn find_focus(tree: &Tree, query: &str) -> Vec<(Id, Named)> {
    let wanted = fold(query);
    let mut found = Vec::new();
    for category in tree.categories.iter().filter(|category| !category.archived) {
        for focus in category.focuses.iter().filter(|focus| !focus.archived) {
            let plain = fold(&focus.name);
            let full = format!("{}/{plain}", fold(&category.name));
            if plain == wanted || full == wanted {
                found.push((focus.id, Named { focus: focus.name.clone(), category: Some(category.name.clone()) }));
            }
        }
    }
    found
}

/// `name` as it is compared: trimmed and in lower case.
fn fold(name: &str) -> String {
    name.trim().to_lowercase()
}

/// `category/focus` for the list of several matches.
fn full_name(named: &Named) -> String {
    match &named.category {
        Some(category) => format!("{category}/{}", named.focus),
        None => named.focus.clone(),
    }
}

/// How far `running` reaches for a command that looks without the lock, leaving the stamp that
/// proves the counter was seen alive when one is due. A stamp that cannot be written only leaves
/// the proof older; the answer does not change, so nobody is told.
fn look(store: &Store, running: &Running, moment: Moment) -> Reach {
    let seen = store.seen();
    let reach = liveness::reach(running, seen.as_ref(), moment.clocks, moment.knows_boot, || store.lock_is_held());
    let wall = moment.clocks.wall;
    if let Some(stamp) = liveness::stamp_to_leave(running, seen.as_ref(), &reach, wall, moment.knows_boot) {
        let _ = store.save_seen(&stamp);
    }
    reach
}

/// How far `running` reaches for a command that holds the lock. No window can be measuring it
/// then, so one that was ended with its last refresh.
fn reach_for_writer(store: &Store, running: &Running, moment: Moment) -> Reach {
    liveness::reach_without_window(running, store.seen().as_ref(), moment.clocks, moment.knows_boot)
}

/// The line a status or a stop gives for a counter cut short: the focus, the work, and after
/// which moment nothing was counted and why.
fn cut_line(named: &Named, duration: &str, cut: Cut, moment: Moment) -> String {
    let time = liveness::moment_words(cut.at, moment.clocks.wall, moment.offset());
    let key = match cut.reason {
        Reason::Restart => "cli.status-cut-restart",
        Reason::WindowGone => "cli.status-cut-window",
    };
    t!(key, focus = named.focus.as_str(), duration = duration, time = time)
}

/// The line for a counter that was stopped and recorded, saying the cut when there was one.
fn stopped_line(named: &Named, seconds: u64, cut: Option<&Cut>, moment: Moment) -> String {
    let duration = short(seconds);
    match cut {
        Some(cut) => cut_line(named, &duration, *cut, moment),
        None => t!("cli.stopped", focus = named.focus.as_str(), duration = duration.as_str()),
    }
}

/// `,"cut":{"at":…,"reason":"…"}` for a JSON object when the counter was cut, else nothing.
fn cut_json(cut: Option<&Cut>) -> String {
    cut.map_or_else(String::new, |cut| format!(",\"cut\":{{\"at\":{},\"reason\":\"{}\"}}", cut.at, cut.reason.word()))
}

/// `seconds` in words, with the language's unit words.
fn short(seconds: u64) -> String {
    let (hour, minute, second) = (t!("units.hour"), t!("units.minute"), t!("units.second"));
    duration::short(seconds, &Units { hour: &hour, minute: &minute, second: &second })
}

/// `text` as a JSON string, quotes included.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if control < ' ' => {
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Flag;
    use crate::span::{ClockSource, Span, SpanKind};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use qframe::storage::AppLock;
    use qframe::uptime::Uptime;

    use crate::tree::{Category, Focus};

    /// 2026-09-18 10:00:00 UTC.
    const WALL: i64 = 1_789_725_600;
    const OFFSET: i16 = 180;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qfocus-test-cli-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn done(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    fn focus(id: u128, name: &str) -> Focus {
        Focus { id: Id::new(1, id), name: name.to_owned(), goal: None, archived: false, order: 0.0 }
    }

    fn category(id: u128, name: &str, focuses: Vec<Focus>) -> Category {
        Category {
            id: Id::new(2, id),
            name: name.to_owned(),
            icon: None,
            goal: None,
            archived: false,
            order: 0.0,
            focuses,
        }
    }

    /// Work/Rust, Work/Reading, Home/Reading, Work/Old (archived), Gone/Piano (archived category).
    fn seed(dir: &Path) -> Paths {
        let paths = Paths::at(dir, "test");
        let mut old = focus(4, "Old");
        old.archived = true;
        let mut gone = category(3, "Gone", vec![focus(5, "Piano")]);
        gone.archived = true;
        let tree = Tree {
            categories: vec![
                category(1, "Work", vec![focus(1, "Rust"), focus(2, "Reading"), old]),
                category(2, "Home", vec![focus(3, "Reading")]),
                gone,
            ],
        };
        fs::create_dir_all(dir).expect("dir");
        fs::write(paths.tree_file(), tree.write()).expect("tree");
        paths
    }

    /// A moment `seconds` after the seeded start, clocks in step, in a +03:00 zone.
    fn at(seconds: u64) -> Moment {
        let uptime = Duration::from_secs(1_000 + seconds);
        Moment {
            clocks: Clocks {
                wall: WALL + i64::try_from(seconds).expect("fits"),
                uptime: Uptime { awake: uptime, elapsed: uptime },
            },
            offset_minutes: Some(OFFSET),
            knows_boot: true,
        }
    }

    fn ids() -> impl Fn() -> Id {
        let counter = std::cell::Cell::new(0_u128);
        move || {
            counter.set(counter.get() + 1);
            Id::new(3, counter.get())
        }
    }

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    struct Outcome {
        code: u8,
        out: String,
        err: String,
    }

    fn go(paths: &Paths, moment: Moment, words: &[&str]) -> Outcome {
        go_in("en", paths, moment, words)
    }

    fn go_in(language: &str, paths: &Paths, moment: Moment, words: &[&str]) -> Outcome {
        let request = parse(&args(words)).expect("parses").expect("a command");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = scope(translator(Some(language)), || {
            execute(&request, paths.clone(), moment, ids(), &mut out, &mut err).expect("writes")
        });
        Outcome { code, out: String::from_utf8(out).expect("utf-8"), err: String::from_utf8(err).expect("utf-8") }
    }

    #[test]
    fn arguments_are_read_with_options_anywhere() {
        assert_eq!(parse(&args(&[])), Ok(None));
        assert_eq!(parse(&args(&["--json"])), Ok(None));
        assert_eq!(
            parse(&args(&["--json", "start", "Deep", "work", "--switch"])),
            Ok(Some(Request { command: Command::Start { name: "Deep work".to_owned(), switch: true }, json: true }))
        );
        assert_eq!(parse(&args(&["stop"])), Ok(Some(Request { command: Command::Stop, json: false })));
        assert_eq!(parse(&args(&["status", "--json"])), Ok(Some(Request { command: Command::Status, json: true })));
        assert_eq!(parse(&args(&["today"])), Ok(Some(Request { command: Command::Today, json: false })));
        assert_eq!(parse(&args(&["-h"])), Ok(Some(Request { command: Command::Help, json: false })));
        assert_eq!(parse(&args(&["start"])), Err(ParseError::MissingName));
        assert_eq!(parse(&args(&["dance"])), Err(ParseError::UnknownCommand("dance".to_owned())));
    }

    #[test]
    fn run_answers_help_and_unknown_commands_without_touching_data() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert!(run(&args(&[]), &mut out, &mut err).is_none());
        assert!(run(&args(&["--help"]), &mut out, &mut err).is_some());
        assert!(String::from_utf8_lossy(&out).contains("qfocus start"));
        assert!(err.is_empty());
        assert!(run(&args(&["dance"]), &mut out, &mut err).is_some());
        let err = String::from_utf8_lossy(&err);
        assert!(err.contains("dance"), "{err}");
        assert!(err.contains("qfocus start"), "{err}");
    }

    #[test]
    fn help_prints_the_usage_in_the_active_language() {
        let dir = temp("help");
        let paths = seed(&dir);
        let english = go(&paths, at(0), &["--help"]);
        assert_eq!(english.code, EXIT_OK);
        assert!(english.out.contains("open the interface"), "{}", english.out);
        let turkish = go_in("tr", &paths, at(0), &["-h"]);
        assert!(turkish.out.contains("arayüzü açar"), "{}", turkish.out);
        done(&dir);
    }

    #[test]
    fn start_then_status_then_stop_records_the_work() {
        let dir = temp("cycle");
        let paths = seed(&dir);

        let started = go(&paths, at(0), &["start", "rust"]);
        assert_eq!(started.code, EXIT_OK, "{}", started.err);
        assert_eq!(started.out, "Started Rust.\n");
        assert!(paths.running_file().exists());
        assert!(!paths.lock_file().exists() || AppLock::acquire(&paths.lock_file()).expect("lock").is_some());

        let status = go(&paths, at(2_820), &["status"]);
        assert_eq!(status.code, EXIT_OK);
        assert_eq!(status.out, "Rust · 47 min\n");

        let today = go(&paths, at(2_820), &["today"]);
        assert_eq!(today.code, EXIT_OK);
        assert_eq!(today.out, "47 min\n");

        let stopped = go(&paths, at(3_000), &["stop"]);
        assert_eq!(stopped.code, EXIT_OK, "{}", stopped.err);
        assert_eq!(stopped.out, "Rust · 50 min\n");
        assert!(!paths.running_file().exists());

        let store = Store::open_read_only(paths.clone());
        assert_eq!(store.sessions.len(), 1);
        assert_eq!(store.sessions[0].work_seconds(), 3_000);
        assert_eq!(store.sessions[0].offset_minutes, OFFSET);
        assert_eq!(store.sessions[0].started, WALL);
        assert_eq!(store.sessions[0].ended, WALL + 3_000);

        let after = go(&paths, at(3_100), &["today"]);
        assert_eq!(after.out, "50 min\n");
        let quiet = go(&paths, at(3_100), &["status"]);
        assert_eq!(quiet.code, EXIT_NOTHING_RUNNING);
        assert!(quiet.out.is_empty());
        assert!(quiet.err.is_empty());
        done(&dir);
    }

    #[test]
    fn the_name_can_carry_the_category_and_ignores_case_and_spaces() {
        let dir = temp("names");
        let paths = seed(&dir);
        let found = go(&paths, at(0), &["start", " home/READING "]);
        assert_eq!(found.code, EXIT_OK, "{}", found.err);
        assert_eq!(found.out, "Started Reading.\n");
        let running = Store::open_read_only(paths.clone()).running().expect("file").expect("readable");
        assert_eq!(running.focus, Id::new(1, 3));
        done(&dir);
    }

    #[test]
    fn a_missing_focus_and_an_archived_one_are_not_found() {
        let dir = temp("missing");
        let paths = seed(&dir);
        let missing = go(&paths, at(0), &["start", "Cooking"]);
        assert_eq!(missing.code, EXIT_NOT_FOUND);
        assert_eq!(missing.err, "No focus is called Cooking.\n");
        assert!(missing.out.is_empty());
        assert_eq!(go(&paths, at(0), &["start", "Old"]).code, EXIT_NOT_FOUND);
        assert_eq!(go(&paths, at(0), &["start", "Piano"]).code, EXIT_NOT_FOUND);
        assert!(!paths.running_file().exists());
        let json = go(&paths, at(0), &["start", "Cooking", "--json"]);
        assert_eq!(json.out, "{\"error\":\"not-found\",\"name\":\"Cooking\"}\n");
        done(&dir);
    }

    #[test]
    fn an_ambiguous_name_lists_the_matches() {
        let dir = temp("ambiguous");
        let paths = seed(&dir);
        let several = go(&paths, at(0), &["start", "Reading"]);
        assert_eq!(several.code, EXIT_NOT_FOUND);
        assert_eq!(several.err, "Reading matches more than one focus; say which one:\nWork/Reading\nHome/Reading\n");
        assert!(!paths.running_file().exists());
        let json = go(&paths, at(0), &["--json", "start", "Reading"]);
        assert_eq!(
            json.out,
            "{\"error\":\"ambiguous\",\"name\":\"Reading\",\"matches\":[{\"focus\":\"Reading\",\"category\":\"Work\"},{\"focus\":\"Reading\",\"category\":\"Home\"}]}\n"
        );
        done(&dir);
    }

    #[test]
    fn a_running_counter_refuses_a_second_start_unless_switched() {
        let dir = temp("switch");
        let paths = seed(&dir);
        assert_eq!(go(&paths, at(0), &["start", "Rust"]).code, EXIT_OK);

        let refused = go(&paths, at(600), &["start", "Home/Reading"]);
        assert_eq!(refused.code, EXIT_ALREADY_RUNNING);
        assert_eq!(refused.err, "Rust is already running for 10 min; --switch stops it and starts Home/Reading.\n");
        let json = go(&paths, at(600), &["start", "Home/Reading", "--json"]);
        assert_eq!(
            json.out,
            "{\"error\":\"already-running\",\"focus\":\"Rust\",\"category\":\"Work\",\"seconds\":600}\n"
        );

        let switched = go(&paths, at(900), &["start", "Home/Reading", "--switch"]);
        assert_eq!(switched.code, EXIT_OK, "{}", switched.err);
        assert_eq!(switched.out, "Rust · 15 min\nStarted Reading.\n");
        let store = Store::open_read_only(paths.clone());
        assert_eq!(store.sessions.len(), 1);
        assert_eq!(store.sessions[0].focus, Id::new(1, 1));
        assert_eq!(store.sessions[0].work_seconds(), 900);
        let running = store.running().expect("file").expect("readable");
        assert_eq!(running.focus, Id::new(1, 3));
        assert_eq!(running.started, WALL + 900);
        done(&dir);
    }

    #[test]
    fn stop_with_nothing_running_is_a_code_of_its_own() {
        let dir = temp("nothing");
        let paths = seed(&dir);
        let stopped = go(&paths, at(0), &["stop"]);
        assert_eq!(stopped.code, EXIT_NOTHING_RUNNING);
        assert_eq!(stopped.err, "Nothing is running.\n");
        let json = go(&paths, at(0), &["stop", "--json"]);
        assert_eq!(json.out, "{\"error\":\"nothing-running\"}\n");
        assert_eq!(json.code, EXIT_NOTHING_RUNNING);
        done(&dir);
    }

    #[test]
    fn an_open_window_holding_the_lock_blocks_writing_but_not_reading() {
        let dir = temp("locked");
        let paths = seed(&dir);
        let window = AppLock::acquire(&paths.lock_file()).expect("lock").expect("free");

        let start = go(&paths, at(0), &["start", "Rust"]);
        assert_eq!(start.code, EXIT_LOCKED);
        assert_eq!(start.err, "An open qfocus window is writing; close it or use that window.\n");
        assert!(!paths.running_file().exists());
        assert_eq!(go(&paths, at(0), &["stop"]).code, EXIT_LOCKED);
        let json = go(&paths, at(0), &["stop", "--json"]);
        assert!(json.out.starts_with("{\"error\":\"locked\",\"holder\":"), "{}", json.out);

        assert_eq!(go(&paths, at(0), &["today"]).code, EXIT_OK);
        assert_eq!(go(&paths, at(0), &["status"]).code, EXIT_NOTHING_RUNNING);
        drop(window);
        assert_eq!(go(&paths, at(0), &["start", "Rust"]).code, EXIT_OK);
        done(&dir);
    }

    #[test]
    fn stop_keeps_the_running_file_when_the_record_cannot_be_written() {
        let dir = temp("unwritable");
        let paths = seed(&dir);
        assert_eq!(go(&paths, at(0), &["start", "Rust"]).code, EXIT_OK);
        fs::write(paths.sessions_dir(), "not a folder").expect("block");

        let stopped = go(&paths, at(60), &["stop"]);
        assert_eq!(stopped.code, EXIT_NOT_FOUND);
        assert!(stopped.err.starts_with("The session could not be recorded: "), "{}", stopped.err);
        assert!(stopped.err.contains(&paths.running_file().display().to_string()), "{}", stopped.err);
        assert!(paths.running_file().exists());
        let json = go(&paths, at(60), &["stop", "--json"]);
        assert!(json.out.starts_with("{\"error\":\"write-failed\",\"path\":"), "{}", json.out);
        assert!(paths.running_file().exists());

        fs::remove_file(paths.sessions_dir()).expect("unblock");
        assert_eq!(go(&paths, at(60), &["stop"]).code, EXIT_OK);
        assert!(!paths.running_file().exists());
        done(&dir);
    }

    #[test]
    fn a_broken_running_file_is_reported_and_left_alone() {
        let dir = temp("broken");
        let paths = seed(&dir);
        fs::write(paths.running_file(), "focus = 7\n").expect("write");
        let status = go(&paths, at(0), &["status"]);
        assert_eq!(status.code, EXIT_NOT_FOUND);
        assert!(status.err.contains("cannot be read"), "{}", status.err);
        assert_eq!(go(&paths, at(0), &["stop"]).code, EXIT_NOT_FOUND);
        assert_eq!(go(&paths, at(0), &["start", "Rust"]).code, EXIT_NOT_FOUND);
        assert_eq!(fs::read_to_string(paths.running_file()).expect("still there"), "focus = 7\n");
        let today = go(&paths, at(0), &["today"]);
        assert_eq!(today.code, EXIT_OK);
        assert_eq!(today.out, "0 s\n");
        done(&dir);
    }

    #[test]
    fn json_answers_are_one_object_with_the_names_and_seconds() {
        let dir = temp("json");
        let paths = seed(&dir);
        let started = go(&paths, at(0), &["start", "Rust", "--json"]);
        assert_eq!(started.out, format!("{{\"focus\":\"Rust\",\"category\":\"Work\",\"started\":{WALL}}}\n"));
        let status = go(&paths, at(2_820), &["status", "--json"]);
        assert_eq!(
            status.out,
            format!(
                "{{\"running\":true,\"focus\":\"Rust\",\"category\":\"Work\",\"seconds\":2820,\"paused\":false,\"started\":{WALL}}}\n"
            )
        );
        let today = go(&paths, at(2_820), &["today", "--json"]);
        assert_eq!(today.out, "{\"date\":\"2026-09-18\",\"seconds\":2820,\"recorded\":0,\"running\":2820}\n");
        let switched = go(&paths, at(3_000), &["start", "Home/Reading", "--switch", "--json"]);
        assert_eq!(
            switched.out,
            format!(
                "{{\"focus\":\"Reading\",\"category\":\"Home\",\"started\":{},\"stopped\":{{\"focus\":\"Rust\",\"category\":\"Work\",\"seconds\":3000}}}}\n",
                WALL + 3_000
            )
        );
        let stopped = go(&paths, at(3_600), &["stop", "--json"]);
        assert_eq!(stopped.out, "{\"focus\":\"Reading\",\"category\":\"Home\",\"seconds\":600}\n");
        let idle = go(&paths, at(3_600), &["status", "--json"]);
        assert_eq!(idle.out, "{\"running\":false}\n");
        assert_eq!(idle.code, EXIT_NOTHING_RUNNING);
        done(&dir);
    }

    #[test]
    fn a_paused_counter_from_the_window_counts_only_its_work() {
        let dir = temp("paused");
        let paths = seed(&dir);
        let running = Running {
            focus: Id::new(1, 1),
            started: WALL,
            offset_minutes: OFFSET,
            spans: vec![
                Span::new(SpanKind::Work, 0, 600, ClockSource::Mono),
                Span::new(SpanKind::Pause, 600, 280, ClockSource::Mono),
            ],
            paused: true,
            // Refreshed twenty seconds before the look, as by a window a moment ago.
            refreshed: WALL + 880,
            flags: vec![Flag::OverCeiling],
            target: None,
            idle_from: None,
            watch: Watch::Window,
        };
        crate::store::running::save(&paths.running_file(), &running).expect("save");
        let status = go(&paths, at(900), &["status"]);
        assert_eq!(status.out, "Rust · 10 min · on a break\n");
        // Stopped with the lock, so the window is gone: it ended with its last refresh.
        let stopped = go(&paths, at(900), &["stop"]);
        assert_eq!(stopped.out, "Rust · 10 min · nothing counted after 13:14: the window closed\n");
        let store = Store::open_read_only(paths.clone());
        assert_eq!(store.sessions[0].ended, WALL + 880);
        assert_eq!(store.sessions[0].work_seconds(), 600);
        // Stopped in the ordinary way: measured by the timer, not recovered, flags kept.
        assert_eq!(store.sessions[0].source, Source::Timer);
        assert_eq!(store.sessions[0].flags, vec![Flag::OverCeiling]);
        done(&dir);
    }

    #[test]
    fn today_sums_the_records_of_the_local_day_and_the_running_counter() {
        let dir = temp("today");
        let paths = seed(&dir);
        assert_eq!(go(&paths, at(0), &["start", "Rust"]).code, EXIT_OK);
        assert_eq!(go(&paths, at(1_800), &["stop"]).code, EXIT_OK);
        assert_eq!(go(&paths, at(2_000), &["start", "Work/Reading"]).code, EXIT_OK);
        let today = go(&paths, at(2_600), &["today"]);
        assert_eq!(today.out, "40 min\n");
        // The next local day: 2026-09-19 00:30 at +03:00 is 21:30 UTC on the 18th.
        let tomorrow = go(&paths, at(11 * 3_600 + 1_800), &["stop"]);
        assert_eq!(tomorrow.code, EXIT_OK);
        let next = go(&paths, at(12 * 3_600), &["today"]);
        assert_eq!(next.out, "0 s\n");
        done(&dir);
    }

    #[test]
    fn a_focus_missing_from_the_tree_is_still_answered() {
        let dir = temp("orphan");
        let paths = seed(&dir);
        let running = Running {
            focus: Id::new(9, 9),
            started: WALL,
            offset_minutes: OFFSET,
            spans: Vec::new(),
            paused: false,
            refreshed: WALL,
            flags: Vec::new(),
            target: None,
            idle_from: None,
            watch: Watch::Window,
        };
        crate::store::running::save(&paths.running_file(), &running).expect("save");
        let status = go(&paths, at(5), &["status", "--json"]);
        assert!(status.out.contains("\"focus\":\"unknown focus\",\"category\":null"), "{}", status.out);
        assert_eq!(go(&paths, at(5), &["status"]).out, "unknown focus · 5 s\n");
        done(&dir);
    }

    #[test]
    fn turkish_answers_use_turkish_units() {
        let dir = temp("turkish");
        let paths = seed(&dir);
        assert_eq!(go_in("tr", &paths, at(0), &["start", "Rust"]).out, "Rust başladı.\n");
        assert_eq!(go_in("tr", &paths, at(2 * 3_600 + 20 * 60), &["status"]).out, "Rust · 2 sa 20 dk\n");
        assert_eq!(go_in("tr", &paths, at(0), &["start", "Rust"]).code, EXIT_ALREADY_RUNNING);
        done(&dir);
    }

    /// A moment `seconds` after the seeded start on a machine that booted again `up` seconds
    /// before it.
    fn after_restart(seconds: u64, up: u64) -> Moment {
        let uptime = Duration::from_secs(up);
        Moment {
            clocks: Clocks {
                wall: WALL + i64::try_from(seconds).expect("fits"),
                uptime: Uptime { awake: uptime, elapsed: uptime },
            },
            offset_minutes: Some(OFFSET),
            knows_boot: true,
        }
    }

    #[test]
    fn a_counter_started_here_is_watched_by_nobody() {
        let dir = temp("watch-none");
        let paths = seed(&dir);
        assert_eq!(go(&paths, at(0), &["start", "Rust"]).code, EXIT_OK);
        let running = Store::open_read_only(paths.clone()).running().expect("file").expect("readable");
        assert_eq!(running.watch, Watch::None);
        done(&dir);
    }

    #[test]
    fn status_says_where_a_restart_cut_the_counter() {
        let dir = temp("restart-status");
        let paths = seed(&dir);
        assert_eq!(go(&paths, at(0), &["start", "Rust"]).code, EXIT_OK);
        // Seen alive fifty minutes in; the machine is off from some time after, and back at ten
        // hours, up for an hour.
        assert_eq!(go(&paths, at(3_000), &["status"]).out, "Rust · 50 min\n");
        let later = after_restart(10 * 3_600, 3_600);
        let status = go(&paths, later, &["status"]);
        assert_eq!(status.code, EXIT_OK);
        assert_eq!(status.out, "Rust · 50 min · nothing counted after 13:50: the computer restarted\n");
        let turkish = go_in("tr", &paths, later, &["status"]);
        assert_eq!(turkish.out, "Rust · 50 dk · 13:50 sonrası sayılmadı: bilgisayar yeniden başladı\n");
        let json = go(&paths, later, &["status", "--json"]);
        assert_eq!(
            json.out,
            format!(
                "{{\"running\":true,\"focus\":\"Rust\",\"category\":\"Work\",\"seconds\":3000,\"paused\":false,\"started\":{WALL},\"cut\":{{\"at\":{},\"reason\":\"restart\"}}}}\n",
                WALL + 3_000
            )
        );
        assert_eq!(go(&paths, later, &["today"]).out, "50 min\n", "today counts the cut time and says nothing more");
        // The next day the date stands before the time.
        let tomorrow = go(&paths, after_restart(30 * 3_600, 3_600), &["status"]);
        assert!(tomorrow.out.ends_with(" 13:50: the computer restarted\n"), "{}", tomorrow.out);
        assert!(tomorrow.out.contains("18 September 13:50"), "{}", tomorrow.out);
        done(&dir);
    }

    #[test]
    fn stop_after_a_restart_ends_the_record_at_the_cut_and_says_so() {
        let dir = temp("restart-stop");
        let paths = seed(&dir);
        assert_eq!(go(&paths, at(0), &["start", "Rust"]).code, EXIT_OK);
        go(&paths, at(1_800), &["today"]);
        let later = after_restart(10 * 3_600, 3_600);
        let stopped = go(&paths, later, &["stop"]);
        assert_eq!(stopped.code, EXIT_OK, "{}", stopped.err);
        assert_eq!(stopped.out, "Rust · 30 min · nothing counted after 13:30: the computer restarted\n");
        let store = Store::open_read_only(paths.clone());
        assert_eq!(store.sessions[0].ended, WALL + 1_800);
        assert_eq!(store.sessions[0].work_seconds(), 1_800);
        assert!(crate::span::check(&store.sessions[0].spans, 1_800).is_empty());
        assert_eq!(go(&paths, later, &["start", "Rust"]).code, EXIT_OK);
        let json = go(&paths, after_restart(20 * 3_600, 3_600), &["stop", "--json"]);
        assert_eq!(
            json.out,
            format!(
                "{{\"focus\":\"Rust\",\"category\":\"Work\",\"seconds\":0,\"cut\":{{\"at\":{},\"reason\":\"restart\"}}}}\n",
                WALL + 10 * 3_600
            )
        );
        done(&dir);
    }

    #[test]
    fn switching_after_a_restart_records_the_old_counter_up_to_the_cut() {
        let dir = temp("restart-switch");
        let paths = seed(&dir);
        assert_eq!(go(&paths, at(0), &["start", "Rust"]).code, EXIT_OK);
        let later = after_restart(10 * 3_600, 3_600);
        let switched = go(&paths, later, &["start", "Home/Reading", "--switch"]);
        assert_eq!(switched.code, EXIT_OK, "{}", switched.err);
        assert_eq!(
            switched.out,
            "Rust · 0 s · nothing counted after 13:00: the computer restarted\nStarted Reading.\n"
        );
        let store = Store::open_read_only(paths.clone());
        assert_eq!(store.sessions[0].ended, WALL);
        done(&dir);
    }

    #[test]
    fn status_leaves_the_seen_stamp_at_most_once_a_minute() {
        let dir = temp("seen");
        let paths = seed(&dir);
        assert_eq!(go(&paths, at(0), &["start", "Rust"]).code, EXIT_OK);
        let stamp = || Store::open_read_only(paths.clone()).seen().map(|seen| seen.seen);
        go(&paths, at(30), &["status"]);
        assert_eq!(stamp(), None, "the start is proof enough");
        go(&paths, at(60), &["status"]);
        assert_eq!(stamp(), Some(WALL + 60));
        go(&paths, at(90), &["status"]);
        go(&paths, at(100), &["today"]);
        assert_eq!(stamp(), Some(WALL + 60));
        go(&paths, at(120), &["today"]);
        assert_eq!(stamp(), Some(WALL + 120));
        done(&dir);
    }

    #[test]
    fn status_never_waits_for_the_lock_and_asks_it_only_about_a_quiet_window() {
        let dir = temp("status-lock");
        let paths = seed(&dir);
        assert_eq!(go(&paths, at(0), &["start", "Rust"]).code, EXIT_OK);
        let window = AppLock::acquire(&paths.lock_file()).expect("lock").expect("free");
        let status = go(&paths, at(120), &["status"]);
        assert_eq!((status.code, status.out.as_str()), (EXIT_OK, "Rust · 2 min\n"));
        assert_eq!(Store::open_read_only(paths.clone()).seen().map(|seen| seen.seen), Some(WALL + 120));

        // A counter a window measures, last refreshed ten minutes ago: while the window holds the
        // lock it is alive; once it is gone the counter ends at the refresh.
        let mut running = Store::open_read_only(paths.clone()).running().expect("file").expect("readable");
        running.watch = Watch::Window;
        crate::store::running::save(&paths.running_file(), &running).expect("save");
        assert_eq!(go(&paths, at(600), &["status"]).out, "Rust · 10 min\n");
        drop(window);
        let gone = go(&paths, at(600), &["status", "--json"]);
        assert!(gone.out.contains(&format!("\"cut\":{{\"at\":{WALL},\"reason\":\"window-gone\"}}")), "{}", gone.out);
        let text = go_in("tr", &paths, at(600), &["status"]);
        assert_eq!(text.out, "Rust · 0 sn · 13:00 sonrası sayılmadı: pencere kapandı\n");
        assert!(AppLock::acquire(&paths.lock_file()).expect("lock").is_some(), "the probe let it go");
        done(&dir);
    }

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json_string("plain"), "\"plain\"");
        assert_eq!(json_string("say \"hi\"\\\n\t"), "\"say \\\"hi\\\"\\\\\\n\\t\"");
        assert_eq!(json_string("\u{1}"), "\"\\u0001\"");
        assert_eq!(json_string("çay"), "\"çay\"");
    }
}
