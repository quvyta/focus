//! A counter the program did not see end: the running file, the dialog it opens, and the
//! broken file that must never be overwritten.

use super::*;

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
    assert_eq!(h.app().store().running().and_then(Result::ok).map(|found| found.flags), Some(vec![Flag::SuspectClock]));
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
pub(super) fn left_running(dir: &Path, focus: Id, started: i64, watch: Watch) {
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
fn a_goal_the_regions_week_had_already_filled_when_a_counter_was_taken_over_is_not_news() {
    let dir = temp("adopt-week");
    let (_, focus) = seeded(&dir);
    // Fifty minutes on Sunday and an hour on the counter left running: a hundred minutes of
    // a week goal of a hundred where the week starts on Sunday, sixty where it starts on
    // Monday. The counter is taken over under the machine's language and region, measured
    // over a Monday week in Britain, then the region moves to the United States while it
    // runs: the goal that move fills is not a crossing.
    recorded(&dir, 30, focus, 5 * 86_400, vec![Span::new(SpanKind::Work, 0, 3_000, ClockSource::Mono)], "");
    let store = Store::open(Paths::at(&dir, "test"));
    let mut tree = store.tree.clone();
    tree.categories[0].focuses[0].goal = Some(Goal { amount: 6_000, period: Period::Week });
    fs::write(store.paths.tree_file(), tree.write()).expect("tree written");
    drop(store);
    left_running(&dir, focus, NOON - 3_600, Watch::None);
    let clock = FakeClock::new();
    let prefs = Prefs { stop_at_goal: true, ..Prefs::default() };
    let mut h = harness(app_with(&dir, &clock, &prefs).knows_boot(true), 80, 24);
    h.set_locale("tr").set_region(Some("GB"));
    assert!(h.app().timer().is_some(), "{}", h.screen());
    clock.pass(1);
    h.advance(Duration::from_secs(1));
    assert!(h.app().timer().is_some(), "sixty-one minutes of a hundred:\n{}", h.screen());
    h.set_region(Some("US"));
    clock.pass(60);
    h.advance(Duration::from_secs(1));
    assert!(h.app().timer().is_some(), "the new week filled the goal, so nothing stops:\n{}", h.screen());
    h.press("space");
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
    walk_to(&mut h, Row::Focus(focus));
    h.press("t");
    h.type_text("0010");
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
    // The dialog wraps the path wherever the line ends, which depends on how long the
    // temporary folder's name is; read the dialog's text as one run to find it.
    let dialog: String = screen
        .lines()
        .filter_map(|line| line.rsplit('▌').next().filter(|_| line.contains('▌')))
        .map(str::trim)
        .collect();
    assert!(dialog.contains(ASIDE), "{screen}");
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
