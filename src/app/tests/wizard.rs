//! The first start: the wizard opens while qfocus has no settings file of its own, writes
//! nothing at all until it finishes, and then writes both files in one go.
//!
//! Every test lives in a temporary root: the shared Quvyta folder, the records and the fonts the
//! appearance step looks at are all inside it, so neither the person's settings nor a real font
//! is ever touched.

use super::*;

use qframe::date::TimeOfDay;
use qframe::icons::nerd_font::Install;
use qframe::widgets::Setup;

/// qfocus on the machine in `root`, on a screen of `width` by `height`, built the way
/// [`crate::app::run`] builds it: the wizard when there is no settings file, and nothing at all
/// once there is one.
pub(super) fn start(root: &Path, clock: &FakeClock, width: u16, height: u16) -> Harness<QFocus> {
    start_asking(root, clock, width, height, None)
}

/// [`start`], asking for updates over `updates` as a person's qfocus does.
pub(super) fn start_asking(
    root: &Path,
    clock: &FakeClock,
    width: u16,
    height: u16,
    updates: Option<crate::config::UpdateFolders>,
) -> Harness<QFocus> {
    let config = root.join("config");
    let fonts = root.join("fonts");
    let i18n = crate::config::spoken();
    // No real font folder is looked at, and nothing would be installed or registered.
    let setup = Setup::new_in(&config, Ecosystem::QUVYTA, crate::config::APP, &i18n, Msg::Setup)
        .on_finish(Msg::SetUp)
        .install(Install::new().target(fonts.join("QuvytaNerdFont")).register(false))
        .font_dirs(vec![fonts]);
    let setup = Some(setup).filter(Setup::needed);
    // While the wizard is open the preferences come from it: resolving them the usual way would
    // make the shared file before anything had been chosen.
    let preferences = match &setup {
        Some(setup) => setup.preferences().clone(),
        None => crate::config::preferences_in(&config),
    };
    let appearance = Appearance::new(Ecosystem::QUVYTA, crate::config::APP, preferences).in_folder(&config);
    let settings =
        Settings::open(config.join("focus.conf")).member_of(&Ecosystem::QUVYTA).schema(Prefs::schema()).self_heal(true);
    let store = Store::open(Paths::at(root.join("data"), "test"));
    let mut app = QFocus::new(store, true, Some(180), clock.reader(), settings, appearance);
    if let Some(setup) = setup {
        app = app.with_setup(setup, Some(config));
    }
    harness(app.update_notice(updates), width, height)
}

/// What the shared Quvyta folder holds, by name, in order; empty when there is no folder at all.
fn names(root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(root.join("config")) else { return Vec::new() };
    let mut names: Vec<String> =
        entries.map(|entry| entry.expect("entry").file_name().to_string_lossy().into()).collect();
    names.sort();
    names
}

/// The settings file as it stands on disk, read back through the schema.
fn written(root: &Path) -> Settings {
    let path = root.join("config").join("focus.conf");
    let text = fs::read_to_string(&path).expect("focus.conf is written");
    Settings::parse_str("focus.conf", &text).member_of(&Ecosystem::QUVYTA).schema(Prefs::schema())
}

/// Chooses the Nordic theme on the appearance step, with the pointer.
fn choose_nordic(h: &mut Harness<QFocus>) {
    pick(h, "Theme", "Nordic");
}

/// Goes on to qfocus's own step with Next and chooses four settings there, none of them the
/// default, with the pointer: the times and lengths with the wheel, since the keyboard cannot type
/// them yet (see the ignored test on the Settings page).
fn choose(h: &mut Harness<QFocus>) {
    h.click_text("Next");
    h.advance(Duration::from_millis(100));
    // An English calendar starts the week on Sunday, so Saturday is a day nobody follows by
    // itself: choosing it pins the week.
    pick(h, "Week starts on", "Saturday");
    roll(h, "Day turns at", 0, 4);
    roll(h, "Away after", 1, -10);
    roll(h, "Session ceiling", 0, -6);
}

/// Clicks Finish, the wizard's last button.
fn finish(h: &mut Harness<QFocus>) {
    h.click_text("Finish");
    h.advance(Duration::from_millis(100));
}

/// What [`choose`] chose.
fn chosen() -> Prefs {
    Prefs {
        week_start: Some(Weekday::Saturday),
        rollover: TimeOfDay::new(4, 0, 0),
        idle_after: Duration::from_secs(5 * 60),
        ceiling: 6 * 3_600,
        ..Prefs::default()
    }
}

#[test]
fn it_opens_while_qfocus_has_no_settings_file_and_steps_aside_once_there_is_one() {
    let root = temp("wizard-first-start");
    let clock = FakeClock::new();
    let h = start(&root, &clock, 80, 30);
    assert!(h.app().setting_up(), "the first start asks:\n{}", h.screen());
    let screen = h.screen();
    for text in ["qfocus", "Appearance", "Your day", "In every Quvyta application", "Start with the defaults"] {
        assert!(screen.contains(text), "`{text}` is missing:\n{screen}");
    }
    assert!(!screen.contains("Today"), "no page of qfocus's own is drawn under it:\n{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");

    // Someone who has used qfocus before keeps their file and is never asked.
    fs::create_dir_all(root.join("config")).expect("folder");
    fs::write(root.join("config").join("focus.conf"), "day-rollover = \"03:00\"\n").expect("settings");
    let again = start(&root, &clock, 80, 30);
    assert!(!again.app().setting_up(), "{}", again.screen());
    assert_eq!(again.app().prefs().rollover, TimeOfDay::new(3, 0, 0));
    assert!(again.screen().contains("Today"), "the normal screen opens at once:\n{}", again.screen());
    done(&root);
}

#[test]
fn half_way_through_the_wizard_nothing_has_been_written_at_all() {
    let root = temp("wizard-half-way");
    let clock = FakeClock::new();
    let mut h = start(&root, &clock, 80, 30);
    let config = root.join("config");
    assert_eq!(names(&root), Vec::<String>::new(), "nothing is written before it is asked");

    choose_nordic(&mut h);
    choose(&mut h);
    h.advance(Duration::from_millis(100));

    assert_eq!(h.app().prefs(), &chosen(), "the choices are held in memory");
    assert!(!config.exists(), "the shared Quvyta folder is not even made");
    assert!(!config.join("focus.conf").exists(), "qfocus's own file is not made");
    assert!(!config.join("quvyta.conf").exists(), "the shared file is not made");

    // Next start: the wizard is there again, with nothing remembered.
    let again = start(&root, &clock, 80, 30);
    assert!(again.app().setting_up(), "{}", again.screen());
    assert_eq!(again.app().prefs(), &Prefs::default(), "nothing was remembered");
    done(&root);
}

#[test]
fn finishing_writes_both_files_and_qfocus_own_settings_are_the_ones_chosen() {
    let root = temp("wizard-finish");
    let clock = FakeClock::new();
    let mut h = start(&root, &clock, 80, 30);
    choose_nordic(&mut h);
    choose(&mut h);
    finish(&mut h);

    assert!(!h.app().setting_up(), "the wizard is over:\n{}", h.screen());
    assert_eq!(names(&root), ["focus.conf", "quvyta.conf"]);
    let back = written(&root);
    assert!(back.diagnostics().is_empty(), "{:?}", back.diagnostics());
    assert_eq!(Prefs::from_settings(&back), chosen(), "the four settings travelled into the file");
    let text = fs::read_to_string(root.join("config").join("focus.conf")).expect("qfocus's file");
    for line in ["week-start = \"saturday\"", "day-rollover = \"04:00\"", "idle-after = 300", "ceiling = 21600"] {
        assert!(text.contains(line), "`{line}` is missing:\n{text}");
    }
    let shared = fs::read_to_string(root.join("config").join("quvyta.conf")).expect("the shared file");
    assert!(shared.contains("theme = \"nordic\""), "{shared}");

    // Next start: the file is there, so nothing is asked and the settings are the chosen ones.
    let again = start(&root, &clock, 80, 30);
    assert!(!again.app().setting_up(), "{}", again.screen());
    assert_eq!(again.app().prefs(), &chosen());
    done(&root);
}

#[test]
fn starting_with_the_defaults_finishes_from_the_first_step_and_writes_the_defaults() {
    let root = temp("wizard-defaults");
    let clock = FakeClock::new();
    let mut h = start(&root, &clock, 80, 30);
    h.click_text("Start with the defaults");
    h.advance(Duration::from_millis(100));

    assert!(!h.app().setting_up(), "{}", h.screen());
    assert_eq!(names(&root), ["focus.conf", "quvyta.conf"]);
    let back = written(&root);
    assert!(back.diagnostics().is_empty(), "{:?}", back.diagnostics());
    // qfocus never writes a value that is its default, so the defaults are what the file gives
    // back and none of the four keys stands in it.
    assert_eq!(Prefs::from_settings(&back), Prefs::default());
    assert_eq!(h.app().prefs(), &Prefs::default());
    let text = fs::read_to_string(root.join("config").join("focus.conf")).expect("qfocus's file");
    for key in [crate::prefs::WEEK_START, crate::prefs::DAY_ROLLOVER, crate::prefs::IDLE_AFTER, crate::prefs::CEILING] {
        assert!(!text.contains(key), "`{key}` is a default and is not written:\n{text}");
    }
    done(&root);
}

#[test]
fn after_finishing_the_normal_screen_opens_and_the_settings_page_shows_what_was_chosen() {
    let root = temp("wizard-settings-page");
    let clock = FakeClock::new();
    let mut h = start(&root, &clock, 80, 40);
    choose_nordic(&mut h);
    choose(&mut h);
    finish(&mut h);

    assert_eq!(h.app().page(), Page::Today);
    assert!(h.screen().contains("Today"), "the tabs are back:\n{}", h.screen());
    assert_eq!(h.env().theme().id(), "nordic", "the look chosen on the first step is in force");

    h.press("4");
    let screen = h.screen();
    for text in ["Saturday", "04 : 00", "0 h 05 min", "6 h 00 min"] {
        assert!(screen.contains(text), "`{text}` is missing from the settings page:\n{screen}");
    }
    assert_eq!(forbidden(&screen), None, "{screen}");
    done(&root);
}
