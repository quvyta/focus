//! What the person sees: the dashboard that stands over a quiet screen, the header with its
//! tabs and the body under it, the hints along the bottom, and the question asked on the way
//! out. Nothing here changes the records; it only draws what the application already holds.

use super::*;

impl QFocus {
    /// The dashboard: a layer over the page for a terminal left open, with the time large, the
    /// day's strip, the focuses worked on most today and how full the week is. The page keeps
    /// its state underneath; the first input brings it back as it was.
    pub(super) fn dashboard(&self, ui: &mut View<'_, Msg>) {
        let now = self.now();
        let local = DateTime::from_unix(now.wall, self.offset_minutes()).time;
        let time = format!("{:02}:{:02}", local.hour, local.minute);
        let width = ui.size().width.saturating_sub(2 * DASHBOARD_MARGIN).max(1);
        let blocks = stats::strip(&self.sessions, None, self.date, self.prefs.rollover);
        let units = units();
        let top = stats::top_focuses(&self.sessions, stats::Range::day(self.date), DASHBOARD_TOP, self.prefs.rollover);
        let goals = goal_rows(&self.store.tree, &self.sessions, self.date, &self.prefs, self.week_start(), None);
        let week = stats::goal_range(crate::tree::Period::Week, self.date, self.week_start());
        let week_total = stats::total(&self.sessions, week, self.prefs.rollover);
        let dialog = Modal::new().dismissable(false).width(width);
        ui.add_with(dialog, |ui| {
            ui.column(|ui| {
                ui.add(BigText::new(time).variant("accent"));
                today::strip_view(&self.today, &self.store.tree, &blocks, self.prefs.rollover, ui);
                if !top.is_empty() {
                    ui.add(Text::new(t!("dashboard.top-title")).role("secondary").no_wrap());
                    let bars = top.iter().map(|(id, seconds)| {
                        let name =
                            self.store.tree.focus(*id).map_or_else(|| t!("records.unknown-focus"), |f| f.name.clone());
                        Bar::new(name, (*seconds / 60) as f32).value_text(short(*seconds, &units.as_units()))
                    });
                    ui.add(BarChart::<Msg>::new(bars).gap(0)).fill_width();
                }
                if goals.is_empty() {
                    ui.add(
                        Text::new(t!("dashboard.week", duration = short(week_total, &units.as_units())))
                            .role("secondary"),
                    )
                    .fill_width();
                } else {
                    ui.add(Text::new(t!("dashboard.goals-title")).role("secondary").no_wrap());
                    goal_gauges(&goals, &units.as_units(), ui);
                }
            })
            .gap(1)
            .align(Align::Center)
            .fill_width();
        });
    }

    /// The top: the name, the day and its total, the tabs, and every standing warning under them.
    pub(super) fn header(&self, ui: &mut View<'_, Msg>) {
        let day = self.date;
        let month = t!(&format!("months.{}", day.month()));
        let long = t!(
            "today.date",
            weekday = t!(&format!("days.{}", day.weekday().number())),
            day = u32::from(day.day()),
            month = month.clone()
        );
        let short_date = t!("today.date-short", day = u32::from(day.day()), month = month);
        let name = t!("app.name");
        let total = short(self.total_today(), &units().as_units());
        // The day gives up its weekday, then itself, before the total is cut: on a narrow
        // terminal, and in languages with long day and month names, the total is what the
        // header is read for.
        // The row keeps two cells to spare: the spacer between the day and the total takes one
        // and the row's own measure rounds one more away.
        let room = ui.size().width.saturating_sub(HEADER_PADDING * 2);
        let fits = |date: &str| {
            [qframe::text::width(&name), 2, qframe::text::width(date), 2, qframe::text::width(&total)]
                .iter()
                .sum::<u16>()
                + 2
                <= room
        };
        let date = [long, short_date].into_iter().find(|date| fits(date));
        ui.column(|ui| {
            ui.row(|ui| {
                ui.add(Text::rich([Span::new(name).color("accent").bold()]).no_wrap());
                if let Some(date) = date {
                    ui.add(Text::new(date).role("secondary").no_wrap());
                }
                ui.spacer();
                ui.add(Text::new(total).bold().no_wrap());
            })
            .gap(2)
            .fill_width();
            ui.add(
                Tabs::new([t!("tabs.today"), t!("tabs.charts"), t!("tabs.records"), t!("tabs.settings")])
                    .active(self.page.index())
                    .on_select(Msg::Page),
            )
            .fill_width();
            if !self.on_disk {
                warning_line(t!("app.memory-only"), ui);
            }
            match self.store.access {
                Access::Writer => {}
                Access::ReadOnly { holder: Some(pid) } if self.on_disk => {
                    warning_line(t!("app.read-only-pid", pid = pid), ui);
                }
                Access::ReadOnly { .. } if self.on_disk => warning_line(t!("app.read-only"), ui),
                Access::ReadOnly { .. } => {}
                Access::NoLock => info_line(t!("app.no-lock"), ui),
            }
            // On the Records page the count stands above the table, with the way to the list; on
            // the Charts page it stands above the charts drawn without those records.
            let problems = u32::try_from(self.store.diagnostics.len()).unwrap_or(u32::MAX);
            if problems > 0 && self.page == Page::Today {
                warning_line(t!("app.diagnostics", n = problems), ui);
            }
            if self.offset.is_none() {
                info_line(t!("app.unknown-zone"), ui);
            }
            if self.on_disk && !self.pending.is_empty() {
                warning_line(t!("app.unsaved", n = u32::try_from(self.pending.len()).unwrap_or(u32::MAX)), ui);
            }
        })
        .padding(Padding::symmetric(0, HEADER_PADDING))
        .fill_width();
    }

    /// The open page: on Today the counter while one runs, else the tree; on Charts the charts;
    /// on Records the records.
    pub(super) fn body(&self, ui: &mut View<'_, Msg>) {
        match (self.page, &self.timer) {
            (Page::Today, timer) => {
                // The strip stands over the counter as well as over the tree: the counter is
                // the day's open block and grows on it with every tick.
                let running = timer.as_ref().map(TimerScreen::running);
                let blocks = stats::strip(&self.sessions, running.as_ref(), self.date, self.prefs.rollover);
                ui.column(|ui| {
                    today::strip_view(&self.today, &self.store.tree, &blocks, self.prefs.rollover, ui);
                    match timer {
                        Some(screen) => {
                            let goals = self.timer_goals(screen);
                            timer_screen::view(screen, &goals, &units().as_units(), self.quiet, ui);
                        }
                        None => {
                            let goals = goal_rows(
                                &self.store.tree,
                                &self.sessions,
                                self.date,
                                &self.prefs,
                                self.week_start(),
                                None,
                            );
                            today::view(
                                &self.today,
                                &self.store.tree,
                                &self.totals,
                                &goals,
                                &units().as_units(),
                                (self.can_start(), self.prefs.default_goal),
                                ui,
                            );
                        }
                    }
                })
                .fill();
            }
            (Page::Charts, timer) => {
                let goals =
                    goal_rows(&self.store.tree, &self.sessions, self.date, &self.prefs, self.week_start(), None);
                charts::view(
                    &self.charts,
                    &self.sessions,
                    &self.store,
                    self.date,
                    &self.prefs,
                    &goals,
                    timer.is_some(),
                    &units().as_units(),
                    ui,
                );
            }
            (Page::Records, _) => {
                records::view(&self.records, &self.sessions, &self.store, &units().as_units(), self.can_edit(), ui);
            }
            (Page::Settings, _) => {
                settings::view(
                    &self.settings_screen,
                    &self.prefs,
                    &self.appearance,
                    self.date,
                    &units().as_units(),
                    self.can_edit(),
                    ui,
                );
            }
        }
    }

    /// The keys the open screen answers to.
    pub(super) fn hints(&self, ui: &mut View<'_, Msg>) {
        let icons = ui.env().icons();
        let enter = icons.glyph("enter").into_owned();
        let arrows = format!("{}{}", icons.glyph("arrow-left"), icons.glyph("arrow-right"));
        let updown = format!("{}{}", icons.glyph("arrow-up"), icons.glyph("arrow-down"));
        let hints = KeyHints::new();
        let hints = match (self.page, self.timer.is_some(), self.records.view_kind()) {
            (Page::Today, true, _) => {
                hints.hint("space", t!("hints.stop")).action(Scope::App, "pause").action(Scope::App, "note")
            }
            (Page::Today, false, _) => hints
                .hint(enter, t!("hints.start"))
                .action(Scope::App, "goal")
                .action(Scope::App, "countdown")
                .action(Scope::App, "archive")
                .action(Scope::App, "rename"),
            (Page::Charts, _, _) => {
                let keys = if self.charts.bars_lie_down(ui.size().width) { updown.clone() } else { arrows };
                hints.hint(keys, t!("hints.pick")).action(Scope::App, "today").action(Scope::App, "records")
            }
            (Page::Records, _, records::ViewKind::All) => hints
                .hint(enter, t!("hints.correct"))
                .action(Scope::App, "add")
                .action(Scope::App, "spans")
                .hint("delete", t!("hints.delete"))
                .action(Scope::App, "search")
                .action(Scope::App, "export")
                .action(Scope::App, "undo"),
            (Page::Records, _, records::ViewKind::Trash | records::ViewKind::Archive) => {
                hints.hint(enter, t!("hints.restore")).action(Scope::App, "search")
            }
            (Page::Records, _, records::ViewKind::Problems) => {
                let lost = self.sessions.iter().any(|session| self.store.tree.focus(session.focus).is_none());
                if lost { hints.hint(enter, t!("hints.attach")) } else { hints }.action(Scope::App, "today")
            }
            (Page::Settings, _, _) => {
                hints.hint(updown, t!("hints.move")).hint(enter, t!("hints.change")).action(Scope::App, "undo")
            }
        };
        let hints = if self.form.is_some() { KeyHints::new().action(Scope::App, "back") } else { hints };
        ui.add(hints.action_right(Scope::Global, "quit")).fill_width();
    }

    /// The question asked before leaving while a counter runs: three ways, none of them the
    /// default.
    pub(super) fn quit_question(&self, ui: &mut View<'_, Msg>) {
        let name = self.timer.as_ref().map(|screen| screen.focus_name().to_owned()).unwrap_or_default();
        let dialog = Modal::new()
            .title(t!("app.quit-title", focus = name))
            .on_close(Msg::QuitCancel)
            .action(Button::new(t!("app.quit-cancel")).on_press(Msg::QuitCancel))
            .action(Button::new(t!("app.quit-leave")).on_press(Msg::QuitLeave))
            .action(Button::new(t!("app.quit-finish")).variant("primary").on_press(Msg::QuitFinish));
        ui.add_with(dialog, |ui| {
            ui.add(Text::new(t!("app.quit-message")).role("secondary")).fill_width();
        });
    }
}
