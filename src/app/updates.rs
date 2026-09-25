//! The Quvyta-wide update notice: once qfocus is open it asks crates.io, at most once a day, whether
//! a newer version of itself is out, and says so in the corner when one is.
//!
//! The question is asked on a thread of the framework's own, so the start never waits for it; no
//! network is silence. The switch is the ecosystem's, `update-notice` in the shared `quvyta.conf`,
//! and it is read here before anything is asked: with it off nothing is asked at all.
//!
//! While the first-start wizard is open nothing is asked. Its last step shows the switch, held in
//! memory with everything else, so a person who turns it off there is never asked; the question
//! comes when the wizard finishes, with the choice written.

use qframe::prelude::*;
use qframe::runtime::UpdateCheck;
use qframe::storage::Ecosystem;
use qframe::widgets::AppearanceChange;

use super::{Msg, QFocus};
use crate::config::{APP, UpdateFolders};

impl QFocus {
    /// The same application, asking at start whether a newer version is out while the ecosystem's
    /// update notice in `folders` is on, and offering that switch on the Settings page. `None`
    /// asks nothing and offers no switch, which is every test that has not said otherwise.
    #[must_use]
    pub fn update_notice(mut self, folders: Option<UpdateFolders>) -> Self {
        self.settings_screen = self.settings_screen.clone().with_updates(folders.is_some());
        self.updates = folders;
        self
    }

    /// The question for a newer version of qfocus, when the Quvyta-wide update notice is on and the
    /// wizard is not open.
    ///
    /// The switch is read from the shared file here, not only where the question is sent: once
    /// it is turned off anywhere in the ecosystem nothing is asked at all, whoever runs the question.
    pub(super) fn ask_for_update(&self) -> Command<Msg> {
        let Some(folders) = &self.updates else { return Command::none() };
        if self.setting_up() || !Ecosystem::QUVYTA.update_notice_in(&folders.config) {
            return Command::none();
        }
        let check = UpdateCheck::new(
            Ecosystem::QUVYTA,
            APP,
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
            Msg::NewVersion,
        )
        .in_folders(folders.config.clone(), folders.state.clone());
        Command::check_for_update(check)
    }

    /// The wizard has finished and written its files: the switch held on its last step is
    /// written when it differs from what the shared file says, so starting with the defaults
    /// writes nothing more.
    pub(super) fn keep_held_update_notice(&mut self, held: bool) -> Command<Msg> {
        if self.updates.is_none() || held == self.appearance.preferences().update_notice() {
            return Command::none();
        }
        self.appearance.update(AppearanceChange::UpdateNotice(held), &mut self.settings)
    }
}
