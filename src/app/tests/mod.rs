//! What every screen test needs: a clock a test moves by hand, a harness with the built-in
//! locale and keymap files, a store seeded with a few focuses, and the tour that walks one
//! screen through every language. The checks themselves live in the files beside this one,
//! one per subject.

mod charts;
mod counter;
mod goals;
mod idle;
mod languages;
mod quit;
mod records;
mod recover;
mod settings;
mod tree;
mod wizard;

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use qframe::date::Weekday;
use qframe::env::{AssetDirs, Env};
use qframe::event::{MouseButton, MouseKind};
use qframe::icons::GlyphMode;

use super::*;
use crate::session::Source;
use crate::span::{ClockSource, Span, SpanKind};
use crate::tree::{Category, Focus, Goal, Period};
use crate::ui::record_form;

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
        locale_sources: crate::locales().iter().map(|(file, text)| ((*file).to_owned(), (*text).to_owned())).collect(),
        keymap_source: Some(("keymap.toml".to_owned(), KEYMAP.to_owned())),
        ..AssetDirs::default()
    };
    Env::load(&dirs).expect("the built-in files load")
}

fn harness(app: QFocus, width: u16, height: u16) -> Harness<QFocus> {
    let mut harness = Harness::with_env(app, env(), width, height);
    // The machine's region would set the week; the tests set it themselves.
    harness.set_region(None).set_locale("en").set_glyph_mode(GlyphMode::Unicode).set_reduced_motion(true);
    harness
}

fn app_at(dir: &Path, clock: &FakeClock) -> QFocus {
    QFocus::new(
        Store::open(Paths::at(dir, "test")),
        true,
        Some(180),
        clock.reader(),
        Settings::in_memory(),
        appearance(dir),
    )
}

/// The family's appearance over `dir`: a test never reads or writes the person's own settings.
fn appearance(dir: &Path) -> Appearance {
    Appearance::new(Family::QUVYTA, crate::config::APP, crate::config::preferences_in(dir)).in_folder(dir)
}

/// The application over `dir` with `prefs` in force, as a settings file would give them.
fn app_with(dir: &Path, clock: &FakeClock, prefs: &Prefs) -> QFocus {
    let mut settings = Settings::in_memory();
    prefs.write(&mut settings);
    QFocus::new(Store::open(Paths::at(dir, "test")), true, Some(180), clock.reader(), settings, appearance(dir))
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

/// Every screen a person meets in a day of use, in `code` at forty columns: the Today page
/// empty and full, the counter and its break, the goal and countdown fields, each scale of
/// the charts with a bar picked, each view of the records with the form and the spans, the
/// whole settings page, and the quit and purge dialogs. Each comes back with its name and the
/// harness it was drawn in, so a check can look at the cells as well as the text.
fn tour(code: &str, name: &str, check: &mut dyn FnMut(&str, &Harness<QFocus>)) {
    // Toasts are looked at where they are raised, then let go, so they do not stand over
    // the next screen.
    let settle = |h: &mut Harness<QFocus>| {
        h.hover(0, 0).advance(Duration::from_secs(10));
    };
    let clock = FakeClock::new();
    // The first screen anyone meets: both steps of the setup wizard, in a root of its own so
    // nothing of the person's is read or written.
    let first = temp(&format!("tour-first-{name}"));
    let mut h = wizard::start(&first, &clock, 40, 30);
    h.set_locale(code);
    check("wizard, appearance", &h);
    h.send(Msg::Setup(qframe::widgets::SetupMsg::Next));
    check("wizard, the day", &h);
    settle(&mut h);
    done(&first);

    let empty = temp(&format!("tour-empty-{name}"));
    let mut h = harness(app_at(&empty, &clock), 40, 24);
    h.set_locale(code);
    check("today, empty", &h);
    settle(&mut h);
    done(&empty);

    let dir = temp(&format!("tour-{name}"));
    history(&dir);
    let store = Store::open(Paths::at(&dir, "test"));
    let mut tree = store.tree.clone();
    tree.categories[0].goal = Some(Goal { amount: 7_200, period: Period::Day });
    tree.categories[0].focuses[0].goal = Some(Goal { amount: 3_600, period: Period::Week });
    fs::write(store.paths.tree_file(), tree.write()).expect("tree written");
    drop(store);
    let mut h = harness(app_at(&dir, &clock), 40, 30);
    h.set_locale(code);
    check("today", &h);
    settle(&mut h);
    let rust = Row::Focus(Id::new(2, 2)).key();
    h.send(Msg::Today(today::Msg::Select(rust.clone())));
    h.press("g");
    check("goal field", &h);
    h.press("esc");
    settle(&mut h);
    h.press("t");
    check("countdown field", &h);
    h.press("esc");
    check("cancelled", &h);
    settle(&mut h);
    let (x, y) = h.find("Rust").expect("the focus is listed");
    h.mouse(MouseKind::Down(MouseButton::Right), x, y).mouse(MouseKind::Up(MouseButton::Right), x, y);
    check("row menu", &h);
    settle(&mut h);
    h.press("esc");
    h.send(Msg::Today(today::Msg::Activate(rust)));
    clock.pass(600);
    h.advance(Duration::from_secs(1));
    check("counter", &h);
    settle(&mut h);
    h.press("p");
    check("break", &h);
    settle(&mut h);
    h.press("n");
    check("note field", &h);
    settle(&mut h);
    h.press("esc");
    h.press("ctrl+q");
    check("quit dialog", &h);
    settle(&mut h);
    h.press("esc");
    for scale in 0..4 {
        h.press("2");
        h.send(Msg::Charts(crate::ui::charts::Msg::Scale(scale)));
        check(&format!("charts {scale}"), &h);
        settle(&mut h);
        h.send(Msg::Charts(crate::ui::charts::Msg::Select(0)));
        check(&format!("charts {scale}, picked"), &h);
        settle(&mut h);
    }
    h.press("3");
    for view in 0..4 {
        h.send(Msg::Records(crate::ui::records::Msg::View(view)));
        check(&format!("records {view}"), &h);
        settle(&mut h);
    }
    h.send(Msg::Records(crate::ui::records::Msg::View(0)));
    h.send(Msg::Records(crate::ui::records::Msg::Select(1)));
    h.press("s");
    check("spans", &h);
    settle(&mut h);
    h.press("esc");
    h.press("a");
    check("record form", &h);
    settle(&mut h);
    h.press("esc");
    h.press("delete");
    check("records, one removed", &h);
    settle(&mut h);
    h.resize(40, 160);
    h.press("4");
    check("settings", &h);
    settle(&mut h);
    h.send(Msg::Settings(crate::ui::settings::Msg::EmptyTrash));
    check("purge dialog", &h);
    settle(&mut h);
    done(&dir);
}

/// Whether a character two cells wide lost its second cell: drawn in the last column, or
/// with something else drawn over the cell it covers. Either would shift or break the
/// columns after it in a real terminal.
fn broken_wide_cell(h: &Harness<QFocus>) -> Option<String> {
    let buffer = h.buffer();
    let (width, height) = (buffer.area.width, buffer.area.height);
    for y in 0..height {
        for x in 0..width {
            let symbol = buffer[(x, y)].symbol();
            if qframe::text::width(symbol) < 2 {
                continue;
            }
            if x + 1 >= width {
                return Some(format!("{symbol} is cut at the right edge of row {y}"));
            }
            let next = buffer[(x + 1, y)].symbol();
            if !next.is_empty() && next != " " {
                return Some(format!("{next} is drawn over the second half of {symbol} at {x},{y}"));
            }
        }
    }
    None
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
