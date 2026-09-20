//! Silence: the net time it stops, the question it asks afterwards, and the dashboard that
//! takes over a quiet screen.

use super::*;

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
