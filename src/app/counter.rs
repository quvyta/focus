//! The counter's life: finding a counter left running and taking it over, starting one,
//! ticking it, asking about a session that ran past the ceiling, stopping it, and keeping the
//! running file on disk so the next start knows what happened.
//!
//! Time here comes from the monotonic clock; the wall clock only names the day.

use super::*;

impl QFocus {
    /// Takes up a running file the last run left. In memory there is no file to find.
    ///
    /// A counter nobody measured that is still alive, left by the command line or by a window
    /// told to leave it running, carries on here without a question: the time since counts, as it
    /// would have with the window open. Anything else opens the recovery dialog, saying where the
    /// counter stopped counting and why.
    ///
    /// A file that cannot be read is moved aside at once when this instance may write: the next
    /// counter would otherwise overwrite it at its first refresh, and the next start would report
    /// it again. An instance that only looks leaves it for the one that writes.
    pub(super) fn find_running(&mut self) -> Command<Msg> {
        if !self.on_disk {
            return Command::none();
        }
        let now = self.now();
        if let Some(Ok(running)) = self.store.running()
            && self.store.access.can_write()
        {
            let seen = self.store.seen();
            let reach = liveness::reach_without_window(&running, seen.as_ref(), now, self.knows_boot);
            if reach.cut.is_none() {
                return self.adopt(running, now);
            }
        }
        self.recover = self.store.running().map(|found| match found {
            Ok(running) => {
                let focus = self.store.tree.focus(running.focus).map(|focus| focus.name.clone());
                let ago = u64::try_from(now.wall.saturating_sub(running.refreshed)).unwrap_or(0);
                // An instance that only looks cannot tell a window from its own lock, so it asks
                // whether one holds it, as the command line does.
                let seen = self.store.seen();
                let reach = if self.store.access.can_write() {
                    liveness::reach_without_window(&running, seen.as_ref(), now, self.knows_boot)
                } else {
                    liveness::reach(&running, seen.as_ref(), now, self.knows_boot, || self.store.lock_is_held())
                };
                Recover::Found { running, focus, ago, reach }
            }
            Err(problems) => {
                let here = self.store.paths.running_file();
                if !self.store.access.can_write() {
                    return Recover::Broken { problems, path: here, kept: Kept::Untouched };
                }
                match self.store.set_aside_running(&broken_stamp(now.wall, self.offset_minutes())) {
                    Ok(aside) => {
                        // The lines and columns now belong to the copy; that is the file to open.
                        let (old, new) = (here.display().to_string(), aside.display().to_string());
                        let problems = problems
                            .into_iter()
                            .map(|mut problem| {
                                if let Some(location) = problem.location.as_mut().filter(|at| at.file == old) {
                                    location.file.clone_from(&new);
                                }
                                problem
                            })
                            .collect();
                        Recover::Broken { problems, path: aside, kept: Kept::Aside }
                    }
                    Err(error) => Recover::Broken { problems, path: here, kept: Kept::Stuck(error.to_string()) },
                }
            }
        });
        Command::none()
    }

    /// Carries on a counter nobody measured and that is still alive at `now`: the time since its
    /// last refresh counts as the kind that was open, the counter screen opens as after
    /// "Continue", and from here this window measures it, so the file says so at once.
    fn adopt(&mut self, running: Running, now: Clocks) -> Command<Msg> {
        let timer = Timer::restore(
            running.focus,
            running.started,
            running.offset_minutes,
            running.spans,
            running.paused,
            running.refreshed,
            now.wall,
            now,
        )
        .ceiling(self.prefs.ceiling)
        .with_flags(running.flags)
        .with_unclaimed_idle(running.idle_from)
        .source(Source::Timer);
        let (category, name) =
            self.names(running.focus).unwrap_or_else(|| (String::new(), t!("recover.unknown-focus")));
        let duration = short(timer.work_seconds(now), &units().as_units());
        let said = Toast::info(t!("recover.adopted", focus = name.clone(), duration = duration));
        self.open_timer(timer, now, category, name, running.target, said)
    }

    /// Starts counting `focus`.
    pub(super) fn start(&mut self, focus: Id) -> Command<Msg> {
        self.start_for(focus, None)
    }

    /// Starts counting `focus`, counting down to `target` seconds of work when one is given.
    pub(super) fn start_for(&mut self, focus: Id, target: Option<u32>) -> Command<Msg> {
        if !self.can_start() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let Some((category, name)) = self.names(focus) else { return Command::none() };
        let now = self.now();
        let timer = Timer::start(focus, now, self.offset_minutes()).ceiling(self.prefs.ceiling);
        let said = Toast::info(t!("today.started", name = name.clone()));
        self.open_timer(timer, now, category, name, target, said)
    }

    /// Opens the Timer screen over `timer`, writes the running file, starts the ticks and shows
    /// `said`.
    pub(super) fn open_timer(
        &mut self,
        timer: Timer,
        now: Clocks,
        category: String,
        name: String,
        target: Option<u32>,
        said: Toast<Msg>,
    ) -> Command<Msg> {
        let mut screen =
            TimerScreen::new(timer, now, category, name, Uptime::detects_suspend()).idle_after(self.prefs.idle_after);
        if let Some(target) = target {
            screen = screen.target(target);
        }
        screen.saved(self.save_running(&screen));
        // Goals already reached before this session are not news; only a crossing is said.
        self.goals_said = self.timer_goals(&screen).iter().filter(|goal| goal.reached()).map(|goal| goal.id).collect();
        self.goals_week = self.week_start();
        self.timer = Some(screen);
        Command::batch([Command::toast(said), Command::focus(timer_screen::STOP), self.sync_tick()])
    }

    /// Writes the running file for `screen`, measured by this window, or says why it could not
    /// be. In memory nothing is written and nothing is wrong.
    fn save_running(&self, screen: &TimerScreen) -> Result<(), String> {
        self.save_running_as(screen, Watch::Window)
    }

    /// Writes the running file for `screen`, saying `watch` measures it from here on.
    fn save_running_as(&self, screen: &TimerScreen, watch: Watch) -> Result<(), String> {
        if !self.on_disk {
            return Ok(());
        }
        let running = Running { watch, ..screen.running() };
        self.store.save_running(&running).map_err(|error| error.to_string())
    }

    /// Ends the running counter at `now`, records the session and returns to Today.
    pub(super) fn stop(&mut self, now: Clocks) -> Command<Msg> {
        self.stop_with(now, None)
    }

    /// The counter is being stopped past the ceiling: the session is not recorded yet, the
    /// person is asked whether to fix its duration first, and the counter goes on until they
    /// answer. Cancel has focus, so the key that stopped the counter answers nothing.
    fn ask_over_ceiling(&self) -> Command<Msg> {
        let Some(screen) = self.timer.as_ref() else { return Command::none() };
        let units = units();
        let question = Confirm::new(
            t!("timer.ceiling-title", duration = short(screen.work_seconds(), &units.as_units())),
            Msg::StopFix,
        )
        .message(t!("timer.ceiling-message", ceiling = short(u64::from(self.prefs.ceiling), &units.as_units())))
        .confirm_label(t!("timer.ceiling-fix"))
        .alternative(t!("timer.ceiling-leave"), Msg::StopLeave)
        .cancel_label(t!("app.quit-cancel"))
        .on_cancel(Msg::StopCancel);
        Command::confirm(question)
    }

    /// Opens the form on the duration of the session the counter would stop with now. The
    /// counter keeps running underneath: nothing is recorded until the form is saved, and
    /// cancelling it leaves the counter as it was.
    pub(super) fn fix_over_ceiling(&mut self) -> Command<Msg> {
        let Some(screen) = self.timer.as_ref() else { return Command::none() };
        let now = self.now();
        let preview = screen.clone().finish(now, Id::new(0, 0));
        self.open_form(RecordForm::of(Purpose::Finish, &preview))
    }

    /// Ends the running counter at `now` and records it with the person's `changes` applied:
    /// the way out of the form that fixes a duration over the ceiling.
    pub(super) fn stop_fixed(&mut self, changes: &Changes, now: Clocks) -> Command<Msg> {
        self.stop_with(now, Some(changes))
    }

    /// Ends the running counter at `now`, with `changes` applied to the session when the person
    /// typed some, records it and returns to Today.
    fn stop_with(&mut self, now: Clocks, changes: Option<&Changes>) -> Command<Msg> {
        let Some(screen) = self.timer.take() else { return Command::none() };
        let mut session = screen.finish(now, clock::new_id());
        if let Some(changes) = changes {
            session = edit::adjusted(&session, changes);
        }
        let focus = session.focus;
        let name = self.store.tree.focus(focus).map_or_else(|| t!("records.unknown-focus"), |focus| focus.name.clone());
        let duration = short(session.work_seconds(), &units().as_units());
        let recorded = self.record(session);
        let cleared = self.clear_running();
        let _: (Command<Msg>, _) =
            today::update(&mut self.today, &mut self.store.tree, today::Msg::Select(Row::Focus(focus).key()));
        Command::batch([
            recorded,
            cleared,
            Command::toast(Toast::success(t!("timer.stopped", name = name, duration = duration))),
            Command::focus(today::TREE),
            self.sync_tick(),
        ])
    }

    /// Removes the running file once nothing runs, unless a record still waits in memory: then
    /// the file is the only copy of the session and stays.
    pub(super) fn clear_running(&self) -> Command<Msg> {
        if !self.on_disk || !self.pending.is_empty() {
            return Command::none();
        }
        match self.store.clear_running() {
            Ok(()) => Command::none(),
            Err(error) => Command::toast(Toast::danger(t!("app.write-failed", reason = error.to_string()))),
        }
    }

    /// Starts or stops the tick task to match what needs it: a running counter, or a lock still
    /// held by another instance.
    pub(super) fn sync_tick(&mut self) -> Command<Msg> {
        let needed = self.timer.is_some() || (self.on_disk && !self.store.access.can_write());
        match (needed, self.tick) {
            (true, None) => {
                let task = Task::new("tick", |cx| {
                    while cx.sleep(std::time::Duration::from_secs(1)) {
                        cx.send(Msg::Tick);
                    }
                    Ok(Msg::Tick)
                });
                self.tick = Some(task.id());
                Command::task(task)
            }
            (false, Some(id)) => {
                self.tick = None;
                Command::cancel_task(id)
            }
            _ => Command::none(),
        }
    }

    /// A second passed: the counter ticks, and a lock another instance holds is tried again.
    pub(super) fn tick(&mut self) -> Command<Msg> {
        if self.on_disk && !self.store.access.can_write() && self.store.retry_lock() {
            return self.sync_tick();
        }
        if self.timer.is_some() { self.timer_message(timer_screen::Msg::Tick) } else { self.sync_tick() }
    }

    /// Applies a message of the Timer screen and does what it asks.
    pub(super) fn timer_message(&mut self, message: timer_screen::Msg) -> Command<Msg> {
        let Some(screen) = self.timer.as_mut() else { return Command::none() };
        let now = (self.clock)();
        let (command, request) = timer_screen::update(screen, message, now, &units().as_units());
        match request {
            Some(timer_screen::Request::Save) => {
                let result = if self.on_disk {
                    self.store.save_running(&screen.running()).map_err(|error| error.to_string())
                } else {
                    Ok(())
                };
                screen.saved(result);
                Command::batch([command, self.goals_crossed(now)])
            }
            Some(timer_screen::Request::Stop) if screen.running().flags.contains(&Flag::OverCeiling) => {
                Command::batch([command, self.ask_over_ceiling()])
            }
            Some(timer_screen::Request::Stop) => Command::batch([command, self.stop(now)]),
            Some(timer_screen::Request::Reached) => {
                // The countdown is up: the counter goes on unless the person asked it to stop.
                if self.prefs.stop_at_goal {
                    Command::batch([command, self.stop(now)])
                } else {
                    Command::batch([command, self.goals_crossed(now)])
                }
            }
            None => Command::batch([command, self.goals_crossed(now)]),
        }
    }

    /// The goals of the focus the counter counts and of its category, with the counter's work
    /// counted in.
    pub(super) fn timer_goals(&self, screen: &TimerScreen) -> Vec<GoalRow> {
        let focus = screen.focus();
        let category = self.store.tree.categories.iter().find(|c| c.focuses.iter().any(|f| f.id == focus));
        let mine = |goal: &GoalRow| goal.id == focus || category.is_some_and(|c| c.id == goal.id);
        goal_rows(
            &self.store.tree,
            &self.sessions,
            self.date,
            &self.prefs,
            self.week_start(),
            Some((focus, screen.work_seconds())),
        )
        .into_iter()
        .filter(mine)
        .collect()
    }

    /// Says once each goal the running counter has just reached, and stops the counter for it
    /// when the person asked for that.
    fn goals_crossed(&mut self, now: Clocks) -> Command<Msg> {
        let Some(screen) = self.timer.as_ref() else { return Command::none() };
        let week = self.week_start();
        if week != self.goals_week {
            self.goals_said =
                self.timer_goals(screen).iter().filter(|goal| goal.reached()).map(|goal| goal.id).collect();
            self.goals_week = week;
            return Command::none();
        }
        let crossed: Vec<GoalRow> = self
            .timer_goals(screen)
            .into_iter()
            .filter(|goal| goal.reached() && !self.goals_said.contains(&goal.id))
            .collect();
        if crossed.is_empty() {
            return Command::none();
        }
        let units = units();
        let mut commands: Vec<Command<Msg>> = crossed
            .iter()
            .map(|goal| {
                let text = t!(
                    "timer.goal-reached-toast",
                    name = goal.name.clone(),
                    amount = short(goal.amount, &units.as_units())
                );
                Command::toast(Toast::success(text))
            })
            .collect();
        self.goals_said.extend(crossed.iter().map(|goal| goal.id));
        if self.prefs.stop_at_goal {
            commands.push(self.stop(now));
        }
        Command::batch(commands)
    }

    /// Brings the running file up to this moment, saying `watch` measures it from here on.
    /// `true` when it is on disk, or when nothing runs; `false` when the write failed, which the
    /// counter's screen then says.
    pub(super) fn refresh_running(&mut self, watch: Watch) -> bool {
        let now = self.now();
        let Some(mut screen) = self.timer.take() else { return true };
        let (_, _): (Command<Msg>, _) =
            timer_screen::update(&mut screen, timer_screen::Msg::Tick, now, &units().as_units());
        let saved = self.save_running_as(&screen, watch);
        let failed = saved.is_err();
        screen.saved(saved);
        self.timer = Some(screen);
        !failed
    }

    /// The dashboard comes over the page, or leaves it: the page underneath is not touched, so
    /// what was selected and filtered stays. While it stands a task turns its clock at every
    /// minute; the first wait ends on the minute so the clock never lags behind the real one.
    pub(super) fn idle(&mut self, away: bool) -> Command<Msg> {
        self.idle = away;
        match (away, self.minute) {
            (true, None) => self.arm_minute(),
            (false, Some(id)) => {
                self.minute = None;
                Command::cancel_task(id)
            }
            _ => Command::none(),
        }
    }

    /// Starts the wait for the next minute of the dashboard's clock.
    fn arm_minute(&mut self) -> Command<Msg> {
        let wall = self.now().wall;
        let until = 60 - u64::try_from(wall.rem_euclid(60)).unwrap_or(0);
        let task = Task::new("dashboard-clock", move |cx| {
            // A cancelled wait ends the task; its message is then ignored anyway.
            if cx.sleep(Duration::from_secs(until)) { Ok(Msg::Minute) } else { Err("stopped".to_owned()) }
        });
        self.minute = Some(task.id());
        Command::task(task)
    }

    /// A minute turned: the frame is drawn again with it, and the next minute is waited for as
    /// long as the dashboard stands.
    pub(super) fn minute(&mut self) -> Command<Msg> {
        self.minute = None;
        if self.idle { self.arm_minute() } else { Command::none() }
    }
}
