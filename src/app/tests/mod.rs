//! What every screen test needs: a clock a test moves by hand, a harness with the built-in
//! locale and keymap files, a store seeded with a few focuses, and the tour that walks one
//! screen through every language. The checks themselves live in the files beside this one,
//! one per subject.

mod charts;
mod counter;
mod goals;
mod idle;
mod languages;
mod member;
mod quit;
mod records;
mod recover;
mod settings;
mod sweep;
mod tree;
mod updates;
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

/// The shared appearance over `dir`: a test never reads or writes the person's own settings.
fn appearance(dir: &Path) -> Appearance {
    Appearance::new(Ecosystem::QUVYTA, crate::config::APP, crate::config::preferences_in(dir)).in_folder(dir)
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

/// How the tour draws: the terminal's width and the glyph column in force.
#[derive(Clone, Copy)]
struct Look {
    width: u16,
    glyphs: GlyphMode,
}

impl Look {
    /// Forty columns in Unicode glyphs: the narrowest terminal qfocus promises to read in.
    const NARROW: Self = Self { width: 40, glyphs: GlyphMode::Unicode };
}

/// Every screen a person meets in a day of use, in `code` at the width and glyphs of `look`: the Today page
/// empty and full, the counter and its break, the goal and countdown fields, each scale of
/// the charts with a bar picked, each view of the records with the form and the spans, the
/// whole settings page, and the quit and purge dialogs. Each comes back with its name and the
/// harness it was drawn in, so a check can look at the cells as well as the text.
fn tour(code: &str, name: &str, look: Look, check: &mut dyn FnMut(&str, &Harness<QFocus>)) {
    let width = look.width;
    // Toasts are looked at where they are raised, then let go, so they do not stand over
    // the next screen.
    let settle = |h: &mut Harness<QFocus>| {
        h.hover(0, 0).advance(Duration::from_secs(10));
    };
    let clock = FakeClock::new();
    // The first screen anyone meets: both steps of the setup wizard, in a root of its own so
    // nothing of the person's is read or written.
    let first = temp(&format!("tour-first-{name}"));
    let mut h = wizard::start(&first, &clock, width, 30);
    h.set_locale(code).set_glyph_mode(look.glyphs);
    check("wizard, appearance", &h);
    // Tab reaches Next whatever the language calls it.
    tab_to(&mut h, "wizard-next");
    h.press("enter");
    check("wizard, the day", &h);
    settle(&mut h);
    done(&first);

    let empty = temp(&format!("tour-empty-{name}"));
    let mut h = harness(app_at(&empty, &clock), width, 24);
    h.set_locale(code).set_glyph_mode(look.glyphs);
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
    let mut h = harness(app_at(&dir, &clock), width, 30);
    h.set_locale(code).set_glyph_mode(look.glyphs);
    check("today", &h);
    settle(&mut h);
    walk_to(&mut h, Row::Focus(Id::new(2, 2)));
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
    walk_to(&mut h, Row::Focus(Id::new(2, 2)));
    h.press("enter");
    assert!(h.app().timer().is_some(), "Enter on the row starts it:\n{}", h.screen());
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
        // The scales are reached with Tab and the next one chosen with →, in any language; the
        // keys then go to its chart, so Tab comes back for each.
        if scale > 0 {
            tab_to(&mut h, crate::ui::charts::SCALES);
            h.press("right");
        }
        assert_eq!(h.app().charts().scale().index(), scale, "{}", h.screen());
        check(&format!("charts {scale}"), &h);
        settle(&mut h);
        tab_to(&mut h, crate::ui::charts::CHART);
        // A standing chart is walked with ←/→, the year's days with the arrows and Enter, and the
        // day's hours, lying down on a narrow screen, with ↑/↓.
        for key in ["right", "enter", "down"] {
            if h.app().charts().selected().is_none() {
                h.press(key);
            }
        }
        assert!(h.app().charts().selected().is_some(), "a bar is picked:\n{}", h.screen());
        check(&format!("charts {scale}, picked"), &h);
        settle(&mut h);
    }
    h.press("3");
    for view in 0..4 {
        // Each view is the next one to the right; Tab comes back to the chooser for each.
        if view > 0 {
            tab_to(&mut h, crate::ui::records::VIEWS);
            h.press("right");
        }
        assert_eq!(h.app().records().view_kind().index(), view, "{}", h.screen());
        check(&format!("records {view}"), &h);
        settle(&mut h);
    }
    tab_to(&mut h, crate::ui::records::VIEWS);
    h.press("home");
    tab_to(&mut h, crate::ui::records::TABLE);
    for _ in 0..10 {
        if h.app().records().selected() == Some(1) {
            break;
        }
        h.press("down");
    }
    assert_eq!(h.app().records().selected(), Some(1), "{}", h.screen());
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
    h.resize(width, 160);
    h.press("4");
    check("settings", &h);
    settle(&mut h);
    tab_to(&mut h, crate::ui::settings::PURGE);
    h.press("enter");
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

/// The cells of the screen's line `y` as text, with the column each character stands in, so a
/// place found in the text can be clicked whatever width the characters before it take.
fn cells_of_line(h: &Harness<QFocus>, y: i32) -> (Vec<char>, Vec<i32>) {
    let buffer = h.buffer();
    let Ok(row) = u16::try_from(y) else { return (Vec::new(), Vec::new()) };
    let (mut chars, mut columns) = (Vec::new(), Vec::new());
    for x in 0..buffer.area.width {
        for c in buffer[(x, row)].symbol().chars() {
            chars.push(c);
            columns.push(i32::from(x));
        }
    }
    (chars, columns)
}

/// The column where `word` starts on line `y` at or after column `from`, standing as a word of its
/// own: `Pazar` is not found inside `Pazartesi`.
fn word_on_line(h: &Harness<QFocus>, y: i32, word: &str, from: i32) -> Option<i32> {
    let (chars, columns) = cells_of_line(h, y);
    let wanted: Vec<char> = word.chars().collect();
    let edge = |at: Option<&char>| at.is_none_or(|c| !c.is_alphanumeric());
    (0..chars.len().saturating_sub(wanted.len() - 1)).find_map(|start| {
        let fits = columns[start] >= from
            && chars[start..start + wanted.len()] == wanted[..]
            && (start == 0 || edge(chars.get(start - 1)))
            && edge(chars.get(start + wanted.len()));
        fits.then_some(columns[start])
    })
}

/// The line and the column just past the end of the row labelled `label`, the first on screen.
fn row_of(h: &Harness<QFocus>, label: &str) -> (i32, i32) {
    let (x, y) = h.find(label).unwrap_or_else(|| panic!("`{label}` is not on screen:\n{}", h.screen()));
    (y, x + i32::try_from(label.chars().count()).unwrap_or(0))
}

/// Opens the drop-down of the settings row labelled `label` with a click and clicks `option` in
/// the list it opens.
fn pick(h: &mut Harness<QFocus>, label: &str, option: &str) {
    let (y, after) = row_of(h, label);
    let (arrow, _) = cells_of_line(h, y);
    assert!(arrow.contains(&'▾'), "the row of `{label}` has no drop-down:\n{}", h.screen());
    let x = word_on_line(h, y, "▾", after).unwrap_or_else(|| panic!("no arrow right of `{label}`:\n{}", h.screen()));
    h.click(x, y);
    h.advance(Duration::from_millis(100));
    let height = i32::from(h.buffer().area.height);
    // The field still shows the value in force; the option is clicked in the list, off its line.
    let (ox, oy) = (0..height)
        .filter(|line| *line != y)
        .find_map(|line| word_on_line(h, line, option, 0).map(|x| (x, line)))
        .unwrap_or_else(|| panic!("`{option}` is not in the open list of `{label}`:\n{}", h.screen()));
    h.click(ox, oy);
    h.advance(Duration::from_millis(100));
}

/// The column of the `segment`th run of digits right of the label on the row labelled `label`: the
/// hours of a time or a length are the first, the minutes the second.
fn segment_in_row(h: &Harness<QFocus>, label: &str, segment: usize) -> (i32, i32) {
    let (y, after) = row_of(h, label);
    let (chars, columns) = cells_of_line(h, y);
    let starts: Vec<i32> = (0..chars.len())
        .filter(|at| columns[*at] >= after && chars[*at].is_ascii_digit())
        .filter(|at| *at == 0 || !chars[at - 1].is_ascii_digit())
        .map(|at| columns[at])
        .collect();
    let x = *starts
        .get(segment)
        .unwrap_or_else(|| panic!("the row of `{label}` has no segment {segment}:\n{}", h.screen()));
    (x, y)
}

/// Turns the wheel `notches` times over segment `segment` of the field on the row labelled
/// `label`, up when `notches` is positive: the way a person sets a time or a length on a settings
/// row with the pointer.
fn roll(h: &mut Harness<QFocus>, label: &str, segment: usize, notches: i32) {
    let (x, y) = segment_in_row(h, label, segment);
    let kind = if notches > 0 { MouseKind::ScrollUp } else { MouseKind::ScrollDown };
    for _ in 0..notches.unsigned_abs() {
        h.mouse(kind, x, y);
    }
    h.advance(Duration::from_millis(100));
}

/// Clicks segment `segment` of the field on the row labelled `label` and types `digits`, as a
/// person sets a time or a length from the keyboard.
fn type_in_row(h: &mut Harness<QFocus>, label: &str, segment: usize, digits: &str) {
    let (x, y) = segment_in_row(h, label, segment);
    h.click(x, y);
    h.type_text(digits);
    h.advance(Duration::from_millis(100));
}

/// Presses Tab until the widget `id` has the keyboard, as a person reaches a control without the
/// mouse; fails when forty presses do not reach it.
fn tab_to(h: &mut Harness<QFocus>, id: &str) {
    for _ in 0..40 {
        if h.is_focused(id) {
            return;
        }
        h.press("tab");
    }
    assert!(h.is_focused(id), "Tab never reaches `{id}`:\n{}", h.screen());
}

/// Walks the Today tree with ↓ until `row` is the selected one, as a person picks a row from the
/// keyboard; fails when twenty presses do not reach it.
fn walk_to(h: &mut Harness<QFocus>, row: Row) {
    for _ in 0..20 {
        if h.app().today().selected().as_ref() == Some(&row) {
            return;
        }
        h.press("down");
    }
    assert_eq!(h.app().today().selected(), Some(row), "↓ never reaches the row:\n{}", h.screen());
}

/// Clicks the first place on screen where `word` stands as a word of its own: `Ay`, the month, and
/// not the start of `Ayarlar`, the Settings tab.
fn click_word(h: &mut Harness<QFocus>, word: &str) {
    let height = i32::from(h.buffer().area.height);
    let (x, y) = (0..height)
        .find_map(|line| word_on_line(h, line, word, 0).map(|x| (x, line)))
        .unwrap_or_else(|| panic!("`{word}` is not on screen as a word:\n{}", h.screen()));
    h.click(x, y);
}
