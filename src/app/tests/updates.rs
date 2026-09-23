//! The Quvyta-wide update notice: qfocus asks once at start whether a newer version is out while the
//! ecosystem's switch is on, says so in the corner when one is, and the switch on the Settings page,
//! or on the wizard's last step, turns the question off for the whole ecosystem.
//!
//! Every test keeps the shared Quvyta folder and qfocus's state folder in a temporary root, so none
//! reads or turns off the person's own switch; the harness answers the question itself and never
//! reaches the network.

use super::*;

use crate::config::UpdateFolders;

use super::wizard::start_asking;

/// A version newer than any qfocus will be for a long while, and not the one the tests run.
const NEWER: &str = "9.4.7";

/// The words of the switch's row, the framework's own.
const ROW: &str = "Say when an update is out";

/// The folders of the update notice under `root`: the shared Quvyta folder is the one the appearance
/// writes to, so the switch and the question read the same file.
fn folders(root: &Path) -> UpdateFolders {
    UpdateFolders { config: root.join("config"), state: root.join("state") }
}

/// qfocus with records in `root/data`, the shared Quvyta folder in `root/config`, asking for updates
/// over `updates`, as a person starts it once the wizard is behind them.
fn started(root: &Path, clock: &FakeClock, updates: Option<UpdateFolders>) -> Harness<QFocus> {
    let config = root.join("config");
    let appearance =
        Appearance::new(Family::QUVYTA, crate::config::APP, crate::config::preferences_in(&config)).in_folder(&config);
    let store = Store::open(Paths::at(root.join("data"), "test"));
    let app = QFocus::new(store, true, Some(180), clock.reader(), Settings::in_memory(), appearance);
    harness(app.update_notice(updates), 80, 60)
}

/// Clicks the switch on the row of the update notice: a switch is colour alone and stands at the
/// right edge of the column, where the arrow of the drop-downs above it stands too.
fn click_switch(h: &mut Harness<QFocus>) {
    let (_, row) = h.find(ROW).unwrap_or_else(|| panic!("the switch is on screen:\n{}", h.screen()));
    let (edge, _) = h.find("▾").expect("a drop-down in the same column");
    h.click(edge - 1, row);
    h.advance(Duration::from_millis(100));
}

/// What the shared file says of the switch, or `None` when the file does not name it.
fn written_switch(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join("config").join("quvyta.conf")).ok()?;
    text.lines().find(|line| line.starts_with("update-notice")).map(str::to_owned)
}

#[test]
fn a_newer_version_is_asked_for_once_with_qfocus_own_name_and_said_in_the_corner() {
    let root = temp("updates-newer");
    let clock = FakeClock::new();
    let mut h = started(&root, &clock, Some(folders(&root)));
    let asked = h.update_checks().to_vec();
    assert_eq!(asked.len(), 1, "qfocus asks once at start");
    assert_eq!((asked[0].package(), asked[0].current()), ("quvyta-focus", env!("CARGO_PKG_VERSION")));
    assert!(!h.screen().contains("is out"), "nothing is said before the answer:\n{}", h.screen());

    h.set_latest_version(Some(NEWER));
    let screen = h.screen();
    assert!(screen.contains(&format!("quvyta-focus {NEWER} is out")), "the notice names the new version:\n{screen}");
    assert!(screen.contains(env!("CARGO_PKG_VERSION")), "and the one running:\n{screen}");
    assert_eq!(h.update_checks().len(), 1, "and it was asked only once");
    done(&root);

    let same = temp("updates-same");
    let mut older = started(&same, &clock, Some(folders(&same)));
    older.set_latest_version(Some("0.0.1"));
    assert!(!older.screen().contains("is out"), "an older version is not news:\n{}", older.screen());
    done(&same);
}

#[test]
fn the_switch_on_the_settings_page_turns_the_question_off_for_the_ecosystem_and_back_on() {
    let root = temp("updates-switch");
    let clock = FakeClock::new();
    let mut h = started(&root, &clock, Some(folders(&root)));
    assert_eq!(h.update_checks().len(), 1, "on until someone turns it off");
    h.press("4");
    let screen = h.screen();
    let (_, below) = screen.lines().enumerate().find(|(_, line)| line.contains("Pillar")).expect("the pillar row");
    let (_, row) = screen.lines().enumerate().find(|(_, line)| line.contains(ROW)).expect("the switch's row");
    assert!(row > below, "the switch comes right after the appearance section:\n{screen}");
    let (_, time) = screen.lines().enumerate().find(|(_, line)| line.contains("Week starts on")).expect("time");
    assert!(row < time, "and before qfocus's own rows:\n{screen}");

    click_switch(&mut h);
    assert!(!Family::QUVYTA.update_notice_in(&root.join("config")), "the shared file says off:\n{}", h.screen());
    assert_eq!(written_switch(&root).as_deref(), Some("update-notice = false"));

    let mut off = started(&root, &clock, Some(folders(&root)));
    assert!(off.update_checks().is_empty(), "a qfocus started with it off asks nothing at all");
    off.set_latest_version(Some(NEWER));
    assert!(!off.screen().contains("is out"), "{}", off.screen());

    off.press("4");
    click_switch(&mut off);
    assert!(Family::QUVYTA.update_notice_in(&root.join("config")), "turned back on:\n{}", off.screen());
    let on = started(&root, &clock, Some(folders(&root)));
    assert_eq!(on.update_checks().len(), 1, "and the next start asks again");
    done(&root);
}

#[test]
fn without_the_shared_folders_nothing_is_asked_and_no_switch_is_offered() {
    let root = temp("updates-none");
    let clock = FakeClock::new();
    let mut h = started(&root, &clock, None);
    assert!(h.update_checks().is_empty(), "nothing is asked");
    h.set_latest_version(Some(NEWER));
    assert!(!h.screen().contains("is out"), "{}", h.screen());
    h.press("4");
    let screen = h.screen();
    assert!(screen.contains("Pillar"), "the Settings page is open:\n{screen}");
    assert!(!screen.contains(ROW), "no switch that would do nothing:\n{screen}");
    done(&root);
}

#[test]
fn the_wizard_asks_nothing_while_open_and_a_switch_turned_off_on_its_last_step_is_written_on_finish() {
    let root = temp("updates-wizard-off");
    let clock = FakeClock::new();
    let mut h = start_asking(&root, &clock, 80, 40, Some(folders(&root)));
    assert!(h.app().setting_up(), "{}", h.screen());
    assert!(h.update_checks().is_empty(), "nothing is asked while the wizard is open");

    h.click_text("Next");
    h.advance(Duration::from_millis(100));
    assert!(h.screen().contains("Your day"), "{}", h.screen());
    click_switch(&mut h);
    assert!(!root.join("config").exists(), "the choice is held, nothing is written before Finish");
    assert!(h.update_checks().is_empty(), "{}", h.screen());

    h.click_text("Finish");
    h.advance(Duration::from_millis(100));
    assert!(!h.app().setting_up(), "the wizard is over:\n{}", h.screen());
    assert_eq!(written_switch(&root).as_deref(), Some("update-notice = false"), "written with the rest");
    assert!(h.update_checks().is_empty(), "turned off before it was ever asked, it is never asked");

    let again = start_asking(&root, &clock, 80, 40, Some(folders(&root)));
    assert!(again.update_checks().is_empty(), "and the next start asks nothing either");
    done(&root);
}

#[test]
fn starting_with_the_defaults_leaves_the_notice_on_writes_nothing_more_and_asks_once_it_is_over() {
    let root = temp("updates-wizard-defaults");
    let clock = FakeClock::new();
    let mut h = start_asking(&root, &clock, 80, 40, Some(folders(&root)));
    assert!(h.update_checks().is_empty(), "{}", h.screen());
    h.click_text("Start with the defaults");
    h.advance(Duration::from_millis(100));

    assert!(!h.app().setting_up(), "{}", h.screen());
    assert_eq!(written_switch(&root), None, "the default is not written");
    let asked = h.update_checks().to_vec();
    assert_eq!(asked.len(), 1, "asked once, when the wizard is over");
    assert_eq!(asked[0].package(), "quvyta-focus");
    h.set_latest_version(Some(NEWER));
    assert!(h.screen().contains(&format!("quvyta-focus {NEWER} is out")), "{}", h.screen());
    done(&root);
}
