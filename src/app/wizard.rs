//! The first start: the framework's setup wizard, with qfocus's own step for the four settings
//! that shape a day.
//!
//! It opens while qfocus has no `focus.conf` of its own, and only then: someone who has used an
//! earlier version keeps their file and never sees it. The framework owns the appearance step,
//! the buttons and the two files; nothing at all is written until Finish, so a qfocus closed
//! half-way leaves the settings folder exactly as it was and the wizard comes again next start.
//!
//! qfocus's step asks the same four things the Settings page does, with the same controls and
//! the same floors: the day the week starts on, the hour the day turns at, when silence counts
//! as being away, and the ceiling a session is flagged over. On Finish the framework writes the
//! shared keys and makes `focus.conf`; the four go into that same file straight after.
//!
//! Where qfocus asks for its updates, the same step ends with the Quvyta-wide update notice, the
//! switch of the one thing qfocus does over the network, so it can be turned off before it is
//! ever done. It is held like the rest and written on Finish only when it differs from what the
//! shared file says; nothing is asked while the wizard is open.

use qframe::prelude::*;
use qframe::storage::Family;
use qframe::widgets::{Appearance, ScrollView, SettingsList, Setup, SetupWizard};

use super::{Msg, QFocus, today, units};
use crate::ui::settings;

/// The widget id of the list on qfocus's own step.
const DAY_LIST: &str = "wizard-day";

/// The widget id the keyboard starts on: the appearance rows of the framework's step.
pub(super) const FIRST: &str = "setup-appearance";

/// Rows the wizard takes around its page: the padding, the name, the blank line under it, the
/// steps, the blank lines around the page and the row of buttons.
const AROUND_PAGE: u16 = 8;

/// The fewest rows the page keeps, however short the terminal is.
const LEAST_PAGE_ROWS: u16 = 8;

impl QFocus {
    /// Whether the first-run wizard has the screen.
    pub(super) fn setting_up(&self) -> bool {
        self.setup.as_ref().is_some_and(Setup::needed)
    }

    /// The wizard wrote the shared keys and made `focus.conf`: qfocus's own four settings go in
    /// beside them, the Settings page carries on from the look that was chosen, and the normal
    /// screen opens.
    pub(super) fn finish_setup(&mut self) -> Command<Msg> {
        let Some(setup) = self.setup.take() else { return Command::none() };
        // The update notice as the last step left it, held until now.
        let held = self.appearance.preferences().update_notice();
        // The appearance rows of the Settings page start from what the wizard chose; the wizard's
        // own Appearance held those values without writing them.
        let appearance = Appearance::new(Family::QUVYTA, crate::config::APP, setup.preferences().clone());
        self.appearance = match &self.config_folder {
            Some(folder) => appearance.in_folder(folder),
            None => appearance,
        };
        // A value that is the default is taken out rather than written, so a person who started
        // with the defaults gets a file that holds the shared keys and nothing else.
        self.prefs.write(&mut self.settings);
        self.refresh_day();
        let kept = self.keep_held_update_notice(held);
        Command::batch([self.save_settings(), Command::focus(today::TREE), kept, self.ask_for_update()])
    }

    /// The wizard, while it is wanted: the framework's appearance step, then qfocus's own.
    pub(super) fn setup_wizard(&self, ui: &mut View<'_, Msg>) {
        let Some(setup) = &self.setup else { return };
        let rows = ui.size().height.saturating_sub(AROUND_PAGE).max(LEAST_PAGE_ROWS);
        ui.column(|ui| {
            ui.add(Text::rich([Span::new(t!("app.name")).color("accent").bold()]).no_wrap()).fill_width();
            ui.spacer().height(Length::Cells(1));
            // Every step is given the rows that are left, so the buttons stand at the bottom
            // wherever the person is and a page longer than the screen scrolls instead of
            // pushing them off it.
            SetupWizard::new(setup)
                .step(t!("wizard.step-day"), |ui| self.day_step(ui))
                .page_height(rows)
                .show(ui)
                .fill_width();
        })
        .padding(Padding::symmetric(1, 2))
        .fill();
    }

    /// qfocus's own step: the four times that shape a day, as the Settings page asks them.
    fn day_step(&self, ui: &mut View<'_, Msg>) {
        ui.add(Text::new(t!("wizard.day-intro")).role("secondary")).fill_width();
        ui.spacer().height(Length::Cells(1));
        let week_start = self.prefs.week_starts_on(ui.env().i18n().first_weekday());
        let units = units();
        ui.add_with(ScrollView::new(), |ui| {
            SettingsList::show(ui, |list| {
                settings::time_rows(list, &self.settings_screen, &self.prefs, week_start, &units.as_units());
                // The Quvyta-wide update notice, where qfocus asks for its updates: the one thing
                // qfocus would do over the network, offered before it is ever done.
                if self.updates.is_some() {
                    self.appearance.updates(list, |change| Msg::Settings(settings::Msg::Appearance(change)));
                }
            })
            .fill_width()
            .id(DAY_LIST);
        })
        .fill();
    }
}
