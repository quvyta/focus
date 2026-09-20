//! The person's data and every way it changes: a session recorded, corrected by hand or
//! removed, days deleted, the trash emptied, the lists reset, the tree and the settings
//! written. Each step also says how to take it back, because these are the records of work
//! nobody wants to lose by pressing one key.

use super::*;

/// "3 records", counted for the purge's question and its answer.
fn purge_records(count: usize) -> String {
    t!("settings.purge-records", n = u32::try_from(count).unwrap_or(u32::MAX))
}

/// "2 archived rows", counted for the purge's question and its answer.
fn purge_rows(count: usize) -> String {
    t!("settings.purge-rows", n = u32::try_from(count).unwrap_or(u32::MAX))
}

impl QFocus {
    /// Writes the catalogue; a failure is said and the catalogue stays as it is in memory.
    pub(super) fn save_tree(&self) -> Command<Msg> {
        if !self.on_disk {
            return Command::none();
        }
        match self.store.save_tree() {
            Ok(()) => Command::none(),
            Err(error) => Command::toast(Toast::danger(t!("app.tree-write-failed", reason = error.to_string()))),
        }
    }

    /// Writes `session` and everything waiting before it. What cannot be written waits in
    /// memory and is said; the running file is cleared only for a session that is on disk.
    pub(super) fn record(&mut self, session: Session) -> Command<Msg> {
        self.pending.push(session);
        let mut problem = None;
        while let Some(next) = self.pending.first().cloned() {
            match self.store.record(&next) {
                Ok(()) => {
                    self.pending.remove(0);
                }
                Err(error) => {
                    problem = Some(error.to_string());
                    break;
                }
            }
        }
        self.refresh_day();
        match problem {
            Some(reason) if self.on_disk => Command::toast(Toast::danger(t!("app.write-failed", reason = reason))),
            _ => Command::none(),
        }
    }

    /// Writes the settings file off the drawing thread; the screen says if it could not be.
    pub(super) fn save_settings(&self) -> Command<Msg> {
        self.settings.save_command(|result| Msg::Settings(settings::Msg::Stored(result)))
    }

    /// Moves every current session to the trash, one line per record, and offers the way back.
    /// A pass that stops half way says how far it got; the rest stays current for another pass.
    pub(super) fn reset_stats(&mut self) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let total = u32::try_from(self.sessions.len()).unwrap_or(u32::MAX);
        if total == 0 {
            return Command::toast(Toast::info(t!("settings.nothing-to-reset")));
        }
        let now = self.now();
        let (moved, error) = self.store.void_all(now.wall);
        self.refresh_day();
        let done = u32::try_from(moved.len()).unwrap_or(u32::MAX);
        if !moved.is_empty() {
            self.last_undo = Some(Undo::Wiped { sessions: moved, rows: Vec::new() });
        }
        match error {
            Some(error) => Command::toast(Toast::danger(t!(
                "settings.reset-partial",
                done = done,
                total = total,
                reason = error.to_string()
            ))),
            None => {
                Command::toast(Toast::success(t!("settings.reset-done", n = done)).action(t!("today.undo"), Msg::Undo))
            }
        }
    }

    /// Moves every current session whose day, grouped as the Records page and the charts group
    /// it, is before `date` to the trash, one line per record, and offers the way back.
    pub(super) fn delete_older(&mut self, date: Date) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let rollover = self.prefs.rollover;
        let older = |session: &Session| crate::day::day_of(session, rollover) < date;
        let total =
            u32::try_from(self.store.sessions.iter().filter(|session| older(session)).count()).unwrap_or(u32::MAX);
        if total == 0 {
            return Command::toast(Toast::info(t!("settings.nothing-older")));
        }
        let now = self.now();
        let (moved, error) = self.store.void_matching(older, now.wall);
        self.refresh_day();
        let done = u32::try_from(moved.len()).unwrap_or(u32::MAX);
        if !moved.is_empty() {
            self.last_undo = Some(Undo::Wiped { sessions: moved, rows: Vec::new() });
        }
        match error {
            Some(error) => Command::toast(Toast::danger(t!(
                "settings.reset-partial",
                done = done,
                total = total,
                reason = error.to_string()
            ))),
            None => {
                Command::toast(Toast::success(t!("settings.reset-done", n = done)).action(t!("today.undo"), Msg::Undo))
            }
        }
    }

    /// Moves every current session to the trash and archives every category and focus, so the
    /// lists are empty and the past is in the trash; the way back brings both.
    pub(super) fn delete_all(&mut self) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let total = u32::try_from(self.sessions.len()).unwrap_or(u32::MAX);
        let now = self.now();
        let (moved, error) = self.store.void_all(now.wall);
        self.refresh_day();
        let done = u32::try_from(moved.len()).unwrap_or(u32::MAX);
        if let Some(error) = error {
            if !moved.is_empty() {
                self.last_undo = Some(Undo::Wiped { sessions: moved, rows: Vec::new() });
            }
            let text = t!("settings.reset-partial", done = done, total = total, reason = error.to_string());
            return Command::toast(Toast::danger(text));
        }
        let mut rows = Vec::new();
        for category in &mut self.store.tree.categories {
            if !category.archived {
                category.archived = true;
                rows.push(Row::Category(category.id));
            }
            for focus in &mut category.focuses {
                if !focus.archived {
                    focus.archived = true;
                    rows.push(Row::Focus(focus.id));
                }
            }
        }
        let archived = u32::try_from(rows.len()).unwrap_or(u32::MAX);
        let saved = self.save_tree();
        self.today = Today::new();
        if !moved.is_empty() || !rows.is_empty() {
            self.last_undo = Some(Undo::Wiped { sessions: moved, rows });
        }
        let toast =
            Toast::success(t!("settings.delete-done", n = done, rows = archived)).action(t!("today.undo"), Msg::Undo);
        Command::batch([saved, Command::toast(toast)])
    }

    /// The focuses something outside the store's records still refers to: the records waiting in
    /// memory, the running counter and a running file waiting to be recovered. Emptying the
    /// trash must not take them out of the tree.
    fn kept_focuses(&self) -> Vec<Id> {
        let mut focuses: Vec<Id> = self.pending.iter().map(|session| session.focus).collect();
        focuses.extend(self.timer.as_ref().map(|screen| screen.running().focus));
        if self.on_disk
            && let Some(Ok(running)) = self.store.running()
        {
            focuses.push(running.focus);
        }
        focuses
    }

    /// Says what emptying the trash would take and asks, in the danger tone, before doing it.
    /// With nothing to take nothing is asked; copies alone in the trash folder are backups and
    /// wait for the next purge that takes something.
    pub(super) fn ask_purge(&mut self) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let plan = self.store.purge_plan(&self.kept_focuses());
        if plan.is_empty() {
            return Command::toast(Toast::info(t!("settings.purge-nothing")));
        }
        let message = t!("settings.purge-message", records = purge_records(plan.records), rows = purge_rows(plan.rows));
        let question = Confirm::new(t!("settings.purge-title"), Msg::Settings(settings::Msg::Purge))
            .message(message)
            .danger()
            .confirm_label(t!("settings.purge-confirm"))
            .require_word(t!("settings.purge-word"))
            .on_cancel(Msg::Settings(settings::Msg::PurgeCancelled));
        Command::confirm(question)
    }

    /// Empties the trash for good and says what went and what stayed. Nothing of it can be taken
    /// back, so the pending `ctrl+z` goes too: it would reach for records no longer on the disk.
    pub(super) fn purge(&mut self) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let result = self.store.purge(&self.kept_focuses());
        self.last_undo = None;
        self.refresh_day();
        match result {
            Ok(purged) => {
                let done =
                    t!("settings.purged", records = purge_records(purged.records), rows = purge_rows(purged.rows));
                let mut toasts = vec![Command::toast(Toast::success(done))];
                if purged.kept_rows > 0 {
                    let kept = t!("settings.purge-kept", n = u32::try_from(purged.kept_rows).unwrap_or(u32::MAX));
                    toasts.push(Command::toast(Toast::info(kept)));
                }
                Command::batch(toasts)
            }
            Err(error) => Command::toast(Toast::danger(t!("settings.purge-failed", reason = error.to_string()))),
        }
    }

    /// Brings back what a reset or a deletion took: every session from the trash, each with its
    /// own line, and every row from the archive. A pass that stops half way says how far it got.
    fn unwipe(&mut self, sessions: Vec<Id>, rows: Vec<Row>) -> Command<Msg> {
        if !self.can_edit() {
            self.last_undo = Some(Undo::Wiped { sessions, rows });
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let now = self.now();
        let total = u32::try_from(sessions.len()).unwrap_or(u32::MAX);
        let mut restored = 0_u32;
        let mut problem = None;
        let mut left = Vec::new();
        for id in sessions {
            if problem.is_some() {
                left.push(id);
                continue;
            }
            match self.store.restore(id, now.wall) {
                Ok(()) => restored += 1,
                Err(error) => {
                    problem = Some(error.to_string());
                    left.push(id);
                }
            }
        }
        for row in &rows {
            match row {
                Row::Category(id) => {
                    if let Some(category) = self.store.tree.categories.iter_mut().find(|c| c.id == *id) {
                        category.archived = false;
                    }
                }
                Row::Focus(id) => {
                    if let Some(focus) =
                        self.store.tree.categories.iter_mut().flat_map(|c| c.focuses.iter_mut()).find(|f| f.id == *id)
                    {
                        focus.archived = false;
                    }
                }
                Row::AddFocus(_) | Row::AddCategory => {}
            }
        }
        let saved = if rows.is_empty() { Command::none() } else { self.save_tree() };
        self.refresh_day();
        match problem {
            Some(reason) => {
                // What did not come back can be tried again with the next ctrl+z.
                self.last_undo = Some(Undo::Wiped { sessions: left, rows: Vec::new() });
                let text = t!("settings.restore-partial", done = restored, total = total, reason = reason);
                Command::batch([saved, Command::toast(Toast::danger(text))])
            }
            None => Command::batch([saved, Command::toast(Toast::success(t!("settings.restored-all", n = restored)))]),
        }
    }

    /// Opens the empty form for a session typed in by hand. A memory-only store takes it too:
    /// the record waits with the others that could not be written.
    pub(super) fn add_by_hand(&mut self) -> Command<Msg> {
        if !self.can_start() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let now = DateTime::from_unix(self.now().wall, self.offset_minutes());
        self.open_form(RecordForm::add(now))
    }

    /// Opens the form over the current session `id` for `purpose`.
    pub(super) fn open_over(&mut self, purpose: Purpose, id: Id) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        match self.sessions.iter().find(|session| session.id == id) {
            Some(session) => self.open_form(RecordForm::of(purpose, session)),
            None => Command::none(),
        }
    }

    /// Puts `form` over the page and the keyboard on its first field.
    pub(super) fn open_form(&mut self, form: RecordForm) -> Command<Msg> {
        let first = form.first_field();
        self.form = Some(form);
        Command::focus(first)
    }

    /// Applies a message of the record form, and writes the record it hands back.
    pub(super) fn form_message(&mut self, message: record_form::Msg) -> Command<Msg> {
        let Some(form) = self.form.as_mut() else { return Command::none() };
        let now = (self.clock)();
        let (command, outcome) = record_form::update(form, &self.store.tree, now.wall, message);
        match outcome {
            Some(Outcome::Save(changes)) => {
                let purpose = form.purpose().clone();
                self.form = None;
                Command::batch([command, self.save_form(&purpose, &changes, now), self.focus_after_form()])
            }
            Some(Outcome::Cancel) => {
                self.form = None;
                Command::batch([command, Command::toast(Toast::info(t!("app.cancelled"))), self.focus_after_form()])
            }
            None => command,
        }
    }

    /// Where the keyboard goes once the form closes: the table on Records, the counter or the
    /// tree on Today.
    fn focus_after_form(&mut self) -> Command<Msg> {
        self.open_page(self.page.index())
    }

    /// Writes what the form handed back: a new record by hand, a correction that replaces the
    /// session it was opened over, or the session the counter stopped with, fixed.
    fn save_form(&mut self, purpose: &Purpose, changes: &Changes, now: Clocks) -> Command<Msg> {
        let name = self.store.tree.focus(changes.focus).map_or_else(String::new, |focus| focus.name.clone());
        let duration = short(u64::from(changes.seconds), &units().as_units());
        match purpose {
            Purpose::Add => {
                let session = edit::manual(changes, clock::new_id(), now.wall);
                let recorded = self.record(session);
                Command::batch([
                    recorded,
                    Command::toast(Toast::success(t!("records.added", name = name, duration = duration))),
                ])
            }
            Purpose::Correct(id) | Purpose::Attach(id) => {
                let Some(original) = self.sessions.iter().find(|session| session.id == *id).cloned() else {
                    return Command::none();
                };
                let correction = edit::correction(&original, changes, clock::new_id(), now.wall);
                let written = correction.id;
                let recorded = self.record(correction);
                self.last_undo = Some(Undo::Corrected { previous: Box::new(original), correction: written });
                let text = if matches!(purpose, Purpose::Attach(_)) {
                    t!("records.attached", name = name)
                } else {
                    t!("records.corrected", name = name)
                };
                Command::batch([recorded, Command::toast(Toast::success(text).action(t!("today.undo"), Msg::Undo))])
            }
            Purpose::Finish => self.stop_fixed(changes, now),
        }
    }

    /// Moves the session `id` to the trash and offers the way back in the toast.
    pub(super) fn remove(&mut self, id: Id) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let now = self.now();
        let name = self.session_name(id);
        match self.store.void(id, now.wall) {
            Ok(()) => {
                self.last_undo = Some(Undo::Removed(id));
                self.refresh_day();
                Command::toast(Toast::info(t!("records.deleted", name = name)).action(t!("today.undo"), Msg::Undo))
            }
            Err(error) => Command::toast(Toast::danger(t!("app.write-failed", reason = error.to_string()))),
        }
    }

    /// Brings the session `id` back from the trash.
    pub(super) fn bring_back(&mut self, id: Id) -> Command<Msg> {
        if !self.can_edit() {
            return Command::toast(Toast::warning(t!("today.cannot-start")));
        }
        let now = self.now();
        let name = self.session_name(id);
        match self.store.restore(id, now.wall) {
            Ok(()) => {
                if self.last_undo == Some(Undo::Removed(id)) {
                    self.last_undo = None;
                }
                self.refresh_day();
                Command::toast(Toast::success(t!("records.restored", name = name)))
            }
            Err(error) => Command::toast(Toast::danger(t!("app.write-failed", reason = error.to_string()))),
        }
    }

    /// Ctrl+Z: brings back the session removed last, or takes back the correction made last
    /// with a line that replaces it with the version before, at a higher revision
    /// ([`crate::session::resolve`]). Nothing on disk is touched either way.
    pub(super) fn undo(&mut self) -> Command<Msg> {
        match self.last_undo.take() {
            Some(Undo::Removed(id)) => {
                self.last_undo = Some(Undo::Removed(id));
                self.bring_back(id)
            }
            Some(Undo::Wiped { sessions, rows }) => self.unwipe(sessions, rows),
            Some(Undo::Corrected { previous, correction }) => {
                if !self.can_edit() {
                    self.last_undo = Some(Undo::Corrected { previous, correction });
                    return Command::toast(Toast::warning(t!("today.cannot-start")));
                }
                let Some(current) = self.store.sessions.iter().find(|session| session.id == correction) else {
                    return Command::toast(Toast::info(t!("records.nothing-to-undo")));
                };
                let now = self.now();
                let name = self.session_name(correction);
                let revert = Session {
                    id: clock::new_id(),
                    revision: current.revision.saturating_add(1),
                    written: now.wall,
                    replaces: Some(correction),
                    voids: None,
                    ..*previous
                };
                let recorded = self.record(revert);
                Command::batch([recorded, Command::toast(Toast::success(t!("records.correction-undone", name = name)))])
            }
            None => Command::toast(Toast::info(t!("records.nothing-to-undo"))),
        }
    }

    /// The name of the focus of the session `id`, wherever the session is now.
    fn session_name(&self, id: Id) -> String {
        self.sessions
            .iter()
            .chain(self.store.voided.iter())
            .find(|session| session.id == id)
            .and_then(|session| self.store.tree.focus(session.focus))
            .map_or_else(|| t!("records.unknown-focus"), |focus| focus.name.clone())
    }

    /// Writes every current session as CSV and JSON into the export folder, named by today's
    /// date, and says where they went or why they could not be written.
    pub(super) fn export(&self) -> Command<Msg> {
        if !self.on_disk {
            return Command::toast(Toast::warning(t!("records.export-nowhere")));
        }
        let day = self.date;
        let stem = format!("qfocus-{:04}-{:02}-{:02}", day.year(), day.month(), day.day());
        let folder = self.store.paths.export_dir();
        let shown = folder.join(&stem).display().to_string();
        let files = [
            (folder.join(format!("{stem}.csv")), export::csv(&self.sessions, &self.store.tree)),
            (folder.join(format!("{stem}.json")), export::json(&self.sessions, &self.store.tree)),
        ];
        let written = std::fs::create_dir_all(&folder)
            .and_then(|()| files.iter().try_for_each(|(path, text)| atomic_write(path, text.as_bytes())));
        match written {
            Ok(()) => Command::toast(Toast::success(t!("records.exported", path = shown))),
            Err(error) => {
                Command::toast(Toast::danger(t!("records.export-failed", path = shown, reason = error.to_string())))
            }
        }
    }
}
