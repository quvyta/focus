//! Two processes: the command line as the person runs it, one `qfocus` after another.
//!
//! Everything else in the suite runs inside one process, which cannot see the three things that
//! only exist between processes: the exit code a script reads, the lock one live instance holds
//! against another, and a counter left on disk by an instance that never got to stop. So every
//! case here spawns the built binary, asserts on its code and on what it printed, and looks at
//! the files it left.
//!
//! Each case gets a folder of its own and hands the binary its own `HOME` and `XDG_*` variables,
//! so no run can reach the person's settings or records; the folder is removed when the case
//! passes. Nothing waits a fixed time: where a case needs the world to move on it polls for the
//! condition with a deadline, so a loaded machine makes it slower and never wrong.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use qframe::storage::{AppLock, holder_pid};

use qfocus::cli::{EXIT_ALREADY_RUNNING, EXIT_LOCKED, EXIT_NOT_FOUND, EXIT_NOTHING_RUNNING, EXIT_OK};
use qfocus::id::Id;
use qfocus::liveness::RESTART_MARGIN;
use qfocus::span::{ClockSource, Span, SpanKind};
use qfocus::store::{Paths, Running, Store, Watch, machine_name, running};
use qfocus::tree::{Category, Focus, Tree};

/// How long a case waits for something another process must do before it gives up and fails.
const PATIENCE: Duration = Duration::from_secs(30);

/// How long the work a seeded counter holds, in seconds: half an hour, which prints as `30 min`.
const SEEDED_WORK: u32 = 1_800;

/// A data folder of its own, with the tree seeded, and the binary pointed at it.
struct Sandbox {
    /// The folder that stands in for the person's home; everything the run may write is under it.
    home: PathBuf,
    /// The files inside the data folder, named as the binary will name them.
    paths: Paths,
}

impl Sandbox {
    /// A fresh folder for the case called `name`, with a tree the `start` command can find a
    /// focus in: a fresh data folder has no catalogue at all, and `start` on an unknown name
    /// answers [`EXIT_NOT_FOUND`] rather than inventing a focus.
    fn new(name: &str) -> Self {
        let home = std::env::temp_dir().join(format!("qfocus-two-processes-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        // The machine name is the one the binary will compute, so both agree on the file names.
        let paths = Paths::at(home.join("data").join("quvyta").join("focus"), machine_name());
        fs::create_dir_all(paths.root()).expect("the data folder");
        fs::write(paths.tree_file(), seed_tree().write()).expect("the tree");
        Self { home, paths }
    }

    /// The binary with `words` after it, run to the end, its folders pointed inside the sandbox.
    fn go(&self, words: &[&str]) -> Outcome {
        let mut command = self.command(env!("CARGO_BIN_EXE_qfocus"));
        command.args(words);
        let output = command.output().expect("the binary runs");
        Outcome {
            code: output.status.code().expect("an exit code, not a signal"),
            out: String::from_utf8(output.stdout).expect("utf-8 on the answer"),
            err: String::from_utf8(output.stderr).expect("utf-8 on the problems"),
        }
    }

    /// A command with nothing of the surrounding environment but the search path and the
    /// terminal: `HOME` and every `XDG_*` folder point into the sandbox, so even a bug in the
    /// path code cannot land on the person's records, and the language is fixed so the answers
    /// can be compared word for word.
    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_owned()))
            .env("TERM", "xterm-256color")
            .env("LC_ALL", "en_US.UTF-8")
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("XDG_CACHE_HOME", self.home.join("cache"));
        command
    }

    /// The store as it is on disk now, read without taking the lock.
    fn store(&self) -> Store {
        Store::open_read_only(self.paths.clone())
    }

    /// The running counter on disk, which must be there.
    fn counter(&self) -> Running {
        self.store().running().expect("a running file").expect("a readable running file")
    }

    /// Every file under the data folder with its bytes, in path order, apart from the lock file:
    /// taking the lock creates it and stamps the process id in it, which is not a change to the
    /// person's data.
    fn data(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let lock = self.paths.lock_file();
        let mut files = Vec::new();
        let mut pending = vec![self.paths.root().to_path_buf()];
        while let Some(next) = pending.pop() {
            for entry in fs::read_dir(&next).expect("reads the folder") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    pending.push(path);
                } else if path != lock {
                    let bytes = fs::read(&path).expect("reads the file");
                    files.push((path, bytes));
                }
            }
        }
        files.sort();
        files
    }

    /// Removes the folder. Called at the end of a case, so a failing one leaves its files to look
    /// at.
    fn done(self) {
        let _ = fs::remove_dir_all(&self.home);
    }
}

/// What one run of the binary answered.
struct Outcome {
    /// The exit code.
    code: i32,
    /// What it printed as its answer.
    out: String,
    /// What it printed about problems.
    err: String,
}

/// Work/Rust, Work/Reading and Home/Reading: one name that is unique and one that is not.
fn seed_tree() -> Tree {
    let focus = |id: u128, name: &str| Focus {
        id: Id::new(1, id),
        name: name.to_owned(),
        goal: None,
        archived: false,
        order: 0.0,
    };
    let category = |id: u128, name: &str, focuses: Vec<Focus>| Category {
        id: Id::new(2, id),
        name: name.to_owned(),
        icon: None,
        goal: None,
        archived: false,
        order: 0.0,
        focuses,
    };
    Tree {
        categories: vec![
            category(1, "Work", vec![focus(1, "Rust"), focus(2, "Reading")]),
            category(2, "Home", vec![focus(3, "Reading")]),
        ],
    }
}

/// The exit code as the binary gives it.
fn code(exit: u8) -> i32 {
    i32::from(exit)
}

/// Runs `check` until it answers, and fails after [`PATIENCE`] naming `what` was waited for. A
/// case may take longer on a busy machine; it may not pass because a wait happened to be long
/// enough.
fn until<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "waited {PATIENCE:?} in vain for {what}");
        sleep(Duration::from_millis(20));
    }
}

/// The number the one-line JSON object `object` gives for `field`.
fn number(object: &str, field: &str) -> i64 {
    let key = format!("\"{field}\":");
    let start = object.find(&key).unwrap_or_else(|| panic!("no {field} in {object}")) + key.len();
    let rest = &object[start..];
    let end = rest.find(|character: char| !character.is_ascii_digit() && character != '-').unwrap_or(rest.len());
    rest[..end].parse().unwrap_or_else(|_| panic!("{field} is no number in {object}"))
}

/// The moment the machine booted, in seconds since the Unix epoch: the wall clock less the clock
/// that counts from boot through sleep. A counter last seen alive before this moment is one the
/// machine lost when it went off.
fn boot() -> i64 {
    let now = qfocus::clock::now();
    now.wall.saturating_sub(i64::try_from(now.uptime.elapsed.as_secs()).expect("an uptime that fits"))
}

/// A counter on Work/Rust holding [`SEEDED_WORK`] seconds of work, last refreshed at `refreshed`
/// and measured by `watch`, written to the running file as an instance that went away left it.
fn seed_counter(sandbox: &Sandbox, watch: Watch, refreshed: i64) {
    let running = Running {
        focus: Id::new(1, 1),
        started: refreshed - i64::from(SEEDED_WORK),
        offset_minutes: 0,
        spans: vec![Span::new(SpanKind::Work, 0, SEEDED_WORK, ClockSource::Mono)],
        paused: false,
        refreshed,
        flags: Vec::new(),
        target: None,
        idle_from: None,
        watch,
    };
    running::save(&sandbox.paths.running_file(), &running).expect("the seeded counter");
}

/// A live instance: the full-screen program with a terminal of its own, holding the lock until it
/// is killed.
struct Window {
    /// The process that gives the program its terminal; it ends when the program does.
    terminal: Child,
    /// The program's own process id, read from the lock file it stamped.
    pid: u32,
}

impl Window {
    /// Opens the program and waits until it holds the lock.
    ///
    /// The full-screen program needs a terminal, so it is given one by `script`, which runs it on
    /// a pseudo-terminal and throws the drawing away. The wait reads the process id out of the
    /// lock file rather than trying the lock: an attempt of its own would take the lock from
    /// under the instance that is starting up.
    fn open(sandbox: &Sandbox) -> Self {
        let terminal = sandbox
            .command("script")
            .args(["-q", "-c", env!("CARGO_BIN_EXE_qfocus"), "/dev/null"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("a terminal for the window");
        let lock = sandbox.paths.lock_file();
        let pid = until("the window to take the lock", || holder_pid(&lock));
        Self { terminal, pid }
    }

    /// Kills the program outright, as a power cut or an `OOM` killer would, and waits until the
    /// lock is free again: the kernel drops it when the process dies.
    fn kill(mut self, sandbox: &Sandbox) {
        let killed =
            sandbox.command("kill").args(["-KILL", &self.pid.to_string()]).status().expect("the kill command runs");
        assert!(killed.success(), "the window could not be killed");
        self.terminal.wait().expect("the terminal ends with the program");
        let lock = sandbox.paths.lock_file();
        until("the lock to be free again", || AppLock::acquire(&lock).expect("the lock can be asked about").map(drop));
    }
}

#[test]
fn a_counter_one_process_starts_is_seen_and_recorded_by_the_ones_after_it() {
    let sandbox = Sandbox::new("cycle");
    let started = sandbox.go(&["start", "Rust"]);
    assert_eq!((started.code, started.out.as_str()), (code(EXIT_OK), "Started Rust.\n"), "{}", started.err);
    assert!(sandbox.paths.running_file().exists());
    let counter = sandbox.counter();
    assert_eq!(counter.focus, Id::new(1, 1));
    // Nothing measures a counter the command line started: the process that started it is gone.
    assert_eq!(counter.watch, Watch::None);

    // A second process sees it and says how long it has run; the wait is for the clock to move a
    // whole second, not for the answer to arrive.
    let seconds = until("the counter to reach a second", || {
        let json = sandbox.go(&["status", "--json"]);
        assert_eq!(json.code, code(EXIT_OK), "{}", json.err);
        Some(number(&json.out, "seconds")).filter(|seconds| *seconds >= 1)
    });
    let json = sandbox.go(&["status", "--json"]);
    assert!(json.out.starts_with("{\"running\":true,\"focus\":\"Rust\",\"category\":\"Work\","), "{}", json.out);
    assert!(json.out.contains("\"paused\":false"), "{}", json.out);
    assert!(!json.out.contains("\"cut\""), "a counter of this boot is not cut: {}", json.out);
    assert_eq!(number(&json.out, "started"), counter.started);
    assert!(number(&json.out, "seconds") >= seconds, "{}", json.out);

    let status = sandbox.go(&["status"]);
    assert_eq!(status.code, code(EXIT_OK));
    assert!(status.out.starts_with("Rust · "), "{}", status.out);
    assert!(status.out.ends_with(" s\n"), "seconds so far, spelled in English: {}", status.out);
    assert!(status.err.is_empty(), "{}", status.err);

    let today = sandbox.go(&["today", "--json"]);
    assert_eq!(number(&today.out, "recorded"), 0, "nothing is recorded while it runs: {}", today.out);
    assert!(number(&today.out, "running") >= seconds, "{}", today.out);

    let stopped = sandbox.go(&["stop"]);
    assert_eq!(stopped.code, code(EXIT_OK), "{}", stopped.err);
    assert!(stopped.out.starts_with("Rust · "), "{}", stopped.out);
    assert!(!sandbox.paths.running_file().exists(), "the record replaced the running file");

    let store = sandbox.store();
    assert_eq!(store.sessions.len(), 1);
    let session = &store.sessions[0];
    assert_eq!(session.focus, Id::new(1, 1));
    assert!(session.work_seconds() >= u64::try_from(seconds).expect("seconds fit"), "{}", session.work_seconds());
    assert_eq!(session.started, counter.started);

    let after = sandbox.go(&["today", "--json"]);
    assert_eq!(number(&after.out, "recorded"), i64::try_from(session.work_seconds()).expect("fits"));
    assert_eq!(number(&after.out, "running"), 0);
    let quiet = sandbox.go(&["status"]);
    assert_eq!((quiet.code, quiet.out.as_str(), quiet.err.as_str()), (code(EXIT_NOTHING_RUNNING), "", ""));
    sandbox.done();
}

#[test]
fn a_second_start_is_refused_unless_it_is_told_to_switch() {
    let sandbox = Sandbox::new("switch");
    assert_eq!(sandbox.go(&["start", "Rust"]).code, code(EXIT_OK));
    let before = sandbox.data();

    let refused = sandbox.go(&["start", "Home/Reading"]);
    assert_eq!(refused.code, code(EXIT_ALREADY_RUNNING), "{}", refused.err);
    assert!(refused.out.is_empty(), "{}", refused.out);
    assert!(refused.err.starts_with("Rust is already running for "), "{}", refused.err);
    assert!(refused.err.ends_with("--switch stops it and starts Home/Reading.\n"), "{}", refused.err);
    assert_eq!(sandbox.data(), before, "the refused start left the first counter as it was");
    assert!(sandbox.store().sessions.is_empty(), "nothing was recorded");

    let switched = sandbox.go(&["start", "Home/Reading", "--switch"]);
    assert_eq!(switched.code, code(EXIT_OK), "{}", switched.err);
    let lines: Vec<&str> = switched.out.lines().collect();
    assert_eq!(lines.len(), 2, "{}", switched.out);
    assert!(lines[0].starts_with("Rust · "), "{}", switched.out);
    assert_eq!(lines[1], "Started Reading.");

    let store = sandbox.store();
    assert_eq!(store.sessions.len(), 1, "the first counter became the one record");
    assert_eq!(store.sessions[0].focus, Id::new(1, 1));
    assert_eq!(sandbox.counter().focus, Id::new(1, 3), "Home/Reading runs now");
    sandbox.done();
}

#[test]
fn stop_and_status_with_nothing_running_both_answer_their_own_code_and_record_nothing() {
    let sandbox = Sandbox::new("nothing");
    let before = sandbox.data();

    let status = sandbox.go(&["status"]);
    assert_eq!((status.code, status.out.as_str(), status.err.as_str()), (code(EXIT_NOTHING_RUNNING), "", ""));
    let json = sandbox.go(&["status", "--json"]);
    assert_eq!((json.code, json.out.as_str()), (code(EXIT_NOTHING_RUNNING), "{\"running\":false}\n"));

    let stopped = sandbox.go(&["stop"]);
    assert_eq!((stopped.code, stopped.err.as_str()), (code(EXIT_NOTHING_RUNNING), "Nothing is running.\n"));
    assert!(stopped.out.is_empty(), "{}", stopped.out);
    let stopped_json = sandbox.go(&["stop", "--json"]);
    assert_eq!(stopped_json.out, "{\"error\":\"nothing-running\"}\n");

    // An unknown focus is the other refusal that must leave the folder alone.
    let unknown = sandbox.go(&["start", "Cooking"]);
    assert_eq!((unknown.code, unknown.err.as_str()), (code(EXIT_NOT_FOUND), "No focus is called Cooking.\n"));

    assert_eq!(sandbox.data(), before, "nothing but the lock file was written");
    assert!(!sandbox.paths.running_file().exists());
    assert!(!sandbox.paths.sessions_dir().exists());
    assert!(!sandbox.paths.seen_file().exists());
    sandbox.done();
}

#[test]
fn a_counter_the_machine_lost_at_a_restart_is_cut_there_and_its_work_kept() {
    let sandbox = Sandbox::new("restart");
    // Last seen alive an hour before this machine booted, so the counter cannot have been running
    // since: the time from the cut to now belongs to nobody.
    let cut = boot() - RESTART_MARGIN - 3_600;
    seed_counter(&sandbox, Watch::None, cut);

    let status = sandbox.go(&["status"]);
    assert_eq!(status.code, code(EXIT_OK), "{}", status.err);
    assert!(status.out.starts_with("Rust · 30 min · nothing counted after "), "{}", status.out);
    assert!(status.out.ends_with(": the computer restarted\n"), "{}", status.out);
    let json = sandbox.go(&["status", "--json"]);
    assert!(json.out.contains("\"seconds\":1800"), "{}", json.out);
    assert!(json.out.contains(&format!("\"cut\":{{\"at\":{cut},\"reason\":\"restart\"}}")), "{}", json.out);

    let stopped = sandbox.go(&["stop"]);
    assert_eq!(stopped.code, code(EXIT_OK), "{}", stopped.err);
    assert!(stopped.out.ends_with(": the computer restarted\n"), "{}", stopped.out);
    let store = sandbox.store();
    assert_eq!(store.sessions.len(), 1);
    assert_eq!(store.sessions[0].work_seconds(), u64::from(SEEDED_WORK), "the half hour worked is kept");
    assert_eq!(store.sessions[0].ended, cut, "and the record ends where the counting stopped");
    assert!(!sandbox.paths.running_file().exists());
    assert_eq!(sandbox.go(&["status"]).code, code(EXIT_NOTHING_RUNNING));
    sandbox.done();
}

#[test]
fn a_counter_whose_window_was_killed_is_cut_at_its_last_refresh_and_still_recorded() {
    let sandbox = Sandbox::new("killed");
    // A counter a window measures, last refreshed longer ago than a window may be silent, so
    // every look has to ask whether that window is still there.
    let cut = qfocus::clock::now().wall - 60;
    seed_counter(&sandbox, Watch::None, cut);
    let mut running = sandbox.counter();
    running.watch = Watch::Window;
    running::save(&sandbox.paths.running_file(), &running).expect("the seeded counter");
    let before = fs::read(sandbox.paths.running_file()).expect("the seeded file");

    let window = Window::open(&sandbox);
    let alive = sandbox.go(&["status"]);
    assert_eq!(alive.code, code(EXIT_OK), "{}", alive.err);
    assert!(alive.out.starts_with("Rust · 3"), "a live window keeps it counting: {}", alive.out);
    assert!(!alive.out.contains("nothing counted"), "{}", alive.out);
    window.kill(&sandbox);

    assert_eq!(fs::read(sandbox.paths.running_file()).expect("still there"), before, "the killed run kept the file");
    let status = sandbox.go(&["status"]);
    assert_eq!(status.code, code(EXIT_OK), "{}", status.err);
    assert!(status.out.starts_with("Rust · 30 min · nothing counted after "), "{}", status.out);
    assert!(status.out.ends_with(": the window closed\n"), "{}", status.out);

    let stopped = sandbox.go(&["stop"]);
    assert_eq!(stopped.code, code(EXIT_OK), "{}", stopped.err);
    let store = sandbox.store();
    assert_eq!(store.sessions.len(), 1);
    assert_eq!(store.sessions[0].work_seconds(), u64::from(SEEDED_WORK), "the work of the killed run is recorded");
    assert_eq!(store.sessions[0].ended, cut);
    sandbox.done();
}

#[test]
fn a_live_instance_holding_the_lock_turns_every_writer_away_and_frees_it_when_killed() {
    let sandbox = Sandbox::new("locked");
    let before = sandbox.data();
    let window = Window::open(&sandbox);

    let start = sandbox.go(&["start", "Rust"]);
    assert_eq!(start.code, code(EXIT_LOCKED), "{}", start.err);
    assert_eq!(start.err, "An open qfocus window is writing; close it or use that window.\n");
    assert!(start.out.is_empty(), "{}", start.out);
    assert_eq!(sandbox.go(&["stop"]).code, code(EXIT_LOCKED));
    let json = sandbox.go(&["stop", "--json"]);
    assert!(json.out.starts_with("{\"error\":\"locked\",\"holder\":"), "{}", json.out);
    assert!(json.out.contains(&window.pid.to_string()), "the holder is named: {}", json.out);

    // Looking is never refused, and no writer got through.
    assert_eq!(sandbox.go(&["today"]).out, "0 s\n");
    assert_eq!(sandbox.go(&["status"]).code, code(EXIT_NOTHING_RUNNING));
    assert_eq!(sandbox.data(), before, "a writer that was turned away wrote nothing");
    assert!(!sandbox.paths.running_file().exists());

    window.kill(&sandbox);
    let started = sandbox.go(&["start", "Rust"]);
    assert_eq!((started.code, started.out.as_str()), (code(EXIT_OK), "Started Rust.\n"), "{}", started.err);
    sandbox.done();
}

/// A folder that only the sandbox may name, so a mistake in the test cannot reach the person's
/// records: every path a case writes to lies under the temporary folder it made.
#[test]
fn every_case_writes_only_under_its_own_temporary_folder() {
    let sandbox = Sandbox::new("folders");
    let temp = std::env::temp_dir();
    assert!(sandbox.home.starts_with(&temp), "{}", sandbox.home.display());
    assert!(sandbox.paths.root().starts_with(&sandbox.home));
    assert!(sandbox.paths.running_file().starts_with(&sandbox.home));
    assert!(sandbox.paths.lock_file().starts_with(&sandbox.home));
    // The binary answers with these folders, so the data folder it found is the seeded one.
    let started = sandbox.go(&["start", "Rust"]);
    assert_eq!(started.code, code(EXIT_OK), "{}", started.err);
    assert!(Path::new(&sandbox.paths.running_file()).exists());
    sandbox.done();
}
