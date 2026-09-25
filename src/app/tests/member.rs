//! The running application following shared choices made by another Quvyta application.

use std::sync::Arc;

use qframe::event::{Event, KeyKind};
use qframe::geometry::{Rect, Size};
use qframe::i18n::I18n;
use qframe::keymap::{KeyChord, Scope};
use qframe::prelude::{App, Command, View};
use qframe::storage::{Ecosystem, Preferences};
use qframe::widget::{EventCx, MeasureCx, PaintCx, Widget};

use super::*;

fn write(path: &Path, text: &str) {
    fs::write(path, text).expect("the settings file is written");
}

fn shared(language: &str, theme: &str) -> String {
    format!("language = \"{language}\"\ntheme = \"{theme}\"\nicons = \"unicode\"\n")
}

fn follows() -> &'static str {
    "language = \"quvyta\"\ntheme = \"quvyta\"\nicons = \"quvyta\"\n"
}

fn qfocus(folder: &Path, clock: &FakeClock) -> QFocus {
    let settings =
        Settings::open(folder.join("focus.conf")).member_of(&Ecosystem::QUVYTA).schema(Prefs::schema()).self_heal(true);
    let preferences = crate::config::preferences_in(folder);
    let appearance = Appearance::new(Ecosystem::QUVYTA, crate::config::APP, preferences).in_folder(folder);
    QFocus::new(
        Store::open(Paths::at(folder.join("data"), "test")),
        true,
        Some(180),
        clock.reader(),
        settings,
        appearance,
    )
}

/// The member harness owns a built-in environment, so this adapter gives the view the same
/// merged language files as the ordinary application tests and listens for the real Settings
/// chord from qfocus's keymap, without changing how preferences are read.
struct LocalizedQFocus {
    app: QFocus,
    i18n: I18n,
    settings_keys: Vec<KeyChord>,
}

impl LocalizedQFocus {
    fn new(app: QFocus) -> Self {
        let assets = env();
        Self {
            app,
            i18n: assets.i18n().clone(),
            settings_keys: assets.keymap().chords_for(Scope::App, "settings").to_vec(),
        }
    }

    fn page(&self) -> Page {
        self.app.page()
    }
}

/// A cell-free listener that puts qfocus's own Settings binding into the member harness's
/// built-in keymap while the test sends the key as a person does.
struct SettingsTabKey(Vec<KeyChord>);

impl Widget<Msg> for SettingsTabKey {
    fn measure(&self, _cx: &mut MeasureCx<'_>, _available: Size) -> Size {
        Size::new(0, 0)
    }

    fn paint(&self, cx: &mut PaintCx<'_>, _area: Rect) {
        for key in &self.0 {
            cx.listen_key(*key);
        }
    }

    fn event(&self, cx: &mut EventCx<'_, Msg>, event: &Event) -> bool {
        let Event::Key(key) = event else { return false };
        if !self.0.contains(&key.chord) {
            return false;
        }
        if key.kind != KeyKind::Release {
            cx.emit(Msg::Page(Page::Settings.index()));
        }
        true
    }
}

impl App for LocalizedQFocus {
    type Msg = Msg;

    fn init(&mut self) -> Command<Msg> {
        self.app.init()
    }

    fn preferences(&self, preferences: &Preferences) -> Option<Msg> {
        self.app.preferences(preferences)
    }

    fn update(&mut self, msg: Msg) -> Command<Msg> {
        self.app.update(msg)
    }

    fn action(&self, name: &str) -> Option<Msg> {
        self.app.action(name)
    }

    fn view(&self, ui: &mut View<'_, Msg>) {
        let active = ui.env().i18n().active().to_owned();
        let region = ui.env().i18n().region().map(str::to_owned);
        let mut i18n = self.i18n.clone();
        assert!(i18n.set_active(&active), "`{active}` is a language qfocus carries");
        let _ = i18n.set_region(region.as_deref());
        qframe::i18n::scope(Arc::new(i18n), || {
            ui.add(SettingsTabKey(self.settings_keys.clone()));
            self.app.view(ui);
        });
    }
}

fn theme_row(screen: &str) -> String {
    screen.lines().find(|line| line.contains("Theme")).unwrap_or_default().to_owned()
}

#[test]
fn a_shared_theme_change_reaches_the_running_screen_and_the_settings_row() {
    let folder = temp("member-theme");
    fs::create_dir_all(&folder).expect("folder");
    write(&folder.join("quvyta.conf"), &shared("en", "amber"));
    write(&folder.join("focus.conf"), follows());
    let clock = FakeClock::new();
    let mut h = Harness::member_in(
        LocalizedQFocus::new(qfocus(&folder, &clock)),
        Ecosystem::QUVYTA,
        &folder,
        crate::config::APP,
        100,
        44,
    );
    assert_eq!(h.env().theme().id(), "amber");
    assert_ne!(h.env().theme().id(), "nordic");

    write(&folder.join("quvyta.conf"), &shared("en", "nordic"));
    h.poll_preferences();

    assert_eq!(h.env().theme().id(), "nordic", "the running screen changed:\n{}", h.screen());
    h.press("4");
    assert_eq!(h.app().page(), Page::Settings);
    let row = theme_row(&h.screen());
    assert!(row.contains("Nordic"), "the Theme row changed too: {row}");
    assert!(!row.contains("Amber"), "the old theme is gone from the row: {row}");
    done(&folder);
}

#[test]
fn a_shared_theme_change_leaves_the_running_application_theme_alone() {
    let folder = temp("member-own-theme");
    fs::create_dir_all(&folder).expect("folder");
    write(&folder.join("quvyta.conf"), &shared("en", "amber"));
    write(&folder.join("focus.conf"), "language = \"quvyta\"\ntheme = \"iris\"\nicons = \"quvyta\"\n");
    let clock = FakeClock::new();
    let mut h = Harness::member_in(
        LocalizedQFocus::new(qfocus(&folder, &clock)),
        Ecosystem::QUVYTA,
        &folder,
        crate::config::APP,
        100,
        44,
    );
    assert_eq!(h.env().theme().id(), "iris");

    write(&folder.join("quvyta.conf"), &shared("en", "nordic"));
    h.poll_preferences();

    assert_eq!(h.env().theme().id(), "iris", "qfocus's own theme stayed in force:\n{}", h.screen());
    h.press("4");
    assert!(theme_row(&h.screen()).contains("Iris"), "the Settings row stayed with it:\n{}", h.screen());
    done(&folder);
}

#[test]
fn a_shared_language_change_reaches_the_running_tab_labels() {
    let folder = temp("member-language");
    fs::create_dir_all(&folder).expect("folder");
    write(&folder.join("quvyta.conf"), &shared("en", "amber"));
    write(&folder.join("focus.conf"), follows());
    let clock = FakeClock::new();
    let mut h = Harness::member_in(
        LocalizedQFocus::new(qfocus(&folder, &clock)),
        Ecosystem::QUVYTA,
        &folder,
        crate::config::APP,
        100,
        44,
    );
    assert!(h.screen().contains("Today"), "the language files are loaded:\n{}", h.screen());

    write(&folder.join("quvyta.conf"), &shared("tr", "amber"));
    h.poll_preferences();

    assert_eq!(h.env().i18n().active(), "tr");
    let screen = h.screen();
    assert!(screen.contains("Bugün"), "the tab label changed with the language: {screen}");
    assert!(!screen.contains("⟦tabs.today⟧"), "the old or missing label is gone: {screen}");
    done(&folder);
}

#[test]
fn a_choice_made_after_the_own_file_turned_to_the_ecosystem_is_saved_for_the_ecosystem() {
    let folder = temp("member-scope");
    fs::create_dir_all(&folder).expect("folder");
    write(&folder.join("quvyta.conf"), &shared("en", "amber"));
    write(&folder.join("focus.conf"), "language = \"quvyta\"\ntheme = \"iris\"\nicons = \"quvyta\"\n");
    let clock = FakeClock::new();
    let mut h = Harness::member_in(
        LocalizedQFocus::new(qfocus(&folder, &clock)),
        Ecosystem::QUVYTA,
        &folder,
        crate::config::APP,
        100,
        44,
    );
    h.press("4");

    // Another application, or the person in an editor, makes qfocus follow the ecosystem's theme.
    write(&folder.join("focus.conf"), follows());
    h.poll_preferences();

    // Down past the language and its box to Theme, open its list and walk to Nordic.
    h.press("down").press("down").press("enter");
    for _ in 0..30 {
        if h.screen().lines().any(|line| line.contains("▌  Nordic")) {
            break;
        }
        h.press("down");
    }
    assert!(h.screen().lines().any(|line| line.contains("▌  Nordic")), "{}", h.screen());
    h.press("enter");

    assert_eq!(h.env().theme().id(), "nordic", "{}", h.screen());
    let shared_file = fs::read_to_string(folder.join("quvyta.conf")).expect("the shared file");
    assert!(shared_file.contains("theme = \"nordic\""), "the choice is saved for the ecosystem:\n{shared_file}");
    let own = fs::read_to_string(folder.join("focus.conf")).expect("qfocus's own file");
    assert!(own.contains("theme = \"quvyta\""), "qfocus still follows the ecosystem:\n{own}");
    done(&folder);
}
