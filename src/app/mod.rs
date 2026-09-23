//! The application: the store, the screens and what happens between them.
//!
//! The screens under [`crate::ui`] know nothing about the disk. Everything that touches it —
//! the catalogue, the month files, the running file, the lock — is done here, once, in answer
//! to what a screen asks for. The clock is read through one function handed in at the start, so
//! a test can hand in a clock of its own and drive the timer without waiting.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use qframe::date::{Date, DateTime, Weekday, local_offset};
use qframe::prelude::*;
use qframe::runtime::{Confirm, Task, TaskId, Termination, Update};
use qframe::storage::{Family, Settings, atomic_write};
use qframe::uptime::Uptime;
use qframe::widgets::{Appearance, Bar, BarChart, BigText, Modal, Setup, SetupMsg, Span, Toast};

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

mod counter;
mod data;
mod updates;
mod view;
mod wizard;

/// The keymap compiled in, so an installed program carries its keys with it.
const KEYMAP: &str = include_str!("../../keymap.toml");

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

/// Cells between the edge of the terminal and the header on each side.
const HEADER_PADDING: u16 = 1;

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
    // The old settings file is brought over first; the file is then checked against every key
    // qfocus knows and repaired with a backup when it has to be. Both reports are shown on the
    // Settings page.
    let crate::config::Loaded { settings, left_behind } = crate::config::load();
    let (store, on_disk) = match Paths::detect() {
        Some(paths) => (Store::open(paths), true),
        None => {
            let nowhere = std::env::temp_dir().join("quvyta-focus-nowhere");
            (Store::open_read_only(Paths::at(nowhere, machine_name())), false)
        }
    };
    // The first start asks before anything is written. While the wizard is open even resolving
    // the shared preferences the usual way would make `quvyta.conf`, so they come from the
    // wizard, which resolves them without touching a file.
    let i18n = crate::config::spoken();
    let setup = Family::QUVYTA
        .config_dir()
        .map(|_| Setup::new(Family::QUVYTA, crate::config::APP, &i18n, Msg::Setup).on_finish(Msg::SetUp))
        .filter(Setup::needed);
    // The shared look is resolved before the first frame, so the family's language and theme are
    // in force from the start; the rows on the Settings page write it back.
    let preferences = match &setup {
        Some(setup) => setup.preferences().clone(),
        None => crate::config::preferences(),
    };
    let appearance = Appearance::new(Family::QUVYTA, crate::config::APP, preferences.clone());
    let mut app = QFocus::new(store, on_disk, local_offset(), Box::new(clock::now), settings.clone(), appearance)
        .with_left_behind(left_behind);
    if let Some(setup) = setup {
        app = app.with_setup(setup, None);
    }
    let app = app.update_notice(crate::config::UpdateFolders::here());
    let mut runtime =
        Runtime::new(app).settings(&settings).preferences(&preferences).keymap_source("keymap.toml", KEYMAP);
    for &(file, text) in crate::locales() {
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
///
/// Not [`Eq`]: the framework's setup messages carry how far a font install has come, which is
/// measured as a fraction.
#[derive(Debug, Clone, PartialEq)]
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
    /// Something on the first step of the setup wizard, which the framework answers.
    Setup(SetupMsg),
    /// The wizard wrote the shared keys and made the settings file; qfocus writes its own
    /// settings into it.
    SetUp,
    /// A newer version of qfocus is out.
    NewVersion(Update),
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
    /// The rows every application of the family shows for its look, and what they write.
    appearance: Appearance,
    /// The first-run wizard, while qfocus has no settings file of its own; `None` once it is
    /// over and on every start after it.
    setup: Option<Setup<Msg>>,
    /// The family's folder when it is not this platform's own, for a test; the appearance is
    /// rebuilt over it when the wizard finishes.
    config_folder: Option<PathBuf>,
    /// Where the family's update notice is kept, or `None` where qfocus asks for no newer version.
    updates: Option<crate::config::UpdateFolders>,
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
    /// The first day of the week `goals_said` was measured with. The week moves while a counter
    /// runs when the person changes the language, the region or the chosen first day; the goals
    /// that move fills or empties are not crossings and are taken as they stand.
    goals_week: Weekday,
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
    /// The application with `files` from an earlier version's settings folder that were not
    /// moved, shown on the Settings page until read.
    #[must_use]
    pub fn with_left_behind(mut self, files: Vec<qframe::diagnostics::Diagnostic>) -> Self {
        self.settings_screen = self.settings_screen.with_left_behind(files);
        self
    }

    /// The application over `store`. `on_disk` says whether the store is a real folder, `offset`
    /// is the local time zone if known, `clock` reads the clocks, `settings` is the settings
    /// file, from which the preferences are read, and `appearance` holds the family's shared look
    /// and writes a change to it.
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
        appearance: Appearance,
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
            appearance,
            setup: None,
            config_folder: None,
            updates: None,
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
            goals_week: Weekday::Monday,
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

    /// The application with the first-run wizard open, writing into `folder` as the family's
    /// folder when it is not this platform's own. Only a [`Setup`] that is still
    /// [needed](Setup::needed) is worth handing over.
    #[must_use]
    pub fn with_setup(mut self, setup: Setup<Msg>, folder: Option<PathBuf>) -> Self {
        self.setup = Some(setup);
        // The wizard's last step shows the family's update notice; like everything else there it
        // is held until Finish, so a wizard left half-way writes nothing.
        self.appearance = self.appearance.clone().without_saving();
        self.config_folder = folder;
        self
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

    /// The day the week starts on: the chosen one, or the active language's and region's.
    fn week_start(&self) -> Weekday {
        self.prefs.week_starts_on(qframe::i18n::first_weekday())
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
            // The appearance writes its own files, key by key, and keeps the settings qfocus
            // holds in step, so nothing more is saved here.
            Some(settings::Request::Appearance(change)) => {
                Command::batch([command, self.appearance.update(change, &mut self.settings)])
            }
            Some(settings::Request::Prefs(prefs)) => {
                self.prefs = prefs;
                self.prefs.write(&mut self.settings);
                // The day and the goals regroup at once; the records are untouched.
                self.refresh_day();
                // While the wizard asks, the choice is held in memory alone: the file is made
                // when it finishes, and a wizard left half-way leaves nothing behind.
                if self.setting_up() {
                    return command;
                }
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
        // The keyboard starts on the list, so the arrows work before any tab is picked; a
        // counter taken over moves it to its own button after this.
        let found = self.find_running();
        // On the first start the wizard has the screen, so the appearance rows take the keys.
        let first = if self.setting_up() { wizard::FIRST } else { today::TREE };
        // The question for a newer version runs on a thread of its own, so the start never
        // waits for it; while the wizard is open it waits for the wizard instead.
        Command::batch([Command::focus(first), found, self.sync_tick(), self.ask_for_update()])
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
        // While the wizard asks, the application's keys have nothing to act on: no page is
        // drawn under it and nothing may be written before Finish.
        if self.setting_up() {
            return None;
        }
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
        // The first start asks before it shows anything of its own: the wizard has the screen.
        if self.setting_up() {
            self.setup_wizard(ui);
            return;
        }
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
            // The framework owns its step: it applies the change, writes the two files when the
            // wizard finishes, and answers with `Msg::SetUp`.
            Msg::Setup(message) => match self.setup.take() {
                Some(mut setup) => {
                    let done = setup.update(message, &mut self.settings);
                    self.setup = Some(setup);
                    done
                }
                None => Command::none(),
            },
            Msg::SetUp => self.finish_setup(),
            Msg::NewVersion(update) => Command::toast(update.toast()),
        }
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
mod tests;
