//! The recovery dialog: what to do with a counter the last run left on disk.
//!
//! The dialog says why the counter stopped counting: the computer restarted, or the program
//! closed, and nothing after that moment counts. The duration shown is the work up to there.
//!
//! The counter is kept, continued or thrown away, and the person has to choose; the dialog
//! cannot be waved off, because starting another counter would overwrite the file. Throwing
//! away is the one thing that cannot be undone, so it is held rather than pressed. A file that
//! cannot be read is shown with what was wrong and where it now is: moved aside under a
//! time-stamped name so a new counter cannot overwrite it, or left in place, with the reason,
//! when it could not be moved or this instance may not write.

use std::path::PathBuf;

use qframe::diagnostics::Diagnostic;
use qframe::prelude::*;
use qframe::widgets::{HoldToConfirm, Modal};

use super::ago;
use crate::duration::{Units, short};
use crate::liveness::{self, Reach, Reason};
use crate::store::Running;

/// What was found on disk.
#[derive(Debug, Clone)]
pub enum Recover {
    /// A counter that can be brought back.
    Found {
        /// The counter as its last refresh left it.
        running: Running,
        /// The name of its focus, when the catalogue still has it.
        focus: Option<String>,
        /// Seconds since its last refresh, at the moment the dialog opened.
        ago: u64,
        /// How far it reaches: where it was cut and why, when it was.
        reach: Reach,
    },
    /// A file that could not be read, with what was wrong and what became of it.
    Broken {
        /// What was wrong.
        problems: Vec<Diagnostic>,
        /// Where the file is now.
        path: PathBuf,
        /// Whether it was moved aside or left where it was.
        kept: Kept,
    },
}

/// What became of a running file that could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kept {
    /// Moved aside to the path in [`Recover::Broken`], so a new counter cannot overwrite it.
    Aside,
    /// Left where it was because moving it failed, for the reason given.
    Stuck(String),
    /// Left where it was because another instance holds the lock and this one may not write.
    Untouched,
}

/// What was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    /// Record the counter as a session that ended where it was last alive.
    Save,
    /// Carry the counter on from where it was; the time it was not alive stays out.
    Continue,
    /// Throw the counter away.
    Discard,
    /// Close the report of a broken file.
    Close,
}

/// Draws the dialog over the screen. `now` is the wall clock and `offset_minutes` the local time
/// zone, for the moment a restart cut the counter.
pub fn view<M: From<Msg> + Clone + Send + 'static>(
    recover: &Recover,
    units: &Units<'_>,
    now: i64,
    offset_minutes: i16,
    ui: &mut View<'_, M>,
) {
    match recover {
        Recover::Found { running, focus, ago: since, reach } => {
            let focus = focus.clone().unwrap_or_else(|| t!("recover.unknown-focus"));
            let duration = short(running.work_until(reach.until), units);
            let message = match reach.cut {
                Some(cut) if cut.reason == Reason::Restart => t!(
                    "recover.message-restart",
                    focus = focus,
                    duration = duration,
                    time = liveness::moment_words(cut.at, now, offset_minutes)
                ),
                Some(_) => t!("recover.message-closed", focus = focus, duration = duration, ago = ago(*since)),
                None => t!("recover.message", focus = focus, duration = duration, ago = ago(*since)),
            };
            let dialog = Modal::new()
                .title(t!("recover.title"))
                .dismissable(false)
                .action(Button::new(t!("recover.continue")).on_press(M::from(Msg::Continue)))
                .action(Button::new(t!("recover.save")).variant("primary").on_press(M::from(Msg::Save)));
            ui.add_with(dialog, |ui| {
                ui.add(Text::new(message)).fill_width();
                ui.add(HoldToConfirm::new(t!("recover.discard")).color("$danger").on_confirm(M::from(Msg::Discard)));
            });
        }
        Recover::Broken { problems, path, kept } => {
            let dialog = Modal::new()
                .title(t!("recover.broken-title"))
                .variant("danger")
                .on_close(M::from(Msg::Close))
                .action(Button::new(t!("recover.close")).on_press(M::from(Msg::Close)));
            let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
            ui.add_with(dialog, |ui| {
                let path = path.display().to_string();
                let message = match kept {
                    Kept::Aside => t!("recover.broken-aside", path = path),
                    Kept::Stuck(reason) => t!("recover.broken-stuck", path = path, reason = reason.clone()),
                    Kept::Untouched => t!("recover.broken-message", path = path),
                };
                ui.add(Text::new(message).role("secondary")).fill_width();
                for line in lines {
                    ui.add(Text::new(line).no_wrap()).fill_width();
                }
            });
        }
    }
}
