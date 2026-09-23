//! The Timer screen: the counter of the focus being worked on.
//!
//! The screen owns the [`Timer`] and the readings it has seen; the application feeds it a
//! reading with every message, keeps the running file on disk from what [`Screen::running`]
//! reports, and turns the session it ends with into a record. A break dims the screen and
//! nothing blinks.
//!
//! A counter started with a countdown counts down to it on the big clock and, once it is
//! reached, counts up again with the target written beside the time; it never stops on its
//! own here, the application decides that by its setting. The goals of the focus and of its
//! category are read under the clock while they exist.
//!
//! Left alone for a while the screen quietens: the lines around the clock take the faint tone,
//! so the time is what is left to read from across the room. The buttons, the breadcrumb and
//! the key hints keep their look, because the framework gives none of them a quieter one.

use std::time::Duration;

use qframe::prelude::*;
use qframe::widgets::{BigText, Breadcrumb, Modal, Span, TextInput, Toast};

use super::{GoalRow, info_line, warning_line};
use crate::duration::{Units, clock, short};
use crate::id::Id;
use crate::session::Session;
use crate::span::SpanKind;
use crate::store::{Running, Watch};
use crate::timer::{Clocks, IdleDecision, Notice, Timer};

/// The widget id of the note field, for focusing it.
pub const NOTE_INPUT: &str = "timer-note";

/// The widget id of the stop button, which takes the keyboard when the screen opens.
pub const STOP: &str = "timer-stop";

/// How long the running file may go without a refresh, in seconds.
const SAVE_EVERY: i64 = 5;

/// Something that happened on the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// A second passed.
    Tick,
    /// The break was started or ended.
    Pause,
    /// The counter was stopped.
    Stop,
    /// The note field was opened.
    Note,
    /// The note field changed.
    NoteTyped(String),
    /// The note field was submitted.
    NoteSubmit,
    /// The note field was closed without keeping what was typed.
    NoteCancel,
    /// The terminal went silent for the screen's idle threshold (`true`), or input came back
    /// (`false`).
    Idle(bool),
    /// The person said what the silence was.
    Claim(IdleDecision),
}

/// What the screen asks the application for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Write the running file now.
    Save,
    /// End the session and record it.
    Stop,
    /// The countdown reached its target on this reading; the counter goes on.
    Reached,
}

/// The state of the screen.
#[derive(Debug, Clone)]
pub struct Screen {
    timer: Timer,
    category: String,
    focus: String,
    /// The last reading the timer saw.
    now: Clocks,
    /// The wall stamp of the last running file written.
    saved_at: i64,
    /// Why the last write of the running file failed, until one succeeds.
    save_problem: Option<String>,
    /// The note kept for the record.
    note: String,
    /// What is being typed into the note field, while it is open.
    draft: Option<String>,
    /// Whether this system can tell sleep apart, which the screen says when it cannot.
    detects_suspend: bool,
    /// Whether the question about the latest silence is open.
    asking_idle: bool,
    /// How long the terminal may go without input before the work is idle.
    idle_after: Duration,
    /// Seconds of work the counter counts down to, when it was started with a countdown.
    target: Option<u32>,
    /// Whether the countdown has been reached, so it is said once.
    reached: bool,
}

impl Screen {
    /// A screen over `timer`, which last saw the reading `now`, under the names of the category
    /// and the focus it counts.
    #[must_use]
    pub fn new(timer: Timer, now: Clocks, category: String, focus: String, detects_suspend: bool) -> Self {
        // A counter carried on from a file may bring a silence nobody answered for.
        let asking_idle = timer.has_unclaimed_idle();
        Self {
            timer,
            category,
            focus,
            now,
            saved_at: now.wall,
            save_problem: None,
            note: String::new(),
            draft: None,
            detects_suspend,
            asking_idle,
            idle_after: Duration::from_secs(15 * 60),
            target: None,
            reached: false,
        }
    }

    /// Counts down to `seconds` of work instead of up from zero.
    #[must_use]
    pub fn target(mut self, seconds: u32) -> Self {
        self.target = Some(seconds);
        self.reached = self.work_seconds() >= u64::from(seconds);
        self
    }

    /// Sets how long the terminal may go without input before the work is idle.
    #[must_use]
    pub fn idle_after(mut self, after: Duration) -> Self {
        self.idle_after = after;
        self
    }

    /// How long the terminal may go without input before the work is idle; the application
    /// declares the watch with it while the counter runs.
    #[must_use]
    pub fn idle_threshold(&self) -> Duration {
        self.idle_after
    }

    /// The seconds of work the counter counts down to, when it has a countdown.
    #[must_use]
    pub fn countdown(&self) -> Option<u32> {
        self.target
    }

    /// Whether the countdown has been reached.
    #[must_use]
    pub fn is_reached(&self) -> bool {
        self.reached
    }

    /// Whether the question about the latest silence is open.
    #[must_use]
    pub fn is_asking_idle(&self) -> bool {
        self.asking_idle
    }

    /// Whether no input has reached the terminal for a while, so the work is idle.
    #[must_use]
    pub fn is_away(&self) -> bool {
        self.timer.is_away()
    }

    /// The focus being counted.
    #[must_use]
    pub fn focus(&self) -> Id {
        self.timer.focus()
    }

    /// The name of the focus being counted.
    #[must_use]
    pub fn focus_name(&self) -> &str {
        &self.focus
    }

    /// Whether the note field is open.
    #[must_use]
    pub fn is_editing_note(&self) -> bool {
        self.draft.is_some()
    }

    /// Seconds of work so far, as of the last reading.
    #[must_use]
    pub fn work_seconds(&self) -> u64 {
        self.timer.work_seconds(self.now)
    }

    /// The counter as it stands at the last reading, for the running file.
    #[must_use]
    pub fn running(&self) -> Running {
        Running {
            focus: self.timer.focus(),
            started: self.timer.started(),
            offset_minutes: self.timer.offset_minutes(),
            spans: self.timer.spans(self.now),
            paused: self.timer.is_paused(),
            refreshed: self.now.wall,
            flags: self.timer.flags().to_vec(),
            target: self.target,
            idle_from: self.timer.unclaimed_idle_from(),
            watch: Watch::Window,
        }
    }

    /// Takes in how writing the running file went: a failure stays on screen until a write
    /// succeeds, and the counter goes on either way.
    pub fn saved(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.saved_at = self.now.wall;
                self.save_problem = None;
            }
            Err(reason) => self.save_problem = Some(reason),
        }
    }

    /// Ends the session at `now` as a record with `id`, carrying the note.
    #[must_use]
    pub fn finish(self, now: Clocks, id: Id) -> Session {
        let mut session = self.timer.stop(now, id, now.wall);
        session.note = self.note;
        session
    }
}

/// Applies `msg` at the reading `now` and says what the application should do. Notices the
/// timer raises become toasts, with durations in the words of `units`.
pub fn update<M: From<Msg> + Clone + Send + 'static>(
    screen: &mut Screen,
    msg: Msg,
    now: Clocks,
    units: &Units<'_>,
) -> (Command<M>, Option<Request>) {
    match msg {
        Msg::Tick => {
            let notices = screen.timer.tick(now);
            screen.now = now;
            let due = now.wall.saturating_sub(screen.saved_at) >= SAVE_EVERY;
            let commands = toasts(&notices, units, screen.timer.ceiling_seconds());
            if let Some(target) = screen.target.filter(|_| !screen.reached)
                && screen.work_seconds() >= u64::from(target)
            {
                screen.reached = true;
                let toast = Toast::success(t!("timer.target-toast", amount = short(u64::from(target), units)));
                return (Command::batch([commands, Command::toast(toast)]), Some(Request::Reached));
            }
            (commands, due.then_some(Request::Save))
        }
        Msg::Pause => {
            let notices = if screen.timer.is_paused() { screen.timer.resume(now) } else { screen.timer.pause(now) };
            screen.now = now;
            (toasts(&notices, units, screen.timer.ceiling_seconds()), Some(Request::Save))
        }
        Msg::Stop => {
            screen.timer.tick(now);
            screen.now = now;
            (Command::none(), Some(Request::Stop))
        }
        Msg::Note => {
            screen.draft = Some(screen.note.clone());
            (Command::focus(NOTE_INPUT), None)
        }
        Msg::NoteTyped(text) => {
            if let Some(draft) = screen.draft.as_mut() {
                *draft = text;
            }
            (Command::none(), None)
        }
        Msg::NoteSubmit => {
            let Some(draft) = screen.draft.take() else { return (Command::none(), None) };
            screen.note = draft.trim().to_owned();
            let toast = if screen.note.is_empty() {
                None
            } else {
                Some(Command::toast(Toast::success(t!("timer.note-saved"))))
            };
            (Command::batch(toast.into_iter().chain([Command::focus(STOP)])), None)
        }
        Msg::NoteCancel => {
            if screen.draft.take().is_some() {
                (Command::batch([Command::toast(Toast::info(t!("app.cancelled"))), Command::focus(STOP)]), None)
            } else {
                (Command::none(), None)
            }
        }
        Msg::Idle(true) => {
            // The silence began the threshold ago; the timer re-kinds what it measured since.
            let seconds = u32::try_from(screen.idle_after.as_secs()).unwrap_or(u32::MAX);
            let notices = screen.timer.idle_since(now, seconds);
            screen.now = now;
            (toasts(&notices, units, screen.timer.ceiling_seconds()), Some(Request::Save))
        }
        Msg::Idle(false) => {
            let was_away = screen.timer.is_away();
            let notices = screen.timer.idle_over(now);
            screen.now = now;
            if was_away {
                screen.asking_idle = true;
            }
            (toasts(&notices, units, screen.timer.ceiling_seconds()), was_away.then_some(Request::Save))
        }
        Msg::Claim(decision) => {
            screen.asking_idle = false;
            let seconds = u64::from(screen.timer.idle_seconds(now));
            if !screen.timer.claim_idle(now, decision) {
                return (Command::none(), None);
            }
            screen.now = now;
            let duration = short(seconds, units);
            let toast = match decision {
                IdleDecision::Count => Toast::success(t!("timer.idle-counted", duration = duration)),
                IdleDecision::DontCount => Toast::info(t!("timer.idle-skipped", duration = duration)),
                IdleDecision::Break => Toast::info(t!("timer.idle-break-made", duration = duration)),
            };
            (Command::batch([Command::toast(toast), Command::focus(STOP)]), Some(Request::Save))
        }
    }
}

/// The toasts for what a reading revealed; `ceiling` is the work the session is flagged over.
fn toasts<M: Clone + Send + 'static>(notices: &[Notice], units: &Units<'_>, ceiling: u32) -> Command<M> {
    Command::batch(notices.iter().map(|notice| {
        let toast = match notice {
            Notice::Suspended(seconds, kind) => {
                let duration = short(u64::from(*seconds), units);
                Toast::info(match kind {
                    SpanKind::Pause => t!("timer.suspended-break", duration = duration),
                    SpanKind::Idle => t!("timer.suspended-idle", duration = duration),
                    SpanKind::Work | SpanKind::Gap => t!("timer.suspended", duration = duration),
                })
            }
            Notice::Lagged(seconds, kind) => {
                let duration = short(u64::from(*seconds), units);
                Toast::info(match kind {
                    SpanKind::Pause => t!("timer.lagged-break", duration = duration),
                    SpanKind::Idle => t!("timer.lagged-idle", duration = duration),
                    SpanKind::Work | SpanKind::Gap => t!("timer.lagged", duration = duration),
                })
            }
            Notice::ClockJump(seconds) => {
                Toast::warning(t!("timer.clock-jump", duration = short(seconds.unsigned_abs(), units)))
            }
            Notice::OverCeiling => {
                Toast::warning(t!("timer.over-ceiling", duration = short(u64::from(ceiling), units)))
            }
        };
        Command::toast(toast.duration(Duration::from_secs(8)))
    }))
}

/// Draws the screen: the path to the focus, the big clock, the countdown and goal lines, the
/// break line, the buttons and the note, with durations in the words of `units`. `goals` are
/// the goals of the focus and its category, measured with this counter's work in them. A break
/// takes the muted tone; nothing moves on its own. While the terminal is silent one line says
/// so, so the person sees why the net time stands still; when input comes back the question
/// about the silence stands over the screen. `quiet` fades the lines around the clock, for a
/// screen nobody has touched for a while.
pub fn view<M: From<Msg> + Clone + Send + 'static>(
    screen: &Screen,
    goals: &[GoalRow],
    units: &Units<'_>,
    quiet: bool,
    ui: &mut View<'_, M>,
) {
    let tone = if quiet { "faint" } else { "secondary" };
    let paused = screen.timer.is_paused();
    let work = screen.work_seconds();
    // Before the countdown is reached the clock shows what is left; after it, the work again.
    let shown = match screen.target {
        Some(target) if !screen.reached => u64::from(target).saturating_sub(work),
        _ => work,
    };
    ui.column(|ui| {
        if let Some(reason) = &screen.save_problem {
            warning_line(t!("timer.save-failed", reason = reason.as_str()), ui);
        }
        if !screen.detects_suspend {
            info_line(t!("timer.no-suspend-detection"), ui);
        }
        if screen.timer.is_away() {
            let silence = short(u64::from(screen.timer.idle_seconds(screen.now)), units);
            info_line(t!("timer.away", duration = silence), ui);
        }
        ui.column(|ui| {
            ui.add(Breadcrumb::<M>::new([screen.category.as_str(), screen.focus.as_str()]));
            let big = BigText::new(clock(shown)).variant(if paused { "dim" } else { "accent" });
            ui.add(big);
            if let Some(target) = screen.target {
                let amount = short(u64::from(target), units);
                let line = if screen.reached {
                    t!("timer.target-reached", amount = amount, work = short(work, units))
                } else {
                    t!("timer.countdown", amount = amount)
                };
                let line = super::in_glyphs(line, ui.env().icons().mode());
                ui.add(Text::new(line).role(tone).no_wrap());
            }
            for goal in goals {
                goal_line(goal, units, quiet, ui);
            }
            if paused {
                ui.add(Text::new(t!("timer.paused")).role("faint"));
            }
            buttons(screen, paused, ui);
            if let Some(draft) = &screen.draft {
                ui.add(
                    TextInput::new(draft)
                        .placeholder(t!("timer.note-placeholder"))
                        .on_change(|text| M::from(Msg::NoteTyped(text)))
                        .on_submit(|_| M::from(Msg::NoteSubmit)),
                )
                .id(NOTE_INPUT)
                .width(Length::Cells(48));
            } else if !screen.note.is_empty() {
                ui.add(Text::new(&screen.note).role("faint").no_wrap());
            }
        })
        .gap(1)
        .align(Align::Center)
        .justify(Align::Center)
        .fill();
    })
    .fill();
    if screen.asking_idle {
        idle_question(screen, units, ui);
    }
}

/// One goal under the clock: how far it has come, or that it is reached and by how much it is
/// passed, behind the pillar that marks it as the line to read; faint when the screen is quiet.
fn goal_line<M: 'static>(goal: &GoalRow, units: &Units<'_>, quiet: bool, ui: &mut View<'_, M>) {
    let amount = short(goal.amount, units);
    let text = if goal.reached() {
        t!("timer.goal-reached", amount = amount, over = short(goal.done.saturating_sub(goal.amount), units))
    } else {
        t!("timer.goal-line", done = short(goal.done, units), amount = amount)
    };
    let text = super::in_glyphs(text, ui.env().icons().mode());
    let pillar = ui.env().icons().glyph("pillar").into_owned();
    let line = Text::rich([Span::new(format!("{pillar} ")).color("accent"), Span::new(text)]).no_wrap();
    ui.add(if quiet { line.role("faint") } else { line });
}

/// The question about the latest silence: it cannot be waved off, because the record is
/// flagged until it is answered, and none of the three answers is the default, because the
/// key that ended the silence must not answer it by accident.
fn idle_question<M: From<Msg> + Clone + Send + 'static>(screen: &Screen, units: &Units<'_>, ui: &mut View<'_, M>) {
    let duration = short(u64::from(screen.timer.idle_seconds(screen.now)), units);
    let dialog = Modal::new()
        .title(t!("timer.idle-title", duration = duration))
        .dismissable(false)
        .action(Button::new(t!("timer.idle-count")).on_press(M::from(Msg::Claim(IdleDecision::Count))))
        .action(Button::new(t!("timer.idle-skip")).on_press(M::from(Msg::Claim(IdleDecision::DontCount))))
        .action(Button::new(t!("timer.idle-break")).on_press(M::from(Msg::Claim(IdleDecision::Break))));
    ui.add_with(dialog, |ui| {
        ui.add(Text::new(t!("timer.idle-message")).role("secondary")).fill_width();
    });
}

/// The three buttons: stop, break or resume, note. They stand in one row wherever the language's
/// own words leave room for all three, and take a second row rather than lose one of them.
fn buttons<M: From<Msg> + Clone + Send + 'static>(screen: &Screen, paused: bool, ui: &mut View<'_, M>) {
    let stop = t!("timer.stop");
    let pause = if paused { t!("timer.resume") } else { t!("timer.pause") };
    let note = t!("timer.note");
    let rows = super::button_rows(ui.size().width, &[(&stop, ""), (&pause, "p"), (&note, "n")]);
    let mut made = 0_usize;
    ui.column(|ui| {
        for count in rows {
            ui.row(|ui| {
                for _ in 0..count {
                    match made {
                        0 => {
                            ui.add(Button::new(stop.as_str()).on_press(M::from(Msg::Stop))).id(STOP);
                        }
                        1 => {
                            ui.add(Button::new(pause.as_str()).shortcut("p").on_press(M::from(Msg::Pause)));
                        }
                        _ => {
                            ui.add(
                                Button::new(note.as_str())
                                    .shortcut("n")
                                    .disabled(screen.is_editing_note())
                                    .on_press(M::from(Msg::Note)),
                            );
                        }
                    }
                    made += 1;
                }
            })
            .gap(2);
        }
    })
    .gap(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use qframe::uptime::Uptime;

    const UNITS: Units<'static> = Units { hour: "h", minute: "min", second: "s" };

    fn at(seconds: u64) -> Clocks {
        let seconds = i64::try_from(seconds).unwrap_or(0);
        Clocks {
            wall: 1_758_124_800 + seconds,
            uptime: Uptime {
                awake: Duration::from_secs(1_000 + seconds.unsigned_abs()),
                elapsed: Duration::from_secs(1_000 + seconds.unsigned_abs()),
            },
        }
    }

    fn screen() -> Screen {
        let focus = Id::new(1, 1);
        Screen::new(Timer::start(focus, at(0), 180), at(0), "Work".to_owned(), "Rust".to_owned(), true)
    }

    #[test]
    fn the_running_file_is_asked_for_every_five_seconds_and_on_a_break() {
        let mut screen = screen();
        let mut asked = 0;
        for second in 1..=10 {
            let (_, request): (Command<Msg>, _) = update(&mut screen, Msg::Tick, at(second), &UNITS);
            if request == Some(Request::Save) {
                asked += 1;
                screen.saved(Ok(()));
            }
        }
        assert_eq!(asked, 2);
        let (_, request): (Command<Msg>, _) = update(&mut screen, Msg::Pause, at(11), &UNITS);
        assert_eq!(request, Some(Request::Save));
        assert!(screen.running().paused);
        assert_eq!(screen.work_seconds(), 11);
    }

    #[test]
    fn a_failed_save_stays_until_one_succeeds() {
        let mut screen = screen();
        screen.saved(Err("disk full".to_owned()));
        assert_eq!(screen.save_problem.as_deref(), Some("disk full"));
        let _: (Command<Msg>, _) = update(&mut screen, Msg::Tick, at(1), &UNITS);
        assert!(screen.save_problem.is_some());
        screen.saved(Ok(()));
        assert!(screen.save_problem.is_none());
    }

    #[test]
    fn silence_and_its_answer_each_ask_for_the_running_file() {
        let mut screen = screen();
        let _: (Command<Msg>, _) = update(&mut screen, Msg::Tick, at(600), &UNITS);
        let (_, request): (Command<Msg>, _) = update(&mut screen, Msg::Idle(true), at(1_500), &UNITS);
        assert_eq!(request, Some(Request::Save));
        assert!(screen.is_away());
        assert_eq!(screen.work_seconds(), 600);
        let (_, request): (Command<Msg>, _) = update(&mut screen, Msg::Idle(false), at(2_100), &UNITS);
        assert_eq!(request, Some(Request::Save));
        assert!(!screen.is_away());
        assert!(screen.is_asking_idle());
        let (_, request): (Command<Msg>, _) = update(&mut screen, Msg::Claim(IdleDecision::Count), at(2_100), &UNITS);
        assert_eq!(request, Some(Request::Save));
        assert!(!screen.is_asking_idle());
        assert_eq!(screen.work_seconds(), 2_100);
        // Input coming back with no silence measured is nothing to save or ask about.
        let (_, request): (Command<Msg>, _) = update(&mut screen, Msg::Idle(false), at(2_200), &UNITS);
        assert_eq!(request, None);
        assert!(!screen.is_asking_idle());
    }

    #[test]
    fn a_countdown_is_reached_once_and_the_counter_goes_on() {
        let mut screen = screen().target(60);
        assert_eq!(screen.countdown(), Some(60));
        let (_, request): (Command<Msg>, _) = update(&mut screen, Msg::Tick, at(30), &UNITS);
        assert_eq!(request, Some(Request::Save), "a plain tick: the file is due, nothing is reached");
        screen.saved(Ok(()));
        assert!(!screen.is_reached());
        let (_, request): (Command<Msg>, _) = update(&mut screen, Msg::Tick, at(60), &UNITS);
        assert_eq!(request, Some(Request::Reached));
        assert!(screen.is_reached());
        let (_, request): (Command<Msg>, _) = update(&mut screen, Msg::Tick, at(61), &UNITS);
        assert_eq!(request, Some(Request::Save), "said once; the file is simply due");
        assert_eq!(screen.work_seconds(), 61, "the counter did not stop");
        // A counter restored past its target is reached from the start.
        let restored =
            Screen::new(Timer::start(Id::new(1, 1), at(0), 180), at(90), "W".to_owned(), "R".to_owned(), true);
        assert!(restored.target(60).is_reached());
    }

    #[test]
    fn the_note_goes_into_the_record() {
        let mut screen = screen();
        let _: (Command<Msg>, _) = update(&mut screen, Msg::Note, at(1), &UNITS);
        let _: (Command<Msg>, _) = update(&mut screen, Msg::NoteTyped(" reading ".to_owned()), at(2), &UNITS);
        let _: (Command<Msg>, _) = update(&mut screen, Msg::NoteSubmit, at(3), &UNITS);
        assert!(!screen.is_editing_note());
        let (_, request): (Command<Msg>, _) = update(&mut screen, Msg::Stop, at(60), &UNITS);
        assert_eq!(request, Some(Request::Stop));
        let session = screen.finish(at(60), Id::new(2, 2));
        assert_eq!(session.note, "reading");
        assert_eq!(session.work_seconds(), 60);
    }
}
