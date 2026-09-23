//! The counter from start to record, including the question a session over the ceiling asks.

use super::*;

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
fn a_ceiling_of_an_hour_flags_a_session_the_default_would_have_let_pass() {
    let dir = temp("ceiling-setting");
    seeded(&dir);
    let clock = FakeClock::new();
    // An hour, the lowest the setting takes; under the twelve hours qfocus starts with, a session
    // this long is nothing worth asking about.
    let prefs = Prefs { ceiling: 3_600, ..Prefs::default() };
    let mut h = harness(app_with(&dir, &clock, &prefs), 80, 24);
    h.click_text("Rust");
    clock.pass(3_000);
    h.advance(Duration::from_secs(1));
    assert!(
        !h.app().timer().is_some_and(|screen| screen.running().flags.contains(&Flag::OverCeiling)),
        "fifty minutes is under the ceiling:\n{}",
        h.screen()
    );
    clock.pass(700);
    h.advance(Duration::from_secs(1));
    assert!(
        h.app().timer().is_some_and(|screen| screen.running().flags.contains(&Flag::OverCeiling)),
        "an hour and a minute is over the ceiling the person chose:\n{}",
        h.screen()
    );
    h.press("space").advance(Duration::from_millis(200));
    let screen = h.screen();
    assert!(screen.contains("This session lasted 1 h"), "the question names the session's length:\n{screen}");
    assert!(screen.contains("Fix the duration") && screen.contains("Leave as is"), "{screen}");
    done(&dir);
}
