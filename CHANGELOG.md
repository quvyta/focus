# Changelog

Every release of quvyta-focus, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/). While the version starts with 0, a minor release may change the files qfocus writes; the notes say so when it does, and qfocus moves older files forward by itself.

## 0.1.8 - 2026-09-20

### Changed

- The language, the theme and the icons are now the family's: each of these rows carries a box, "In every Quvyta application", checked while qfocus follows the family. Checked, the choice goes to `quvyta.conf` beside the settings and every Quvyta application that follows it changes too; cleared, it stays in qfocus's own file. Reduced motion and the pillar stay qfocus's own. Nothing has to be done to an existing settings file: a value already in it keeps qfocus on it until the box is checked again.

### Fixed

- A row of buttons too wide for the window now flows onto more rows instead of losing its last button. In the records footer some buttons could not be reached with the mouse at all at several widths.
- Wording read through in every language: Turkish and Russian had a handful of wrong or inconsistent words, German durations now read Std/Min beside the framework's own, Spanish, French and Portuguese say that a record is added by hand, and a Japanese particle lost its space. Russian weekday names are lowercase, as the language writes them.

## 0.1.7 - 2026-09-20

### Changed

- The week start is read from the framework everywhere, so the region counts from the first frame on, before anything is drawn.
- The note placeholder in the record form is no longer cut at forty columns: the label moves above the field. The German and French placeholders are shorter.
- The README's moving picture is recorded with the framework's shared recorder.

## 0.1.6 - 2026-09-19

### Added

- A moving picture at the top of the README: a counter started, paused and stopped, then the charts and a session's spans. It is drawn from made-up records by `docs/screenshots/make-gif.sh`.

### Fixed

- At launch the arrow keys move through the list of focuses right away; before, nothing had the keyboard until a tab was picked.

## 0.1.5 - 2026-09-19

### Changed

- The week starts where your region starts it, when your system names one: Sunday in the United States, Monday in the United Kingdom, whatever the language. Without a region the language decides, as before. A day chosen under Settings still wins.
- Every language is read at forty columns with no text cut short at all; the only exception left is the placeholder of the note field in the record form.
- Longer setting descriptions, dialog titles, toast messages and dialog buttons wrap in narrow windows instead of being cut.
- Japanese and Chinese text lines up with the terminal's cells and wraps by its own rules.

### Added

- A picture of the Today page in Japanese at the top of the README.

## 0.1.4 - 2026-09-19

### Added

- `qfocus --version` (and `-V`) prints the version, so a bug report can say which one it is about.
- README: a one-line summary and badges at the top.
- This changelog, a contributing guide with a section on translations, and issue templates for bugs, ideas and translations.
- The crates.io page links to <https://quvyta.com/qfocus/>.

## 0.1.3 - 2026-09-18

### Added

- Seven new languages: German, Spanish, French, Brazilian Portuguese, Russian, Japanese and Simplified Chinese, beside English and Turkish. Russian uses its three plural forms. Every language is checked against English key by key, placeholder by placeholder, and read at forty columns so that it cuts no more text short than English does on the same screen.
- Until a day is chosen under Settings, the week starts where the language's calendar starts it: on Sunday in English, Brazilian Portuguese and Japanese, on Monday in the others.
- Pictures at the top of the README: the counter, the week chart, the records and the Today page in Turkish.

### Changed

- The command line help fits a standard 80-column terminal in every language.
- In a narrow window the header drops the weekday before the date, and warning lines scroll instead of being cut.

## 0.1.2 - 2026-09-18

### Changed

- README: installing starts with a one-line command; `cargo install` follows, with a note on adding `~/.cargo/bin` to the `PATH`.

## 0.1.1 - 2026-09-18

### Changed

- The settings moved to the folder the Quvyta applications share: `~/.config/quvyta/focus.conf` on Linux. The first time 0.1.1 opens, it moves the old `settings.toml` there. If `focus.conf` already exists, the old file stays where it is and the Settings page says so. The records stay where they were.
- Built on quvyta-framework 0.1.4.

## 0.1.0 - 2026-09-18

The first beta release.

### Added

- A list of focuses grouped into categories, sorted by dragging or with `ctrl+shift+↑` and `ctrl+shift+↓`. Renaming a focus never rewrites its history; archiving one keeps its records.
- A counter with breaks, notes and an optional countdown. Time with no input is not counted until you say whether it was work, a break or not worth counting.
- Durations measured with the steady clock, so changing the clock or the time zone does not move them. Time asleep counts as whatever was running; time switched off is not counted, and a counter that was cut off stops at the last moment qfocus knew it was alive.
- The Today page with the day's strip of work and breaks, and an idle board after two minutes without a counter.
- Day, week, month and year charts, each compared with the period before.
- Records: search, correct a session, add one by hand, move one to the trash, undo with `ctrl+z`, export as CSV and JSON.
- Daily, weekly and monthly goals for a focus or a category.
- Settings, with **Empty the trash** confirmed by typing a word.
- A command line: `qfocus start`, `stop`, `status` and `today`, with `--json`.
- English and Turkish.
