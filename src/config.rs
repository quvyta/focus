//! Where qfocus keeps its settings: `focus.conf` in the shared Quvyta folder, next to the other
//! Quvyta applications, and its other configuration files in the `focus/` folder beside
//! it.
//!
//! ```text
//! ~/.config/quvyta/
//!     quvyta.conf     the language, theme and icons every Quvyta application shares
//!     focus.conf      qfocus's settings
//!     focus/          its other configuration files
//! ```
//!
//! The language, the theme and the icons belong to the whole ecosystem: `focus.conf` either names
//! qfocus's own value or `"quvyta"`, which means "follow `quvyta.conf`". [`preferences`] resolves
//! them before the runtime starts, so the first frame is already in the right language.
//!
//! Earlier versions kept `settings.toml` inside `focus/`. That file is moved once, at start,
//! before the settings are read; a file that cannot be moved without overwriting something stays
//! where it is and is shown on the Settings page. The records are not settings and do not move.

use std::fs;
use std::path::{Path, PathBuf};

use qframe::diagnostics::Diagnostic;
use qframe::i18n::I18n;
use qframe::storage::{Ecosystem, Preferences, Settings, config_dir};

use crate::prefs::Prefs;

/// The application's id in the ecosystem: its settings file is `focus.conf` and its other files are
/// under `focus/`.
pub const APP: &str = "focus";

/// The folder under the platform's config directory that held `settings.toml` before. On Linux
/// it is the shared Quvyta `focus/` folder itself, so only the settings file moves.
const LEGACY: &str = "quvyta/focus";

/// The settings file's name in [`LEGACY`].
const LEGACY_FILE: &str = "settings.toml";

/// The settings, read after the old file was brought over.
#[derive(Debug)]
pub struct Loaded {
    /// The settings, checked against every key qfocus knows and repaired with a backup when they
    /// have to be.
    pub settings: Settings,
    /// The files from before that were not moved, each with the reason. Nothing was overwritten
    /// to move them, so each one is still whole where it was.
    pub left_behind: Vec<Diagnostic>,
}

/// Brings the old settings over and reads them from the platform's shared Quvyta folder. Without a home
/// folder the settings stay in memory and say why.
#[must_use]
pub fn load() -> Loaded {
    match Ecosystem::QUVYTA.config_dir() {
        Some(folder) => {
            let legacy = config_dir(LEGACY).unwrap_or_else(|| folder.join(APP));
            load_in(&folder, &legacy)
        }
        None => Loaded { settings: checked(Settings::load_member(&Ecosystem::QUVYTA, APP)), left_behind: Vec::new() },
    }
}

/// [`load`] with `folder` as the shared Quvyta folder and `legacy` as the folder the old
/// `settings.toml` may be in, so a test never touches the user's own settings.
#[must_use]
pub fn load_in(folder: &Path, legacy: &Path) -> Loaded {
    let left_behind = adopt(folder, legacy);
    Loaded { settings: checked(Settings::open(folder.join(format!("{APP}.conf")))), left_behind }
}

/// The language, theme and icons as qfocus sees them: its own when its file names one, else the
/// shared one, else what this machine asks for.
#[must_use]
pub fn preferences() -> Preferences {
    Ecosystem::QUVYTA.preferences(APP, &spoken())
}

/// [`preferences`] with `folder` as the shared Quvyta folder, so a test never touches the user's own
/// settings.
#[must_use]
pub fn preferences_in(folder: &Path) -> Preferences {
    Ecosystem::QUVYTA.preferences_in(folder, APP, &spoken())
}

/// Where the Quvyta-wide update notice is kept and where qfocus remembers when it last asked
/// whether a newer version of itself is out.
///
/// The switch is the ecosystem's, one for every Quvyta application, so it is read from the shared
/// `quvyta.conf` rather than from `focus.conf`. A test gives folders of its own, so nothing it
/// does reads or turns off the person's own switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFolders {
    /// The shared Quvyta configuration folder, whose shared file holds the switch.
    pub config: PathBuf,
    /// qfocus's state folder, which remembers when the question was last asked.
    pub state: PathBuf,
}

impl UpdateFolders {
    /// This machine's folders, or `None` without a home folder, where nothing could remember the
    /// switch or the last question and so nothing is asked.
    #[must_use]
    pub fn here() -> Option<Self> {
        let quvyta = Ecosystem::QUVYTA;
        quvyta.config_dir().zip(quvyta.state_dir(APP)).map(|(config, state)| Self { config, state })
    }
}

/// The languages qfocus speaks, for choosing the machine's one before the runtime is built: a
/// language the application does not carry is no use to it.
#[must_use]
pub fn spoken() -> I18n {
    let mut i18n = I18n::builtin();
    for &(file, text) in crate::locales() {
        i18n.add_source(file, text);
    }
    i18n
}

/// Moves the old settings into the shared Quvyta layout and returns what stayed behind.
///
/// Only an old settings file is a reason to look. Where the old folder and `focus/` are one
/// folder under two spellings (a file system that ignores case) the framework could take them
/// for two, and would then warn at every start about files that are already in place; once the
/// settings file has moved there is nothing more to bring over.
fn adopt(folder: &Path, legacy: &Path) -> Vec<Diagnostic> {
    if fs::symlink_metadata(legacy.join(LEGACY_FILE)).is_err() {
        return Vec::new();
    }
    Ecosystem::QUVYTA.adopt_in(folder, APP, legacy).diagnostics().to_vec()
}

/// `settings` checked against qfocus's keys and healed. Being a member of the ecosystem makes
/// `"quvyta"` a valid value of the shared keys, so healing does not throw away a file that says
/// qfocus follows the ecosystem.
fn checked(settings: Settings) -> Settings {
    settings.member_of(&Ecosystem::QUVYTA).schema(Prefs::schema()).self_heal(true)
}

#[cfg(test)]
mod tests {
    use qframe::date::Weekday;

    use super::*;
    use crate::prefs::WEEK_START;

    /// A fresh folder under the system's temporary folder; never the user's own config folder.
    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qfocus-test-config-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("folder");
        dir
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("folder");
        fs::write(path, text).expect("file");
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("readable")
    }

    const SUNDAY: &str = "week-start = \"sunday\"\n";

    #[test]
    fn the_old_settings_file_becomes_focus_conf_and_the_other_files_stay_in_focus() {
        let folder = temp("same-folder");
        let legacy = folder.join("focus");
        write(&legacy.join("settings.toml"), SUNDAY);
        write(&legacy.join("settings.toml.bak"), "old backup\n");
        write(&legacy.join("keymap.toml"), "[app]\n");

        let loaded = load_in(&folder, &legacy);

        assert!(loaded.left_behind.is_empty(), "{:?}", loaded.left_behind);
        assert_eq!(read(&folder.join("focus.conf")), SUNDAY);
        assert!(!legacy.join("settings.toml").exists());
        assert_eq!(read(&legacy.join("settings.toml.bak")), "old backup\n");
        assert_eq!(read(&legacy.join("keymap.toml")), "[app]\n");
        assert_eq!(Prefs::from_settings(&loaded.settings).week_start, Some(Weekday::Sunday));
        assert_eq!(loaded.settings.path(), Some(folder.join("focus.conf").as_path()));
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_second_start_moves_nothing_and_reads_the_same() {
        let folder = temp("again");
        let legacy = folder.join("focus");
        write(&legacy.join("settings.toml"), SUNDAY);
        let _ = load_in(&folder, &legacy);

        let loaded = load_in(&folder, &legacy);

        assert!(loaded.left_behind.is_empty(), "{:?}", loaded.left_behind);
        assert_eq!(read(&folder.join("focus.conf")), SUNDAY);
        assert_eq!(Prefs::from_settings(&loaded.settings).week_start, Some(Weekday::Sunday));
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn an_existing_focus_conf_wins_and_the_old_file_stays_whole_and_is_reported() {
        let folder = temp("both");
        let legacy = folder.join("focus");
        write(&legacy.join("settings.toml"), SUNDAY);
        write(&folder.join("focus.conf"), "week-start = \"saturday\"\n");

        let loaded = load_in(&folder, &legacy);

        assert_eq!(read(&legacy.join("settings.toml")), SUNDAY, "the old file is never touched");
        assert_eq!(read(&folder.join("focus.conf")), "week-start = \"saturday\"\n");
        assert_eq!(Prefs::from_settings(&loaded.settings).week_start, Some(Weekday::Saturday));
        assert_eq!(loaded.left_behind.len(), 1, "{:?}", loaded.left_behind);
        assert!(loaded.left_behind[0].message.contains("settings.toml"), "{}", loaded.left_behind[0]);
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_fresh_start_writes_nothing_until_a_setting_changes() {
        let folder = temp("fresh");
        let legacy = folder.join("focus");

        let mut loaded = load_in(&folder, &legacy);

        assert!(loaded.left_behind.is_empty());
        assert!(loaded.settings.diagnostics().is_empty(), "{:?}", loaded.settings.diagnostics());
        assert!(!folder.join("focus.conf").exists());
        assert!(!legacy.exists());
        loaded.settings.set(WEEK_START, "sunday".to_owned());
        loaded.settings.save().expect("saved");
        assert_eq!(read(&folder.join("focus.conf")), SUNDAY);
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn an_old_folder_elsewhere_moves_whole_and_is_removed() {
        let folder = temp("elsewhere");
        let legacy = folder.join("Old").join("focus");
        write(&legacy.join("settings.toml"), SUNDAY);
        write(&legacy.join("keys").join("keymap.toml"), "[app]\n");

        let loaded = load_in(&folder, &legacy);

        assert!(loaded.left_behind.is_empty(), "{:?}", loaded.left_behind);
        assert_eq!(read(&folder.join("focus.conf")), SUNDAY);
        assert_eq!(read(&folder.join("focus").join("keys").join("keymap.toml")), "[app]\n");
        assert!(!legacy.exists());
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_damaged_old_file_is_moved_as_it_was_then_healed_with_a_backup() {
        let folder = temp("damaged");
        let legacy = folder.join("focus");
        let text = "week-start = \"someday\"\n";
        write(&legacy.join("settings.toml"), text);

        let loaded = load_in(&folder, &legacy);

        assert!(loaded.left_behind.is_empty(), "{:?}", loaded.left_behind);
        assert_eq!(read(&folder.join("focus.conf.bak")), text, "what the user wrote is kept");
        assert!(!loaded.settings.diagnostics().is_empty());
        assert_eq!(Prefs::from_settings(&loaded.settings).week_start, None);
        let _ = fs::remove_dir_all(&folder);
    }
}
