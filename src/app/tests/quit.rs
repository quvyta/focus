//! Leaving: the question a running counter asks, and what a hangup or a terminate leaves behind.

use super::*;

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
