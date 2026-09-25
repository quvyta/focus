//! The settings page, from the rollover and the region down to the danger zone.

use super::*;

use crate::ui::settings;
use qframe::date::TimeOfDay;

/// The application over `dir` with a settings file in it, so writes can be read back.
fn app_on_file(dir: &Path, clock: &FakeClock) -> (QFocus, PathBuf) {
    let path = dir.join("focus.conf");
    fs::create_dir_all(dir).expect("folder");
    let settings = Settings::open(&path).member_of(&Ecosystem::QUVYTA).schema(Prefs::schema()).self_heal(true);
    (QFocus::new(Store::open(Paths::at(dir, "test")), true, Some(180), clock.reader(), settings, appearance(dir)), path)
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
    let mut h = harness(app_at(&dir, &clock), 80, 44);
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
        "Delete records",
        "Delete older",
        "Delete everything",
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
    // At forty columns each held control reads whole; the group's line says to hold them.
    for text in ["Toplu işlemler", "basılı tutunca", "Kayıtları sil", "Eskileri sil", "Her şeyi sil"] {
        assert!(screen.contains(text), "{text} missing:\n{screen}");
    }
    assert_eq!(forbidden(&screen), None, "{screen}");
    done(&dir);
}

#[test]
fn old_settings_files_left_behind_are_reported_on_the_settings_page_until_read() {
    let dir = temp("left-behind");
    seeded(&dir);
    let clock = FakeClock::new();
    let file = qframe::diagnostics::Diagnostic::warning(
        None,
        "/cfg/quvyta/focus/settings.toml: /cfg/quvyta/focus.conf already exists; this file stays and nothing is merged",
    );
    let mut h = harness(app_at(&dir, &clock).with_left_behind(vec![file]), 100, 44);
    h.press("4");
    let screen = h.screen();
    assert!(screen.contains("Some old settings files stayed where they were"), "{screen}");
    assert!(screen.contains("focus.conf already exists"), "{screen}");
    assert!(!screen.contains("The settings file was repaired"), "{screen}");
    let (x, y) = h.find("Read").expect("the dismiss button");
    h.click(x, y);
    let screen = h.screen();
    assert!(!screen.contains("Some old settings files stayed where they were"), "{screen}");
    assert!(screen.contains("Week starts on"), "{screen}");
    done(&dir);
}

#[test]
fn the_keys_walk_the_settings_and_a_switch_moves_with_space() {
    let dir = temp("settings-keys");
    seeded(&dir);
    let clock = FakeClock::new();
    let mut h = harness(app_at(&dir, &clock), 80, 40);
    h.press("4");
    // Reduce motion is the seventh row, under the three shared rows and the box each of them
    // carries; the stop-at-goal switch is six rows below it.
    for _ in 0..6 {
        h.press("down");
    }
    // The harness runs with reduced motion on, so the first press turns it off. Reduce motion
    // is shared now: with its box checked the change goes to the ecosystem's file, not qfocus's.
    let shared = dir.join("quvyta.conf");
    h.press("space");
    assert!(!h.env().reduced_motion());
    let written = fs::read_to_string(&shared).expect("the shared file");
    assert!(written.contains("reduced-motion = false"), "{written}");
    h.press("space");
    assert!(h.env().reduced_motion());
    let written = fs::read_to_string(&shared).expect("the shared file");
    assert!(written.contains("reduced-motion = true"), "{written}");
    assert_eq!(h.app().settings().reduced_motion(), None, "qfocus's own file keeps no value of its own");
    // The stop-at-goal switch is seven rows below, past reduce motion's own box.
    for _ in 0..7 {
        h.press("down");
    }
    h.press("space");
    assert!(h.app().prefs().stop_at_goal, "{}", h.screen());
    h.press("space");
    assert!(!h.app().prefs().stop_at_goal);
    // The wheel over the minutes of the idle threshold applies each length it passes; one under
    // the floor is not applied, the reason stands under the label, and turning back up applies it.
    roll(&mut h, "Away after", 1, -14);
    assert_eq!(h.app().prefs().idle_after, Duration::from_secs(60));
    assert!(!h.screen().contains("At least"), "{}", h.screen());
    roll(&mut h, "Away after", 1, -1);
    assert_eq!(h.app().prefs().idle_after, Duration::from_secs(60));
    assert!(h.screen().contains("At least 1 min"), "{}", h.screen());
    roll(&mut h, "Away after", 1, 10);
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
    h.press("4");
    roll(&mut h, "Day turns at", 0, 4);
    assert!(h.screen().lines().next().is_some_and(|top| top.ends_with("0 s")), "{}", h.screen());
    assert_eq!(h.app().prefs().rollover, TimeOfDay::new(4, 0, 0));
    let written = fs::read_to_string(&path).expect("settings written");
    assert_eq!(written, "day-rollover = \"04:00\"\n");
    // The record itself is untouched.
    let month = fs::read_to_string(h.app().store().paths.month_file(2026, 9)).expect("month");
    assert_eq!(month.lines().count(), 1);
    roll(&mut h, "Day turns at", 0, -4);
    assert!(h.screen().lines().next().is_some_and(|top| top.ends_with("30 min")), "{}", h.screen());
    assert_eq!(fs::read_to_string(&path).expect("settings written"), "", "a default is not written");
    // The week window follows the first day of the week: the language's until one is chosen.
    assert_eq!(h.app().prefs().week_start, None);
    // The digit keys would go to the time field the wheel left selected; the tab is clicked.
    h.click_text("Charts").click_text("Week");
    let screen = h.screen();
    let labels = screen.lines().find(|line| line.contains("Sun")).unwrap_or_default();
    assert!(
        labels.trim_start().starts_with("Sun"),
        "an English week runs from Sunday, as the calendar's does:\n{screen}"
    );
    h.press("4");
    pick(&mut h, "Week starts on", "Saturday");
    assert_eq!(h.app().prefs().week_start, Some(Weekday::Saturday));
    h.click_text("Charts");
    let screen = h.screen();
    let labels = screen.lines().find(|line| line.contains("Sun")).unwrap_or_default();
    assert!(labels.trim_start().starts_with("Sat"), "the week now runs from Saturday:\n{screen}");
    // A shared setting is applied at once and, since the box under the row is checked, written
    // in the shared file; qfocus's own file says it follows the ecosystem.
    h.press("4");
    pick(&mut h, "Language", "Türkçe");
    assert!(h.screen().contains("Grafikler"), "{}", h.screen());
    let written = fs::read_to_string(&path).expect("settings written");
    assert!(written.contains("language = \"quvyta\""), "{written}");
    assert!(written.contains("week-start = \"saturday\""), "{written}");
    let shared = fs::read_to_string(dir.join("quvyta.conf")).expect("the shared file");
    assert!(shared.contains("language = \"tr\""), "{shared}");
    // Choosing the language's own first day unpins the week again.
    pick(&mut h, "Hafta başı", "Pazartesi");
    assert_eq!(h.app().prefs().week_start, None, "Monday is where a Turkish week starts anyway");
    let written = fs::read_to_string(&path).expect("settings written");
    assert!(!written.contains("week-start"), "{written}");
    done(&dir);
}

#[test]
fn the_look_goes_to_the_ecosystem_while_the_box_is_checked_and_to_qfocus_alone_once_it_is_cleared() {
    let dir = temp("appearance-scope");
    seeded(&dir);
    let clock = FakeClock::new();
    let (app, path) = app_on_file(&dir, &clock);
    let mut h = harness(app, 100, 40);
    h.press("4");
    let screen = h.screen();
    assert!(screen.contains("In every Quvyta application"), "{screen}");
    let shared = dir.join("quvyta.conf");

    // The box is checked on a fresh machine, so the theme goes to the ecosystem and qfocus's own
    // file says it follows.
    pick(&mut h, "Theme", "Nordic");
    assert_eq!(h.env().theme().id(), "nordic", "{}", h.screen());
    assert!(fs::read_to_string(&shared).expect("the shared file").contains("theme = \"nordic\""));
    assert!(fs::read_to_string(&path).expect("qfocus's file").contains("theme = \"quvyta\""));

    // With the box cleared the next theme stays here; the ecosystem keeps the one it had.
    // A click on its label makes it the list's row, and Space moves the box.
    let (row, _) = row_of(&h, "Theme");
    let x = word_on_line(&h, row + 1, "In", 0).unwrap_or_else(|| panic!("the theme's box:\n{}", h.screen()));
    h.click(x, row + 1).press("space");
    h.advance(Duration::from_millis(100));
    pick(&mut h, "Theme", "Amber");
    assert_eq!(h.env().theme().id(), "amber", "{}", h.screen());
    let written = fs::read_to_string(&path).expect("qfocus's file");
    assert!(written.contains("theme = \"amber\""), "{written}");
    let shared_file = fs::read_to_string(&shared).expect("the shared file");
    assert!(shared_file.contains("theme = \"nordic\""), "{shared_file}");
    done(&dir);
}

/// The words of the screen's line that holds `label`.
fn row_words(screen: &str, label: &str) -> Vec<String> {
    let line = screen.lines().find(|line| line.contains(label)).unwrap_or_default();
    line.split_whitespace().map(str::to_owned).collect()
}

#[test]
fn the_region_sets_where_the_week_starts_and_without_one_the_language_does() {
    let dir = temp("week-region");
    let (_, focus) = seeded(&dir);
    // Fifty minutes on Sunday, five days before this Friday, and five minutes today.
    recorded(&dir, 30, focus, 5 * 86_400, vec![Span::new(SpanKind::Work, 0, 3_000, ClockSource::Mono)], "");
    recorded(&dir, 31, focus, 3_600, vec![Span::new(SpanKind::Work, 0, 300, ClockSource::Mono)], "");
    let clock = FakeClock::new();
    // Language, region, the first day's full and short names, and the settings row's label.
    let cases = [
        ("en", None, "Sunday", "Sun", "Week starts on"),
        ("tr", None, "Pazartesi", "Pzt", "Hafta başı"),
        ("tr", Some("US"), "Pazar", "Paz", "Hafta başı"),
        ("en", Some("GB"), "Monday", "Mon", "Week starts on"),
    ];
    for (language, region, day, short, label) in cases {
        let mut h = harness(app_at(&dir, &clock), 100, 60);
        h.set_locale(language).set_region(region);
        h.press("4");
        let screen = h.screen();
        assert!(
            row_words(&screen, label).iter().any(|word| word == day),
            "{language} in {region:?} starts the week on {day}:\n{screen}"
        );
        h.press("2").click_text(if language == "en" { "Week" } else { "Hafta" });
        let screen = h.screen();
        let labels = screen.lines().find(|line| line.contains(short) && line.split_whitespace().count() == 7);
        assert!(
            labels.is_some_and(|line| line.trim_start().starts_with(short)),
            "the week chart starts on {short} for {language} in {region:?}:\n{screen}"
        );
    }
    done(&dir);
}

#[test]
fn a_weekly_goal_is_measured_over_the_regions_week() {
    let dir = temp("week-region-goal");
    let (_, focus) = seeded(&dir);
    // Fifty minutes on Sunday and five today: fifty-five of a sixty-minute week goal where
    // the week starts on Sunday, five where it starts on Monday.
    recorded(&dir, 30, focus, 5 * 86_400, vec![Span::new(SpanKind::Work, 0, 3_000, ClockSource::Mono)], "");
    recorded(&dir, 31, focus, 3_600, vec![Span::new(SpanKind::Work, 0, 300, ClockSource::Mono)], "");
    let store = Store::open(Paths::at(&dir, "test"));
    let mut tree = store.tree.clone();
    tree.categories[0].focuses[0].goal = Some(Goal { amount: 3_600, period: Period::Week });
    fs::write(store.paths.tree_file(), tree.write()).expect("tree written");
    drop(store);
    let prefs = Prefs { stop_at_goal: true, ..Prefs::default() };
    for (region, stops) in [(Some("US"), true), (Some("GB"), false)] {
        let clock = FakeClock::new();
        let mut h = harness(app_with(&dir, &clock, &prefs), 80, 24);
        h.set_locale("tr").set_region(region);
        h.click_text("Rust");
        assert!(h.app().timer().is_some(), "{}", h.screen());
        clock.pass(400);
        h.advance(Duration::from_secs(1));
        assert_eq!(h.app().timer().is_none(), stops, "the goal is crossed in {region:?}:\n{}", h.screen());
        if !stops {
            h.press("space");
        }
        drop(h);
    }
    done(&dir);
}

#[test]
fn choosing_the_regions_own_first_day_leaves_the_week_unpinned() {
    let dir = temp("week-region-unpinned");
    let clock = FakeClock::new();
    let (app, path) = app_on_file(&dir, &clock);
    let mut h = harness(app, 80, 24);
    h.set_locale("tr").set_region(Some("US"));
    h.press("4");
    pick(&mut h, "Hafta başı", "Pazartesi");
    assert_eq!(h.app().prefs().week_start, Some(Weekday::Monday), "Monday is not where this week starts");
    pick(&mut h, "Hafta başı", "Pazar");
    assert_eq!(h.app().prefs().week_start, None, "Sunday is where the region starts it anyway");
    let written = fs::read_to_string(&path).unwrap_or_default();
    assert!(!written.contains("week-start"), "{written}");
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
    hold(&mut h, "Delete records");
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
    hold(&mut h, "Delete records");
    assert!(h.app().store().sessions.is_empty());
    h.press("ctrl+z");
    assert_eq!(h.app().store().sessions.len(), 3);
    hold(&mut h, "Delete records");
    hold(&mut h, "Delete records");
    assert!(h.screen().contains("no records to delete"), "{}", h.screen());
    done(&dir);
}

#[test]
fn holding_delete_empties_the_lists_too_and_ctrl_z_brings_everything_back() {
    let dir = temp("settings-delete");
    let (rust, reading, review) = seeded_two(&dir);
    two_sessions(&dir, rust);
    let clock = FakeClock::new();
    let mut h = harness(app_at(&dir, &clock), 80, 44);
    h.press("4");
    hold(&mut h, "Delete everything");
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
    hold(&mut h, "Delete older");
    assert!(h.screen().contains("No records began before"), "{}", h.screen());
    assert_eq!(h.app().store().sessions.len(), 7);
    // Tuesday the 15th: the 14th, the 10th and August go; the 15th itself stays.
    // The date field opens its calendar on a click; a year on, then three days back, and Enter.
    h.click_text("September 18, 2025");
    h.advance(Duration::from_millis(100));
    h.press("shift+pgdn").press("left").press("left").press("left").press("enter");
    assert!(h.screen().contains("September 15, 2026"), "{}", h.screen());
    hold(&mut h, "Delete older");
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
    hold(&mut h, "Delete records");
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
    assert!(screen.contains("Çöp tamamen boşaltılsın mı?"), "{screen}");
    assert!(screen.contains("2 kayıt ve 1 arşivli satır diskten"), "{screen}");
    assert!(screen.contains("Kalıcı olarak sil"), "{screen}");
    assert!(screen.contains("Onaylamak için sil yaz"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.press("esc");
    assert!(h.screen().contains("Çöpü boşaltmaktan vazgeçildi."), "{}", h.screen());
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
    hold(&mut h, "Delete records");
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
    let mut h = harness(app_at(&dir, &clock), 80, 60);
    h.press("4");
    let month = fs::read(first.paths.month_file(2026, 9)).expect("month");
    // The controls are muted: holding them and clicking the trash's button do nothing at all.
    for label in ["Delete records", "Delete older", "Delete everything"] {
        hold(&mut h, label);
    }
    h.click_text("Empty the trash");
    h.advance(Duration::from_millis(200));
    assert_eq!(h.app().store().sessions.len(), 1, "{}", h.screen());
    assert!(!h.screen().contains("Empty the trash for good?"), "{}", h.screen());
    assert!(!h.screen().contains("This qfocus only looks"), "nothing even reached the application:\n{}", h.screen());
    // Under them the application refuses as well, should anything ever reach it: the messages the
    // muted controls would send are sent here directly, since no person can send them any more.
    h.send(Msg::Settings(settings::Msg::ResetStats));
    assert!(h.screen().contains("This qfocus only looks"), "{}", h.screen());
    assert_eq!(h.app().store().sessions.len(), 1);
    h.send(Msg::Settings(settings::Msg::EmptyTrash));
    assert!(!h.screen().contains("Empty the trash for good?"), "{}", h.screen());
    h.send(Msg::Settings(settings::Msg::Purge));
    assert!(h.screen().contains("This qfocus only looks"), "{}", h.screen());
    assert_eq!(h.app().store().voided.len(), 1);
    assert_eq!(fs::read(first.paths.month_file(2026, 9)).expect("month"), month);
    assert!(first.paths.trash_file(2026, 9).exists());
    drop(first);
    done(&dir);
}

#[test]
fn emptying_the_trash_keeps_the_archived_focus_a_counter_left_behind_still_runs_on() {
    let dir = temp("purge-keeps-the-counters-focus");
    let (rust, _, review) = seeded_two(&dir);
    // Every record of the archived focus is gone from the disk; only the counter still on it says
    // the person is using it. Emptying the trash must not take the row out from under them.
    let session = recorded(&dir, 30, rust, 3_600, vec![Span::new(SpanKind::Work, 0, 600, ClockSource::Mono)], "");
    let mut store = Store::open(Paths::at(&dir, "test"));
    store.void(session.id, NOON).expect("void");
    drop(store);
    super::recover::left_running(&dir, review, NOON - 3_600, Watch::None);

    let clock = FakeClock::new();
    let mut h = harness(app_at(&dir, &clock), 80, 60);
    assert!(
        h.app().timer().is_some_and(|screen| screen.running().focus == review),
        "the counter left behind carries on:\n{}",
        h.screen()
    );
    h.press("4");
    h.click_text("Empty the trash");
    h.advance(Duration::from_millis(200));
    h.type_text("delete").press("enter");

    let screen = h.screen();
    assert!(screen.contains("1 archived row kept: records use it."), "{screen}");
    assert!(
        h.app().store().tree.focus(review).is_some(),
        "the focus the counter runs on is still in the tree:\n{screen}"
    );
    let saved = fs::read_to_string(h.app().store().paths.tree_file()).expect("tree");
    assert!(saved.contains("Review"), "and in the file it was written to:\n{saved}");
    assert!(h.app().timer().is_some(), "the counter itself is untouched:\n{screen}");
    done(&dir);
}

/// Walks down the settings list until the row labelled `label` has the keyboard, as a person does.
fn down_to_setting(h: &mut Harness<QFocus>, label: &str) {
    for _ in 0..20 {
        if h.screen().lines().any(|line| line.trim_start().starts_with('▌') && line.contains(label)) {
            return;
        }
        h.press("down");
    }
    panic!("Down never reaches the `{label}` row:\n{}", h.screen());
}

#[test]
fn the_day_turns_at_a_time_typed_from_the_keyboard_and_the_file_follows() {
    let dir = temp("settings-typed-rollover");
    seeded(&dir);
    let clock = FakeClock::new();
    let (app, path) = app_on_file(&dir, &clock);
    let mut h = harness(app, 80, 40);
    h.press("4");
    down_to_setting(&mut h, "Day turns at");
    h.type_text("0530");
    h.press("down");
    assert_eq!(h.app().prefs().rollover, TimeOfDay::new(5, 30, 0), "{}", h.screen());
    let written = fs::read_to_string(&path).expect("settings written");
    assert!(written.contains("day-rollover = \"05:30\""), "{written}");
    let screen = h.screen();
    let row = screen.lines().find(|line| line.contains("Day turns at")).unwrap_or_default();
    assert!(row.contains("05") && row.contains("30"), "the row shows the typed time:\n{screen}");
    done(&dir);
}

#[test]
fn away_after_keeps_a_typed_length_and_refuses_one_under_the_floor() {
    let dir = temp("settings-typed-away");
    seeded(&dir);
    let clock = FakeClock::new();
    let (app, path) = app_on_file(&dir, &clock);
    let mut h = harness(app, 80, 40);
    h.press("4");
    down_to_setting(&mut h, "Away after");
    h.press("right").type_text("07");
    h.press("down");
    assert_eq!(h.app().prefs().idle_after, Duration::from_secs(7 * 60), "{}", h.screen());
    let written = fs::read_to_string(&path).expect("settings written");
    assert!(written.contains("idle-after = 420"), "{written}");
    let screen = h.screen();
    let row = screen.lines().find(|line| line.contains("Away after")).unwrap_or_default();
    assert!(row.contains("0 h 07 min"), "the row shows the typed length:\n{screen}");

    h.press("up").type_text("0000");
    h.press("down");
    assert_eq!(h.app().prefs().idle_after, Duration::from_secs(7 * 60), "a value under the floor is not applied");
    let written = fs::read_to_string(&path).expect("settings written");
    assert!(written.contains("idle-after = 420"), "{written}");
    let screen = h.screen();
    assert!(screen.contains("At least 1 min"), "the row says why it refused the length:\n{screen}");
    let row = screen.lines().find(|line| line.contains("Away after")).unwrap_or_default();
    assert!(row.contains("0 h 00 min"), "the refused length stays in the field:\n{screen}");
    done(&dir);
}

#[test]
fn the_session_ceiling_keeps_a_length_typed_from_the_keyboard() {
    let dir = temp("settings-typed-ceiling");
    seeded(&dir);
    let clock = FakeClock::new();
    let (app, path) = app_on_file(&dir, &clock);
    let mut h = harness(app, 80, 40);
    h.press("4");
    down_to_setting(&mut h, "Session ceiling");
    h.type_text("0230");
    h.press("down");
    assert_eq!(h.app().prefs().ceiling, 2 * 3_600 + 30 * 60, "{}", h.screen());
    let written = fs::read_to_string(&path).expect("settings written");
    assert!(written.contains("ceiling = 9000"), "{written}");
    let screen = h.screen();
    let row = screen.lines().find(|line| line.contains("Session ceiling")).unwrap_or_default();
    assert!(row.contains("2 h 30 min"), "the row shows the typed length:\n{screen}");
    done(&dir);
}

#[test]
fn a_suggested_goal_typed_on_its_row_is_what_the_goal_field_starts_from() {
    let dir = temp("settings-typed-suggested-goal");
    let (_, focus) = seeded(&dir);
    let clock = FakeClock::new();
    let (app, path) = app_on_file(&dir, &clock);
    let mut h = harness(app, 80, 40);
    h.press("4");
    down_to_setting(&mut h, "Suggested goal");
    h.type_text("0145");
    h.press("tab");
    assert_eq!(h.app().prefs().default_goal, 6_300, "{}", h.screen());
    let written = fs::read_to_string(&path).expect("settings written");
    assert!(written.contains("default-goal = 6300"), "{written}");
    let screen = h.screen();
    let row = screen.lines().find(|line| line.contains("Suggested goal")).unwrap_or_default();
    assert!(row.contains("1 h 45 min"), "the row shows the typed length:\n{screen}");

    h.press("1");
    tab_to(&mut h, today::TREE);
    walk_to(&mut h, Row::Focus(focus));
    h.press("g");
    assert!(h.app().today().goal_edit().is_some_and(|edit| edit.amount == 6_300), "{}", h.screen());
    assert!(h.screen().contains("1 h 45 min"), "{}", h.screen());
    done(&dir);
}

#[test]
fn a_time_and_a_length_are_typed_into_their_rows_from_the_keyboard() {
    let dir = temp("settings-typed");
    seeded(&dir);
    let clock = FakeClock::new();
    let mut h = harness(app_at(&dir, &clock), 80, 40);
    h.press("4");
    type_in_row(&mut h, "Day turns at", 0, "0415");
    assert_eq!(h.app().prefs().rollover, TimeOfDay::new(4, 15, 0), "{}", h.screen());
    // The minutes of the idle threshold, clicked and typed, are minutes.
    type_in_row(&mut h, "Away after", 1, "05");
    assert_eq!(h.app().prefs().idle_after, Duration::from_secs(300), "{}", h.screen());
    type_in_row(&mut h, "Session ceiling", 0, "06");
    assert_eq!(h.app().prefs().ceiling, 6 * 3_600, "{}", h.screen());
    done(&dir);
}

#[test]
fn the_suggested_goal_set_on_its_row_is_what_the_goal_field_starts_from() {
    let dir = temp("settings-suggested-goal");
    let (_, focus) = seeded(&dir);
    let clock = FakeClock::new();
    let mut h = harness(app_at(&dir, &clock), 80, 40);
    h.press("4");
    // Half an hour more than the hour qfocus starts with, turned on the row's minutes.
    roll(&mut h, "Suggested goal", 1, 30);
    assert_eq!(h.app().prefs().default_goal, 5_400, "{}", h.screen());
    // The digit keys would go to the field the wheel left selected; the tab is clicked.
    h.click_text("Today");
    tab_to(&mut h, today::TREE);
    walk_to(&mut h, Row::Focus(focus));
    h.press("g");
    assert!(h.app().today().goal_edit().is_some_and(|edit| edit.amount == 5_400), "{}", h.screen());
    assert!(h.screen().contains("1 h 30 min"), "{}", h.screen());
    done(&dir);
}
