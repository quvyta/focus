//! The application: the store, the screens and what happens between them.
//!
//! The screens under [`crate::ui`] know nothing about the disk. Everything that touches it —
//! the catalogue, the month files, the running file, the lock — is done here, once, in answer
//! to what a screen asks for. The clock is read through one function handed in at the start, so
//! a test can hand in a clock of its own and drive the timer without waiting.

use std::io;
use std::time::Duration;

use qframe::date::{Date, DateTime, local_offset};
use qframe::prelude::*;
use qframe::runtime::{Confirm, Task, TaskId, Termination};
use qframe::storage::{Settings, atomic_write};
use qframe::uptime::Uptime;
use qframe::widgets::{Bar, BarChart, BigText, Modal, Span, Toast};

use crate::clock;
use crate::day::{DayTotals, totals};
use crate::duration::{Units, short};
use crate::export;
use crate::id::Id;
use crate::liveness;
use crate::prefs::Prefs;
use crate::session::edit::{self, Changes};
use crate::session::{Flag, Session, Source};
use crate::stats;
use crate::store::{Access, Paths, Running, Store, Watch, broken_stamp, machine_name};
use crate::timer::{Clocks, Timer};
use crate::ui::charts::{self, Charts};
use crate::ui::record_form::{self, Outcome, Purpose, RecordForm};
use crate::ui::records::{self, Records};
use crate::ui::recover::{self, Kept, Recover};
use crate::ui::settings::{self, Settings as SettingsScreen};
use crate::ui::timer::{self as timer_screen, Screen as TimerScreen};
use crate::ui::today::{self, Row, Today};
use crate::ui::{GoalRow, goal_gauges, goal_rows, info_line, warning_line};

/// The folder under the platform's config directory the settings live in.
const SETTINGS_APP: &str = "quvyta/focus";

/// The keymap compiled in, so an installed program carries its keys with it.
const KEYMAP: &str = include_str!("../keymap.toml");

/// How long the terminal may go without input, with no counter running, before the dashboard
/// stands over the page: two minutes.
const DASHBOARD_AFTER: Duration = Duration::from_secs(2 * 60);

/// How long the terminal may go without input, with a counter running, before the counter's
/// screen quietens: two minutes.
const QUIET_AFTER: Duration = Duration::from_secs(2 * 60);

/// How many focuses the dashboard names.
const DASHBOARD_TOP: usize = 3;

/// Cells the dashboard keeps free on each side of the screen.
const DASHBOARD_MARGIN: u16 = 2;

/// "3 records", counted for the purge's question and its answer.
fn purge_records(count: usize) -> String {
    t!("settings.purge-records", n = u32::try_from(count).unwrap_or(u32::MAX))
}

/// "2 archived rows", counted for the purge's question and its answer.
fn purge_rows(count: usize) -> String {
    t!("settings.purge-rows", n = u32::try_from(count).unwrap_or(u32::MAX))
}

/// Starts the application: settings from the platform's config folder, the records from its
/// data folder, and the runtime drives the screens until the person leaves.
///
/// Without a data folder the application still opens and says at the top that nothing is
/// written; the store then points at a folder that is never created.
///
/// # Errors
///
/// Returns the terminal's error when the screen cannot be taken over or restored.
pub fn run() -> io::Result<()> {
    // The file is checked against every key qfocus knows and repaired with a backup when it has
    // to be; the repairs are shown on the Settings page.
    let settings = Settings::load(SETTINGS_APP).schema(Prefs::schema()).self_heal(true);
    let (store, on_disk) = match Paths::detect() {
        Some(paths) => (Store::open(paths), true),
        None => {
            let nowhere = std::env::temp_dir().join("quvyta-focus-nowhere");
            (Store::open_read_only(Paths::at(nowhere, machine_name())), false)
        }
    };
    let app = QFocus::new(store, on_disk, local_offset(), Box::new(clock::now), settings.clone());
    let mut runtime = Runtime::new(app).settings(&settings).keymap_source("keymap.toml", KEYMAP);
    for (file, text) in crate::locales() {
        runtime = runtime.locale_source(file, text);
    }
    runtime.run()
}

/// The pages of the application, in the order of the tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    /// The tree of focuses and the day's work; the counter while one runs.
    #[default]
    Today,
    /// The records over a day, a week, a month and a year.
    Charts,
    /// Every session, the archive, the trash and the problems.
    Records,
    /// What the person can change, and the sweeping actions.
    Settings,
}

impl Page {
    /// The pages in the order of the tabs.
    const ALL: [Self; 4] = [Self::Today, Self::Charts, Self::Records, Self::Settings];

    /// Where the page sits among the tabs.
    fn index(self) -> usize {
        Self::ALL.iter().position(|page| *page == self).unwrap_or(0)
    }
}

/// Everything that can happen in the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// Something happened on the Today screen.
    Today(today::Msg),
    /// Something happened on the Charts screen.
    Charts(charts::Msg),
    /// Something happened on the Records screen.
    Records(records::Msg),
    /// Something happened on the Settings screen.
    Settings(settings::Msg),
    /// A tab was opened, by position.
    Page(usize),
    /// Open the search field of the Records screen.
    Search,
    /// Write the export files.
    Export,
    /// Bring back the session removed last, or take back the correction made last.
    Undo,
    /// Open the form for a session typed in by hand.
    Add,
    /// Lay out the spans of the selected session.
    Spans,
    /// Set, change or remove the goal of the selected row.
    Goal,
    /// Start the selected focus with a countdown.
    Countdown,
    /// Something happened on the record form.
    Form(record_form::Msg),
    /// Something happened on the Timer screen.
    Timer(timer_screen::Msg),
    /// Something was chosen in the recovery dialog.
    Recover(recover::Msg),
    /// A second passed.
    Tick,
    /// Esc: close the open layer, if any.
    Back,
    /// Archive the selected row.
    Archive,
    /// Rename the selected row.
    Rename,
    /// Start or end the break.
    Pause,
    /// Open the note field.
    Note,
    /// Space: stop the counter, or start the focus worked on last.
    Toggle,
    /// The runtime is about to leave while a counter runs: ask what to do with it first.
    Quit,
    /// Finish the running session and leave.
    QuitFinish,
    /// Leave with the counter still running on disk.
    QuitLeave,
    /// Stay.
    QuitCancel,
    /// The system asked the program to end while a counter runs: the file is refreshed, then
    /// the question before leaving is asked, within the grace the runtime gives.
    Terminated,
    /// The terminal went away while a counter runs: the file is refreshed and the program
    /// leaves; nobody is there to ask.
    HungUp,
    /// The counter stopped over the ceiling: fix its duration in the form before it is recorded.
    StopFix,
    /// The counter stopped over the ceiling: record it as it is.
    StopLeave,
    /// The counter stopped over the ceiling: keep counting after all.
    StopCancel,
    /// With no counter running the terminal went silent for two minutes (`true`), or
    /// input came back (`false`): the dashboard stands over the page, or leaves it.
    Idle(bool),
    /// With a counter running the terminal went silent for two minutes (`true`), or input
    /// came back (`false`): the counter's screen quietens, or wakes.
    Quiet(bool),
    /// A minute turned while the dashboard stands, so its clock is drawn again.
    Minute,
}

impl From<today::Msg> for Msg {
    fn from(message: today::Msg) -> Self {
        Self::Today(message)
    }
}

impl From<charts::Msg> for Msg {
    fn from(message: charts::Msg) -> Self {
        Self::Charts(message)
    }
}

impl From<records::Msg> for Msg {
    fn from(message: records::Msg) -> Self {
        Self::Records(message)
    }
}

impl From<settings::Msg> for Msg {
    fn from(message: settings::Msg) -> Self {
        Self::Settings(message)
    }
}

impl From<timer_screen::Msg> for Msg {
    fn from(message: timer_screen::Msg) -> Self {
        Self::Timer(message)
    }
}

impl From<recover::Msg> for Msg {
    fn from(message: recover::Msg) -> Self {
        Self::Recover(message)
    }
}

impl From<record_form::Msg> for Msg {
    fn from(message: record_form::Msg) -> Self {
        Self::Form(message)
    }
}

/// What `ctrl+z` takes back: the last thing done to the records.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Undo {
    /// A session was moved to the trash.
    Removed(Id),
    /// A session was corrected: what it was before, and the correction's identifier.
    Corrected {
        /// The version the correction replaced.
        previous: Box<Session>,
        /// The correction written.
        correction: Id,
    },
    /// Every session was moved to the trash, and these rows of the tree were archived with them.
    Wiped {
        /// The sessions moved, in the order they went.
        sessions: Vec<Id>,
        /// The categories and focuses archived.
        rows: Vec<Row>,
    },
}

/// The application.
pub struct QFocus {
    store: Store,
    /// Whether the store is a real folder; without one nothing is written and the person is
    /// told.
    on_disk: bool,
    /// Minutes local time is ahead of UTC, or `None` when the system does not say; stamps then
    /// carry offset zero and the header says so.
    offset: Option<i16>,
    /// The clocks, read through this so a test can bring its own.
    clock: Box<dyn Fn() -> Clocks>,
    /// Whether the clock that runs through sleep counts from the machine's boot, so a restart
    /// since a counter nobody measured was last seen can be told.
    knows_boot: bool,
    /// What the person set about the application.
    prefs: Prefs,
    /// The settings file, with the framework's keys and qfocus's own.
    settings: Settings,
    page: Page,
    today: Today,
    charts: Charts,
    records: Records,
    settings_screen: SettingsScreen,
    timer: Option<TimerScreen>,
    recover: Option<Recover>,
    /// The record form, while it is open.
    form: Option<RecordForm>,
    /// What was done to the records last, which `ctrl+z` takes back.
    last_undo: Option<Undo>,
    /// Whether the question asked before leaving with a running counter is open.
    quit_asked: bool,
    /// Goals already reached, or said to be, while the counter runs: each is said once.
    goals_said: Vec<Id>,
    /// Whether the dashboard stands over the page: no counter runs and nothing was touched for
    /// a while. The page under it keeps its state.
    idle: bool,
    /// Whether the counter's screen has quietened: a counter runs and nothing was touched.
    quiet: bool,
    /// The task that turns the dashboard's clock every minute, while it stands.
    minute: Option<TaskId>,
    /// The task ticking every second while a counter runs or the lock is being retried.
    tick: Option<TaskId>,
    /// Records that could not be written, kept until they can be.
    pending: Vec<Session>,
    /// The current sessions: those on disk and those waiting in memory, together.
    sessions: Vec<Session>,
    /// The local day being shown and what was worked on it.
    date: Date,
    totals: DayTotals,
}

impl QFocus {
    /// The application over `store`. `on_disk` says whether the store is a real folder, `offset`
    /// is the local time zone if known, `clock` reads the clocks and `settings` is the settings
    /// file, from which the preferences are read.
    ///
    /// A running file the last run left is not looked at here but in [`App::init`], where the
    /// runtime opens the recovery dialog before the first frame.
    #[must_use]
    pub fn new(
        store: Store,
        on_disk: bool,
        offset: Option<i16>,
        clock: Box<dyn Fn() -> Clocks>,
        settings: Settings,
    ) -> Self {
        let mut app = Self {
            store,
            on_disk,
            offset,
            clock,
            knows_boot: Uptime::detects_suspend(),
            prefs: Prefs::from_settings(&settings),
            settings_screen: SettingsScreen::new(settings.diagnostics().to_vec()),
            settings,
            page: Page::Today,
            today: Today::new(),
            charts: Charts::new(),
            records: Records::new(),
            timer: None,
            recover: None,
            form: None,
            last_undo: None,
            quit_asked: false,
            goals_said: Vec::new(),
            idle: false,
            quiet: false,
            minute: None,
            tick: None,
            pending: Vec::new(),
            sessions: Vec::new(),
            date: Date::from_days(0),
            totals: DayTotals::default(),
        };
        app.refresh_day();
        app
    }

    /// Sets whether the clocks count from the machine's boot, which is
    /// [`Uptime::detects_suspend`] unless set; a test replays either kind of machine with it.
    #[must_use]
    pub fn knows_boot(mut self, knows: bool) -> Self {
        self.knows_boot = knows;
        self
    }

    /// The Today screen.
    #[must_use]
    pub fn today(&self) -> &Today {
        &self.today
    }

    /// The Charts screen.
    #[must_use]
    pub fn charts(&self) -> &Charts {
        &self.charts
    }

    /// The Records screen.
    #[must_use]
    pub fn records(&self) -> &Records {
        &self.records
    }

    /// The page that is open.
    #[must_use]
    pub fn page(&self) -> Page {
        self.page
    }

    /// The Timer screen, while a counter runs.
    #[must_use]
    pub fn timer(&self) -> Option<&TimerScreen> {
        self.timer.as_ref()
    }

    /// The store.
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The record form, while it is open.
    #[must_use]
    pub fn form(&self) -> Option<&RecordForm> {
        self.form.as_ref()
    }

    /// Records that could not be written and are kept in memory.
    #[must_use]
    pub fn pending(&self) -> &[Session] {
        &self.pending
    }

    /// The person's preferences.
    #[must_use]
    pub fn prefs(&self) -> &Prefs {
        &self.prefs
    }

    /// The settings file as it stands in memory.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Whether the dashboard stands over the page.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.idle
    }

    /// Whether the counter's screen has quietened.
    #[must_use]
    pub fn is_quiet(&self) -> bool {
        self.quiet
    }

    /// The offset stamps are written with: the local one, or zero when it is unknown.
    fn offset_minutes(&self) -> i16 {
        self.offset.unwrap_or(0)
    }

    /// Whether a counter may be started here: a real folder needs the lock, memory needs nothing.
    fn can_start(&self) -> bool {
        !self.on_disk || self.store.access.can_write()
    }

    /// Whether records may be removed and brought back: only on disk, and only with the lock.
    fn can_edit(&self) -> bool {
        self.on_disk && self.store.access.can_write()
    }

    /// Reads the clocks.
    fn now(&self) -> Clocks {
        (self.clock)()
    }

    /// Takes up a running file the last run left. In memory there is no file to find.
    ///
    /// A counter nobody measured that is still alive, left by the command line or by a window
    /// told to leave it running, carries on here without a question: the time since counts, as it
    /// would have with the window open. Anything else opens the recovery dialog, saying where the
    /// counter stopped counting and why.
    ///
    /// A file that cannot be read is moved aside at once when this instance may write: the next
    /// counter would otherwise overwrite it at its first refresh, and the next start would report
    /// it again. An instance that only looks leaves it for the one that writes.
    fn find_running(&mut self) -> Command<Msg> {
        if !self.on_disk {
            return Command::none();
        }
        let now = self.now();
        if let Some(Ok(running)) = self.store.running()
            && self.store.access.can_write()
        {
            let seen = self.store.seen();
            let reach = liveness::reach_without_window(&running, seen.as_ref(), now, self.knows_boot);
            if reach.cut.is_none() {
                return self.adopt(running, now);
            }
        }
        self.recover = self.store.running().map(|found| match found {
            Ok(running) => {
                let focus = self.store.tree.focus(running.focus).map(|focus| focus.name.clone());
                let ago = u64::try_from(now.wall.saturating_sub(running.refreshed)).unwrap_or(0);
                // An instance that only looks cannot tell a window from its own lock, so it asks
                // whether one holds it, as the command line does.
                let seen = self.store.seen();
                let reach = if self.store.access.can_write() {
                    liveness::reach_without_window(&running, seen.as_ref(), now, self.knows_boot)
                } else {
                    liveness::reach(&running, seen.as_ref(), now, self.knows_boot, || self.store.lock_is_held())
                };
                Recover::Found { running, focus, ago, reach }
            }
            Err(problems) => {
                let here = self.store.paths.running_file();
                if !self.store.access.can_write() {
                    return Recover::Broken { problems, path: here, kept: Kept::Untouched };
                }
                match self.store.set_aside_running(&broken_stamp(now.wall, self.offset_minutes())) {
                    Ok(aside) => {
                        // The lines and columns now belong to the copy; that is the file to open.
                        let (old, new) = (here.display().to_string(), aside.display().to_string());
                        let problems = problems
                            .into_iter()
                            .map(|mut problem| {
                                if let Some(location) = problem.location.as_mut().filter(|at| at.file == old) {
                                    location.file.clone_from(&new);
                                }
                                problem
                            })
                            .collect();
                        Recover::Broken { problems, path: aside, kept: Kept::Aside }
                    }
                    Err(error) => Recover::Broken { problems, path: here, kept: Kept::Stuck(error.to_string()) },
                }
            }
        });
        Command::none()
    }

    /// Carries on a counter nobody measured and that is still alive at `now`: the time since its
    /// last refresh counts as the kind that was open, the counter screen opens as after
    /// "Continue", and from here this window measures it, so the file says so at once.
    fn adopt(&mut self, running: Running, now: Clocks) -> Command<Msg> {
        let timer = Timer::restore(
            running.focus,
            running.started,
            running.offset_minutes,
            running.spans,
            running.paused,
            running.refreshed,
            now.wall,
            now,
        )
        .ceiling(self.prefs.ceiling)
        .with_flags(running.flags)
        .with_unclaimed_idle(running.idle_from)
        .source(Source::Timer);
        let (category, name) =
            self.names(running.focus).unwrap_or_else(|| (String::new(), t!("recover.unknown-focus")));
        let duration = short(timer.work_seconds(now), &units().as_units());
        let said = Toast::info(t!("recover.adopted", focus = name.clone(), duration = duration));
        self.open_timer(timer, now, category, name, running.target, said)
    }

    /// Works out the local day and what was worked on it, from the records on disk and the ones
    /// waiting in memory.
    fn refresh_day(&mut self) {
        let now = self.now();
        self.date = DateTime::from_unix(now.wall, self.offset_minutes()).date;
        let mut sessions = self.store.sessions.clone();
        sessions.extend(self.pending.iter().cloned());
        self.totals = totals(&sessions, self.date, self.prefs.rollover);
        self.sessions = sessions;
    }

    /// The day's work: what is recorded plus what the running counter has counted so far.
    fn total_today(&self) -> u64 {
        self.totals.total.saturating_add(self.timer.as_ref().map_or(0, TimerScreen::work_seconds))
    }

    /// Writes the catalogue; a failure is said and the catalogue stays as it is in memory.
    fn save_tree(&self) -> Command<Msg> {
        if !self.on_disk {
            return Command::none();
        }
        match self.store.save_tree() {
            Ok(()) => Command::none(),
            Err(error) => Command::toast(Toast::danger(t!("app.tree-write-failed", reason = error.to_string()))),
        }
    }

    /// Writes `session` and everything waiting before it. What cannot be written waits in
    /// memory and is said; the running file is cleared only for a session that is on disk.
    fn record(&mut self, session: Session) -> Command<Msg> {
        self.pending.push(session);
        let mut problem = None;
        while let Some(next) = self.pending.first().cloned() {
            match self.store.record(&next) {
                Ok(()) => {
                    self.pending.remove(0);
                }
                Err(error) => {
                    problem = Some(error.to_string());
                    break;
                }
            }
        }
        self.refresh_day();
        match problem {
            Some(reason) if self.on_disk => Command::toast(Toast::danger(t!("app.write-failed", reason = reason))),
            _ => Command::none(),
        }
    }

    /// Starts counting `focus`.
    fn start(&mut self, focus: Id) -> Command<Msg> {
        self.start_for(focus, None)
    }

    /// Starts counting `focus`, counting down to `target` seconds of work when one is given.
    fn start_for(&mut self, focus: Id, target: Option<u32>) -> Command<Msg> {
        if !self.can_start() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let Some((category, name)) = self.names(focus) else { return Command::none() };
        let now = self.now();
        let timer = Timer::start(focus, now, self.offset_minutes()).ceiling(self.prefs.ceiling);
        let said = Toast::info(t!("today.started", name = name.clone()));
        self.open_timer(timer, now, category, name, target, said)
    }

    /// Opens the Timer screen over `timer`, writes the running file, starts the ticks and shows
    /// `said`.
    fn open_timer(
        &mut self,
        timer: Timer,
        now: Clocks,
        category: String,
        name: String,
        target: Option<u32>,
        said: Toast<Msg>,
    ) -> Command<Msg> {
        let mut screen =
            TimerScreen::new(timer, now, category, name, Uptime::detects_suspend()).idle_after(self.prefs.idle_after);
        if let Some(target) = target {
            screen = screen.target(target);
        }
        screen.saved(self.save_running(&screen));
        // Goals already reached before this session are not news; only a crossing is said.
        self.goals_said = self.timer_goals(&screen).iter().filter(|goal| goal.reached()).map(|goal| goal.id).collect();
        self.timer = Some(screen);
        Command::batch([Command::toast(said), Command::focus(timer_screen::STOP), self.sync_tick()])
    }

    /// Writes the running file for `screen`, measured by this window, or says why it could not
    /// be. In memory nothing is written and nothing is wrong.
    fn save_running(&self, screen: &TimerScreen) -> Result<(), String> {
        self.save_running_as(screen, Watch::Window)
    }

    /// Writes the running file for `screen`, saying `watch` measures it from here on.
    fn save_running_as(&self, screen: &TimerScreen, watch: Watch) -> Result<(), String> {
        if !self.on_disk {
            return Ok(());
        }
        let running = Running { watch, ..screen.running() };
        self.store.save_running(&running).map_err(|error| error.to_string())
    }

    /// Ends the running counter at `now`, records the session and returns to Today.
    fn stop(&mut self, now: Clocks) -> Command<Msg> {
        self.stop_with(now, None)
    }

    /// The counter is being stopped past the ceiling: the session is not recorded yet, the
    /// person is asked whether to fix its duration first, and the counter goes on until they
    /// answer. Cancel has focus, so the key that stopped the counter answers nothing.
    fn ask_over_ceiling(&self) -> Command<Msg> {
        let Some(screen) = self.timer.as_ref() else { return Command::none() };
        let units = units();
        let question = Confirm::new(
            t!("timer.ceiling-title", duration = short(screen.work_seconds(), &units.as_units())),
            Msg::StopFix,
        )
        .message(t!("timer.ceiling-message", ceiling = short(u64::from(self.prefs.ceiling), &units.as_units())))
        .confirm_label(t!("timer.ceiling-fix"))
        .alternative(t!("timer.ceiling-leave"), Msg::StopLeave)
        .cancel_label(t!("app.quit-cancel"))
        .on_cancel(Msg::StopCancel);
        Command::confirm(question)
    }

    /// Opens the form on the duration of the session the counter would stop with now. The
    /// counter keeps running underneath: nothing is recorded until the form is saved, and
    /// cancelling it leaves the counter as it was.
    fn fix_over_ceiling(&mut self) -> Command<Msg> {
        let Some(screen) = self.timer.as_ref() else { return Command::none() };
        let now = self.now();
        let preview = screen.clone().finish(now, Id::new(0, 0));
        self.open_form(RecordForm::of(Purpose::Finish, &preview))
    }

    /// Ends the running counter at `now` and records it with the person's `changes` applied:
    /// the way out of the form that fixes a duration over the ceiling.
    fn stop_fixed(&mut self, changes: &Changes, now: Clocks) -> Command<Msg> {
        self.stop_with(now, Some(changes))
    }

    /// Ends the running counter at `now`, with `changes` applied to the session when the person
    /// typed some, records it and returns to Today.
    fn stop_with(&mut self, now: Clocks, changes: Option<&Changes>) -> Command<Msg> {
        let Some(screen) = self.timer.take() else { return Command::none() };
        let mut session = screen.finish(now, clock::new_id());
        if let Some(changes) = changes {
            session = edit::adjusted(&session, changes);
        }
        let focus = session.focus;
        let name = self.store.tree.focus(focus).map_or_else(|| t!("records.unknown-focus"), |focus| focus.name.clone());
        let duration = short(session.work_seconds(), &units().as_units());
        let recorded = self.record(session);
        let cleared = self.clear_running();
        let _: (Command<Msg>, _) =
            today::update(&mut self.today, &mut self.store.tree, today::Msg::Select(Row::Focus(focus).key()));
        Command::batch([
            recorded,
            cleared,
            Command::toast(Toast::success(t!("timer.stopped", name = name, duration = duration))),
            Command::focus(today::TREE),
            self.sync_tick(),
        ])
    }

    /// Removes the running file once nothing runs, unless a record still waits in memory: then
    /// the file is the only copy of the session and stays.
    fn clear_running(&self) -> Command<Msg> {
        if !self.on_disk || !self.pending.is_empty() {
            return Command::none();
        }
        match self.store.clear_running() {
            Ok(()) => Command::none(),
            Err(error) => Command::toast(Toast::danger(t!("app.write-failed", reason = error.to_string()))),
        }
    }

    /// The names of the category and the focus `id`.
    fn names(&self, id: Id) -> Option<(String, String)> {
        self.store.tree.categories.iter().find_map(|category| {
            category
                .focuses
                .iter()
                .find(|focus| focus.id == id)
                .map(|focus| (category.name.clone(), focus.name.clone()))
        })
    }

    /// Starts or stops the tick task to match what needs it: a running counter, or a lock still
    /// held by another instance.
    fn sync_tick(&mut self) -> Command<Msg> {
        let needed = self.timer.is_some() || (self.on_disk && !self.store.access.can_write());
        match (needed, self.tick) {
            (true, None) => {
                let task = Task::new("tick", |cx| {
                    while cx.sleep(std::time::Duration::from_secs(1)) {
                        cx.send(Msg::Tick);
                    }
                    Ok(Msg::Tick)
                });
                self.tick = Some(task.id());
                Command::task(task)
            }
            (false, Some(id)) => {
                self.tick = None;
                Command::cancel_task(id)
            }
            _ => Command::none(),
        }
    }

    /// A second passed: the counter ticks, and a lock another instance holds is tried again.
    fn tick(&mut self) -> Command<Msg> {
        if self.on_disk && !self.store.access.can_write() && self.store.retry_lock() {
            return self.sync_tick();
        }
        if self.timer.is_some() { self.timer_message(timer_screen::Msg::Tick) } else { self.sync_tick() }
    }

    /// Applies a message of the Timer screen and does what it asks.
    fn timer_message(&mut self, message: timer_screen::Msg) -> Command<Msg> {
        let Some(screen) = self.timer.as_mut() else { return Command::none() };
        let now = (self.clock)();
        let (command, request) = timer_screen::update(screen, message, now, &units().as_units());
        match request {
            Some(timer_screen::Request::Save) => {
                let result = if self.on_disk {
                    self.store.save_running(&screen.running()).map_err(|error| error.to_string())
                } else {
                    Ok(())
                };
                screen.saved(result);
                Command::batch([command, self.goals_crossed(now)])
            }
            Some(timer_screen::Request::Stop) if screen.running().flags.contains(&Flag::OverCeiling) => {
                Command::batch([command, self.ask_over_ceiling()])
            }
            Some(timer_screen::Request::Stop) => Command::batch([command, self.stop(now)]),
            Some(timer_screen::Request::Reached) => {
                // The countdown is up: the counter goes on unless the person asked it to stop.
                if self.prefs.stop_at_goal {
                    Command::batch([command, self.stop(now)])
                } else {
                    Command::batch([command, self.goals_crossed(now)])
                }
            }
            None => Command::batch([command, self.goals_crossed(now)]),
        }
    }

    /// The goals of the focus the counter counts and of its category, with the counter's work
    /// counted in.
    fn timer_goals(&self, screen: &TimerScreen) -> Vec<GoalRow> {
        let focus = screen.focus();
        let category = self.store.tree.categories.iter().find(|c| c.focuses.iter().any(|f| f.id == focus));
        let mine = |goal: &GoalRow| goal.id == focus || category.is_some_and(|c| c.id == goal.id);
        goal_rows(&self.store.tree, &self.sessions, self.date, &self.prefs, Some((focus, screen.work_seconds())))
            .into_iter()
            .filter(mine)
            .collect()
    }

    /// Says once each goal the running counter has just reached, and stops the counter for it
    /// when the person asked for that.
    fn goals_crossed(&mut self, now: Clocks) -> Command<Msg> {
        let Some(screen) = self.timer.as_ref() else { return Command::none() };
        let crossed: Vec<GoalRow> = self
            .timer_goals(screen)
            .into_iter()
            .filter(|goal| goal.reached() && !self.goals_said.contains(&goal.id))
            .collect();
        if crossed.is_empty() {
            return Command::none();
        }
        let units = units();
        let mut commands: Vec<Command<Msg>> = crossed
            .iter()
            .map(|goal| {
                let text = t!(
                    "timer.goal-reached-toast",
                    name = goal.name.clone(),
                    amount = short(goal.amount, &units.as_units())
                );
                Command::toast(Toast::success(text))
            })
            .collect();
        self.goals_said.extend(crossed.iter().map(|goal| goal.id));
        if self.prefs.stop_at_goal {
            commands.push(self.stop(now));
        }
        Command::batch(commands)
    }

    /// Applies a message of the Today screen and does what it asks.
    fn today_message(&mut self, message: today::Msg) -> Command<Msg> {
        let (command, request) = today::update(&mut self.today, &mut self.store.tree, message);
        match request {
            Some(today::Request::Start(focus)) => Command::batch([command, self.start(focus)]),
            Some(today::Request::StartFor(focus, seconds)) => {
                Command::batch([command, self.start_for(focus, Some(seconds))])
            }
            Some(today::Request::Changed) => Command::batch([command, self.save_tree()]),
            None => command,
        }
    }

    /// Applies a message of the Records screen and does what it asks.
    fn records_message(&mut self, message: records::Msg) -> Command<Msg> {
        let (command, request) =
            records::update(&mut self.records, &self.sessions, &self.store.voided, &mut self.store.tree, message);
        match request {
            Some(records::Request::Void(id)) => Command::batch([command, self.remove(id)]),
            Some(records::Request::Restore(id)) => Command::batch([command, self.bring_back(id)]),
            Some(records::Request::Changed) => Command::batch([command, self.save_tree()]),
            Some(records::Request::Export) => Command::batch([command, self.export()]),
            Some(records::Request::Add) => Command::batch([command, self.add_by_hand()]),
            Some(records::Request::Correct(id)) => Command::batch([command, self.open_over(Purpose::Correct(id), id)]),
            Some(records::Request::Attach(id)) => Command::batch([command, self.open_over(Purpose::Attach(id), id)]),
            None => command,
        }
    }

    /// Applies a message of the Settings screen and does what it asks.
    fn settings_message(&mut self, message: settings::Msg) -> Command<Msg> {
        let (command, request) = settings::update(&mut self.settings_screen, &self.prefs, message);
        match request {
            Some(settings::Request::Shared(change)) => {
                match change {
                    settings::Shared::Language(code) => self.settings.set(Settings::LANGUAGE, code),
                    settings::Shared::Theme(id) => self.settings.set(Settings::THEME, id),
                    settings::Shared::Icons(mode) => self.settings.set(Settings::ICONS, mode.name().to_owned()),
                    settings::Shared::ReducedMotion(reduced) => self.settings.set(Settings::REDUCED_MOTION, reduced),
                    settings::Shared::Pillar(style) => self.settings.set(Settings::PILLAR, style.name().to_owned()),
                };
                Command::batch([command, self.save_settings()])
            }
            Some(settings::Request::Prefs(prefs)) => {
                self.prefs = prefs;
                self.prefs.write(&mut self.settings);
                // The day and the goals regroup at once; the records are untouched.
                self.refresh_day();
                Command::batch([command, self.save_settings()])
            }
            Some(settings::Request::ResetStats) => Command::batch([command, self.reset_stats()]),
            Some(settings::Request::DeleteOlder(date)) => Command::batch([command, self.delete_older(date)]),
            Some(settings::Request::DeleteAll) => Command::batch([command, self.delete_all()]),
            Some(settings::Request::EmptyTrash) => Command::batch([command, self.ask_purge()]),
            Some(settings::Request::Purge) => Command::batch([command, self.purge()]),
            Some(settings::Request::PurgeCancelled) => {
                Command::batch([command, Command::toast(Toast::info(t!("settings.purge-cancelled")))])
            }
            None => command,
        }
    }

    /// Writes the settings file off the drawing thread; the screen says if it could not be.
    fn save_settings(&self) -> Command<Msg> {
        self.settings.save_command(|result| Msg::Settings(settings::Msg::Stored(result)))
    }

    /// Moves every current session to the trash, one line per record, and offers the way back.
    /// A pass that stops half way says how far it got; the rest stays current for another pass.
    fn reset_stats(&mut self) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let total = u32::try_from(self.sessions.len()).unwrap_or(u32::MAX);
        if total == 0 {
            return Command::toast(Toast::info(t!("settings.nothing-to-reset")));
        }
        let now = self.now();
        let (moved, error) = self.store.void_all(now.wall);
        self.refresh_day();
        let done = u32::try_from(moved.len()).unwrap_or(u32::MAX);
        if !moved.is_empty() {
            self.last_undo = Some(Undo::Wiped { sessions: moved, rows: Vec::new() });
        }
        match error {
            Some(error) => Command::toast(Toast::danger(t!(
                "settings.reset-partial",
                done = done,
                total = total,
                reason = error.to_string()
            ))),
            None => {
                Command::toast(Toast::success(t!("settings.reset-done", n = done)).action(t!("today.undo"), Msg::Undo))
            }
        }
    }

    /// Moves every current session whose day, grouped as the Records page and the charts group
    /// it, is before `date` to the trash, one line per record, and offers the way back.
    fn delete_older(&mut self, date: Date) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let rollover = self.prefs.rollover;
        let older = |session: &Session| crate::day::day_of(session, rollover) < date;
        let total =
            u32::try_from(self.store.sessions.iter().filter(|session| older(session)).count()).unwrap_or(u32::MAX);
        if total == 0 {
            return Command::toast(Toast::info(t!("settings.nothing-older")));
        }
        let now = self.now();
        let (moved, error) = self.store.void_matching(older, now.wall);
        self.refresh_day();
        let done = u32::try_from(moved.len()).unwrap_or(u32::MAX);
        if !moved.is_empty() {
            self.last_undo = Some(Undo::Wiped { sessions: moved, rows: Vec::new() });
        }
        match error {
            Some(error) => Command::toast(Toast::danger(t!(
                "settings.reset-partial",
                done = done,
                total = total,
                reason = error.to_string()
            ))),
            None => {
                Command::toast(Toast::success(t!("settings.reset-done", n = done)).action(t!("today.undo"), Msg::Undo))
            }
        }
    }

    /// Moves every current session to the trash and archives every category and focus, so the
    /// lists are empty and the past is in the trash; the way back brings both.
    fn delete_all(&mut self) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let total = u32::try_from(self.sessions.len()).unwrap_or(u32::MAX);
        let now = self.now();
        let (moved, error) = self.store.void_all(now.wall);
        self.refresh_day();
        let done = u32::try_from(moved.len()).unwrap_or(u32::MAX);
        if let Some(error) = error {
            if !moved.is_empty() {
                self.last_undo = Some(Undo::Wiped { sessions: moved, rows: Vec::new() });
            }
            let text = t!("settings.reset-partial", done = done, total = total, reason = error.to_string());
            return Command::toast(Toast::danger(text));
        }
        let mut rows = Vec::new();
        for category in &mut self.store.tree.categories {
            if !category.archived {
                category.archived = true;
                rows.push(Row::Category(category.id));
            }
            for focus in &mut category.focuses {
                if !focus.archived {
                    focus.archived = true;
                    rows.push(Row::Focus(focus.id));
                }
            }
        }
        let archived = u32::try_from(rows.len()).unwrap_or(u32::MAX);
        let saved = self.save_tree();
        self.today = Today::new();
        if !moved.is_empty() || !rows.is_empty() {
            self.last_undo = Some(Undo::Wiped { sessions: moved, rows });
        }
        let toast =
            Toast::success(t!("settings.delete-done", n = done, rows = archived)).action(t!("today.undo"), Msg::Undo);
        Command::batch([saved, Command::toast(toast)])
    }

    /// The focuses something outside the store's records still refers to: the records waiting in
    /// memory, the running counter and a running file waiting to be recovered. Emptying the
    /// trash must not take them out of the tree.
    fn kept_focuses(&self) -> Vec<Id> {
        let mut focuses: Vec<Id> = self.pending.iter().map(|session| session.focus).collect();
        focuses.extend(self.timer.as_ref().map(|screen| screen.running().focus));
        if self.on_disk
            && let Some(Ok(running)) = self.store.running()
        {
            focuses.push(running.focus);
        }
        focuses
    }

    /// Says what emptying the trash would take and asks, in the danger tone, before doing it.
    /// With nothing to take nothing is asked; copies alone in the trash folder are backups and
    /// wait for the next purge that takes something.
    fn ask_purge(&mut self) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let plan = self.store.purge_plan(&self.kept_focuses());
        if plan.is_empty() {
            return Command::toast(Toast::info(t!("settings.purge-nothing")));
        }
        let message = t!("settings.purge-message", records = purge_records(plan.records), rows = purge_rows(plan.rows));
        let question = Confirm::new(t!("settings.purge-title"), Msg::Settings(settings::Msg::Purge))
            .message(message)
            .danger()
            .confirm_label(t!("settings.purge-confirm"))
            .require_word(t!("settings.purge-word"))
            .on_cancel(Msg::Settings(settings::Msg::PurgeCancelled));
        Command::confirm(question)
    }

    /// Empties the trash for good and says what went and what stayed. Nothing of it can be taken
    /// back, so the pending `ctrl+z` goes too: it would reach for records no longer on the disk.
    fn purge(&mut self) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let result = self.store.purge(&self.kept_focuses());
        self.last_undo = None;
        self.refresh_day();
        match result {
            Ok(purged) => {
                let done =
                    t!("settings.purged", records = purge_records(purged.records), rows = purge_rows(purged.rows));
                let mut toasts = vec![Command::toast(Toast::success(done))];
                if purged.kept_rows > 0 {
                    let kept = t!("settings.purge-kept", n = u32::try_from(purged.kept_rows).unwrap_or(u32::MAX));
                    toasts.push(Command::toast(Toast::info(kept)));
                }
                Command::batch(toasts)
            }
            Err(error) => Command::toast(Toast::danger(t!("settings.purge-failed", reason = error.to_string()))),
        }
    }

    /// Brings back what a reset or a deletion took: every session from the trash, each with its
    /// own line, and every row from the archive. A pass that stops half way says how far it got.
    fn unwipe(&mut self, sessions: Vec<Id>, rows: Vec<Row>) -> Command<Msg> {
        if !self.can_edit() {
            self.last_undo = Some(Undo::Wiped { sessions, rows });
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let now = self.now();
        let total = u32::try_from(sessions.len()).unwrap_or(u32::MAX);
        let mut restored = 0_u32;
        let mut problem = None;
        let mut left = Vec::new();
        for id in sessions {
            if problem.is_some() {
                left.push(id);
                continue;
            }
            match self.store.restore(id, now.wall) {
                Ok(()) => restored += 1,
                Err(error) => {
                    problem = Some(error.to_string());
                    left.push(id);
                }
            }
        }
        for row in &rows {
            match row {
                Row::Category(id) => {
                    if let Some(category) = self.store.tree.categories.iter_mut().find(|c| c.id == *id) {
                        category.archived = false;
                    }
                }
                Row::Focus(id) => {
                    if let Some(focus) =
                        self.store.tree.categories.iter_mut().flat_map(|c| c.focuses.iter_mut()).find(|f| f.id == *id)
                    {
                        focus.archived = false;
                    }
                }
                Row::AddFocus(_) | Row::AddCategory => {}
            }
        }
        let saved = if rows.is_empty() { Command::none() } else { self.save_tree() };
        self.refresh_day();
        match problem {
            Some(reason) => {
                // What did not come back can be tried again with the next ctrl+z.
                self.last_undo = Some(Undo::Wiped { sessions: left, rows: Vec::new() });
                let text = t!("settings.restore-partial", done = restored, total = total, reason = reason);
                Command::batch([saved, Command::toast(Toast::danger(text))])
            }
            None => Command::batch([saved, Command::toast(Toast::success(t!("settings.restored-all", n = restored)))]),
        }
    }

    /// Opens the empty form for a session typed in by hand. A memory-only store takes it too:
    /// the record waits with the others that could not be written.
    fn add_by_hand(&mut self) -> Command<Msg> {
        if !self.can_start() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let now = DateTime::from_unix(self.now().wall, self.offset_minutes());
        self.open_form(RecordForm::add(now))
    }

    /// Opens the form over the current session `id` for `purpose`.
    fn open_over(&mut self, purpose: Purpose, id: Id) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        match self.sessions.iter().find(|session| session.id == id) {
            Some(session) => self.open_form(RecordForm::of(purpose, session)),
            None => Command::none(),
        }
    }

    /// Puts `form` over the page and the keyboard on its first field.
    fn open_form(&mut self, form: RecordForm) -> Command<Msg> {
        let first = form.first_field();
        self.form = Some(form);
        Command::focus(first)
    }

    /// Applies a message of the record form, and writes the record it hands back.
    fn form_message(&mut self, message: record_form::Msg) -> Command<Msg> {
        let Some(form) = self.form.as_mut() else { return Command::none() };
        let now = (self.clock)();
        let (command, outcome) = record_form::update(form, &self.store.tree, now.wall, message);
        match outcome {
            Some(Outcome::Save(changes)) => {
                let purpose = form.purpose().clone();
                self.form = None;
                Command::batch([command, self.save_form(&purpose, &changes, now), self.focus_after_form()])
            }
            Some(Outcome::Cancel) => {
                self.form = None;
                Command::batch([command, Command::toast(Toast::info(t!("app.cancelled"))), self.focus_after_form()])
            }
            None => command,
        }
    }

    /// Where the keyboard goes once the form closes: the table on Records, the counter or the
    /// tree on Today.
    fn focus_after_form(&mut self) -> Command<Msg> {
        self.open_page(self.page.index())
    }

    /// Writes what the form handed back: a new record by hand, a correction that replaces the
    /// session it was opened over, or the session the counter stopped with, fixed.
    fn save_form(&mut self, purpose: &Purpose, changes: &Changes, now: Clocks) -> Command<Msg> {
        let name = self.store.tree.focus(changes.focus).map_or_else(String::new, |focus| focus.name.clone());
        let duration = short(u64::from(changes.seconds), &units().as_units());
        match purpose {
            Purpose::Add => {
                let session = edit::manual(changes, clock::new_id(), now.wall);
                let recorded = self.record(session);
                Command::batch([
                    recorded,
                    Command::toast(Toast::success(t!("records.added", name = name, duration = duration))),
                ])
            }
            Purpose::Correct(id) | Purpose::Attach(id) => {
                let Some(original) = self.sessions.iter().find(|session| session.id == *id).cloned() else {
                    return Command::none();
                };
                let correction = edit::correction(&original, changes, clock::new_id(), now.wall);
                let written = correction.id;
                let recorded = self.record(correction);
                self.last_undo = Some(Undo::Corrected { previous: Box::new(original), correction: written });
                let text = if matches!(purpose, Purpose::Attach(_)) {
                    t!("records.attached", name = name)
                } else {
                    t!("records.corrected", name = name)
                };
                Command::batch([recorded, Command::toast(Toast::success(text).action(t!("today.undo"), Msg::Undo))])
            }
            Purpose::Finish => self.stop_fixed(changes, now),
        }
    }

    /// Moves the session `id` to the trash and offers the way back in the toast.
    fn remove(&mut self, id: Id) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let now = self.now();
        let name = self.session_name(id);
        match self.store.void(id, now.wall) {
            Ok(()) => {
                self.last_undo = Some(Undo::Removed(id));
                self.refresh_day();
                Command::toast(Toast::info(t!("records.deleted", name = name)).action(t!("today.undo"), Msg::Undo))
            }
            Err(error) => Command::toast(Toast::danger(t!("app.write-failed", reason = error.to_string()))),
        }
    }

    /// Brings the session `id` back from the trash.
    fn bring_back(&mut self, id: Id) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let now = self.now();
        let name = self.session_name(id);
        match self.store.restore(id, now.wall) {
            Ok(()) => {
                if self.last_undo == Some(Undo::Removed(id)) {
                    self.last_undo = None;
                }
                self.refresh_day();
                Command::toast(Toast::success(t!("records.restored", name = name)))
            }
            Err(error) => Command::toast(Toast::danger(t!("app.write-failed", reason = error.to_string()))),
        }
    }

    /// Ctrl+Z: brings back the session removed last, or takes back the correction made last
    /// with a line that replaces it with the version before, at a higher revision
    /// ([`crate::session::resolve`]). Nothing on disk is touched either way.
    fn undo(&mut self) -> Command<Msg> {
        match self.last_undo.take() {
            Some(Undo::Removed(id)) => {
                self.last_undo = Some(Undo::Removed(id));
                self.bring_back(id)
            }
            Some(Undo::Wiped { sessions, rows }) => self.unwipe(sessions, rows),
            Some(Undo::Corrected { previous, correction }) => {
                if !self.can_edit() {
                    self.last_undo = Some(Undo::Corrected { previous, correction });
                    return Command::toast(Toast::warning(t!("today.cannot-start")));
                }
                let Some(current) = self.store.sessions.iter().find(|session| session.id == correction) else {
                    return Command::toast(Toast::info(t!("records.nothing-to-undo")));
                };
                let now = self.now();
                let name = self.session_name(correction);
                let revert = Session {
                    id: clock::new_id(),
                    revision: current.revision.saturating_add(1),
                    written: now.wall,
                    replaces: Some(correction),
                    voids: None,
                    ..*previous
                };
                let recorded = self.record(revert);
                Command::batch([recorded, Command::toast(Toast::success(t!("records.correction-undone", name = name)))])
            }
            None => Command::toast(Toast::info(t!("records.nothing-to-undo"))),
        }
    }

    /// The name of the focus of the session `id`, wherever the session is now.
    fn session_name(&self, id: Id) -> String {
        self.sessions
            .iter()
            .chain(self.store.voided.iter())
            .find(|session| session.id == id)
            .and_then(|session| self.store.tree.focus(session.focus))
            .map_or_else(|| t!("records.unknown-focus"), |focus| focus.name.clone())
    }

    /// Writes every current session as CSV and JSON into the export folder, named by today's
    /// date, and says where they went or why they could not be written.
    fn export(&self) -> Command<Msg> {
        if !self.on_disk {
            return Command::toast(Toast::warning(t!("records.export-nowhere")));
        }
        let day = self.date;
        let stem = format!("qfocus-{:04}-{:02}-{:02}", day.year(), day.month(), day.day());
        let folder = self.store.paths.export_dir();
        let shown = folder.join(&stem).display().to_string();
        let files = [
            (folder.join(format!("{stem}.csv")), export::csv(&self.sessions, &self.store.tree)),
            (folder.join(format!("{stem}.json")), export::json(&self.sessions, &self.store.tree)),
        ];
        let written = std::fs::create_dir_all(&folder)
            .and_then(|()| files.iter().try_for_each(|(path, text)| atomic_write(path, text.as_bytes())));
        match written {
            Ok(()) => Command::toast(Toast::success(t!("records.exported", path = shown))),
            Err(error) => {
                Command::toast(Toast::danger(t!("records.export-failed", path = shown, reason = error.to_string())))
            }
        }
    }

    /// Opens the tab at `index` and puts the keyboard on its main widget.
    fn open_page(&mut self, index: usize) -> Command<Msg> {
        let Some(page) = Page::ALL.get(index).copied() else { return Command::none() };
        self.page = page;
        match page {
            Page::Today => Command::focus(if self.timer.is_some() { timer_screen::STOP } else { today::TREE }),
            Page::Charts => Command::focus(charts::CHART),
            Page::Records => Command::focus(records::TABLE),
            Page::Settings => Command::focus(settings::LIST),
        }
    }

    /// Does what the recovery dialog chose.
    fn recovered(&mut self, choice: recover::Msg) -> Command<Msg> {
        let Some(found) = self.recover.take() else { return Command::none() };
        let Recover::Found { running, focus, reach, .. } = found else { return Command::none() };
        let now = self.now();
        match choice {
            recover::Msg::Save => {
                // Ended where the counter was last alive: the timer is restored at that moment and
                // stopped there, so its spans fill the session exactly.
                let then = Clocks { wall: reach.until, uptime: now.uptime };
                let timer = Timer::restore(
                    running.focus,
                    running.started,
                    running.offset_minutes,
                    running.spans,
                    running.paused,
                    running.refreshed,
                    reach.until,
                    then,
                )
                .with_flags(running.flags);
                let session = timer.stop(then, clock::new_id(), now.wall);
                let duration = short(session.work_seconds(), &units().as_units());
                let recorded = self.record(session);
                let name = focus.unwrap_or_else(|| t!("recover.unknown-focus"));
                Command::batch([
                    recorded,
                    self.clear_running(),
                    Command::toast(Toast::success(t!("recover.saved", focus = name, duration = duration))),
                ])
            }
            recover::Msg::Continue => {
                let timer = Timer::restore(
                    running.focus,
                    running.started,
                    running.offset_minutes,
                    running.spans,
                    running.paused,
                    running.refreshed,
                    reach.until,
                    now,
                )
                .with_flags(running.flags)
                .with_unclaimed_idle(running.idle_from);
                let (category, name) =
                    self.names(running.focus).unwrap_or_else(|| (String::new(), t!("recover.unknown-focus")));
                let said = Toast::info(t!("today.started", name = name.clone()));
                self.open_timer(timer, now, category, name, running.target, said)
            }
            recover::Msg::Discard => {
                let cleared = match self.store.clear_running() {
                    Ok(()) => Command::none(),
                    Err(error) => Command::toast(Toast::danger(t!("app.write-failed", reason = error.to_string()))),
                };
                Command::batch([cleared, Command::toast(Toast::info(t!("recover.discarded")))])
            }
            recover::Msg::Close => Command::none(),
        }
    }

    /// Esc: the open layer closes, one at a time; at the root nothing happens.
    fn back(&mut self) -> Command<Msg> {
        if self.quit_asked {
            self.quit_asked = false;
            return Command::toast(Toast::info(t!("app.cancelled")));
        }
        if self.form.is_some() {
            return self.form_message(record_form::Msg::Cancel);
        }
        if self.records.is_dumping() {
            return self.records_message(records::Msg::CloseDump);
        }
        if self.records.is_searching() {
            return self.records_message(records::Msg::SearchClose);
        }
        if self.page == Page::Today && self.timer.as_ref().is_some_and(TimerScreen::is_editing_note) {
            return self.timer_message(timer_screen::Msg::NoteCancel);
        }
        if self.page == Page::Today && self.today.edit().is_some() {
            return self.today_message(today::Msg::Cancel);
        }
        if self.page == Page::Today && self.today.goal_edit().is_some() {
            return self.today_message(today::Msg::GoalCancel);
        }
        if self.page == Page::Today && self.today.timed().is_some() {
            return self.today_message(today::Msg::TimedCancel);
        }
        Command::none()
    }

    /// Space: stops the counter, or starts the focus worked on last.
    fn toggle(&mut self) -> Command<Msg> {
        if self.timer.is_some() {
            return self.timer_message(timer_screen::Msg::Stop);
        }
        let last = self
            .store
            .sessions
            .iter()
            .chain(self.pending.iter())
            .max_by_key(|session| session.ended)
            .map(|session| session.focus)
            .filter(|focus| self.store.tree.focus(*focus).is_some_and(|focus| !focus.archived));
        match last {
            Some(focus) => self.start(focus),
            None => Command::toast(Toast::info(t!("today.nothing-worked"))),
        }
    }

    /// The runtime asked to leave while a counter runs: the question opens, or stays open if it
    /// already is.
    fn quit(&mut self) -> Command<Msg> {
        self.quit_asked = true;
        Command::none()
    }

    /// Leaves with the counter still on disk, refreshed to this moment and marked as measured by
    /// nobody, so it goes on counting until it is stopped or the machine restarts.
    fn quit_leaving(&mut self) -> Command<Msg> {
        self.quit_asked = false;
        // A file that could not be refreshed would lose the counter if the program left now,
        // so the person stays and sees why on the screen.
        if self.refresh_running(Watch::None) { Command::quit() } else { Command::none() }
    }

    /// Brings the running file up to this moment, saying `watch` measures it from here on.
    /// `true` when it is on disk, or when nothing runs; `false` when the write failed, which the
    /// counter's screen then says.
    fn refresh_running(&mut self, watch: Watch) -> bool {
        let now = self.now();
        let Some(mut screen) = self.timer.take() else { return true };
        let (_, _): (Command<Msg>, _) =
            timer_screen::update(&mut screen, timer_screen::Msg::Tick, now, &units().as_units());
        let saved = self.save_running_as(&screen, watch);
        let failed = saved.is_err();
        screen.saved(saved);
        self.timer = Some(screen);
        !failed
    }

    /// The dashboard comes over the page, or leaves it: the page underneath is not touched, so
    /// what was selected and filtered stays. While it stands a task turns its clock at every
    /// minute; the first wait ends on the minute so the clock never lags behind the real one.
    fn idle(&mut self, away: bool) -> Command<Msg> {
        self.idle = away;
        match (away, self.minute) {
            (true, None) => self.arm_minute(),
            (false, Some(id)) => {
                self.minute = None;
                Command::cancel_task(id)
            }
            _ => Command::none(),
        }
    }

    /// Starts the wait for the next minute of the dashboard's clock.
    fn arm_minute(&mut self) -> Command<Msg> {
        let wall = self.now().wall;
        let until = 60 - u64::try_from(wall.rem_euclid(60)).unwrap_or(0);
        let task = Task::new("dashboard-clock", move |cx| {
            // A cancelled wait ends the task; its message is then ignored anyway.
            if cx.sleep(Duration::from_secs(until)) { Ok(Msg::Minute) } else { Err("stopped".to_owned()) }
        });
        self.minute = Some(task.id());
        Command::task(task)
    }

    /// A minute turned: the frame is drawn again with it, and the next minute is waited for as
    /// long as the dashboard stands.
    fn minute(&mut self) -> Command<Msg> {
        self.minute = None;
        if self.idle { self.arm_minute() } else { Command::none() }
    }

    /// The terminal is gone: the counter is refreshed on disk and the program leaves. A write
    /// that fails cannot be shown to anyone; the file from the last refresh, at most a few
    /// seconds old, is what recovery finds. The file still says a window measures it: this is
    /// the last moment the counter is known to be alive, and it counts no further.
    fn hung_up(&mut self) -> Command<Msg> {
        self.refresh_running(Watch::Window);
        Command::quit()
    }

    /// The system asks the program to end: the counter is refreshed on disk first, so the
    /// grace running out loses nothing, then the person is asked as for a quit of their own.
    fn terminated(&mut self) -> Command<Msg> {
        self.refresh_running(Watch::Window);
        self.quit()
    }
}

impl App for QFocus {
    type Msg = Msg;

    fn init(&mut self) -> Command<Msg> {
        // A lock held by another instance is retried on the ticks, so they start before the
        // first frame; and a counter the last run left is asked about before anything else.
        let found = self.find_running();
        Command::batch([found, self.sync_tick()])
    }

    fn before_quit(&self) -> Option<Msg> {
        // Nothing runs: the runtime may leave. A counter runs: it is asked about first, and
        // the answer leaves with `Command::quit`, which is not asked about again.
        self.timer.is_some().then_some(Msg::Quit)
    }

    fn terminating(&self, cause: Termination) -> Option<Msg> {
        // Nothing runs: the runtime may leave at once. A cause this version does not know is
        // treated like a hangup: save without asking.
        self.timer.as_ref()?;
        Some(match cause {
            Termination::Hangup => Msg::HungUp,
            Termination::Terminate => Msg::Terminated,
            _ => Msg::HungUp,
        })
    }

    fn update(&mut self, msg: Msg) -> Command<Msg> {
        self.apply(msg)
    }

    fn action(&self, name: &str) -> Option<Msg> {
        match name {
            "back" => Some(Msg::Back),
            "archive" => Some(Msg::Archive),
            "rename" => Some(Msg::Rename),
            "pause" => Some(Msg::Pause),
            "note" => Some(Msg::Note),
            "toggle" => Some(Msg::Toggle),
            "today" => Some(Msg::Page(Page::Today.index())),
            "charts" => Some(Msg::Page(Page::Charts.index())),
            "records" => Some(Msg::Page(Page::Records.index())),
            "settings" => Some(Msg::Page(Page::Settings.index())),
            "search" => Some(Msg::Search),
            "export" => Some(Msg::Export),
            "undo" => Some(Msg::Undo),
            "add" => Some(Msg::Add),
            "spans" => Some(Msg::Spans),
            "goal" => Some(Msg::Goal),
            "countdown" => Some(Msg::Countdown),
            _ => None,
        }
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        // The watches live here, not on the screens: the counter runs on whichever page is open,
        // and a watch is only answered while the frame declares it. With a counter the silence
        // is the work's concern and, later, the screen's; without one it is the dashboard's.
        match &self.timer {
            Some(screen) => {
                ui.on_idle(screen.idle_threshold(), |away| Msg::Timer(timer_screen::Msg::Idle(away)));
                ui.on_idle(QUIET_AFTER, Msg::Quiet);
            }
            None => ui.on_idle(DASHBOARD_AFTER, Msg::Idle),
        }
        AppShell::new().header(|ui| self.header(ui)).body(|ui| self.body(ui)).footer(|ui| self.hints(ui)).show(ui);
        if let Some(recover) = &self.recover {
            recover::view(recover, &units().as_units(), self.now().wall, self.offset_minutes(), ui);
        }
        if let Some(form) = &self.form {
            record_form::view(form, &self.store.tree, self.date, self.now().wall, ui);
        }
        if self.quit_asked {
            self.quit_question(ui);
        }
        if self.idle {
            self.dashboard(ui);
        }
    }
}

impl QFocus {
    /// Applies one message.
    fn apply(&mut self, msg: Msg) -> Command<Msg> {
        // While the form is open its fields take the keys; an application key that reaches
        // here would act on the page under it, so it does nothing but close the form with Esc.
        if self.form.is_some()
            && !matches!(
                msg,
                Msg::Form(_)
                    | Msg::Back
                    | Msg::Tick
                    | Msg::Quit
                    | Msg::QuitFinish
                    | Msg::QuitLeave
                    | Msg::QuitCancel
                    | Msg::Terminated
                    | Msg::HungUp
                    | Msg::Timer(_)
                    | Msg::Recover(_)
                    | Msg::Idle(_)
                    | Msg::Quiet(_)
                    | Msg::Minute
            )
        {
            return Command::none();
        }
        match msg {
            Msg::Form(message) => self.form_message(message),
            Msg::Add if self.page == Page::Records => self.records_message(records::Msg::Add),
            Msg::Spans if self.page == Page::Records => self.records_message(records::Msg::Spans),
            Msg::Add | Msg::Spans => Command::none(),
            Msg::Today(message) => self.today_message(message),
            Msg::Charts(message) => charts::update(&mut self.charts, message),
            Msg::Records(message) => self.records_message(message),
            Msg::Settings(message) => self.settings_message(message),
            Msg::Timer(message) => self.timer_message(message),
            Msg::Recover(choice) => self.recovered(choice),
            Msg::Page(index) => self.open_page(index),
            Msg::Search if self.page == Page::Records => self.records_message(records::Msg::Search),
            Msg::Export if self.page == Page::Records => self.export(),
            Msg::Search | Msg::Export => Command::none(),
            Msg::Undo => self.undo(),
            Msg::Tick => self.tick(),
            Msg::Back => self.back(),
            Msg::Archive if self.page == Page::Records => self.records_message(records::Msg::Delete),
            Msg::Rename if self.page == Page::Records => self.records_message(records::Msg::Correct),
            Msg::Archive if self.page == Page::Today && self.timer.is_none() && !self.today.is_editing() => {
                self.today_message(today::Msg::Archive)
            }
            Msg::Rename if self.page == Page::Today && self.timer.is_none() && !self.today.is_editing() => {
                self.today_message(today::Msg::Rename)
            }
            Msg::Goal if self.page == Page::Today && self.timer.is_none() && !self.today.is_editing() => {
                let suggested = self.prefs.default_goal;
                self.today_message(today::Msg::Goal(suggested))
            }
            Msg::Countdown if self.page == Page::Today && self.timer.is_none() && !self.today.is_editing() => {
                let suggested = self.prefs.default_goal;
                self.today_message(today::Msg::Timed(suggested))
            }
            Msg::Archive | Msg::Rename | Msg::Goal | Msg::Countdown => Command::none(),
            Msg::Pause => self.timer_message(timer_screen::Msg::Pause),
            Msg::Note => self.timer_message(timer_screen::Msg::Note),
            Msg::Toggle => self.toggle(),
            Msg::Quit => self.quit(),
            Msg::QuitFinish => {
                self.quit_asked = false;
                let now = self.now();
                Command::batch([self.stop(now), Command::quit()])
            }
            Msg::QuitLeave => self.quit_leaving(),
            Msg::Terminated => self.terminated(),
            Msg::HungUp => self.hung_up(),
            Msg::QuitCancel => self.back(),
            Msg::StopFix => self.fix_over_ceiling(),
            Msg::StopLeave => {
                let now = self.now();
                self.stop(now)
            }
            Msg::StopCancel => Command::toast(Toast::info(t!("app.cancelled"))),
            Msg::Idle(away) => self.idle(away),
            Msg::Quiet(away) => {
                self.quiet = away;
                Command::none()
            }
            Msg::Minute => self.minute(),
        }
    }

    /// The dashboard: a layer over the page for a terminal left open, with the time large, the
    /// day's strip, the focuses worked on most today and how full the week is. The page keeps
    /// its state underneath; the first input brings it back as it was.
    fn dashboard(&self, ui: &mut View<'_, Msg>) {
        let now = self.now();
        let local = DateTime::from_unix(now.wall, self.offset_minutes()).time;
        let time = format!("{:02}:{:02}", local.hour, local.minute);
        let width = ui.size().width.saturating_sub(2 * DASHBOARD_MARGIN).max(1);
        let blocks = stats::strip(&self.sessions, None, self.date, self.prefs.rollover);
        let units = units();
        let top = stats::top_focuses(&self.sessions, stats::Range::day(self.date), DASHBOARD_TOP, self.prefs.rollover);
        let goals = goal_rows(&self.store.tree, &self.sessions, self.date, &self.prefs, None);
        let week = stats::goal_range(crate::tree::Period::Week, self.date, self.prefs.week_start);
        let week_total = stats::total(&self.sessions, week, self.prefs.rollover);
        let dialog = Modal::new().dismissable(false).width(width);
        ui.add_with(dialog, |ui| {
            ui.column(|ui| {
                ui.add(BigText::new(time).variant("accent"));
                today::strip_view(&self.today, &self.store.tree, &blocks, self.prefs.rollover, ui);
                if !top.is_empty() {
                    ui.add(Text::new(t!("dashboard.top-title")).role("secondary").no_wrap());
                    let bars = top.iter().map(|(id, seconds)| {
                        let name =
                            self.store.tree.focus(*id).map_or_else(|| t!("records.unknown-focus"), |f| f.name.clone());
                        Bar::new(name, (*seconds / 60) as f32).value_text(short(*seconds, &units.as_units()))
                    });
                    ui.add(BarChart::<Msg>::new(bars).gap(0)).fill_width();
                }
                if goals.is_empty() {
                    ui.add(
                        Text::new(t!("dashboard.week", duration = short(week_total, &units.as_units())))
                            .role("secondary"),
                    )
                    .fill_width();
                } else {
                    ui.add(Text::new(t!("dashboard.goals-title")).role("secondary").no_wrap());
                    goal_gauges(&goals, &units.as_units(), ui);
                }
            })
            .gap(1)
            .align(Align::Center)
            .fill_width();
        });
    }

    /// The top: the name, the day and its total, the tabs, and every standing warning under them.
    fn header(&self, ui: &mut View<'_, Msg>) {
        let day = self.date;
        let date = t!(
            "today.date",
            weekday = t!(&format!("days.{}", day.weekday().number())),
            day = u32::from(day.day()),
            month = t!(&format!("months.{}", day.month()))
        );
        ui.column(|ui| {
            ui.row(|ui| {
                ui.add(Text::rich([Span::new(t!("app.name")).color("accent").bold()]).no_wrap());
                ui.add(Text::new(date).role("secondary").no_wrap());
                ui.spacer();
                ui.add(Text::new(short(self.total_today(), &units().as_units())).bold().no_wrap());
            })
            .gap(2)
            .fill_width();
            ui.add(
                Tabs::new([t!("tabs.today"), t!("tabs.charts"), t!("tabs.records"), t!("tabs.settings")])
                    .active(self.page.index())
                    .on_select(Msg::Page),
            )
            .fill_width();
            if !self.on_disk {
                warning_line(t!("app.memory-only"), ui);
            }
            match self.store.access {
                Access::Writer => {}
                Access::ReadOnly { holder: Some(pid) } if self.on_disk => {
                    warning_line(t!("app.read-only-pid", pid = pid), ui);
                }
                Access::ReadOnly { .. } if self.on_disk => warning_line(t!("app.read-only"), ui),
                Access::ReadOnly { .. } => {}
                Access::NoLock => info_line(t!("app.no-lock"), ui),
            }
            // On the Records page the count stands above the table, with the way to the list; on
            // the Charts page it stands above the charts drawn without those records.
            let problems = u32::try_from(self.store.diagnostics.len()).unwrap_or(u32::MAX);
            if problems > 0 && self.page == Page::Today {
                warning_line(t!("app.diagnostics", n = problems), ui);
            }
            if self.offset.is_none() {
                info_line(t!("app.unknown-zone"), ui);
            }
            if self.on_disk && !self.pending.is_empty() {
                warning_line(t!("app.unsaved", n = u32::try_from(self.pending.len()).unwrap_or(u32::MAX)), ui);
            }
        })
        .padding(Padding::symmetric(0, 1))
        .fill_width();
    }

    /// The open page: on Today the counter while one runs, else the tree; on Charts the charts;
    /// on Records the records.
    fn body(&self, ui: &mut View<'_, Msg>) {
        match (self.page, &self.timer) {
            (Page::Today, timer) => {
                // The strip stands over the counter as well as over the tree: the counter is
                // the day's open block and grows on it with every tick.
                let running = timer.as_ref().map(TimerScreen::running);
                let blocks = stats::strip(&self.sessions, running.as_ref(), self.date, self.prefs.rollover);
                ui.column(|ui| {
                    today::strip_view(&self.today, &self.store.tree, &blocks, self.prefs.rollover, ui);
                    match timer {
                        Some(screen) => {
                            let goals = self.timer_goals(screen);
                            timer_screen::view(screen, &goals, &units().as_units(), self.quiet, ui);
                        }
                        None => {
                            let goals = goal_rows(&self.store.tree, &self.sessions, self.date, &self.prefs, None);
                            today::view(
                                &self.today,
                                &self.store.tree,
                                &self.totals,
                                &goals,
                                &units().as_units(),
                                (self.can_start(), self.prefs.default_goal),
                                ui,
                            );
                        }
                    }
                })
                .fill();
            }
            (Page::Charts, timer) => {
                let goals = goal_rows(&self.store.tree, &self.sessions, self.date, &self.prefs, None);
                charts::view(
                    &self.charts,
                    &self.sessions,
                    &self.store,
                    self.date,
                    &self.prefs,
                    &goals,
                    timer.is_some(),
                    &units().as_units(),
                    ui,
                );
            }
            (Page::Records, _) => {
                records::view(&self.records, &self.sessions, &self.store, &units().as_units(), self.can_edit(), ui);
            }
            (Page::Settings, _) => {
                settings::view(&self.settings_screen, &self.prefs, self.date, &units().as_units(), self.can_edit(), ui);
            }
        }
    }

    /// The keys the open screen answers to.
    fn hints(&self, ui: &mut View<'_, Msg>) {
        let icons = ui.env().icons();
        let enter = icons.glyph("enter").into_owned();
        let arrows = format!("{}{}", icons.glyph("arrow-left"), icons.glyph("arrow-right"));
        let updown = format!("{}{}", icons.glyph("arrow-up"), icons.glyph("arrow-down"));
        let hints = KeyHints::new();
        let hints = match (self.page, self.timer.is_some(), self.records.view_kind()) {
            (Page::Today, true, _) => {
                hints.hint("space", t!("hints.stop")).action(Scope::App, "pause").action(Scope::App, "note")
            }
            (Page::Today, false, _) => hints
                .hint(enter, t!("hints.start"))
                .action(Scope::App, "goal")
                .action(Scope::App, "countdown")
                .action(Scope::App, "archive")
                .action(Scope::App, "rename"),
            (Page::Charts, _, _) => {
                hints.hint(arrows, t!("hints.pick")).action(Scope::App, "today").action(Scope::App, "records")
            }
            (Page::Records, _, records::ViewKind::All) => hints
                .hint(enter, t!("hints.correct"))
                .action(Scope::App, "add")
                .action(Scope::App, "spans")
                .hint("delete", t!("hints.delete"))
                .action(Scope::App, "search")
                .action(Scope::App, "export")
                .action(Scope::App, "undo"),
            (Page::Records, _, records::ViewKind::Trash | records::ViewKind::Archive) => {
                hints.hint(enter, t!("hints.restore")).action(Scope::App, "search")
            }
            (Page::Records, _, records::ViewKind::Problems) => {
                let lost = self.sessions.iter().any(|session| self.store.tree.focus(session.focus).is_none());
                if lost { hints.hint(enter, t!("hints.attach")) } else { hints }.action(Scope::App, "today")
            }
            (Page::Settings, _, _) => {
                hints.hint(updown, t!("hints.move")).hint(enter, t!("hints.change")).action(Scope::App, "undo")
            }
        };
        let hints = if self.form.is_some() { KeyHints::new().action(Scope::App, "back") } else { hints };
        ui.add(hints.action_right(Scope::Global, "quit")).fill_width();
    }

    /// The question asked before leaving while a counter runs: three ways, none of them the
    /// default.
    fn quit_question(&self, ui: &mut View<'_, Msg>) {
        let name = self.timer.as_ref().map(|screen| screen.focus_name().to_owned()).unwrap_or_default();
        let dialog = Modal::new()
            .title(t!("app.quit-title", focus = name))
            .on_close(Msg::QuitCancel)
            .action(Button::new(t!("app.quit-cancel")).on_press(Msg::QuitCancel))
            .action(Button::new(t!("app.quit-leave")).on_press(Msg::QuitLeave))
            .action(Button::new(t!("app.quit-finish")).variant("primary").on_press(Msg::QuitFinish));
        ui.add_with(dialog, |ui| {
            ui.add(Text::new(t!("app.quit-message")).role("secondary")).fill_width();
        });
    }
}

/// The words for hours, minutes and seconds in the active language.
fn units() -> OwnedUnits {
    OwnedUnits { hour: t!("units.hour"), minute: t!("units.minute"), second: t!("units.second") }
}

/// The unit words, owned, so they can be borrowed as [`Units`] for as long as needed.
struct OwnedUnits {
    hour: String,
    minute: String,
    second: String,
}

impl OwnedUnits {
    fn as_units(&self) -> Units<'_> {
        Units { hour: &self.hour, minute: &self.minute, second: &self.second }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::time::Duration;

    use qframe::date::{TimeOfDay, Weekday};
    use qframe::env::{AssetDirs, Env};
    use qframe::event::{MouseButton, MouseKind};
    use qframe::icons::GlyphMode;

    use super::*;
    use crate::session::Source;
    use crate::span::{ClockSource, Span, SpanKind};
    use crate::tree::{Category, Focus, Goal, Period};
    use crate::ui::record_form;
    use crate::ui::records::clock_of;

    /// Noon UTC on 2026-09-18, a Friday.
    const NOON: i64 = 1_789_732_800;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qfocus-test-app-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn done(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    /// A clock a test moves by hand.
    #[derive(Clone)]
    struct FakeClock(Rc<Cell<Clocks>>);

    impl FakeClock {
        fn new() -> Self {
            let uptime = Uptime { awake: Duration::from_secs(5_000), elapsed: Duration::from_secs(5_000) };
            Self(Rc::new(Cell::new(Clocks { wall: NOON, uptime })))
        }

        fn now(&self) -> Clocks {
            self.0.get()
        }

        fn pass(&self, seconds: u64) {
            let mut clocks = self.0.get();
            clocks.wall += i64::try_from(seconds).unwrap_or(0);
            clocks.uptime.awake += Duration::from_secs(seconds);
            clocks.uptime.elapsed += Duration::from_secs(seconds);
            self.0.set(clocks);
        }

        fn reader(&self) -> Box<dyn Fn() -> Clocks> {
            let clock = self.clone();
            Box::new(move || clock.now())
        }
    }

    fn env() -> Env {
        let dirs = AssetDirs {
            locale_sources: crate::locales()
                .iter()
                .map(|(file, text)| ((*file).to_owned(), (*text).to_owned()))
                .collect(),
            keymap_source: Some(("keymap.toml".to_owned(), KEYMAP.to_owned())),
            ..AssetDirs::default()
        };
        Env::load(&dirs).expect("the built-in files load")
    }

    fn harness(app: QFocus, width: u16, height: u16) -> Harness<QFocus> {
        let mut harness = Harness::with_env(app, env(), width, height);
        harness.set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
        harness
    }

    fn app_at(dir: &Path, clock: &FakeClock) -> QFocus {
        QFocus::new(Store::open(Paths::at(dir, "test")), true, Some(180), clock.reader(), Settings::in_memory())
    }

    /// The application over `dir` with `prefs` in force, as a settings file would give them.
    fn app_with(dir: &Path, clock: &FakeClock, prefs: &Prefs) -> QFocus {
        let mut settings = Settings::in_memory();
        prefs.write(&mut settings);
        QFocus::new(Store::open(Paths::at(dir, "test")), true, Some(180), clock.reader(), settings)
    }

    fn seeded(dir: &Path) -> (Id, Id) {
        let category = Id::new(1, 1);
        let focus = Id::new(2, 2);
        let store = Store::open(Paths::at(dir, "test"));
        let mut tree = store.tree.clone();
        tree.categories.push(Category {
            id: category,
            name: "Work".to_owned(),
            icon: None,
            goal: None,
            archived: false,
            order: 0.0,
            focuses: vec![Focus { id: focus, name: "Rust".to_owned(), goal: None, archived: false, order: 0.0 }],
        });
        fs::create_dir_all(dir).expect("folder");
        fs::write(store.paths.tree_file(), tree.write()).expect("tree written");
        (category, focus)
    }

    fn forbidden(screen: &str) -> Option<&'static str> {
        ["[", "]", "|", "===", "-->"].into_iter().find(|mark| screen.contains(mark))
    }

    /// A session of `work` spans starting `before` seconds before noon, written to the month
    /// file as the last run would have left it.
    fn recorded(dir: &Path, id: u64, focus: Id, before: i64, spans: Vec<Span>, note: &str) -> Session {
        let started = NOON - before;
        let ended = started + i64::from(spans.last().map_or(0, |span| u32::try_from(span.end()).unwrap_or(0)));
        let session = Session {
            id: Id::new(id, 7),
            revision: 1,
            written: ended,
            focus,
            started,
            offset_minutes: 180,
            ended,
            spans,
            source: Source::Timer,
            replaces: None,
            voids: None,
            continues: None,
            flags: Vec::new(),
            note: note.to_owned(),
        };
        let paths = Paths::at(dir, "test");
        crate::store::sessions::append(&paths.month_file(2026, 9), &session).expect("session written");
        session
    }

    /// Two sessions under Rust: an hour at noon with a break inside it, and ten minutes at one.
    fn two_sessions(dir: &Path, focus: Id) -> (Session, Session) {
        let long = recorded(
            dir,
            10,
            focus,
            3 * 3_600,
            vec![
                Span::new(SpanKind::Work, 0, 1_800, ClockSource::Mono),
                Span::new(SpanKind::Pause, 1_800, 600, ClockSource::Mono),
                Span::new(SpanKind::Work, 2_400, 1_200, ClockSource::Mono),
            ],
            "meeting notes",
        );
        let short = recorded(dir, 11, focus, 2 * 3_600, vec![Span::new(SpanKind::Work, 0, 600, ClockSource::Mono)], "");
        (long, short)
    }

    /// A second category, Life, with Reading under it, beside Work and Rust.
    fn seeded_two(dir: &Path) -> (Id, Id, Id) {
        let (_, focus) = seeded(dir);
        let life = Id::new(3, 3);
        let reading = Id::new(4, 4);
        let store = Store::open(Paths::at(dir, "test"));
        let mut tree = store.tree.clone();
        tree.categories[0].focuses.push(Focus {
            id: Id::new(5, 5),
            name: "Review".to_owned(),
            goal: None,
            archived: true,
            order: 1.0,
        });
        tree.categories.push(Category {
            id: life,
            name: "Life".to_owned(),
            icon: None,
            goal: None,
            archived: false,
            order: 1.0,
            focuses: vec![Focus { id: reading, name: "Reading".to_owned(), goal: None, archived: false, order: 0.0 }],
        });
        fs::write(store.paths.tree_file(), tree.write()).expect("tree written");
        (focus, reading, Id::new(5, 5))
    }

    /// A day's worth of history around noon of Friday the 18th: today's two sessions, Wednesday
    /// two hours of Rust, Tuesday three quarters of an hour of the archived Review, Monday half
    /// an hour of Reading, last Thursday an hour of Rust, and an hour of Rust in August.
    fn history(dir: &Path) {
        let (rust, reading, review) = seeded_two(dir);
        two_sessions(dir, rust);
        let day = 86_400;
        let work = |seconds: u32| vec![Span::new(SpanKind::Work, 0, seconds, ClockSource::Mono)];
        recorded(dir, 20, rust, 2 * day + 2 * 3_600, work(7_200), "");
        recorded(dir, 21, review, 3 * day + 3_600, work(2_700), "");
        recorded(dir, 22, reading, 4 * day - 5 * 3_600, work(1_800), "");
        recorded(dir, 23, rust, 8 * day, work(3_600), "");
        recorded(dir, 24, rust, 29 * day, work(3_600), "");
    }

    fn charts_at(dir: &Path, clock: &FakeClock, width: u16, height: u16) -> Harness<QFocus> {
        let mut h = harness(app_at(dir, clock), width, height);
        h.press("2");
        assert_eq!(h.app().page(), Page::Charts);
        h
    }

    #[test]
    fn the_charts_tab_opens_with_2_and_the_day_reads_its_hours_and_categories() {
        let dir = temp("charts-day");
        history(&dir);
        let clock = FakeClock::new();
        let mut h = charts_at(&dir, &clock, 80, 24);
        let screen = h.screen();
        assert!(screen.contains("+1 h compared with yesterday"), "{screen}");
        assert!(screen.contains("By the hour"), "{screen}");
        assert!(screen.contains("00 01 02"), "the hours stand with their labels:\n{screen}");
        assert!(screen.contains("By category"), "{screen}");
        assert!(screen.contains("Work 1 h"), "the share reads as words:\n{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        // The keys go to the chart: End picks the last hour, Left walks back to the one worked.
        assert!(h.is_focused(charts::CHART), "{screen}");
        h.press("end");
        assert_eq!(h.app().charts().selected(), Some(23));
        for _ in 0..11 {
            h.press("left");
        }
        assert_eq!(h.app().charts().selected(), Some(12));
        assert!(h.screen().contains("12:00 to 13:00 · 50 min"), "{}", h.screen());
        // A narrower terminal lays the worked hours down with their labels and values.
        h.resize(60, 24);
        let screen = h.screen();
        assert!(screen.contains("12:00"), "{screen}");
        assert!(screen.contains("50 min"), "{screen}");
        assert!(screen.contains("13:00"), "{screen}");
        assert!(!screen.contains("00 01 02"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        // Delete belongs to the other pages; on the charts it archives nothing.
        h.press("delete");
        assert!(h.app().store().tree.categories.iter().all(|category| !category.archived), "{}", h.screen());
        h.press("1");
        assert_eq!(h.app().page(), Page::Today);
        done(&dir);
    }

    #[test]
    fn the_week_stacks_the_categories_with_a_legend_and_names_the_picked_day() {
        let dir = temp("charts-week");
        history(&dir);
        let clock = FakeClock::new();
        let mut h = charts_at(&dir, &clock, 80, 24);
        h.click_text("Week");
        assert_eq!(h.app().charts().scale(), charts::Scale::Week);
        assert!(h.is_focused(charts::CHART), "{}", h.screen());
        let screen = h.screen();
        assert!(screen.contains("+3 h 15 min compared with last week"), "{screen}");
        assert!(screen.contains("Mon") && screen.contains("Sun"), "{screen}");
        assert!(screen.contains("2 h"), "Wednesday's stack carries its total:\n{screen}");
        let (_, legend) = h.find("Life").expect("the legend names the second category");
        let (_, labels) = h.find("Mon").expect("the day labels");
        assert!(legend > labels, "the legend stands under the chart:\n{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.press("right").press("right");
        assert_eq!(h.app().charts().selected(), Some(1));
        let screen = h.screen();
        assert!(screen.contains("Tuesday · 45 min · Work 45 min"), "the archived focus counts:\n{screen}");
        h.press("home");
        assert!(h.screen().contains("Monday · 30 min · Life 30 min"), "{}", h.screen());
        // Lying down at forty columns, every day keeps its full name.
        h.resize(40, 24);
        let screen = h.screen();
        assert!(screen.contains("Wednesday"), "{screen}");
        assert!(screen.contains("Life"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.press("down");
        assert_eq!(h.app().charts().selected(), Some(1), "the lying chart walks with up and down");
        done(&dir);
    }

    #[test]
    fn the_month_reads_a_day_off_the_trend_and_ranks_the_focuses() {
        let dir = temp("charts-month");
        history(&dir);
        let clock = FakeClock::new();
        let mut h = charts_at(&dir, &clock, 80, 24);
        h.click_text("Month");
        let screen = h.screen();
        assert!(screen.contains("+4 h 15 min compared with last month"), "{screen}");
        assert!(screen.contains("Day by day"), "{screen}");
        let (_, hint) = h.find("Pick a bar").expect("the hint stands under the day axis");
        let axis = screen.lines().nth(usize::try_from(hint - 2).unwrap_or(0)).unwrap_or_default();
        assert_eq!(axis.trim_end(), "1  4  7  10 13 16", "the days thin under the trend:\n{screen}");
        assert!(screen.contains("Most worked on"), "{screen}");
        let (_, rust) = h.find("Rust").expect("the most worked focus");
        let (_, reading) = h.find("Reading").expect("the least worked focus");
        assert!(rust < reading, "most first:\n{screen}");
        assert!(screen.contains("45 min"), "the archived Review is ranked too:\n{screen}");
        assert!(screen.contains("6 sessions · average 52 min 30 s"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.press("end");
        assert_eq!(h.app().charts().selected(), Some(17));
        assert!(h.screen().contains("18 September · 1 h"), "{}", h.screen());
        h.press("left");
        assert!(h.screen().contains("17 September · 0 s"), "{}", h.screen());
        h.press("esc");
        assert_eq!(h.app().charts().selected(), None);
        assert!(h.screen().contains("Pick a bar"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn the_year_grid_ends_today_and_is_cut_to_the_newest_weeks_when_narrow() {
        let dir = temp("charts-year");
        history(&dir);
        let clock = FakeClock::new();
        let mut h = charts_at(&dir, &clock, 80, 24);
        h.click_text("Year");
        let screen = h.screen();
        assert!(screen.contains("+6 h 15 min compared with the year before"), "{screen}");
        assert!(screen.contains("The last 52 weeks"), "{screen}");
        let (_, hint) = h.find("Pick a day").expect("the hint stands under the grid");
        let axis = screen.lines().nth(usize::try_from(hint - 9).unwrap_or(0)).unwrap_or_default();
        assert_eq!(axis.trim_end(), "Sep Oct Nov Dec Jan Feb Mar Apr May Jun Jul Aug Sep", "over the weeks:\n{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.press("enter");
        assert_eq!(h.app().charts().selected(), Some(51 * 7 + 4), "Enter picks today first");
        assert!(h.screen().contains("Friday, 18 September · 1 h"), "{}", h.screen());
        h.press("left").press("enter");
        assert!(h.screen().contains("Friday, 11 September · 0 s"), "a week back:\n{}", h.screen());
        h.resize(40, 24);
        let screen = h.screen();
        assert!(screen.contains("The last 40 of 52 weeks"), "{screen}");
        assert!(
            screen.contains("Dec") && !screen.contains("Nov"),
            "the months begin with the first week shown:\n{screen}"
        );
        assert_eq!(forbidden(&screen), None, "{screen}");
        done(&dir);
    }

    #[test]
    fn empty_charts_say_so_on_every_scale_and_the_running_counter_is_named() {
        let dir = temp("charts-empty");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = charts_at(&dir, &clock, 80, 24);
        for (scale, text) in [
            ("Day", "No records today yet."),
            ("Week", "No records this week yet."),
            ("Month", "No records this month yet."),
            ("Year", "No records in the last year."),
        ] {
            h.click_text(scale);
            let screen = h.screen();
            assert!(screen.contains(text), "{scale}:\n{screen}");
            assert!(!screen.contains("compared"), "no comparison over nothing:\n{screen}");
            assert_eq!(forbidden(&screen), None, "{screen}");
        }
        h.press("1").click_text("Rust");
        clock.pass(120);
        h.advance(Duration::from_secs(1));
        h.press("2").click_text("Day");
        let screen = h.screen();
        assert!(screen.contains("The running counter is not in the charts yet"), "{screen}");
        assert!(screen.contains("No records today yet."), "the counter is not counted:\n{screen}");
        done(&dir);
    }

    #[test]
    fn unreadable_records_are_counted_above_the_charts() {
        let dir = temp("charts-problems");
        history(&dir);
        let month = Paths::at(&dir, "test").month_file(2026, 9);
        let mut text = fs::read_to_string(&month).expect("month");
        text.push_str("qf1;not a record\n");
        fs::write(&month, text).expect("month");
        let clock = FakeClock::new();
        let h = charts_at(&dir, &clock, 80, 24);
        let screen = h.screen();
        assert!(screen.contains("1 record could not be read; the charts are drawn without it."), "{screen}");
        assert_eq!(screen.matches("could not be read").count(), 1, "said once:\n{screen}");
        assert!(screen.contains("By the hour"), "the charts are still drawn:\n{screen}");
        done(&dir);
    }

    #[test]
    fn ascii_and_turkish_charts_keep_clean() {
        let dir = temp("charts-ascii");
        history(&dir);
        let clock = FakeClock::new();
        let mut h = charts_at(&dir, &clock, 80, 24);
        h.set_glyph_mode(GlyphMode::Ascii).set_locale("tr");
        let screen = h.screen();
        assert!(screen.contains("Grafikler"), "{screen}");
        assert!(screen.contains("Düne göre +1 sa"), "{screen}");
        assert!(screen.contains("Saat saat"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.click_text("Hafta");
        let screen = h.screen();
        assert!(screen.contains("Geçen haftaya göre +3 sa 15 dk"), "{screen}");
        assert!(screen.contains("Pzt") && screen.contains("Paz"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        assert!(h.is_focused(charts::CHART), "the keys follow the scale:\n{screen}");
        h.press("right");
        assert!(h.screen().contains("Pazartesi · 30 dk · Life 30 dk"), "{}", h.screen());
        // "Ay" is also the start of the Ayarlar tab, so the scale is chosen by message.
        h.send(Msg::Charts(charts::Msg::Scale(charts::Scale::Month.index())));
        let screen = h.screen();
        assert!(screen.contains("6 oturum · ortalama 52 dk 30 sn"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.click_text("Yıl");
        let screen = h.screen();
        assert!(screen.contains("Son 52 hafta"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.resize(40, 24);
        for scale in ["Gün", "Hafta", "Ay", "Yıl"] {
            h.click_text(scale);
            assert_eq!(forbidden(&h.screen()), None, "{}", h.screen());
        }
        done(&dir);
    }

    /// The strip's row on the Today page: under the header and the tabs.
    const STRIP_ROW: i32 = 2;

    /// The column of the strip cell that `hour` o'clock falls in, on a strip `width` columns
    /// wide with one column of padding on each side.
    fn strip_x(hour: u32, width: u16) -> i32 {
        1 + i32::try_from(u64::from(hour) * 3_600 * u64::from(width - 2) / 86_400).unwrap_or(0)
    }

    /// The background of the legend swatch before the name `name`, wherever it stands first.
    fn swatch(h: &Harness<QFocus>, name: &str) -> Option<qframe::color::Rgb> {
        let (x, y) = h.find(name).expect("named in a legend");
        h.bg(u16::try_from(x - 3).unwrap_or(0), u16::try_from(y).unwrap_or(0))
    }

    #[test]
    fn the_day_strip_reads_its_blocks_with_the_pointer_and_the_keys_and_names_the_tones() {
        let dir = temp("strip");
        history(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 24);
        let screen = h.screen();
        assert!(screen.contains("12:00") && screen.contains("02:00"), "the hours run under the strip:\n{screen}");
        assert_eq!(screen.matches("Work").count(), 2, "the legend and the tree name the category:\n{screen}");
        assert_eq!(screen.matches("Life").count(), 1, "a category with nothing today is only in the tree:\n{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        // The block of noon carries the tone the legend names.
        let noon = (u16::try_from(strip_x(12, 80)).unwrap_or(0), u16::try_from(STRIP_ROW).unwrap_or(0));
        let tone = h.bg(noon.0, noon.1);
        assert!(tone.is_some());
        assert_eq!(tone, swatch(&h, "Work"), "{screen}");
        assert_ne!(tone, h.bg(u16::try_from(strip_x(3, 80)).unwrap_or(0), noon.1), "three o'clock is a hole");
        h.hover(strip_x(12, 80), STRIP_ROW);
        assert!(h.screen().contains("Rust  12:00–12:30  30 min"), "the pointer reads the block:\n{}", h.screen());
        h.click(strip_x(12, 80), STRIP_ROW);
        assert_eq!(h.app().today().block(), Some(0));
        assert!(h.is_focused(today::STRIP), "{}", h.screen());
        h.hover(0, 0);
        h.press("right");
        assert_eq!(h.app().today().block(), Some(1));
        assert!(h.screen().contains("Rust  12:40–13:00  20 min"), "the break is a hole:\n{}", h.screen());
        h.press("end");
        assert!(h.screen().contains("Rust  13:00–13:10  10 min"), "{}", h.screen());
        // The zoom keys ask for a stretch and 0 for the whole day; the strip never slides.
        h.press("+");
        assert!(h.app().today().zoom().is_some(), "{}", h.screen());
        assert_eq!(forbidden(&h.screen()), None, "{}", h.screen());
        h.press("0");
        assert_eq!(h.app().today().zoom(), None);
        // The same category has the same tone on the Week chart.
        h.press("2").click_text("Week");
        assert_eq!(swatch(&h, "Work"), tone, "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn the_running_counter_is_a_live_block_on_the_strip() {
        let dir = temp("strip-running");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 24);
        assert!(!h.screen().contains("00:00"), "an empty day draws no strip:\n{}", h.screen());
        h.click_text("Rust");
        clock.pass(120);
        h.advance(Duration::from_secs(1));
        let screen = h.screen();
        assert!(screen.contains("00:00"), "the strip stands over the counter:\n{screen}");
        assert!(screen.contains("Work"), "{screen}");
        // Noon UTC is three in the afternoon where the counter runs.
        h.click(strip_x(15, 80), STRIP_ROW);
        assert!(h.screen().contains("Rust  15:00–15:02  running  2 min"), "{}", h.screen());
        clock.pass(60);
        h.advance(Duration::from_secs(1));
        assert!(h.screen().contains("Rust  15:00–15:03  running  3 min"), "it grows:\n{}", h.screen());
        h.set_locale("tr");
        assert!(h.screen().contains("Rust  15:00–15:03  sürüyor  3 dk"), "{}", h.screen());
        h.set_locale("en");
        h.press("p");
        clock.pass(60);
        h.advance(Duration::from_secs(1));
        assert!(h.screen().contains("Rust  15:00–15:03  3 min"), "on a break the block is closed:\n{}", h.screen());
        h.set_glyph_mode(GlyphMode::Ascii).set_locale("tr");
        let screen = h.screen();
        assert!(screen.contains("Rust  15:00"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        done(&dir);
    }

    #[test]
    fn a_narrow_strip_thins_its_hours_then_drops_the_legend() {
        let dir = temp("strip-narrow");
        history(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 40, 24);
        let screen = h.screen();
        assert!(screen.contains("04:00") && !screen.contains("02:00"), "{screen}");
        assert_eq!(screen.matches("Work").count(), 2, "the legend stays at forty:\n{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.resize(30, 24);
        let screen = h.screen();
        assert_eq!(screen.matches("Work").count(), 1, "below forty the legend goes:\n{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.set_glyph_mode(GlyphMode::Ascii).set_locale("tr");
        h.click(strip_x(12, 30), STRIP_ROW);
        let screen = h.screen();
        // Two blocks share the cell of noon on a strip this narrow; the later one is read.
        assert!(screen.contains("Rust  12:40–13:00  20 dk"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        done(&dir);
    }

    fn records_at(dir: &Path, clock: &FakeClock, width: u16, height: u16) -> Harness<QFocus> {
        let mut h = harness(app_at(dir, clock), width, height);
        h.press("3");
        assert_eq!(h.app().page(), Page::Records);
        h
    }

    #[test]
    fn an_empty_store_shows_the_invitation() {
        let dir = temp("empty");
        let clock = FakeClock::new();
        let h = harness(app_at(&dir, &clock), 80, 20);
        let screen = h.screen();
        assert!(screen.contains("Nothing to focus on yet"), "{screen}");
        assert!(screen.contains("Friday, 18 September"), "{screen}");
        assert!(screen.contains("0 s"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        done(&dir);
    }

    #[test]
    fn adding_a_category_then_a_focus_selects_the_new_row() {
        let dir = temp("add");
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 20);
        h.click_text("Add a category");
        assert!(h.is_focused(today::NAME_INPUT), "{}", h.screen());
        h.press("enter");
        assert!(h.screen().contains("A name cannot be empty"), "{}", h.screen());
        h.type_text("Work").press("enter");
        let category = h.app().store().tree.categories[0].id;
        assert_eq!(h.app().today().selected(), Some(Row::Category(category)));
        assert!(h.screen().contains("Add a focus"), "{}", h.screen());
        h.click_text("Add a focus");
        h.type_text("Rust").press("enter");
        let focus = h.app().store().tree.categories[0].focuses[0].id;
        assert_eq!(h.app().today().selected(), Some(Row::Focus(focus)));
        assert!(fs::read_to_string(h.app().store().paths.tree_file()).is_ok_and(|text| text.contains("Rust")));
        // A second focus with the same name is allowed and mentioned.
        h.press("down").press("enter").type_text("Rust").press("enter");
        assert!(h.screen().contains("already one called Rust"), "{}", h.screen());
        assert_eq!(h.app().store().tree.categories[0].focuses.len(), 2);
        done(&dir);
    }

    #[test]
    fn starting_ticking_and_stopping_records_the_session() {
        let dir = temp("run");
        let (_, focus) = seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 20);
        h.click_text("Rust");
        assert!(h.app().timer().is_some(), "{}", h.screen());
        assert!(h.screen().contains("Work"), "{}", h.screen());
        assert!(h.app().store().running().is_some_and(|found| found.is_ok()));
        clock.pass(90);
        h.advance(Duration::from_secs(1));
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(90));
        assert!(h.screen().lines().next().is_some_and(|top| top.contains("1 min 30 s")), "{}", h.screen());
        h.press("p");
        assert!(h.screen().contains("On a break"), "{}", h.screen());
        clock.pass(30);
        h.advance(Duration::from_secs(1));
        h.press("p");
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(90));
        clock.pass(30);
        h.advance(Duration::from_secs(1));
        h.press("n").type_text("a note").press("enter");
        h.press("space");
        assert!(h.app().timer().is_none(), "{}", h.screen());
        let screen = h.screen();
        // The header, the category and the focus row all carry the two minutes once the record
        // is written.
        let rows: Vec<&str> = screen.lines().filter(|row| row.trim_end().ends_with("2 min")).collect();
        assert_eq!(rows.len(), 3, "{screen}");
        for (row, name) in rows.iter().zip(["qfocus", "Work", "Rust"]) {
            assert!(row.contains(name), "{screen}");
        }
        assert_eq!(h.app().today().selected(), Some(Row::Focus(focus)));
        let month = h.app().store().paths.month_file(2026, 9);
        let lines = fs::read_to_string(&month).expect("month file");
        assert_eq!(lines.lines().count(), 1, "{lines}");
        assert!(lines.contains("a note"), "{lines}");
        assert!(h.app().store().running().is_none());
        assert_eq!(forbidden(&screen), None);
        done(&dir);
    }

    /// Starts Rust and lets thirteen hours pass, which is over the ceiling.
    fn over_the_ceiling(dir: &Path, clock: &FakeClock) -> Harness<QFocus> {
        seeded(dir);
        let mut h = harness(app_at(dir, clock), 80, 24);
        h.click_text("Rust");
        clock.pass(13 * 3_600);
        h.advance(Duration::from_secs(1));
        assert!(h.app().timer().is_some_and(|screen| screen.running().flags.contains(&Flag::OverCeiling)));
        h.press("space").advance(Duration::from_millis(200));
        let screen = h.screen();
        assert!(h.app().timer().is_some(), "the counter goes on while the question stands:\n{screen}");
        assert!(screen.contains("This session lasted 13 h"), "{screen}");
        assert!(screen.contains("Fix the duration") && screen.contains("Leave as is"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h
    }

    #[test]
    fn stopping_over_the_ceiling_asks_and_the_form_fixes_the_duration_before_the_record() {
        let dir = temp("ceiling-fix");
        let clock = FakeClock::new();
        let mut h = over_the_ceiling(&dir, &clock);
        // Cancel has focus; the third way and the fix follow it in the tab order.
        h.press("tab").press("tab").press("enter");
        let screen = h.screen();
        assert!(h.app().form().is_some(), "{screen}");
        assert!(screen.contains("Fix the duration"), "{screen}");
        assert!(h.is_focused(record_form::DURATION_INPUT), "{screen}");
        assert!(screen.contains("13 h 00 min"), "{screen}");
        assert!(h.app().store().sessions.is_empty(), "nothing is written before the form");
        h.type_text("08");
        assert!(h.screen().contains("8 h 00 min"), "{}", h.screen());
        h.click_text("Save");
        assert!(h.app().form().is_none(), "{}", h.screen());
        assert!(h.app().timer().is_none(), "{}", h.screen());
        assert!(h.screen().contains("Rust: 8 h recorded"), "{}", h.screen());
        let session = h.app().store().sessions.last().cloned().expect("recorded");
        assert_eq!(session.work_seconds(), 8 * 3_600);
        assert!(!session.flags.contains(&Flag::OverCeiling), "{session:?}");
        assert_eq!(session.source, Source::Timer);
        assert!(session.spans.iter().all(|span| span.clock == ClockSource::Wall));
        assert!(h.app().store().running().is_none());
        done(&dir);
    }

    #[test]
    fn stopping_over_the_ceiling_can_leave_the_session_as_it_is_or_go_on_counting() {
        let dir = temp("ceiling-leave");
        let clock = FakeClock::new();
        let mut h = over_the_ceiling(&dir, &clock);
        h.press("esc");
        assert!(h.app().timer().is_some(), "{}", h.screen());
        assert!(h.screen().contains("Cancelled"), "{}", h.screen());
        assert!(!h.screen().contains("This session lasted"), "{}", h.screen());
        h.press("space").advance(Duration::from_millis(200));
        h.press("tab").press("tab").press("enter");
        assert!(h.app().form().is_some(), "{}", h.screen());
        h.press("esc");
        assert!(h.app().form().is_none());
        assert!(h.app().timer().is_some(), "cancelling the form keeps the counter:\n{}", h.screen());
        h.press("space").advance(Duration::from_millis(200));
        h.click_text("Leave as is");
        assert!(h.app().timer().is_none(), "{}", h.screen());
        let session = h.app().store().sessions.last().cloned().expect("recorded");
        assert_eq!(session.work_seconds(), 13 * 3_600);
        assert!(session.flags.contains(&Flag::OverCeiling), "{session:?}");
        h.set_locale("tr").set_glyph_mode(GlyphMode::Ascii);
        assert_eq!(forbidden(&h.screen()), None, "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn a_narrow_screen_still_shows_the_tree_and_the_counter() {
        let dir = temp("narrow");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 40, 18);
        let screen = h.screen();
        assert!(screen.contains("Rust"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.click_text("Rust");
        h.press("p");
        let screen = h.screen();
        assert!(screen.contains("On a break"), "{screen}");
        assert!(screen.contains("Note"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        done(&dir);
    }

    #[test]
    fn a_second_instance_only_looks_until_the_first_closes() {
        let dir = temp("readonly");
        seeded(&dir);
        let clock = FakeClock::new();
        let first = Store::open(Paths::at(&dir, "test"));
        assert_eq!(first.access, Access::Writer);
        let mut h = harness(app_at(&dir, &clock), 80, 20);
        let screen = h.screen();
        assert!(screen.contains("Another qfocus"), "{screen}");
        h.click_text("Rust");
        assert!(h.app().timer().is_none());
        assert!(h.screen().contains("only looks"), "{}", h.screen());
        drop(first);
        h.advance(Duration::from_secs(1));
        assert!(!h.screen().contains("Another qfocus"), "{}", h.screen());
        h.click_text("Rust");
        assert!(h.app().timer().is_some());
        done(&dir);
    }

    #[test]
    fn ascii_glyphs_and_turkish_keep_the_screens_clean() {
        let dir = temp("ascii");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 20);
        h.set_glyph_mode(GlyphMode::Ascii).set_locale("tr");
        let screen = h.screen();
        assert!(screen.contains("Kategori ekle"), "{screen}");
        assert!(screen.contains("18 Eylül Cuma"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.click_text("Rust");
        let screen = h.screen();
        assert!(screen.contains("Durdur"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.press("ctrl+q");
        let screen = h.screen();
        assert!(screen.contains("Rust çalışıyor"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        done(&dir);
    }

    #[test]
    fn a_running_file_opens_the_recovery_dialog_and_save_records_it() {
        let dir = temp("recover");
        let (_, focus) = seeded(&dir);
        let clock = FakeClock::new();
        let store = Store::open(Paths::at(&dir, "test"));
        let running = Running {
            focus,
            started: NOON - 600,
            offset_minutes: 180,
            spans: vec![Span::new(SpanKind::Work, 0, 300, ClockSource::Mono)],
            paused: false,
            refreshed: NOON - 300,
            flags: Vec::new(),
            target: None,
            idle_from: None,
            watch: Watch::Window,
        };
        store.save_running(&running).expect("running written");
        drop(store);
        let mut h = harness(app_at(&dir, &clock), 80, 20);
        // The dialog stands on the very first frame, before any key is pressed.
        let screen = h.screen();
        assert!(screen.contains("A counter was left running"), "{screen}");
        assert!(screen.contains("Rust was running with 5 min counted"), "{screen}");
        assert!(flat(&screen).contains("The program closed 5 minutes ago; the time since was not counted"), "{screen}");
        h.click_text("Save");
        assert!(h.app().store().running().is_none());
        let lines = fs::read_to_string(h.app().store().paths.month_file(2026, 9)).expect("month file");
        assert_eq!(lines.lines().count(), 1, "{lines}");
        assert!(lines.contains("recovered"), "{lines}");
        assert!(h.screen().contains("5 min"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn continuing_a_recovered_counter_keeps_its_flags() {
        let dir = temp("continue");
        let (_, focus) = seeded(&dir);
        let clock = FakeClock::new();
        let store = Store::open(Paths::at(&dir, "test"));
        let running = Running {
            focus,
            started: NOON - 600,
            offset_minutes: 180,
            spans: vec![Span::new(SpanKind::Work, 0, 300, ClockSource::Mono)],
            paused: false,
            refreshed: NOON - 300,
            flags: vec![Flag::SuspectClock],
            target: None,
            idle_from: None,
            watch: Watch::Window,
        };
        store.save_running(&running).expect("running written");
        drop(store);
        let mut h = harness(app_at(&dir, &clock), 80, 20);
        h.click_text("Continue");
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(300));
        assert_eq!(
            h.app().store().running().and_then(Result::ok).map(|found| found.flags),
            Some(vec![Flag::SuspectClock])
        );
        h.press("space");
        let lines = fs::read_to_string(h.app().store().paths.month_file(2026, 9)).expect("month file");
        assert!(lines.contains("suspect-clock"), "{lines}");
        assert!(lines.contains("recovered"), "{lines}");
        done(&dir);
    }

    /// The words on `screen` with the bars and the line breaks gone, so a sentence a dialog wraps
    /// can be looked for whole.
    fn flat(screen: &str) -> String {
        screen.replace('▌', " ").split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// `key` with `args` in the language the environment starts in, which is the one text made
    /// before the first frame is in.
    fn said_at_start(key: &str, args: &[(&str, &str)]) -> String {
        let env = env();
        let i18n = env.i18n();
        let args: Vec<(&str, qframe::i18n::Arg)> = args.iter().map(|(name, value)| (*name, (*value).into())).collect();
        i18n.translate(key, &args)
    }

    /// Leaves a running file of Rust with five minutes of work, started at `started` and last
    /// refreshed five minutes later, measured by `watch`.
    fn left_running(dir: &Path, focus: Id, started: i64, watch: Watch) {
        let store = Store::open(Paths::at(dir, "test"));
        let running = Running {
            focus,
            started,
            offset_minutes: 180,
            spans: vec![Span::new(SpanKind::Work, 0, 300, ClockSource::Mono)],
            paused: false,
            refreshed: started + 300,
            flags: Vec::new(),
            target: None,
            idle_from: None,
            watch,
        };
        store.save_running(&running).expect("running written");
    }

    #[test]
    fn a_counter_nobody_measured_carries_on_in_the_window_without_a_question() {
        let dir = temp("adopt");
        let (_, focus) = seeded(&dir);
        let clock = FakeClock::new();
        // Started from the command line an hour ago; the machine has been up since before.
        left_running(&dir, focus, NOON - 3_600, Watch::None);
        let mut h = harness(app_at(&dir, &clock).knows_boot(true), 160, 20);
        let screen = h.screen();
        assert!(!screen.contains("A counter was left running"), "{screen}");
        let hour = said_at_start("units.hour", &[]);
        let said = said_at_start("recover.adopted", &[("focus", "Rust"), ("duration", &format!("1 {hour}"))]);
        assert!(flat(&screen).contains(&said), "{said} in {screen}");
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(3_600));
        let written = h.app().store().running().and_then(Result::ok).expect("the running file is there");
        assert_eq!(written.watch, Watch::Window, "this window measures it now");
        assert_eq!(written.started, NOON - 3_600);
        clock.pass(60);
        h.advance(Duration::from_secs(1));
        h.press("space");
        let session = h.app().store().sessions.first().cloned().expect("recorded");
        assert_eq!(session.work_seconds(), 3_660);
        assert_eq!(session.source, Source::Timer, "nothing was lost, nothing recovered");
        assert!(crate::span::check(&session.spans, 3_660).is_empty(), "{:?}", session.spans);
        done(&dir);
    }

    #[test]
    fn a_counter_cut_by_a_restart_opens_the_dialog_with_the_reason_and_saves_up_to_the_cut() {
        let dir = temp("adopt-restart");
        let (_, focus) = seeded(&dir);
        let clock = FakeClock::new();
        // Started at 11:00 local, last seen at 11:05; the machine booted again at 13:36.
        left_running(&dir, focus, NOON - 4 * 3_600, Watch::None);
        let mut h = harness(app_at(&dir, &clock).knows_boot(true), 100, 20);
        let screen = flat(&h.screen());
        assert!(screen.contains("A counter was left running"), "{screen}");
        assert!(screen.contains("Rust was running with 5 min counted"), "{screen}");
        assert!(screen.contains("The computer restarted; nothing after 11:05 was counted"), "{screen}");
        h.click_text("Save");
        let session = h.app().store().sessions.first().cloned().expect("recorded");
        assert_eq!(session.ended, NOON - 4 * 3_600 + 300);
        assert_eq!(session.work_seconds(), 300);
        done(&dir);
    }

    #[test]
    fn without_knowing_the_boot_a_counter_nobody_measured_is_always_carried_on() {
        let dir = temp("adopt-no-boot");
        let (_, focus) = seeded(&dir);
        let clock = FakeClock::new();
        left_running(&dir, focus, NOON - 4 * 3_600, Watch::None);
        let h = harness(app_at(&dir, &clock).knows_boot(false), 80, 20);
        assert!(h.app().recover.is_none());
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(4 * 3_600));
        done(&dir);
    }

    #[test]
    fn continuing_after_the_window_closed_leaves_the_time_it_was_closed_out() {
        let dir = temp("continue-gap");
        let (_, focus) = seeded(&dir);
        let clock = FakeClock::new();
        left_running(&dir, focus, NOON - 3_600, Watch::Window);
        let mut h = harness(app_at(&dir, &clock).knows_boot(true), 80, 20);
        assert!(flat(&h.screen()).contains("The program closed 55 minutes ago"), "{}", h.screen());
        h.click_text("Continue");
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(300));
        h.press("space");
        let session = h.app().store().sessions.first().cloned().expect("recorded");
        let gap = Span::new(SpanKind::Gap, 300, 3_300, ClockSource::Wall);
        assert!(session.spans.contains(&gap), "{:?}", session.spans);
        done(&dir);
    }

    /// The name a broken running file is set aside under when the app opens at noon, 15:00 local.
    const ASIDE: &str = "running-test-broken-2026-09-18-150000.toml";

    #[test]
    fn a_recovered_countdown_keeps_counting_down() {
        let dir = temp("recover-countdown");
        let (_, focus) = seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 24);
        h.send(Msg::Today(today::Msg::Select(Row::Focus(focus).key())));
        h.press("t");
        h.send(Msg::Today(today::Msg::TimedAmount(Duration::from_secs(600))));
        h.click_text("Start");
        clock.pass(120);
        h.advance(Duration::from_secs(1));
        let written = h.app().store().running().and_then(Result::ok).expect("the running file is there");
        assert_eq!(written.target, Some(600));
        // The program dies here; the next start finds the file and carries on from it.
        drop(h);
        let mut h = harness(app_at(&dir, &clock), 80, 24);
        assert!(h.screen().contains("A counter was left running"), "{}", h.screen());
        h.click_text("Continue");
        assert_eq!(h.app().timer().and_then(TimerScreen::countdown), Some(600));
        assert_eq!(h.app().timer().map(TimerScreen::is_reached), Some(false));
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(120));
        h.hover(0, 0).advance(Duration::from_secs(10));
        assert!(h.screen().contains("Counting down from 10 min"), "{}", h.screen());
        assert_eq!(h.app().store().running().and_then(Result::ok).and_then(|found| found.target), Some(600));
        done(&dir);
    }

    #[test]
    fn a_running_file_without_a_target_recovers_counting_up() {
        let dir = temp("recover-up");
        let (_, focus) = seeded(&dir);
        let clock = FakeClock::new();
        let path = Paths::at(&dir, "test").running_file();
        let text = format!(
            "focus = \"{focus}\"\nstarted = {}\noffset = 180\npaused = false\nrefreshed = {}\nflags = []\n\n[[span]]\nkind = \"work\"\noffset = 0\nseconds = 300\nclock = \"mono\"\n",
            NOON - 300,
            NOON
        );
        fs::write(&path, text).expect("an old running file");
        let mut h = harness(app_at(&dir, &clock), 80, 24);
        h.click_text("Continue");
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(300));
        assert_eq!(h.app().timer().and_then(TimerScreen::countdown), None);
        h.hover(0, 0).advance(Duration::from_secs(10));
        assert!(!h.screen().contains("Counting down"), "{}", h.screen());
        done(&dir);
    }

    /// Leaves a running file with five minutes of work, then fifteen silent ones the person
    /// never answered for.
    fn left_with_unanswered_silence(dir: &Path, focus: Id) {
        let store = Store::open(Paths::at(dir, "test"));
        let running = Running {
            focus,
            started: NOON - 1_200,
            offset_minutes: 180,
            spans: vec![
                Span::new(SpanKind::Work, 0, 300, ClockSource::Mono),
                Span::new(SpanKind::Idle, 300, 900, ClockSource::Mono),
            ],
            paused: false,
            refreshed: NOON,
            flags: vec![Flag::UnclaimedIdle],
            target: None,
            idle_from: Some(300),
            watch: Watch::Window,
        };
        store.save_running(&running).expect("running written");
    }

    #[test]
    fn saving_a_recovered_unanswered_silence_keeps_it_flagged() {
        let dir = temp("recover-idle-save");
        let (_, focus) = seeded(&dir);
        let clock = FakeClock::new();
        left_with_unanswered_silence(&dir, focus);
        let mut h = harness(app_at(&dir, &clock), 80, 24);
        h.click_text("Save");
        let lines = fs::read_to_string(h.app().store().paths.month_file(2026, 9)).expect("month file");
        assert!(lines.contains("unclaimed-idle"), "{lines}");
        assert!(h.app().timer().is_none());
        done(&dir);
    }

    #[test]
    fn a_recovered_unanswered_silence_is_asked_about_again() {
        let dir = temp("recover-idle");
        let (_, focus) = seeded(&dir);
        let clock = FakeClock::new();
        left_with_unanswered_silence(&dir, focus);
        let mut h = harness(app_at(&dir, &clock), 80, 24);
        h.click_text("Continue");
        assert_eq!(h.app().timer().map(TimerScreen::is_asking_idle), Some(true));
        let screen = h.screen();
        assert!(screen.contains("You were away for 15 min"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(300));
        h.click_text("Count");
        assert_eq!(h.app().timer().map(TimerScreen::is_asking_idle), Some(false));
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(1_200));
        let written = h.app().store().running().and_then(Result::ok).expect("the running file is there");
        assert_eq!(written.idle_from, None);
        assert!(written.flags.is_empty(), "{:?}", written.flags);
        h.press("space");
        let lines = fs::read_to_string(h.app().store().paths.month_file(2026, 9)).expect("month file");
        assert!(!lines.contains("unclaimed-idle"), "{lines}");
        assert!(lines.contains("recovered"), "{lines}");
        done(&dir);
    }

    #[test]
    fn a_broken_running_file_is_moved_aside_before_a_new_counter_can_overwrite_it() {
        let dir = temp("broken");
        seeded(&dir);
        let clock = FakeClock::new();
        let paths = Paths::at(&dir, "test");
        let bytes = b"focus = \"not an id\"\nstarted = \xff".to_vec();
        fs::write(paths.running_file(), &bytes).expect("written");
        let mut h = harness(app_at(&dir, &clock), 120, 24);
        let screen = h.screen();
        assert!(screen.contains("could not be read"), "{screen}");
        assert!(screen.contains("moved aside so a new counter cannot"), "{screen}");
        assert!(screen.contains(ASIDE), "{screen}");
        let Some(Recover::Broken { problems, .. }) = h.app().recover.as_ref() else { panic!("no report") };
        assert!(!problems.is_empty());
        assert!(
            problems.iter().all(|problem| problem.location.as_ref().is_some_and(|at| at.file.ends_with(ASIDE))),
            "the lines point into the copy: {problems:?}"
        );
        let aside = dir.join(ASIDE);
        assert_eq!(fs::read(&aside).expect("the copy"), bytes);
        assert!(!paths.running_file().exists(), "the running path is free");
        h.press("esc");
        assert!(!h.screen().contains("could not be read"), "{}", h.screen());
        h.click_text("Rust");
        assert!(h.app().timer().is_some());
        assert!(h.app().store().running().is_some_and(|found| found.is_ok()), "the new counter has its own file");
        assert_eq!(fs::read(&aside).expect("the copy"), bytes, "the copy is untouched");
        done(&dir);
    }

    #[test]
    fn a_broken_running_file_is_reported_once() {
        let dir = temp("broken-once");
        seeded(&dir);
        let clock = FakeClock::new();
        fs::write(Paths::at(&dir, "test").running_file(), "focus = 1\n").expect("written");
        let mut h = harness(app_at(&dir, &clock), 120, 24);
        assert!(h.screen().contains("could not be read"), "{}", h.screen());
        h.press("esc");
        drop(h);
        clock.pass(60);
        let h = harness(app_at(&dir, &clock), 120, 24);
        assert!(!h.screen().contains("could not be read"), "{}", h.screen());
        let copies = fs::read_dir(&dir)
            .expect("folder")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains("-broken-"))
            .count();
        assert_eq!(copies, 1);
        done(&dir);
    }

    #[test]
    fn a_read_only_instance_leaves_a_broken_running_file_where_it_is() {
        let dir = temp("broken-readonly");
        seeded(&dir);
        let clock = FakeClock::new();
        let first = Store::open(Paths::at(&dir, "test"));
        assert_eq!(first.access, Access::Writer);
        let path = first.paths.running_file();
        fs::write(&path, "focus = 1\n").expect("written");
        let h = harness(app_at(&dir, &clock), 120, 24);
        let screen = h.screen();
        assert!(screen.contains("could not be read"), "{screen}");
        assert!(screen.contains("The file is left as it is"), "{screen}");
        assert_eq!(fs::read_to_string(&path).expect("still there"), "focus = 1\n");
        assert!(!dir.join(ASIDE).exists());
        drop(first);
        done(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_broken_running_file_that_cannot_be_moved_says_why_and_stays() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp("broken-stuck");
        seeded(&dir);
        let clock = FakeClock::new();
        let path = Paths::at(&dir, "test").running_file();
        fs::write(&path, "focus = 1\n").expect("written");
        // The lock is taken first; only then does the folder stop taking new names.
        let app = app_at(&dir, &clock);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).expect("read-only folder");
        let probe = dir.join("probe");
        if fs::write(&probe, "").is_ok() {
            // A user the permissions do not bind, such as root: nothing to test.
            let _ = fs::remove_file(&probe);
        } else {
            let h = harness(app, 120, 24);
            let screen = h.screen();
            assert!(screen.contains("Moving the file aside failed"), "{screen}");
            assert!(screen.contains("denied"), "{screen}");
            assert_eq!(fs::read_to_string(&path).expect("still there"), "focus = 1\n");
        }
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("writable again");
        done(&dir);
    }

    #[test]
    fn quitting_while_running_asks_and_leaving_keeps_the_file() {
        let dir = temp("quit");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 20);
        h.click_text("Rust");
        h.press("ctrl+q");
        assert!(!h.quit_requested());
        assert!(h.screen().contains("Rust is running"), "{}", h.screen());
        h.press("esc");
        assert!(h.screen().contains("Cancelled"), "{}", h.screen());
        h.press("ctrl+q");
        // Asking again while the question is open keeps the one question.
        h.press("ctrl+q");
        assert_eq!(h.screen().matches("Rust is running").count(), 1, "{}", h.screen());
        // The toasts of the start and the cancel are still up, and stand clear of the dialog:
        // its buttons can be pressed at once.
        assert!(h.screen().contains("Cancelled"), "{}", h.screen());
        h.click_text("Leave running");
        assert!(h.quit_requested());
        let left = h.app().store().running().and_then(Result::ok).expect("the counter is on disk");
        assert_eq!(left.watch, Watch::None, "nobody measures it now; it counts on");
        done(&dir);
    }

    #[test]
    fn quitting_while_nothing_runs_leaves_at_once() {
        let dir = temp("quit-idle");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 20);
        h.press("ctrl+q");
        assert!(h.quit_requested());
        assert!(!h.screen().contains("is running"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn a_hangup_refreshes_the_running_file_and_leaves_without_asking() {
        let dir = temp("hangup");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 20);
        h.click_text("Rust");
        // The file is refreshed every five seconds; the hangup comes between two refreshes.
        clock.pass(3);
        h.advance(Duration::from_secs(1));
        h.terminate(Termination::Hangup);
        assert!(h.quit_requested());
        assert!(!h.screen().contains("is running"), "no question for nobody:\n{}", h.screen());
        let running = h.app().store().running().and_then(Result::ok).expect("the counter is on disk");
        assert_eq!(running.refreshed, NOON + 3, "refreshed to the moment of the hangup");
        assert_eq!(running.watch, Watch::Window, "the counter ends with the window");
        assert!(h.app().store().sessions.is_empty(), "nothing is recorded; recovery offers it next time");
        done(&dir);
    }

    #[test]
    fn a_terminate_refreshes_the_running_file_then_asks_and_the_grace_leaves_with_it_fresh() {
        let dir = temp("terminate");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 20);
        h.click_text("Rust");
        clock.pass(3);
        h.advance(Duration::from_secs(1));
        h.terminate(Termination::Terminate);
        assert!(!h.quit_requested());
        assert!(h.screen().contains("Rust is running"), "{}", h.screen());
        let running = h.app().store().running().and_then(Result::ok).expect("the counter is on disk");
        assert_eq!(running.refreshed, NOON + 3);
        // Nobody answers: the runtime leaves after the grace, and the file is already fresh.
        h.advance(Termination::Terminate.grace() + Duration::from_secs(1));
        assert!(h.quit_requested());
        assert!(h.app().store().running().is_some_and(|found| found.is_ok()));
        // With nothing running either signal leaves at once.
        let mut idle = harness(app_at(&dir, &clock), 80, 20);
        idle.click_text("Discard");
        idle.terminate(Termination::Terminate);
        assert!(idle.quit_requested());
        done(&dir);
    }

    #[test]
    fn finishing_from_the_quit_question_records_the_session_and_leaves() {
        let dir = temp("quit-finish");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 20);
        h.click_text("Rust");
        clock.pass(60);
        h.advance(Duration::from_secs(1));
        h.press("ctrl+q");
        h.click_text("Finish and quit");
        assert!(h.quit_requested());
        assert!(h.app().timer().is_none());
        assert!(h.app().store().running().is_none());
        let lines = fs::read_to_string(h.app().store().paths.month_file(2026, 9)).expect("month file");
        assert_eq!(lines.lines().count(), 1, "{lines}");
        done(&dir);
    }

    /// Starts Rust, works ten minutes (the pointer moves at the end of them), then leaves the
    /// terminal alone for twenty-five: the silence is noticed at its fifteenth minute and the
    /// fifteen minutes since the last input become idle.
    fn away_for_25_minutes(dir: &Path, clock: &FakeClock) -> Harness<QFocus> {
        seeded(dir);
        let mut h = harness(app_at(dir, clock), 80, 20);
        h.click_text("Rust");
        clock.pass(600);
        h.advance(Duration::from_secs(600));
        h.hover(0, 0);
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(600));
        clock.pass(900);
        h.advance(Duration::from_secs(900));
        assert!(h.app().timer().is_some_and(TimerScreen::is_away), "{}", h.screen());
        let screen = h.screen();
        assert!(screen.contains("No input for 15 min"), "{screen}");
        assert!(screen.contains("10 min"), "the net time stands still:\n{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        clock.pass(600);
        h.advance(Duration::from_secs(600));
        assert!(h.screen().contains("No input for 25 min"), "{}", h.screen());
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(600));
        assert!(!h.app().timer().is_some_and(TimerScreen::is_asking_idle));
        h
    }

    #[test]
    fn silence_stops_the_net_time_and_the_first_key_back_asks_without_answering() {
        let dir = temp("idle-ask");
        let clock = FakeClock::new();
        let mut h = away_for_25_minutes(&dir, &clock);
        // Enter is the key that ends the silence; it opens the question and answers nothing.
        h.press("enter");
        let screen = h.screen();
        assert!(h.app().timer().is_some_and(TimerScreen::is_asking_idle), "{screen}");
        assert!(screen.contains("You were away for 25 min. Does it count?"), "{screen}");
        assert!(screen.contains("Count") && screen.contains("Don't count") && screen.contains("Turn into a break"));
        assert!(!screen.contains("No input for"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        // Esc does not wave the question off.
        h.press("esc");
        assert!(h.app().timer().is_some_and(TimerScreen::is_asking_idle), "{}", h.screen());
        // The running file already carries the idle span and the flag.
        let running = h.app().store().running().and_then(Result::ok).expect("running file");
        assert_eq!(running.flags, vec![Flag::UnclaimedIdle]);
        assert!(running.spans.iter().any(|span| span.kind == SpanKind::Idle && span.seconds == 1_500), "{running:?}");
        done(&dir);
    }

    #[test]
    fn counting_the_silence_adds_it_to_the_work() {
        let dir = temp("idle-count");
        let clock = FakeClock::new();
        let mut h = away_for_25_minutes(&dir, &clock);
        h.press("enter");
        h.click_text("Count");
        assert!(!h.app().timer().is_some_and(TimerScreen::is_asking_idle));
        assert!(h.screen().contains("25 min counted as work"), "{}", h.screen());
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(2_100));
        assert!(h.is_focused(timer_screen::STOP), "{}", h.screen());
        h.press("space");
        let lines = fs::read_to_string(h.app().store().paths.month_file(2026, 9)).expect("month file");
        assert!(!lines.contains("unclaimed-idle"), "{lines}");
        assert!(h.screen().contains("35 min recorded"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn leaving_the_silence_out_keeps_it_idle_and_flagged_in_the_record() {
        let dir = temp("idle-skip");
        let clock = FakeClock::new();
        let mut h = away_for_25_minutes(&dir, &clock);
        h.press("enter");
        h.click_text("Don't count");
        assert!(h.screen().contains("25 min left out"), "{}", h.screen());
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(600));
        h.press("space");
        let lines = fs::read_to_string(h.app().store().paths.month_file(2026, 9)).expect("month file");
        assert!(lines.contains("unclaimed-idle"), "{lines}");
        assert!(h.screen().contains("10 min recorded"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn turning_the_silence_into_a_break_records_a_pause() {
        let dir = temp("idle-break");
        let clock = FakeClock::new();
        let mut h = away_for_25_minutes(&dir, &clock);
        h.press("enter");
        h.click_text("Turn into a break");
        assert!(h.screen().contains("25 min turned into a break"), "{}", h.screen());
        assert_eq!(h.app().timer().map(TimerScreen::work_seconds), Some(600));
        assert!(!h.app().timer().is_some_and(|screen| screen.running().paused), "history, not the state");
        h.press("space");
        let session = h.app().store().sessions.last().cloned().expect("recorded");
        assert!(session.flags.is_empty(), "{session:?}");
        assert_eq!(
            session.spans.iter().map(|span| (span.kind, span.seconds)).collect::<Vec<_>>(),
            vec![(SpanKind::Work, 600), (SpanKind::Pause, 1_500)]
        );
        done(&dir);
    }

    #[test]
    fn stopping_with_the_question_open_records_the_idle_time_as_it_is() {
        let dir = temp("idle-stop");
        let clock = FakeClock::new();
        let mut h = away_for_25_minutes(&dir, &clock);
        h.press("enter");
        assert!(h.app().timer().is_some_and(TimerScreen::is_asking_idle));
        // The keys go to the question; the way to stop is the quit question over it.
        h.press("ctrl+q");
        h.click_text("Finish and quit");
        assert!(h.quit_requested());
        assert!(h.app().timer().is_none(), "{}", h.screen());
        let session = h.app().store().sessions.last().cloned().expect("recorded");
        assert_eq!(session.flags, vec![Flag::UnclaimedIdle]);
        assert_eq!(session.work_seconds(), 600);
        assert!(session.spans.iter().any(|span| span.kind == SpanKind::Idle && span.seconds == 1_500), "{session:?}");
        assert!(crate::span::check(&session.spans, 2_100).is_empty());
        done(&dir);
    }

    #[test]
    fn silence_during_a_break_changes_nothing_and_the_watch_follows_the_counter_across_pages() {
        let dir = temp("idle-break-page");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 20);
        h.click_text("Rust");
        h.press("p");
        clock.pass(1_800);
        h.advance(Duration::from_secs(1_800));
        assert!(!h.app().timer().is_some_and(TimerScreen::is_away));
        h.press("x");
        assert!(!h.app().timer().is_some_and(TimerScreen::is_asking_idle), "{}", h.screen());
        assert!(!h.screen().contains("Does it count?"), "{}", h.screen());
        h.press("p");
        // On the Charts page the counter still runs and the silence is still noticed.
        h.press("2");
        clock.pass(1_200);
        h.advance(Duration::from_secs(1_200));
        assert!(h.app().timer().is_some_and(TimerScreen::is_away), "{}", h.screen());
        h.press("1");
        assert!(h.screen().contains("Does it count?"), "{}", h.screen());
        assert!(h.app().timer().is_some_and(TimerScreen::is_asking_idle));
        h.set_locale("tr");
        let screen = h.screen();
        assert!(screen.contains("Sayılsın mı?"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        done(&dir);
    }

    #[test]
    fn without_a_data_folder_records_stay_in_memory() {
        let dir = temp("memory");
        let clock = FakeClock::new();
        let store = Store::open_read_only(Paths::at(&dir, "test"));
        let mut h = harness(QFocus::new(store, false, None, clock.reader(), Settings::in_memory()), 80, 20);
        let screen = h.screen();
        assert!(screen.contains("stays in memory"), "{screen}");
        assert!(screen.contains("time zone is unknown"), "{screen}");
        h.click_text("Add a category").type_text("Work").press("enter");
        h.click_text("Add a focus").type_text("Rust").press("enter");
        h.press("enter");
        assert!(h.app().timer().is_some(), "{}", h.screen());
        clock.pass(60);
        h.advance(Duration::from_secs(1));
        h.press("space");
        assert_eq!(h.app().pending().len(), 1);
        assert!(h.screen().contains("1 min"), "{}", h.screen());
        assert!(!dir.exists());
    }

    #[test]
    fn the_records_tab_lists_sessions_newest_first_and_lays_out_the_spans() {
        let dir = temp("records");
        let (_, focus) = seeded(&dir);
        two_sessions(&dir, focus);
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 100, 20);
        let screen = h.screen();
        assert!(screen.contains("Work › Rust"), "{screen}");
        assert!(screen.contains("meeting notes"), "{screen}");
        assert!(screen.contains("50 min ◆"), "{screen}");
        let (_, ten) = h.find("10 min").expect("the short session");
        let (_, fifty) = h.find("50 min").expect("the long session");
        assert!(ten < fifty, "the newest session comes first:\n{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        // Down selects the first row; the second row explains its mark and Enter lays it out.
        h.press("down").press("down");
        assert_eq!(h.app().records().selected(), Some(1));
        assert!(h.screen().contains("1 h span, 50 min work"), "{}", h.screen());
        h.press("s");
        let screen = h.screen();
        assert!(h.app().records().is_dumping());
        assert!(screen.contains("Rust · 2026-09-18"), "{screen}");
        assert!(screen.contains("break"), "{screen}");
        assert!(screen.contains("12:30:00"), "{screen}");
        assert!(screen.contains("monotonic"), "{screen}");
        // The session's own strip runs from its start to its end, with the hours between.
        assert!(screen.contains("12:15") && screen.contains("12:45"), "{screen}");
        let (_, work) = h.find("work").expect("the legend names the kinds");
        let (_, table) = h.find("Kind").expect("the table follows");
        assert!(work < table, "the legend stands over the table:\n{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        assert!(h.is_focused(records::DUMP_STRIP), "the strip takes the keys when the dialog opens:\n{screen}");
        h.press("right").press("right");
        assert_eq!(h.app().records().selected(), Some(1), "the table's row is untouched");
        assert!(h.screen().contains("break  12:30–12:40  10 min"), "{}", h.screen());
        h.press("esc");
        assert!(!h.app().records().is_dumping());
        h.press("1");
        assert_eq!(h.app().page(), Page::Today);
        assert!(h.screen().contains("Add a category"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn an_empty_records_tab_says_so() {
        let dir = temp("records-empty");
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 80, 20);
        let screen = h.screen();
        assert!(screen.contains("No records yet"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.click_text("Trash");
        assert!(h.screen().contains("The trash is empty"), "{}", h.screen());
        h.click_text("Archive");
        assert!(h.screen().contains("Nothing is archived"), "{}", h.screen());
        h.click_text("Problems");
        assert!(h.screen().contains("No problems"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn search_filters_by_note_and_focus_and_the_filter_outlives_the_field() {
        let dir = temp("records-search");
        let (_, focus) = seeded(&dir);
        two_sessions(&dir, focus);
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 100, 20);
        h.press("/");
        assert!(h.is_focused(records::SEARCH_INPUT), "{}", h.screen());
        h.type_text("MEETING").press("enter");
        assert!(!h.app().records().is_searching());
        let screen = h.screen();
        assert!(screen.contains("Filtered by MEETING"), "{screen}");
        assert!(screen.contains("50 min"), "{screen}");
        assert!(!screen.contains("10 min"), "{screen}");
        h.press("/").type_text("x").press("esc");
        assert!(h.screen().contains("No matching records"), "{}", h.screen());
        assert_eq!(h.app().records().query(), "MEETINGx");
        done(&dir);
    }

    #[test]
    fn delete_moves_a_session_to_the_trash_and_ctrl_z_brings_it_back() {
        let dir = temp("records-delete");
        let (_, focus) = seeded(&dir);
        let (_, short) = two_sessions(&dir, focus);
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 100, 20);
        h.press("down").press("delete");
        assert!(h.screen().contains("Rust: session moved to the trash"), "{}", h.screen());
        assert_eq!(h.app().store().sessions.len(), 1);
        assert_eq!(h.app().store().voided.first().map(|s| s.id), Some(short.id));
        assert!(h.app().store().paths.trash_file(2026, 9).exists());
        h.press("ctrl+z");
        assert!(h.screen().contains("Rust: session is back"), "{}", h.screen());
        assert_eq!(h.app().store().sessions.len(), 2);
        assert!(h.app().store().voided.is_empty());
        h.press("ctrl+z");
        assert!(h.screen().contains("Nothing to undo"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn the_trash_view_brings_a_session_back_with_enter() {
        let dir = temp("records-trash");
        let (_, focus) = seeded(&dir);
        two_sessions(&dir, focus);
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 100, 20);
        h.press("down").press("delete");
        h.click_text("Trash");
        assert_eq!(h.app().records().view_kind(), records::ViewKind::Trash);
        let screen = h.screen();
        assert!(screen.contains("10 min"), "{screen}");
        assert!(!screen.contains("50 min ◆"), "the long session is not in the trash:\n{screen}");
        h.press("down").press("enter");
        assert!(h.app().store().voided.is_empty());
        assert!(h.screen().contains("The trash is empty"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn the_archive_view_brings_a_focus_back_and_saves_the_tree() {
        let dir = temp("records-archive");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 20);
        h.click_text("Rust");
        // A click starts the counter; stop it and archive the row instead.
        h.press("space");
        h.press("delete");
        assert!(h.screen().contains("Rust archived"), "{}", h.screen());
        h.press("3").click_text("Archive");
        let screen = h.screen();
        assert!(screen.contains("Rust"), "{screen}");
        assert!(screen.contains("Focus"), "{screen}");
        h.press("down").press("enter");
        assert!(h.screen().contains("Rust is back"), "{}", h.screen());
        assert!(!h.app().store().tree.categories[0].focuses[0].archived);
        let tree = fs::read_to_string(h.app().store().paths.tree_file()).expect("tree");
        assert!(!tree.contains("archived"), "{tree}");
        // The toasts of the start, the stop, the archive and the return stand over the page.
        h.advance(Duration::from_secs(10));
        assert!(h.screen().contains("Nothing is archived"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn the_problems_view_lists_what_could_not_be_read() {
        let dir = temp("records-problems");
        let (_, focus) = seeded(&dir);
        two_sessions(&dir, focus);
        let month = Paths::at(&dir, "test").month_file(2026, 9);
        let mut text = fs::read_to_string(&month).expect("month");
        text.push_str("qf1;not a record\n");
        fs::write(&month, text).expect("month");
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 100, 20);
        assert!(h.screen().contains("1 problem was found"), "{}", h.screen());
        h.click_text("Problems");
        let screen = h.screen();
        assert!(screen.contains("2026-09.log:3:1"), "{screen}");
        assert!(screen.contains("fields"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        done(&dir);
    }

    #[test]
    fn a_narrow_records_page_drops_columns_and_stays_clean() {
        let dir = temp("records-narrow");
        let (_, focus) = seeded(&dir);
        two_sessions(&dir, focus);
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 40, 20);
        let screen = h.screen();
        assert!(screen.contains("Date"), "{screen}");
        assert!(screen.contains("Work"), "{screen}");
        assert!(!screen.contains("Note"), "{screen}");
        assert!(!screen.contains("Source"), "{screen}");
        assert!(!screen.contains("End"), "{screen}");
        assert!(screen.contains("50 min ◆"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.resize(100, 20);
        let screen = h.screen();
        assert!(screen.contains("Note"), "{screen}");
        assert!(screen.contains("Source"), "{screen}");
        done(&dir);
    }

    #[test]
    fn ascii_and_turkish_records_keep_clean() {
        let dir = temp("records-ascii");
        let (_, focus) = seeded(&dir);
        two_sessions(&dir, focus);
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 100, 20);
        h.set_glyph_mode(GlyphMode::Ascii).set_locale("tr");
        let screen = h.screen();
        assert!(screen.contains("Kayıtlar"), "{screen}");
        assert!(screen.contains("Hepsi"), "{screen}");
        assert!(screen.contains("Work > Rust"), "{screen}");
        assert!(screen.contains("50 dk *"), "{screen}");
        assert!(!screen.contains('◆'), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.press("down").press("down").press("s");
        let screen = h.screen();
        assert!(screen.contains("mola"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.press("esc").press("enter");
        let screen = h.screen();
        assert!(screen.contains("Kaydı düzelt"), "{screen}");
        assert!(screen.contains("Work > Rust"), "{screen}");
        assert!(screen.contains("Bitiş 12:50"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.press("esc");
        assert!(h.app().form().is_none());
        h.press("a");
        let screen = h.screen();
        assert!(screen.contains("Elle kayıt ekle"), "{screen}");
        assert!(screen.contains("Bir odak seç"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.click_text("Kaydet");
        let screen = h.screen();
        assert!(screen.contains("Oturum için bir odak seç."), "{screen}");
        assert!(screen.contains("Süre sıfır olamaz."), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        done(&dir);
    }

    #[test]
    fn a_session_is_added_by_hand_through_the_form() {
        let dir = temp("records-add");
        let (_, focus) = seeded(&dir);
        two_sessions(&dir, focus);
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 100, 24);
        h.press("a");
        let screen = h.screen();
        assert!(h.app().form().is_some(), "{screen}");
        assert!(screen.contains("Add a session by hand"), "{screen}");
        assert!(h.is_focused(record_form::FOCUS_SELECT), "{screen}");
        // Saving with nothing typed refuses and writes the reasons beside the fields.
        h.click_text("Save");
        let screen = h.screen();
        assert!(h.app().form().is_some(), "{screen}");
        assert!(screen.contains("Pick a focus for the session."), "{screen}");
        assert!(screen.contains("The duration cannot be zero."), "{screen}");
        assert!(!screen.contains("in the future"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        assert_eq!(h.app().store().sessions.len(), 2, "nothing was written");
        // The focus is picked from the list, the start typed, the duration typed as minutes.
        h.press("enter").press("enter");
        assert!(h.screen().contains("Work › Rust"), "{}", h.screen());
        h.press("tab").press("tab").type_text("1000");
        h.press("tab").type_text("0045");
        let screen = h.screen();
        assert!(screen.contains("Ends at 10:45"), "{screen}");
        assert!(!screen.contains("cannot be zero"), "the reason goes once the field changes:\n{screen}");
        h.press("tab").type_text("typed in").press("enter");
        assert!(h.app().form().is_none(), "{}", h.screen());
        assert!(h.screen().contains("Rust: 45 min added by hand"), "{}", h.screen());
        assert!(h.is_focused(records::TABLE), "{}", h.screen());
        let sessions = &h.app().store().sessions;
        assert_eq!(sessions.len(), 3);
        let added = sessions.iter().find(|session| session.note == "typed in").expect("the new session");
        assert_eq!(added.source, Source::Manual);
        assert_eq!(added.work_seconds(), 2_700);
        assert_eq!(added.spans, vec![Span::new(SpanKind::Work, 0, 2_700, ClockSource::Wall)]);
        assert_eq!(clock_of(added.started, 180), "10:00");
        let lines = fs::read_to_string(h.app().store().paths.month_file(2026, 9)).expect("month file");
        assert_eq!(lines.lines().count(), 3, "{lines}");
        assert!(lines.contains(";manual;"), "{lines}");
        let screen = h.screen();
        assert!(screen.contains("by hand"), "the source column says so:\n{screen}");
        assert!(screen.contains("typed in"), "{screen}");
        done(&dir);
    }

    #[test]
    fn a_correction_replaces_the_session_in_its_month_and_ctrl_z_takes_it_back() {
        let dir = temp("records-correct");
        let (_, focus) = seeded(&dir);
        let (_, short_session) = two_sessions(&dir, focus);
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 100, 24);
        h.press("down").press("enter");
        let screen = h.screen();
        assert!(h.app().form().is_some(), "{screen}");
        assert!(screen.contains("Correct the session"), "{screen}");
        assert!(screen.contains("Work › Rust"), "{screen}");
        assert!(screen.contains("13 : 00"), "the start is filled in:\n{screen}");
        assert!(screen.contains("0 h 10 min"), "and the work:\n{screen}");
        assert!(screen.contains("Ends at 13:10"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        // Half an hour instead of ten minutes, and a note.
        h.click_text("0 h 10 min").press("right").type_text("30");
        assert!(h.screen().contains("Ends at 13:30"), "{}", h.screen());
        h.press("tab").type_text("fixed").press("enter");
        assert!(h.app().form().is_none(), "{}", h.screen());
        assert!(h.screen().contains("Rust: session corrected"), "{}", h.screen());
        let sessions = h.app().store().sessions.clone();
        assert_eq!(sessions.len(), 2, "the correction replaces, it does not add");
        let corrected = sessions.iter().find(|session| session.note == "fixed").expect("the correction");
        assert_eq!(corrected.source, Source::Edited);
        assert_eq!(corrected.revision, 2);
        assert_eq!(corrected.replaces, Some(short_session.id));
        assert_eq!(corrected.work_seconds(), 1_800);
        assert!(corrected.spans.iter().all(|span| span.clock == ClockSource::Wall));
        let month = h.app().store().paths.month_file(2026, 9);
        assert_eq!(fs::read_to_string(&month).expect("month").lines().count(), 3);
        let screen = h.screen();
        assert!(screen.contains("30 min"), "{screen}");
        assert!(screen.contains("edited"), "{screen}");
        assert!(!screen.contains("10 min"), "{screen}");
        // Ctrl+Z writes the old values back as a newer line; the file only grows.
        h.press("ctrl+z");
        assert!(h.screen().contains("Rust: correction taken back"), "{}", h.screen());
        let sessions = &h.app().store().sessions;
        assert_eq!(sessions.len(), 2);
        let back = sessions.iter().find(|session| session.replaces == Some(corrected.id)).expect("the revert");
        assert_eq!(back.revision, 3);
        assert_eq!(back.work_seconds(), 600);
        assert_eq!(back.note, "");
        assert_eq!(fs::read_to_string(&month).expect("month").lines().count(), 4);
        assert!(h.screen().contains("10 min"), "{}", h.screen());
        h.press("ctrl+z");
        assert!(h.screen().contains("Nothing to undo"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn a_session_of_a_lost_focus_is_attached_from_the_problems_view() {
        let dir = temp("records-lost");
        let (_, focus) = seeded(&dir);
        two_sessions(&dir, focus);
        let lost = recorded(
            &dir,
            30,
            Id::new(99, 99),
            5 * 3_600,
            vec![Span::new(SpanKind::Work, 0, 900, ClockSource::Mono)],
            "",
        );
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 100, 24);
        assert!(h.screen().contains("unknown focus"), "{}", h.screen());
        h.click_text("Problems");
        let screen = h.screen();
        assert!(screen.contains("1 session belongs to a focus that is not in the list"), "{screen}");
        assert!(screen.contains("15 min"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        assert!(h.is_focused(records::TABLE), "{screen}");
        h.press("down");
        assert_eq!(h.app().records().selected(), Some(0), "{}", h.screen());
        h.press("enter");
        let screen = h.screen();
        assert!(screen.contains("Attach to a focus"), "{screen}");
        assert!(screen.contains("Pick a focus"), "the lost focus is not on offer:\n{screen}");
        h.press("enter").press("enter");
        h.click_text("Save");
        assert!(h.app().form().is_none(), "{}", h.screen());
        assert!(h.screen().contains("Rust: session attached"), "{}", h.screen());
        let attached =
            h.app().store().sessions.iter().find(|session| session.replaces == Some(lost.id)).expect("attached");
        assert_eq!(attached.focus, focus);
        assert_eq!(attached.spans, lost.spans, "only the focus changed; the measured spans stay");
        assert_eq!(attached.source, Source::Edited);
        assert!(h.screen().contains("No problems"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn an_empty_records_tab_offers_to_add_by_hand() {
        let dir = temp("records-empty-add");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 80, 20);
        h.click_text("Add by hand");
        assert!(h.app().form().is_some(), "{}", h.screen());
        h.press("esc");
        assert!(h.app().form().is_none());
        assert!(h.screen().contains("Cancelled"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn the_form_folds_to_one_column_at_forty_columns() {
        let dir = temp("records-form-narrow");
        let (_, focus) = seeded(&dir);
        two_sessions(&dir, focus);
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 40, 24);
        assert!(h.screen().contains("Add by hand"), "{}", h.screen());
        assert!(h.screen().contains("Export"), "the second row of buttons is there:\n{}", h.screen());
        h.press("down").press("enter");
        let screen = h.screen();
        assert!(h.app().form().is_some(), "{screen}");
        assert!(screen.contains("Focus"), "{screen}");
        assert!(screen.contains("Duration"), "{screen}");
        assert!(screen.contains("Save"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.set_glyph_mode(GlyphMode::Ascii).set_locale("tr");
        let screen = h.screen();
        assert!(screen.contains("Süre"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        done(&dir);
    }

    #[test]
    fn an_instance_that_only_looks_cannot_add_or_correct() {
        let dir = temp("records-readonly-form");
        let (_, focus) = seeded(&dir);
        two_sessions(&dir, focus);
        let first = Store::open(Paths::at(&dir, "test"));
        assert_eq!(first.access, Access::Writer);
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 100, 20);
        h.press("a");
        assert!(h.app().form().is_none());
        assert!(h.screen().contains("only looks"), "{}", h.screen());
        h.press("down").press("f2");
        assert!(h.app().form().is_none());
        drop(first);
        done(&dir);
    }

    #[test]
    fn an_instance_that_only_looks_cannot_remove_records() {
        let dir = temp("records-readonly");
        let (_, focus) = seeded(&dir);
        two_sessions(&dir, focus);
        let first = Store::open(Paths::at(&dir, "test"));
        assert_eq!(first.access, Access::Writer);
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 100, 20);
        h.press("down").press("delete");
        assert!(h.screen().contains("only looks"), "{}", h.screen());
        assert_eq!(h.app().store().sessions.len(), 2);
        assert!(!h.app().store().paths.trash_dir().exists());
        drop(first);
        done(&dir);
    }

    #[test]
    fn export_writes_csv_and_json_next_to_the_records() {
        let dir = temp("records-export");
        let (_, focus) = seeded(&dir);
        two_sessions(&dir, focus);
        let clock = FakeClock::new();
        let mut h = records_at(&dir, &clock, 100, 20);
        h.press("e");
        assert!(h.screen().contains("Exported to"), "{}", h.screen());
        let folder = h.app().store().paths.export_dir();
        let csv = fs::read_to_string(folder.join("qfocus-2026-09-18.csv")).expect("csv");
        assert_eq!(csv.lines().count(), 3, "{csv}");
        assert!(csv.contains("Work,Rust,2026-09-18 12:00:00"), "{csv}");
        let json = fs::read_to_string(folder.join("qfocus-2026-09-18.json")).expect("json");
        assert!(json.contains("\"note\": \"meeting notes\""), "{json}");
        done(&dir);
    }

    /// Work with Rust and Review side by side, and Life with Reading: three focuses to move.
    fn seeded_three(dir: &Path) -> (Id, Id, Id) {
        let (rust, reading, review) = seeded_two(dir);
        let store = Store::open(Paths::at(dir, "test"));
        let mut tree = store.tree.clone();
        tree.categories[0].focuses[1].archived = false;
        fs::write(store.paths.tree_file(), tree.write()).expect("tree written");
        (rust, review, reading)
    }

    #[test]
    fn dragging_a_focus_writes_a_fractional_order_and_the_keys_move_it_back() {
        let dir = temp("reorder");
        let (rust, review, _) = seeded_three(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 20);
        let (x, from) = h.find("Rust").expect("Rust row");
        let (_, to) = h.find("Review").expect("Review row");
        assert!(from < to, "{}", h.screen());
        h.drag((x, from), (x, to));
        let tree = &h.app().store().tree;
        let order_of = |id: Id| tree.focus(id).map(|focus| focus.order).unwrap_or_default();
        assert!(order_of(rust) > order_of(review), "{}", h.screen());
        assert_eq!(order_of(rust), 2.0, "past the last one: one more than its order");
        let (_, rust_row) = h.find("Rust").expect("Rust row");
        assert_eq!(rust_row, to, "{}", h.screen());
        let saved = fs::read_to_string(h.app().store().paths.tree_file()).expect("tree");
        assert!(saved.contains("order = 2.0"), "{saved}");
        assert_eq!(h.app().today().selected(), Some(Row::Focus(rust)));
        // The keys move the selected row one place among its siblings.
        h.press("ctrl+shift+up");
        let tree = &h.app().store().tree;
        let rust_order = tree.focus(rust).map(|focus| focus.order).unwrap_or_default();
        let review_order = tree.focus(review).map(|focus| focus.order).unwrap_or_default();
        assert!(rust_order < review_order, "{}", h.screen());
        let (_, rust_row) = h.find("Rust").expect("Rust row");
        assert_eq!(rust_row, from, "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn the_context_menu_moves_a_focus_to_another_category_and_archives_one() {
        let dir = temp("menu");
        let (rust, _, _) = seeded_three(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 24);
        let (x, y) = h.find("Rust").expect("Rust row");
        h.mouse(MouseKind::Down(MouseButton::Right), x, y).mouse(MouseKind::Up(MouseButton::Right), x, y);
        let screen = h.screen();
        assert!(screen.contains("Move to"), "{screen}");
        assert!(screen.contains("Archive"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        let (mx, my) = h.find("Move to").expect("move entry");
        h.hover(mx, my);
        assert!(h.screen().contains("Life"), "{}", h.screen());
        h.click_text("Life");
        assert!(h.screen().contains("Rust moved to Life"), "{}", h.screen());
        let tree = &h.app().store().tree;
        assert!(tree.categories[0].focuses.iter().all(|focus| focus.id != rust));
        assert_eq!(tree.categories[1].focuses.last().map(|focus| focus.id), Some(rust));
        let saved = fs::read_to_string(h.app().store().paths.tree_file()).expect("tree");
        let life_at = saved.find("Life").expect("Life");
        let rust_at = saved.find("Rust").expect("Rust");
        assert!(rust_at > life_at, "{saved}");
        assert_eq!(h.app().today().selected(), Some(Row::Focus(rust)));
        // The menu key opens the menu of the selected row; its last entry archives it.
        h.press("shift+f10");
        assert!(h.screen().contains("Rename"), "{}", h.screen());
        h.click_text("Archive");
        assert!(h.screen().contains("Rust archived"), "{}", h.screen());
        assert!(h.app().store().tree.focus(rust).is_some_and(|focus| focus.archived));
        done(&dir);
    }

    #[test]
    fn the_menu_on_a_category_adds_a_focus_and_reads_clean_in_ascii_and_turkish_at_forty_columns() {
        let dir = temp("menu-ascii");
        seeded_three(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 40, 20);
        h.set_glyph_mode(GlyphMode::Ascii).set_locale("tr");
        let (x, y) = h.find("Work").expect("Work row");
        h.mouse(MouseKind::Down(MouseButton::Right), x, y);
        let screen = h.screen();
        assert!(screen.contains("Odak ekle"), "{screen}");
        assert!(screen.contains("Arşivle"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.click_text("Odak ekle");
        assert!(h.app().today().edit().is_some(), "{}", h.screen());
        h.type_text("Go").press("enter");
        assert_eq!(h.app().store().tree.categories[0].focuses.len(), 3);
        assert_eq!(forbidden(&h.screen()), None, "{}", h.screen());
        done(&dir);
    }

    /// Rust with a one-hour daily goal and Work with a two-hour one, and today's hour of Rust.
    fn with_goals(dir: &Path) -> Id {
        let (_, focus) = seeded(dir);
        two_sessions(dir, focus);
        let store = Store::open(Paths::at(dir, "test"));
        let mut tree = store.tree.clone();
        tree.categories[0].goal = Some(Goal { amount: 7_200, period: Period::Day });
        tree.categories[0].focuses[0].goal = Some(Goal { amount: 3_600, period: Period::Day });
        fs::write(store.paths.tree_file(), tree.write()).expect("tree written");
        focus
    }

    #[test]
    fn g_sets_a_goal_and_the_tree_shows_a_meter_only_for_rows_that_have_one() {
        let dir = temp("goal-set");
        let (_, focus) = seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 24);
        let before = h.screen();
        assert!(!before.contains("/1 h"), "no goal, no meter:\n{before}");
        h.send(Msg::Today(today::Msg::Select(Row::Focus(focus).key())));
        h.press("g");
        let screen = h.screen();
        assert!(screen.contains("Goal for Rust"), "{screen}");
        assert!(screen.contains("Day"), "{screen}");
        assert!(screen.contains("Save"), "{screen}");
        assert!(!screen.contains("Remove"), "nothing to remove yet:\n{screen}");
        assert!(h.app().today().goal_edit().is_some_and(|edit| edit.amount == 3_600), "the suggestion fills the field");
        // The week is chosen; the suggestion is what is kept.
        h.click_text("Week");
        h.click_text("Save");
        let screen = h.screen();
        assert!(screen.contains("Rust: goal kept"), "{screen}");
        h.hover(0, 0).advance(Duration::from_secs(10));
        let screen = h.screen();
        assert!(screen.contains("0/1 h"), "{screen}");
        assert_eq!(screen.matches("0/1 h").count(), 1, "one meter, for the one goal:\n{screen}");
        let tree = &h.app().store().tree;
        assert_eq!(tree.focus(focus).and_then(|f| f.goal), Some(Goal { amount: 3_600, period: Period::Week }));
        let saved = fs::read_to_string(h.app().store().paths.tree_file()).expect("tree");
        assert!(saved.contains("period = \"week\""), "{saved}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        // Zero is refused with the reason; then the goal is removed.
        h.press("g");
        assert!(h.screen().contains("Remove"), "{}", h.screen());
        h.send(Msg::Today(today::Msg::GoalAmount(Duration::ZERO)));
        h.click_text("Save");
        assert!(h.screen().contains("A goal cannot be zero"), "{}", h.screen());
        h.click_text("Remove");
        assert!(h.screen().contains("Rust: goal removed"), "{}", h.screen());
        h.hover(0, 0).advance(Duration::from_secs(10));
        assert!(!h.screen().contains("/1 h"), "{}", h.screen());
        assert!(h.app().store().tree.focus(focus).is_some_and(|f| f.goal.is_none()));
        // At forty columns the field stacks its parts and stays clean.
        h.resize(40, 24);
        h.press("g");
        let screen = h.screen();
        assert!(screen.contains("Goal for Rust"), "{screen}");
        assert!(screen.contains("Save"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.press("esc");
        assert!(h.app().today().goal_edit().is_none());
        assert!(h.screen().contains("Cancelled"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn goals_are_measured_over_their_windows_and_read_under_the_tree_the_counter_and_the_week() {
        let dir = temp("goal-read");
        with_goals(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 24);
        let screen = h.screen();
        // Rust has an hour today, which fills its goal and half of Work's.
        assert!(screen.contains("1/1 h"), "{screen}");
        assert!(screen.contains("1/2 h"), "{screen}");
        assert!(screen.contains("✓ 1/1 h"), "reached goals carry the mark:\n{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.click_text("Rust");
        clock.pass(600);
        h.hover(0, 0).advance(Duration::from_secs(10));
        let screen = h.screen();
        assert!(screen.contains("1 h goal reached · 10 min over"), "{screen}");
        assert!(screen.contains("1 h 10 min of the 2 h goal"), "{screen}");
        assert!(!screen.contains("goal is reached"), "a goal reached before the session is not news:\n{screen}");
        h.press("space");
        h.press("2").click_text("Week");
        h.hover(0, 0).advance(Duration::from_secs(10));
        let screen = h.screen();
        assert!(screen.contains("1.2/2 h"), "the week shows the goals under the bars:\n{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.set_locale("tr").set_glyph_mode(GlyphMode::Ascii);
        h.press("1");
        let screen = h.screen();
        assert!(screen.contains("1.2/1 sa"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.resize(40, 24);
        let screen = h.screen();
        assert!(!screen.contains("/1 sa"), "the meters are the first thing a narrow tree gives up:\n{screen}");
        assert!(screen.contains("Rust"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        done(&dir);
    }

    #[test]
    fn a_goal_crossed_while_counting_is_said_once_and_stops_the_counter_when_asked() {
        let dir = temp("goal-cross");
        let (_, focus) = seeded(&dir);
        let store = Store::open(Paths::at(&dir, "test"));
        let mut tree = store.tree.clone();
        tree.categories[0].focuses[0].goal = Some(Goal { amount: 120, period: Period::Day });
        fs::write(store.paths.tree_file(), tree.write()).expect("tree written");
        drop(store);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 24);
        h.click_text("Rust");
        assert!(h.screen().contains("0 s of the 2 min goal"), "{}", h.screen());
        clock.pass(120);
        h.advance(Duration::from_secs(1));
        let screen = h.screen();
        assert!(screen.contains("Rust: the 2 min goal is reached"), "{screen}");
        assert!(h.app().timer().is_some(), "the counter goes on");
        clock.pass(60);
        h.advance(Duration::from_secs(1));
        assert_eq!(h.screen().matches("goal is reached").count(), 1, "said once:\n{}", h.screen());
        h.hover(0, 0).advance(Duration::from_secs(10));
        assert!(h.screen().contains("2 min goal reached · 1 min over"), "{}", h.screen());
        h.press("space");
        drop(h);
        // With the setting on, reaching the goal stops the counter. The goal is already full; a new one is set high enough to be crossed by this session.
        let store = Store::open(Paths::at(&dir, "test"));
        let mut tree = store.tree.clone();
        tree.categories[0].focuses[0].goal = Some(Goal { amount: 300, period: Period::Day });
        fs::write(store.paths.tree_file(), tree.write()).expect("tree written");
        drop(store);
        let prefs = Prefs { stop_at_goal: true, ..Prefs::default() };
        let mut h = harness(app_with(&dir, &clock, &prefs), 80, 24);
        assert!(h.app().store().tree.focus(focus).is_some());
        h.click_text("Rust");
        clock.pass(130);
        h.advance(Duration::from_secs(1));
        assert!(h.app().timer().is_none(), "stopped at the goal:\n{}", h.screen());
        assert!(h.screen().contains("recorded"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn t_starts_a_countdown_that_counts_down_says_when_it_is_up_and_goes_on() {
        let dir = temp("countdown");
        let (_, focus) = seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 24);
        h.send(Msg::Today(today::Msg::Select(Row::Focus(focus).key())));
        h.press("t");
        let screen = h.screen();
        assert!(screen.contains("Countdown for Rust"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.send(Msg::Today(today::Msg::TimedAmount(Duration::ZERO)));
        h.click_text("Start");
        assert!(h.screen().contains("A countdown cannot be zero"), "{}", h.screen());
        assert!(h.app().timer().is_none());
        h.send(Msg::Today(today::Msg::TimedAmount(Duration::from_secs(90))));
        h.click_text("Start");
        assert_eq!(h.app().timer().map(TimerScreen::focus), Some(focus));
        assert_eq!(h.app().timer().and_then(TimerScreen::countdown), Some(90));
        h.hover(0, 0).advance(Duration::from_secs(10));
        assert!(h.screen().contains("Counting down from 1 min 30 s"), "{}", h.screen());
        clock.pass(90);
        h.advance(Duration::from_secs(1));
        let screen = h.screen();
        assert!(screen.contains("1 min 30 s are up"), "{screen}");
        assert!(h.app().timer().is_some(), "it does not stop by itself");
        h.hover(0, 0).advance(Duration::from_secs(10));
        assert!(h.screen().contains("target 1 min 30 s · 1 min 30 s"), "{}", h.screen());
        clock.pass(30);
        h.advance(Duration::from_secs(1));
        assert!(h.screen().contains("target 1 min 30 s · 2 min"), "{}", h.screen());
        assert_eq!(forbidden(&h.screen()), None, "{}", h.screen());
        h.press("space");
        assert!(h.screen().contains("2 min recorded"), "{}", h.screen());
        drop(h);
        // The setting stops the counter when the countdown is up.
        let prefs = Prefs { stop_at_goal: true, ..Prefs::default() };
        let mut h = harness(app_with(&dir, &clock, &prefs), 80, 24);
        h.send(Msg::Today(today::Msg::Select(Row::Focus(focus).key())));
        h.press("t");
        h.send(Msg::Today(today::Msg::TimedAmount(Duration::from_secs(60))));
        h.click_text("Start");
        clock.pass(60);
        h.advance(Duration::from_secs(1));
        assert!(h.app().timer().is_none(), "{}", h.screen());
        assert!(h.screen().contains("1 min recorded"), "{}", h.screen());
        drop(h);
        // At forty columns the countdown field stacks and stays clean, in Turkish and ASCII too.
        let mut h = harness(app_at(&dir, &clock), 40, 24);
        h.set_locale("tr").set_glyph_mode(GlyphMode::Ascii);
        h.send(Msg::Today(today::Msg::Select(Row::Focus(focus).key())));
        h.press("t");
        let screen = h.screen();
        assert!(screen.contains("Rust için geri sayım"), "{screen}");
        assert!(screen.contains("Başlat"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.press("esc");
        assert!(h.app().today().timed().is_none());
        done(&dir);
    }

    /// The application over `dir` with a settings file in it, so writes can be read back.
    fn app_on_file(dir: &Path, clock: &FakeClock) -> (QFocus, PathBuf) {
        let path = dir.join("settings.toml");
        fs::create_dir_all(dir).expect("folder");
        let settings = Settings::open(&path).schema(Prefs::schema()).self_heal(true);
        (QFocus::new(Store::open(Paths::at(dir, "test")), true, Some(180), clock.reader(), settings), path)
    }

    /// Holds the left button on the control whose label starts with `text` until it confirms.
    fn hold(h: &mut Harness<QFocus>, text: &str) {
        let (x, y) = h.find(text).expect("the held control");
        h.mouse(MouseKind::Down(MouseButton::Left), x, y);
        for _ in 0..45 {
            h.advance(Duration::from_millis(40));
        }
        h.mouse(MouseKind::Up(MouseButton::Left), x, y);
    }

    #[test]
    fn the_settings_tab_opens_with_4_and_lists_every_setting_clean_in_both_languages() {
        let dir = temp("settings-page");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 40);
        h.press("4");
        assert_eq!(h.app().page(), Page::Settings);
        let screen = h.screen();
        for text in [
            "Language",
            "Theme",
            "Icons",
            "Reduce motion",
            "Pillar",
            "Week starts on",
            "Day turns at",
            "Away after",
            "Session ceiling",
            "Stop at the goal",
            "Suggested goal",
            "Sweeping actions",
            "Hold to delete records",
            "Hold to delete older",
            "Hold to delete everything",
        ] {
            assert!(screen.contains(text), "{text} missing:\n{screen}");
        }
        assert!(screen.contains("0 h 15 min"), "{screen}");
        assert!(screen.contains("12 h 00 min"), "{screen}");
        assert!(h.is_focused(settings::LIST), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.set_locale("tr").set_glyph_mode(GlyphMode::Ascii).resize(40, 60);
        let screen = h.screen();
        assert!(screen.contains("Gün dönümü"), "{screen}");
        // At forty columns the held controls are cut by the framework, but each still reads as
        // its own action.
        for text in ["Toplu işlemler", "Basılı tut: kayıtlar", "Basılı tut: eskileri", "Basılı tut: her şeyi"]
        {
            assert!(screen.contains(text), "{text} missing:\n{screen}");
        }
        assert_eq!(forbidden(&screen), None, "{screen}");
        done(&dir);
    }

    #[test]
    fn the_keys_walk_the_settings_and_a_switch_moves_with_space() {
        let dir = temp("settings-keys");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 40);
        h.press("4");
        // Reduce motion is the fourth row; the stop-at-goal switch is the ninth.
        for _ in 0..3 {
            h.press("down");
        }
        // The harness runs with reduced motion on, so the first press turns it off.
        h.press("space");
        assert_eq!(h.app().settings().reduced_motion(), Some(false));
        assert!(!h.env().reduced_motion());
        h.press("space");
        assert_eq!(h.app().settings().reduced_motion(), Some(true));
        for _ in 0..6 {
            h.press("down");
        }
        h.press("space");
        assert!(h.app().prefs().stop_at_goal, "{}", h.screen());
        h.press("space");
        assert!(!h.app().prefs().stop_at_goal);
        // A length typed under its floor is not applied; the reason stands under the label.
        h.send(Msg::Settings(settings::Msg::Length(settings::Timed::IdleAfter, Duration::from_secs(20))));
        assert_eq!(h.app().prefs().idle_after, Duration::from_secs(900));
        assert!(h.screen().contains("At least 1 min"), "{}", h.screen());
        h.send(Msg::Settings(settings::Msg::Length(settings::Timed::IdleAfter, Duration::from_secs(600))));
        assert_eq!(h.app().prefs().idle_after, Duration::from_secs(600));
        assert!(!h.screen().contains("At least"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn changing_the_rollover_regroups_the_day_and_reverting_brings_it_back_and_the_file_follows() {
        let dir = temp("settings-rollover");
        let (_, focus) = seeded(&dir);
        // Half an hour at one in the morning, local time: today with the day turning at
        // midnight, yesterday with it turning at four.
        recorded(&dir, 30, focus, 14 * 3_600, vec![Span::new(SpanKind::Work, 0, 1_800, ClockSource::Mono)], "");
        let clock = FakeClock::new();
        let (app, path) = app_on_file(&dir, &clock);
        let mut h = harness(app, 80, 24);
        assert!(h.screen().lines().next().is_some_and(|top| top.ends_with("30 min")), "{}", h.screen());
        h.send(Msg::Settings(settings::Msg::Rollover(TimeOfDay::new(4, 0, 0))));
        h.advance(Duration::from_millis(100));
        assert!(h.screen().lines().next().is_some_and(|top| top.ends_with("0 s")), "{}", h.screen());
        assert_eq!(h.app().prefs().rollover, TimeOfDay::new(4, 0, 0));
        let written = fs::read_to_string(&path).expect("settings written");
        assert_eq!(written, "day-rollover = \"04:00\"\n");
        // The record itself is untouched.
        let month = fs::read_to_string(h.app().store().paths.month_file(2026, 9)).expect("month");
        assert_eq!(month.lines().count(), 1);
        h.send(Msg::Settings(settings::Msg::Rollover(TimeOfDay::new(0, 0, 0))));
        h.advance(Duration::from_millis(100));
        assert!(h.screen().lines().next().is_some_and(|top| top.ends_with("30 min")), "{}", h.screen());
        assert_eq!(fs::read_to_string(&path).expect("settings written"), "", "a default is not written");
        // The week window follows the first day of the week.
        h.send(Msg::Settings(settings::Msg::WeekStart(6)));
        assert_eq!(h.app().prefs().week_start, Weekday::Sunday);
        h.press("2").click_text("Week");
        let screen = h.screen();
        let sun = screen.find("Sun").expect("Sunday label");
        let sat = screen.find("Sat").expect("Saturday label");
        assert!(sun < sat, "the week now runs from Sunday:\n{screen}");
        // A shared setting is applied at once and written under the framework's key.
        h.send(Msg::Settings(settings::Msg::Shared(settings::Shared::Language("tr".to_owned()))));
        h.advance(Duration::from_millis(100));
        assert!(h.screen().contains("Grafikler"), "{}", h.screen());
        let written = fs::read_to_string(&path).expect("settings written");
        assert!(written.contains("language = \"tr\""), "{written}");
        assert!(written.contains("week-start = \"sunday\""), "{written}");
        done(&dir);
    }

    #[test]
    fn holding_reset_moves_every_session_to_the_trash_one_line_each_and_ctrl_z_brings_them_back() {
        let dir = temp("settings-reset");
        let focus = with_goals(&dir);
        recorded(&dir, 40, focus, 29 * 86_400, vec![Span::new(SpanKind::Work, 0, 600, ClockSource::Mono)], "");
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 40);
        h.press("4");
        hold(&mut h, "Hold to delete records");
        let screen = h.screen();
        assert!(screen.contains("3 sessions moved to the trash"), "{screen}");
        assert!(h.app().store().sessions.is_empty());
        assert_eq!(h.app().store().voided.len(), 3);
        let paths = h.app().store().paths.clone();
        let september = fs::read_to_string(paths.month_file(2026, 9)).expect("september");
        // The test helper writes every record to September's file; the removal of the August
        // session goes to August, the month of its root, so two removals land here.
        assert_eq!(september.lines().count(), 5, "three records and two removals:\n{september}");
        assert_eq!(fs::read_to_string(paths.trash_file(2026, 9)).expect("trash").lines().count(), 2);
        assert_eq!(fs::read_to_string(paths.trash_file(2026, 8)).expect("trash").lines().count(), 1);
        assert!(!h.app().store().tree.categories[0].archived, "reset keeps the lists");
        assert!(h.screen().lines().next().is_some_and(|top| top.ends_with("0 s")), "{}", h.screen());
        h.press("ctrl+z");
        assert!(h.screen().contains("3 sessions are back"), "{}", h.screen());
        assert_eq!(h.app().store().sessions.len(), 3);
        assert!(h.app().store().voided.is_empty());
        let september = fs::read_to_string(paths.month_file(2026, 9)).expect("september");
        assert_eq!(september.lines().count(), 7, "nothing is rewritten, two revivals are appended");
        assert!(h.screen().lines().next().is_some_and(|top| top.ends_with("1 h")), "{}", h.screen());
        h.press("ctrl+z");
        assert!(h.screen().contains("Nothing to undo"), "{}", h.screen());
        // A second pass takes them again, undo brings them again, and an empty store says so.
        h.send(Msg::Settings(settings::Msg::ResetStats));
        assert!(h.app().store().sessions.is_empty());
        h.press("ctrl+z");
        assert_eq!(h.app().store().sessions.len(), 3);
        h.send(Msg::Settings(settings::Msg::ResetStats));
        h.send(Msg::Settings(settings::Msg::ResetStats));
        assert!(h.screen().contains("no records to delete"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn holding_delete_empties_the_lists_too_and_ctrl_z_brings_everything_back() {
        let dir = temp("settings-delete");
        let (rust, reading, review) = seeded_two(&dir);
        two_sessions(&dir, rust);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 40);
        h.press("4");
        hold(&mut h, "Hold to delete everything");
        let screen = h.screen();
        assert!(screen.contains("2 sessions moved to the trash"), "{screen}");
        let tree = &h.app().store().tree;
        assert!(tree.categories.iter().all(|category| category.archived));
        assert!(tree.focus(review).is_some_and(|focus| focus.archived), "already archived, stays so");
        assert!(tree.focus(reading).is_some_and(|focus| focus.archived));
        let saved = fs::read_to_string(h.app().store().paths.tree_file()).expect("tree");
        assert_eq!(saved.matches("archived = true").count(), 5, "{saved}");
        h.press("1");
        assert!(h.screen().contains("Nothing to focus on yet"), "{}", h.screen());
        h.press("ctrl+z");
        assert!(h.screen().contains("2 sessions are back"), "{}", h.screen());
        let tree = &h.app().store().tree;
        assert!(tree.categories.iter().all(|category| !category.archived));
        assert!(tree.focus(rust).is_some_and(|focus| !focus.archived));
        assert!(tree.focus(review).is_some_and(|focus| focus.archived), "what was archived before stays archived");
        assert_eq!(h.app().store().sessions.len(), 2);
        assert!(h.screen().contains("Rust"), "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn holding_delete_older_moves_only_the_days_before_the_chosen_one_and_ctrl_z_brings_them_back() {
        let dir = temp("settings-older");
        history(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 50);
        h.press("4");
        // The date starts a year back, where there is nothing yet.
        assert!(h.screen().contains("2025"), "{}", h.screen());
        hold(&mut h, "Hold to delete older");
        assert!(h.screen().contains("No records began before"), "{}", h.screen());
        assert_eq!(h.app().store().sessions.len(), 7);
        // Tuesday the 15th: the 14th, the 10th and August go; the 15th itself stays.
        h.send(Msg::Settings(settings::Msg::OlderDate(Date::new(2026, 9, 15).expect("valid date"))));
        hold(&mut h, "Hold to delete older");
        let screen = h.screen();
        assert!(screen.contains("3 sessions moved to the trash"), "{screen}");
        assert_eq!(h.app().store().sessions.len(), 4);
        assert_eq!(h.app().store().voided.len(), 3);
        let cut = Date::new(2026, 9, 15).expect("valid date");
        let rollover = h.app().prefs().rollover;
        assert!(h.app().store().sessions.iter().all(|session| crate::day::day_of(session, rollover) >= cut));
        assert!(h.app().store().tree.categories.iter().all(|category| !category.archived), "the lists stay");
        h.press("ctrl+z");
        assert!(h.screen().contains("3 sessions are back"), "{}", h.screen());
        assert_eq!(h.app().store().sessions.len(), 7);
        assert!(h.app().store().voided.is_empty());
        done(&dir);
    }

    #[test]
    fn emptying_the_trash_asks_with_the_counts_and_cancelling_changes_nothing() {
        let dir = temp("settings-purge-cancel");
        let (rust, _, review) = seeded_two(&dir);
        two_sessions(&dir, rust);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 60);
        h.press("4");
        hold(&mut h, "Hold to delete records");
        let paths = h.app().store().paths.clone();
        let month = fs::read(paths.month_file(2026, 9)).expect("month");
        h.click_text("Empty the trash");
        h.advance(Duration::from_millis(200));
        let screen = h.screen();
        assert!(screen.contains("Empty the trash for good?"), "{screen}");
        assert!(screen.contains("2 records and 1 archived row will be removed from"), "{screen}");
        assert!(screen.contains("ctrl+z will not"), "{screen}");
        assert!(screen.contains("Delete for good"), "{screen}");
        assert!(screen.contains("Type delete to confirm"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.type_text("dele").press("esc");
        assert!(h.screen().contains("Emptying the trash was cancelled."), "{}", h.screen());
        assert_eq!(fs::read(paths.month_file(2026, 9)).expect("month"), month);
        assert!(paths.trash_file(2026, 9).exists());
        assert_eq!(h.app().store().voided.len(), 2);
        assert!(h.app().store().tree.focus(review).is_some());
        h.set_locale("tr").set_glyph_mode(GlyphMode::Ascii);
        h.click_text("Çöpü boşalt");
        h.advance(Duration::from_millis(200));
        let screen = h.screen();
        assert!(screen.contains("Çöp kalıcı olarak boşaltılsın mı?"), "{screen}");
        assert!(screen.contains("2 kayıt ve 1 arşivli satır diskten"), "{screen}");
        assert!(screen.contains("Kalıcı olarak sil"), "{screen}");
        assert!(screen.contains("Onaylamak için sil yaz"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.press("esc");
        assert!(h.screen().contains("Boşaltma iptal edildi."), "{}", h.screen());
        assert_eq!(h.app().store().voided.len(), 2);
        // The soft deletion can still be taken back.
        h.press("ctrl+z");
        assert_eq!(h.app().store().sessions.len(), 2, "{}", h.screen());
        done(&dir);
    }

    #[test]
    fn confirming_empties_the_trash_for_good_and_ctrl_z_brings_nothing_back() {
        let dir = temp("settings-purge");
        let (rust, reading, review) = seeded_two(&dir);
        two_sessions(&dir, rust);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 60);
        h.press("4");
        hold(&mut h, "Hold to delete records");
        h.click_text("Empty the trash");
        h.advance(Duration::from_millis(200));
        // The field has focus and Enter waits for the word.
        h.press("enter");
        assert_eq!(h.app().store().voided.len(), 2, "an empty field confirms nothing");
        h.type_text("delet").press("enter");
        assert_eq!(h.app().store().voided.len(), 2, "half the word confirms nothing");
        h.type_text("e").press("enter");
        let screen = h.screen();
        assert!(screen.contains("2 records and 1 archived row deleted."), "{screen}");
        let paths = h.app().store().paths.clone();
        assert!(!paths.month_file(2026, 9).exists(), "every line of the month was in the trash");
        assert_eq!(fs::read_dir(paths.trash_dir()).expect("trash").count(), 0);
        assert!(h.app().store().voided.is_empty());
        assert!(h.app().store().sessions.is_empty());
        let tree = &h.app().store().tree;
        assert!(tree.focus(review).is_none(), "the archived focus nothing refers to went");
        assert!(tree.focus(rust).is_some() && tree.focus(reading).is_some(), "what is in use stays");
        let saved = fs::read_to_string(paths.tree_file()).expect("tree");
        assert!(!saved.contains("Review"), "{saved}");
        h.press("ctrl+z");
        assert!(h.screen().contains("Nothing to undo"), "{}", h.screen());
        assert!(h.app().store().sessions.is_empty());
        done(&dir);
    }

    #[test]
    fn the_word_is_typed_in_turkish_from_the_keyboard_alone_and_any_i_will_do() {
        let dir = temp("settings-purge-word");
        let (_, focus) = seeded(&dir);
        let first = recorded(&dir, 30, focus, 3_600, vec![Span::new(SpanKind::Work, 0, 600, ClockSource::Mono)], "");
        let mut store = Store::open(Paths::at(&dir, "test"));
        store.void(first.id, NOON).expect("void");
        drop(store);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 60);
        h.set_locale("tr");
        h.press("4");
        // Tab reaches the button and Enter opens the question: no hold, no mouse.
        for _ in 0..40 {
            if h.is_focused(settings::PURGE) {
                break;
            }
            h.press("tab");
        }
        assert!(h.is_focused(settings::PURGE), "{}", h.screen());
        h.press("enter").advance(Duration::from_millis(200));
        assert!(h.screen().contains("Onaylamak için sil yaz"), "{}", h.screen());
        h.type_text("  SIL ").press("enter");
        assert!(h.screen().contains("1 kayıt ve 0 arşivli satır silindi."), "{}", h.screen());
        assert!(h.app().store().voided.is_empty());
        done(&dir);
    }

    #[test]
    fn an_archived_focus_with_records_stays_and_is_counted() {
        let dir = temp("settings-purge-kept");
        let (rust, _, review) = seeded_two(&dir);
        let (long, short) = two_sessions(&dir, rust);
        recorded(&dir, 30, review, 3_600, vec![Span::new(SpanKind::Work, 0, 600, ClockSource::Mono)], "");
        // Only the records of Rust are in the trash.
        let mut store = Store::open(Paths::at(&dir, "test"));
        store.void(long.id, NOON).expect("void");
        store.void(short.id, NOON).expect("void");
        drop(store);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 60);
        h.press("4");
        h.click_text("Empty the trash");
        h.advance(Duration::from_millis(200));
        assert!(h.screen().contains("2 records and 0 archived rows will be removed"), "{}", h.screen());
        h.type_text("delete").press("enter");
        let screen = h.screen();
        assert!(screen.contains("2 records and 0 archived rows deleted."), "{screen}");
        assert!(screen.contains("1 archived row kept: records use it."), "{screen}");
        assert!(h.app().store().tree.focus(review).is_some());
        assert_eq!(h.app().store().sessions.len(), 1);
        done(&dir);
    }

    #[test]
    fn an_empty_trash_says_so_without_asking() {
        let dir = temp("settings-purge-empty");
        let (_, focus) = seeded(&dir);
        let first = recorded(&dir, 30, focus, 3_600, vec![Span::new(SpanKind::Work, 0, 600, ClockSource::Mono)], "");
        // A record removed and brought back leaves only a copy in the trash folder: a backup,
        // not something to empty.
        let mut store = Store::open(Paths::at(&dir, "test"));
        store.void(first.id, NOON).expect("void");
        store.restore(first.id, NOON + 1).expect("restore");
        drop(store);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 60);
        h.press("4");
        let paths = h.app().store().paths.clone();
        assert!(paths.trash_file(2026, 9).exists());
        h.click_text("Empty the trash");
        h.advance(Duration::from_millis(200));
        let screen = h.screen();
        assert!(screen.contains("The trash is already empty."), "{screen}");
        assert!(!screen.contains("Empty the trash for good?"), "{screen}");
        assert!(paths.trash_file(2026, 9).exists(), "the backup stays for the next real purge");
        done(&dir);
    }

    #[test]
    fn the_danger_zone_reads_clean_at_forty_columns_in_both_languages_and_ascii() {
        let dir = temp("settings-danger-narrow");
        seeded(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 40, 80);
        h.press("4");
        // Tab reaches every control of both groups, in the order they stand.
        let mut reached = Vec::new();
        for _ in 0..40 {
            h.press("tab");
            for id in [settings::RESET, settings::OLDER_DATE, settings::OLDER, settings::DELETE, settings::PURGE] {
                if h.is_focused(id) && !reached.contains(&id) {
                    reached.push(id);
                }
            }
        }
        assert_eq!(
            reached,
            vec![settings::RESET, settings::OLDER_DATE, settings::OLDER, settings::DELETE, settings::PURGE]
        );
        let screen = h.screen();
        for text in ["Sweeping actions", "Permanent deletion", "Empty the trash", "This cannot be"] {
            assert!(screen.contains(text), "{text} missing:\n{screen}");
        }
        assert_eq!(forbidden(&screen), None, "{screen}");
        h.set_locale("tr").set_glyph_mode(GlyphMode::Ascii);
        let screen = h.screen();
        for text in ["Toplu işlemler", "Kalıcı silme", "Çöpü boşalt", "Geri alınamaz"] {
            assert!(screen.contains(text), "{text} missing:\n{screen}");
        }
        assert_eq!(forbidden(&screen), None, "{screen}");
        done(&dir);
    }

    #[test]
    fn an_instance_that_only_looks_cannot_reset_or_delete() {
        let dir = temp("settings-readonly");
        let (_, focus) = seeded(&dir);
        let (removed, _) = two_sessions(&dir, focus);
        let mut first = Store::open(Paths::at(&dir, "test"));
        first.void(removed.id, NOON).expect("void");
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 40);
        h.press("4");
        h.send(Msg::Settings(settings::Msg::ResetStats));
        assert!(h.screen().contains("only looks"), "{}", h.screen());
        assert_eq!(h.app().store().sessions.len(), 1);
        let month = fs::read(first.paths.month_file(2026, 9)).expect("month");
        h.send(Msg::Settings(settings::Msg::EmptyTrash));
        assert!(!h.screen().contains("Empty the trash for good?"), "{}", h.screen());
        h.send(Msg::Settings(settings::Msg::Purge));
        assert!(h.screen().contains("only looks"), "{}", h.screen());
        assert_eq!(h.app().store().voided.len(), 1);
        assert_eq!(fs::read(first.paths.month_file(2026, 9)).expect("month"), month);
        assert!(first.paths.trash_file(2026, 9).exists());
        drop(first);
        done(&dir);
    }

    #[test]
    fn two_minutes_of_silence_show_the_dashboard_and_the_first_input_brings_the_page_back_as_it_was() {
        let dir = temp("dashboard");
        let focus = with_goals(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 40);
        h.send(Msg::Today(today::Msg::Select(Row::Focus(focus).key())));
        h.press("3").press("/").type_text("meeting").press("enter");
        assert_eq!(h.app().records().query(), "meeting");
        h.advance(Duration::from_secs(119));
        assert!(!h.app().is_idle());
        h.advance(Duration::from_secs(2));
        assert!(h.app().is_idle(), "{}", h.screen());
        let screen = h.screen();
        assert!(screen.contains("Most worked on today"), "{screen}");
        assert!(screen.contains("Rust"), "{screen}");
        assert!(screen.contains("1/1 h"), "the week's fullness reads from the goals:\n{screen}");
        assert!(screen.contains("12:00"), "the strip is the day's:\n{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        // A minute turns and the dashboard is still there.
        h.advance(Duration::from_secs(60));
        assert!(h.app().is_idle());
        // The first input takes the layer away; the page under it is as it was left.
        h.press("f9");
        assert!(!h.app().is_idle(), "{}", h.screen());
        assert!(!h.screen().contains("Most worked on today"), "{}", h.screen());
        assert_eq!(h.app().page(), Page::Records);
        assert_eq!(h.app().records().query(), "meeting");
        assert_eq!(h.app().today().selected(), Some(Row::Focus(focus)));
        // Without goals the week is read as a total; narrow, in Turkish and ASCII, it stays clean.
        let mut h = harness(app_at(&temp("dashboard-plain"), &clock), 40, 30);
        h.set_locale("tr").set_glyph_mode(GlyphMode::Ascii);
        h.advance(Duration::from_secs(121));
        let screen = h.screen();
        assert!(h.app().is_idle(), "{screen}");
        assert!(screen.contains("Bu hafta: 0 sn"), "{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
        done(&dir);
    }

    #[test]
    fn a_running_counter_shows_no_dashboard_but_its_screen_quietens_and_wakes() {
        let dir = temp("quiet");
        with_goals(&dir);
        let clock = FakeClock::new();
        let mut h = harness(app_at(&dir, &clock), 80, 24);
        h.click_text("Rust");
        clock.pass(121);
        h.advance(Duration::from_secs(121));
        assert!(!h.app().is_idle());
        assert!(h.app().is_quiet(), "{}", h.screen());
        let screen = h.screen();
        assert!(screen.contains("Stop"), "the buttons stay:\n{screen}");
        assert!(screen.contains("goal reached"), "{screen}");
        let (x, y) = h.find("goal reached").expect("goal line");
        let quiet = h.fg(u16::try_from(x).unwrap_or(0), u16::try_from(y).unwrap_or(0));
        h.press("f9");
        assert!(!h.app().is_quiet());
        let awake = h.fg(u16::try_from(x).unwrap_or(0), u16::try_from(y).unwrap_or(0));
        assert_ne!(quiet, awake, "the line takes the faint tone while quiet");
        h.press("space");
        assert!(h.app().timer().is_none());
        done(&dir);
    }
}
