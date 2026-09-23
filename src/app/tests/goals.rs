//! Goals and countdowns: the windows they are measured over and what they say when crossed.

use super::*;

#[test]
fn g_sets_a_goal_and_the_tree_shows_a_meter_only_for_rows_that_have_one() {
    let dir = temp("goal-set");
    let (_, focus) = seeded(&dir);
    let clock = FakeClock::new();
    let mut h = harness(app_at(&dir, &clock), 80, 24);
    let before = h.screen();
    assert!(!before.contains("/1 h"), "no goal, no meter:\n{before}");
    walk_to(&mut h, Row::Focus(focus));
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
    h.type_text("0000");
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
    assert!(screen.contains("1,2/1 sa"), "{screen}");
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
    walk_to(&mut h, Row::Focus(focus));
    h.press("t");
    let screen = h.screen();
    assert!(screen.contains("Countdown for Rust"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.type_text("0000");
    h.click_text("Start");
    assert!(h.screen().contains("A countdown cannot be zero"), "{}", h.screen());
    assert!(h.app().timer().is_none());
    // Start has the keyboard now; the field is clicked on its hours before the length is typed.
    type_in_row(&mut h, "Countdown for Rust", 0, "0002");
    h.click_text("Start");
    assert_eq!(h.app().timer().map(TimerScreen::focus), Some(focus));
    assert_eq!(h.app().timer().and_then(TimerScreen::countdown), Some(120));
    h.hover(0, 0).advance(Duration::from_secs(10));
    assert!(h.screen().contains("Counting down from 2 min"), "{}", h.screen());
    clock.pass(120);
    h.advance(Duration::from_secs(1));
    let screen = h.screen();
    assert!(screen.contains("2 min are up"), "{screen}");
    assert!(h.app().timer().is_some(), "it does not stop by itself");
    h.hover(0, 0).advance(Duration::from_secs(10));
    assert!(h.screen().contains("target 2 min · 2 min"), "{}", h.screen());
    clock.pass(30);
    h.advance(Duration::from_secs(1));
    assert!(h.screen().contains("target 2 min · 2 min 30 s"), "{}", h.screen());
    assert_eq!(forbidden(&h.screen()), None, "{}", h.screen());
    // A terminal in ASCII glyphs has no `·`, so the line joins its parts with a plain dash.
    h.set_glyph_mode(GlyphMode::Ascii);
    assert!(h.screen().contains("target 2 min - 2 min 30 s"), "{}", h.screen());
    h.set_glyph_mode(GlyphMode::Unicode);
    h.press("space");
    assert!(h.screen().contains("2 min 30 s recorded"), "{}", h.screen());
    drop(h);
    // The setting stops the counter when the countdown is up.
    let prefs = Prefs { stop_at_goal: true, ..Prefs::default() };
    let mut h = harness(app_with(&dir, &clock, &prefs), 80, 24);
    walk_to(&mut h, Row::Focus(focus));
    h.press("t");
    h.type_text("0001");
    h.click_text("Start");
    clock.pass(60);
    h.advance(Duration::from_secs(1));
    assert!(h.app().timer().is_none(), "{}", h.screen());
    assert!(h.screen().contains("1 min recorded"), "{}", h.screen());
    drop(h);
    // At forty columns the countdown field stacks and stays clean, in Turkish and ASCII too.
    let mut h = harness(app_at(&dir, &clock), 40, 24);
    h.set_locale("tr").set_glyph_mode(GlyphMode::Ascii);
    walk_to(&mut h, Row::Focus(focus));
    h.press("t");
    let screen = h.screen();
    assert!(screen.contains("Rust için geri sayım"), "{screen}");
    assert!(screen.contains("Başlat"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.press("esc");
    assert!(h.app().today().timed().is_none());
    done(&dir);
}

#[test]
fn the_suggested_goal_is_the_one_the_person_set_not_the_one_qfocus_starts_with() {
    let dir = temp("goal-suggestion");
    let (_, focus) = seeded(&dir);
    let clock = FakeClock::new();
    // An hour and a half instead of the hour qfocus starts with.
    let prefs = Prefs { default_goal: 5_400, ..Prefs::default() };
    let mut h = harness(app_with(&dir, &clock, &prefs), 80, 24);
    walk_to(&mut h, Row::Focus(focus));
    h.press("g");
    let screen = h.screen();
    assert!(screen.contains("Goal for Rust"), "{screen}");
    assert!(
        h.app().today().goal_edit().is_some_and(|edit| edit.amount == 5_400),
        "the field opens on the suggestion the person set:\n{screen}"
    );
    h.press("esc");
    // A countdown starts from the same suggestion.
    h.press("t");
    assert!(
        h.app().today().timed().is_some_and(|timed| timed.amount == 5_400),
        "and so does a countdown:\n{}",
        h.screen()
    );
    done(&dir);
}
