//! The Records screen: every session as a table, with the archive, the trash and the problems as
//! views of the same page.
//!
//! Nothing here touches the disk. Removing, bringing back and exporting are asked of the
//! application through [`Request`]; the screen only knows which row is meant. Filtering and
//! search are done here because the table does not do them. A session whose span is longer than
//! its work carries a mark, and its spans can be laid out one by one in a dialog.

use std::sync::Arc;

use qframe::date::DateTime;
use qframe::icons::GlyphMode;
use qframe::prelude::*;
use qframe::widgets::{
    Column, ColumnWidth, EmptyState, Legend, Modal, Segmented, Table, TableCell, TableRow, TextInput, TimeBlock,
    Timeline, Toast,
};

use super::today::Row;
use super::warning_line;
use crate::duration::{Units, short};
use crate::id::Id;
use crate::session::{Session, Source};
use crate::span::{ClockSource, Span, SpanKind};
use crate::store::Store;
use crate::tree::Tree as Catalog;

/// The widget id of the table, for focusing it.
pub const TABLE: &str = "records-table";

/// The widget id of the search field, for focusing it.
pub const SEARCH_INPUT: &str = "records-search";

/// The widget id of the view chooser.
pub const VIEWS: &str = "records-views";

/// The widget id of the timeline in the spans dialog, for focusing it.
pub const DUMP_STRIP: &str = "records-dump-strip";

/// The span kinds drawn as blocks of a session's timeline, in the order the legend names them,
/// each with its tone. Sleep is not among them: the machine was off, so it is a hole.
const DRAWN_KINDS: [(SpanKind, usize); 3] = [(SpanKind::Work, 0), (SpanKind::Pause, 1), (SpanKind::Idle, 2)];

/// Below this many columns the note column is left out.
const NOTE_BELOW: u16 = 88;

/// Below this many columns the source column is left out too.
const SOURCE_BELOW: u16 = 64;

/// Below this many columns the end column goes as well and the focus stands without its
/// category; the mark and the line under the table still say when a session spanned more than
/// it worked.
const END_BELOW: u16 = 48;

/// Which part of the records is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewKind {
    /// Every current session.
    #[default]
    All,
    /// Archived categories and focuses.
    Archive,
    /// Removed sessions.
    Trash,
    /// What went wrong reading and resolving the files.
    Problems,
}

impl ViewKind {
    /// The views in the order the chooser shows them.
    pub const ALL: [Self; 4] = [Self::All, Self::Archive, Self::Trash, Self::Problems];

    /// The view at `index` of the chooser; out of range is the first.
    #[must_use]
    pub fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }

    /// Where this view sits in the chooser.
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|view| *view == self).unwrap_or(0)
    }
}

/// Something that happened on the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// A view was chosen.
    View(usize),
    /// A row was selected.
    Select(usize),
    /// A row was activated with Enter, Space or a click.
    Activate(usize),
    /// The search field was opened.
    Search,
    /// The search field changed.
    Typed(String),
    /// The search field was closed; the filter stays.
    SearchClose,
    /// The selected session is to be removed.
    Delete,
    /// A session is to be typed in by hand.
    Add,
    /// The selected session is to be corrected, or attached to a focus in the problems view.
    Correct,
    /// The spans of the selected session are to be laid out.
    Spans,
    /// The spans dialog was closed.
    CloseDump,
    /// A block of the session's timeline in the spans dialog was selected.
    DumpBlock(usize),
    /// Export was asked for.
    Export,
}

/// What the screen asks the application for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Move this session to the trash.
    Void(Id),
    /// Bring this session back from the trash.
    Restore(Id),
    /// The catalogue changed and wants writing.
    Changed,
    /// Write the export files.
    Export,
    /// Open the form for a session typed in by hand.
    Add,
    /// Open the form over this session to correct it.
    Correct(Id),
    /// Open the form over this session, whose focus is gone, to attach it to one.
    Attach(Id),
}

/// The state of the screen.
#[derive(Debug, Clone, Default)]
pub struct Records {
    view: ViewKind,
    /// The selected row of the current view, by position in the filtered list.
    selected: Option<usize>,
    /// The text rows are filtered by; empty means every row.
    query: String,
    /// Whether the search field is open.
    searching: bool,
    /// The session whose spans are laid out in the dialog.
    dump: Option<Id>,
    /// The selected block of the session's timeline in the dialog.
    dump_block: Option<usize>,
}

impl Records {
    /// The screen on the list of every session, nothing selected.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The view shown.
    #[must_use]
    pub fn view_kind(&self) -> ViewKind {
        self.view
    }

    /// The selected row, by position in the filtered list.
    #[must_use]
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The text rows are filtered by.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Whether the search field is open.
    #[must_use]
    pub fn is_searching(&self) -> bool {
        self.searching
    }

    /// Whether the spans dialog is open.
    #[must_use]
    pub fn is_dumping(&self) -> bool {
        self.dump.is_some()
    }

    /// The sessions of the current view that match the filter, newest first. The problems view
    /// holds the sessions whose focus is not in the catalogue, waiting to be attached to one.
    fn sessions<'a>(&self, sessions: &'a [Session], voided: &'a [Session], catalog: &Catalog) -> Vec<&'a Session> {
        let source = match self.view {
            ViewKind::All | ViewKind::Problems => sessions,
            ViewKind::Trash => voided,
            ViewKind::Archive => &[],
        };
        let needle = self.query.trim().to_lowercase();
        let mut rows: Vec<&Session> = source
            .iter()
            .filter(|session| self.view != ViewKind::Problems || catalog.focus(session.focus).is_none())
            .filter(|session| {
                if needle.is_empty() {
                    return true;
                }
                let (category, focus) = names(catalog, session.focus);
                [category.as_str(), focus.as_str(), session.note.as_str()]
                    .iter()
                    .any(|text| text.to_lowercase().contains(&needle))
            })
            .collect();
        rows.sort_by(|left, right| right.started.cmp(&left.started).then(right.id.cmp(&left.id)));
        rows
    }

    /// The archived rows: categories first, then focuses, each under its own name, matching the
    /// filter.
    fn archived(&self, catalog: &Catalog) -> Vec<Row> {
        let needle = self.query.trim().to_lowercase();
        let matches = |name: &str| needle.is_empty() || name.to_lowercase().contains(&needle);
        let mut rows = Vec::new();
        for category in &catalog.categories {
            if category.archived && matches(&category.name) {
                rows.push(Row::Category(category.id));
            }
            for focus in category.focuses.iter().filter(|focus| focus.archived && matches(&focus.name)) {
                rows.push(Row::Focus(focus.id));
            }
        }
        rows
    }

    /// The selected session of the current view, if there is one.
    fn selected_session<'a>(
        &self,
        sessions: &'a [Session],
        voided: &'a [Session],
        catalog: &Catalog,
    ) -> Option<&'a Session> {
        let rows = self.sessions(sessions, voided, catalog);
        self.selected.and_then(|index| rows.get(index).copied())
    }
}

/// Applies `msg` to the screen and the catalogue, and says what the application should do.
pub fn update<M: From<Msg> + Clone + Send + 'static>(
    screen: &mut Records,
    sessions: &[Session],
    voided: &[Session],
    catalog: &mut Catalog,
    msg: Msg,
) -> (Command<M>, Option<Request>) {
    match msg {
        Msg::View(index) => {
            let view = ViewKind::from_index(index);
            if view != screen.view {
                screen.view = view;
                screen.selected = None;
                screen.dump = None;
                screen.dump_block = None;
            }
            // The keys go to the rows of the new view, whether the chooser was clicked or keyed.
            (Command::focus(TABLE), None)
        }
        Msg::Select(index) => {
            screen.selected = Some(index);
            (Command::none(), None)
        }
        Msg::Activate(index) => {
            screen.selected = Some(index);
            activate(screen, sessions, voided, catalog)
        }
        Msg::Search => {
            screen.searching = true;
            (Command::focus(SEARCH_INPUT), None)
        }
        Msg::Typed(text) => {
            screen.query = text;
            screen.selected = None;
            (Command::none(), None)
        }
        Msg::SearchClose => {
            if screen.searching {
                screen.searching = false;
                (Command::focus(TABLE), None)
            } else {
                (Command::none(), None)
            }
        }
        Msg::Delete => match (screen.view, screen.selected_session(sessions, voided, catalog)) {
            (ViewKind::All, Some(session)) => (Command::none(), Some(Request::Void(session.id))),
            _ => (Command::none(), None),
        },
        Msg::Add => (Command::none(), Some(Request::Add)),
        Msg::Correct => match (screen.view, screen.selected_session(sessions, voided, catalog)) {
            (ViewKind::All, Some(session)) => (Command::none(), Some(Request::Correct(session.id))),
            (ViewKind::Problems, Some(session)) => (Command::none(), Some(Request::Attach(session.id))),
            _ => (Command::none(), None),
        },
        Msg::Spans => match (screen.view, screen.selected_session(sessions, voided, catalog)) {
            (ViewKind::All, Some(session)) => {
                screen.dump = Some(session.id);
                screen.dump_block = None;
                (Command::focus(DUMP_STRIP), None)
            }
            _ => (Command::none(), None),
        },
        Msg::CloseDump => {
            screen.dump = None;
            screen.dump_block = None;
            (Command::none(), None)
        }
        Msg::DumpBlock(index) => {
            screen.dump_block = Some(index);
            (Command::none(), None)
        }
        Msg::Export => (Command::none(), Some(Request::Export)),
    }
}

/// What activating the selected row does: the form over the session in the list, a return from
/// the trash, a return from the archive, the form to attach a lost session in the problems view.
fn activate<M: From<Msg> + Clone + Send + 'static>(
    screen: &mut Records,
    sessions: &[Session],
    voided: &[Session],
    catalog: &mut Catalog,
) -> (Command<M>, Option<Request>) {
    match screen.view {
        ViewKind::All => match screen.selected_session(sessions, voided, catalog) {
            Some(session) => (Command::none(), Some(Request::Correct(session.id))),
            None => (Command::none(), None),
        },
        ViewKind::Problems => match screen.selected_session(sessions, voided, catalog) {
            Some(session) => (Command::none(), Some(Request::Attach(session.id))),
            None => (Command::none(), None),
        },
        ViewKind::Trash => match screen.selected_session(sessions, voided, catalog) {
            Some(session) => (Command::none(), Some(Request::Restore(session.id))),
            None => (Command::none(), None),
        },
        ViewKind::Archive => {
            let rows = screen.archived(catalog);
            let Some(row) = screen.selected.and_then(|index| rows.get(index)) else {
                return (Command::none(), None);
            };
            let name = match row {
                Row::Category(id) => catalog.categories.iter_mut().find(|c| c.id == *id).map(|category| {
                    category.archived = false;
                    category.name.clone()
                }),
                Row::Focus(id) => catalog
                    .categories
                    .iter_mut()
                    .flat_map(|category| category.focuses.iter_mut())
                    .find(|focus| focus.id == *id)
                    .map(|focus| {
                        focus.archived = false;
                        focus.name.clone()
                    }),
                Row::AddFocus(_) | Row::AddCategory => None,
            };
            let Some(name) = name else { return (Command::none(), None) };
            // The row is gone from the archive; the selection stays where it was, clamped by
            // the view.
            (Command::toast(Toast::success(t!("today.restored", name = name))), Some(Request::Changed))
        }
    }
}

/// The names of the category and the focus `id`, or the words for an unknown focus.
fn names(catalog: &Catalog, id: Id) -> (String, String) {
    catalog
        .categories
        .iter()
        .find_map(|category| {
            category
                .focuses
                .iter()
                .find(|focus| focus.id == id)
                .map(|focus| (category.name.clone(), focus.name.clone()))
        })
        .unwrap_or_else(|| (String::new(), t!("records.unknown-focus")))
}

/// Whether the focus `id` is archived or gone, which mutes its rows.
fn is_faint(catalog: &Catalog, id: Id) -> bool {
    catalog.focus(id).is_none_or(|focus| focus.archived)
}

/// Seconds between the start and the end of `session`.
fn span_seconds(session: &Session) -> u64 {
    u64::try_from(session.ended.saturating_sub(session.started)).unwrap_or(0)
}

/// A moment as `HH:MM` in the offset given.
pub fn clock_of(seconds: i64, offset_minutes: i16) -> String {
    let moment = DateTime::from_unix(seconds, offset_minutes);
    format!("{:02}:{:02}", moment.time.hour, moment.time.minute)
}

/// A moment as `HH:MM:SS` in the offset given.
fn clock_with_seconds(seconds: i64, offset_minutes: i16) -> String {
    let moment = DateTime::from_unix(seconds, offset_minutes);
    format!("{:02}:{:02}:{:02}", moment.time.hour, moment.time.minute, moment.time.second)
}

/// A moment's date as `YYYY-MM-DD` in the offset given.
fn date_of(seconds: i64, offset_minutes: i16) -> String {
    let date = DateTime::from_unix(seconds, offset_minutes).date;
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

/// The word for where a session came from.
fn source_word(source: Source) -> String {
    match source {
        Source::Timer => t!("records.source-timer"),
        Source::Manual => t!("records.source-manual"),
        Source::Recovered => t!("records.source-recovered"),
        Source::Edited => t!("records.source-edited"),
    }
}

/// The word for what a span holds.
fn kind_word(kind: SpanKind) -> String {
    match kind {
        SpanKind::Work => t!("records.span-work"),
        SpanKind::Pause => t!("records.span-pause"),
        SpanKind::Gap => t!("records.span-gap"),
        SpanKind::Idle => t!("records.span-idle"),
    }
}

/// The words for one span of a session. Time counted through a sleep says so beside its kind,
/// and a gap says whether the machine slept or the program was not running: older records hold
/// sleeps as gaps measured by the clock that runs in sleep, a gap is now only time nobody
/// measured, which the wall clock stands in for.
fn span_word(span: &Span) -> String {
    match (span.kind, span.clock) {
        (SpanKind::Gap, ClockSource::Boot) => kind_word(SpanKind::Gap),
        (SpanKind::Gap, _) => t!("records.span-down"),
        (kind, ClockSource::Boot) => t!("records.span-asleep", kind = kind_word(kind)),
        (kind, _) => kind_word(kind),
    }
}

/// The word for the clock a span was measured with.
fn clock_word(clock: ClockSource) -> String {
    match clock {
        ClockSource::Mono => t!("records.clock-mono"),
        ClockSource::Boot => t!("records.clock-boot"),
        ClockSource::Wall => t!("records.clock-wall"),
    }
}

/// The mark beside a work time that is shorter than the span it sits in.
fn span_mark(mode: GlyphMode) -> &'static str {
    match mode {
        GlyphMode::Ascii => "*",
        GlyphMode::Unicode | GlyphMode::Nerd => "◆",
    }
}

/// Which columns of the session table fit in `width` columns of terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Shown {
    end: bool,
    source: bool,
    note: bool,
}

impl Shown {
    fn at(width: u16) -> Self {
        Self { end: width >= END_BELOW, source: width >= SOURCE_BELOW, note: width >= NOTE_BELOW }
    }
}

/// The columns of the session table.
fn session_columns(shown: Shown) -> Vec<Column> {
    // The first cell of a selected row slides one cell right, so the date keeps a cell of room.
    let mut columns = vec![
        Column::new(t!("records.col-date")).width(ColumnWidth::Fixed(11)),
        Column::new(t!("records.col-focus")).min(6),
        Column::new(t!("records.col-start")).width(ColumnWidth::Fit),
    ];
    if shown.end {
        columns.push(Column::new(t!("records.col-end")).width(ColumnWidth::Fit));
    }
    columns.push(Column::new(t!("records.col-net")).width(ColumnWidth::Fit).align(Align::End));
    if shown.source {
        columns.push(Column::new(t!("records.col-source")).width(ColumnWidth::Fit));
    }
    if shown.note {
        columns.push(Column::new(t!("records.col-note")).min(6));
    }
    columns
}

/// The rows of the session table.
fn session_rows(
    rows: &[&Session],
    catalog: &Catalog,
    units: &Units<'_>,
    shown: Shown,
    mark: &str,
    separator: &str,
) -> Arc<[TableRow]> {
    rows.iter()
        .map(|session| {
            let (category, focus) = names(catalog, session.focus);
            let path = if category.is_empty() || !shown.end { focus } else { format!("{category}{separator}{focus}") };
            let work = session.work_seconds();
            let net = if span_seconds(session) == work {
                short(work, units)
            } else {
                format!("{} {mark}", short(work, units))
            };
            let mut cells = vec![
                TableCell::new(date_of(session.started, session.offset_minutes)),
                TableCell::new(path),
                TableCell::new(clock_of(session.started, session.offset_minutes)),
            ];
            if shown.end {
                cells.push(TableCell::new(clock_of(session.ended, session.offset_minutes)));
            }
            cells.push(TableCell::new(net));
            if shown.source {
                cells.push(TableCell::new(source_word(session.source)));
            }
            if shown.note {
                cells.push(TableCell::new(session.note.lines().next().unwrap_or_default()));
            }
            TableRow::new(cells).faint(is_faint(catalog, session.focus))
        })
        .collect()
}

/// Draws the screen: the view chooser, the search field while it is open, then the view.
/// `sessions` are the current ones, those on disk and those still waiting in memory; the trash,
/// the catalogue and the problems are read from `store`. With `can_edit` false the actions that
/// write are muted, for an instance that only looks.
pub fn view<M: From<Msg> + Clone + Send + 'static>(
    screen: &Records,
    sessions: &[Session],
    store: &Store,
    units: &Units<'_>,
    can_edit: bool,
    ui: &mut View<'_, M>,
) {
    ui.column(|ui| {
        let labels =
            [t!("records.view-all"), t!("records.view-archive"), t!("records.view-trash"), t!("records.view-problems")];
        ui.add(Segmented::new(labels).selected(screen.view.index()).on_select(|index| M::from(Msg::View(index))))
            .id(VIEWS);
        if screen.searching {
            ui.add(
                TextInput::new(&screen.query)
                    .placeholder(t!("records.search-placeholder"))
                    .on_change(|text| M::from(Msg::Typed(text)))
                    .on_submit(|_| M::from(Msg::SearchClose)),
            )
            .id(SEARCH_INPUT)
            .fill_width();
        } else if !screen.query.trim().is_empty() {
            ui.add(Text::new(t!("records.filter", query = screen.query.trim())).role("faint").no_wrap()).fill_width();
        }
        match screen.view {
            ViewKind::All => list(screen, sessions, store, units, can_edit, ui),
            ViewKind::Trash => trash(screen, store, units, can_edit, ui),
            ViewKind::Archive => archive(screen, &store.tree, can_edit, ui),
            ViewKind::Problems => problems(screen, store, units, can_edit, ui),
        }
    })
    .gap(1)
    .fill();
    if let Some(session) = screen.dump.and_then(|id| sessions.iter().find(|session| session.id == id)) {
        dump(screen, session, &store.tree, units, ui);
    }
}

/// The list of every session: the problems line, the table, the span line and the actions.
fn list<M: From<Msg> + Clone + Send + 'static>(
    screen: &Records,
    sessions: &[Session],
    store: &Store,
    units: &Units<'_>,
    can_edit: bool,
    ui: &mut View<'_, M>,
) {
    let problems = u32::try_from(store.diagnostics.len()).unwrap_or(u32::MAX);
    if problems > 0 {
        ui.row(|ui| {
            warning_line(t!("records.problems-line", n = problems), ui);
            ui.add(Button::new(t!("records.view-problems")).on_press(M::from(Msg::View(ViewKind::Problems.index()))));
        })
        .gap(1)
        .fill_width();
    }
    if sessions.is_empty() {
        ui.add(
            EmptyState::new(t!("records.empty-title"))
                .message(t!("records.empty-message"))
                .action(Button::new(t!("records.add")).shortcut("a").disabled(!can_edit).on_press(M::from(Msg::Add))),
        )
        .fill();
        return;
    }
    let rows = screen.sessions(sessions, &store.voided, &store.tree);
    session_table(screen, &rows, &store.tree, units, t!("records.no-match"), ui);
    let none = screen.selected.is_none_or(|index| index >= rows.len());
    // The five buttons take as many rows as the width and the language's own words need: a row
    // that is a cell too wide would cut the last label off and leave its key standing alone.
    let labels =
        [t!("records.add"), t!("records.correct"), t!("records.spans"), t!("records.delete"), t!("records.export")];
    let keys = ["a", "f2", "s", "delete", "e"];
    let measured: Vec<(&str, &str)> = labels.iter().map(String::as_str).zip(keys).collect();
    let mut made = 0_usize;
    ui.column(|ui| {
        for count in super::button_rows(ui.size().width, &measured) {
            ui.row(|ui| {
                for _ in 0..count {
                    let label = labels[made].as_str();
                    let key = keys[made];
                    let button = Button::new(label).shortcut(key);
                    ui.add(match made {
                        0 => button.disabled(!can_edit).on_press(M::from(Msg::Add)),
                        1 => button.disabled(!can_edit || none).on_press(M::from(Msg::Correct)),
                        2 => button.disabled(none).on_press(M::from(Msg::Spans)),
                        3 => button.disabled(!can_edit || none).on_press(M::from(Msg::Delete)),
                        _ => button.on_press(M::from(Msg::Export)),
                    });
                    made += 1;
                }
            })
            .gap(2);
        }
    })
    .gap(1);
}

/// The trash: removed sessions and the way back.
fn trash<M: From<Msg> + Clone + Send + 'static>(
    screen: &Records,
    store: &Store,
    units: &Units<'_>,
    can_edit: bool,
    ui: &mut View<'_, M>,
) {
    if store.voided.is_empty() {
        ui.add(EmptyState::new(t!("records.trash-empty")).message(t!("records.trash-message"))).fill();
        return;
    }
    let rows = screen.sessions(&store.sessions, &store.voided, &store.tree);
    session_table(screen, &rows, &store.tree, units, t!("records.no-match"), ui);
    let index = screen.selected.filter(|index| *index < rows.len());
    ui.add(
        Button::new(t!("records.restore"))
            .disabled(!can_edit || index.is_none())
            .on_press(M::from(Msg::Activate(index.unwrap_or(0)))),
    );
}

/// The session table with the line under it that says how a marked session's span and work
/// differ.
fn session_table<M: From<Msg> + Clone + Send + 'static>(
    screen: &Records,
    rows: &[&Session],
    catalog: &Catalog,
    units: &Units<'_>,
    empty: String,
    ui: &mut View<'_, M>,
) {
    let shown = Shown::at(ui.size().width);
    let icons = ui.env().icons();
    let mark = span_mark(icons.mode());
    let separator = if icons.mode() == GlyphMode::Ascii { " > " } else { " › " };
    let selected = screen.selected.filter(|index| *index < rows.len());
    ui.add(
        Table::new(session_columns(shown), session_rows(rows, catalog, units, shown, mark, separator))
            .selected(selected)
            .empty_text(empty)
            .on_select(|index| M::from(Msg::Select(index)))
            .on_activate(|index| M::from(Msg::Activate(index))),
    )
    .id(TABLE)
    .fill();
    if let Some(session) = selected.and_then(|index| rows.get(index)) {
        let span = span_seconds(session);
        let work = session.work_seconds();
        if span != work {
            let line = t!("records.interval", span = short(span, units), work = short(work, units));
            // Under the cells, not under the pillar: the mark stands where the row's text does.
            ui.add(Text::new(format!("{mark} {line}")).role("secondary").no_wrap())
                .padding(Padding::symmetric(0, 2))
                .fill_width();
        }
    }
}

/// The archive: categories and focuses taken out of the lists, and the way back.
fn archive<M: From<Msg> + Clone + Send + 'static>(
    screen: &Records,
    catalog: &Catalog,
    can_edit: bool,
    ui: &mut View<'_, M>,
) {
    let rows = screen.archived(catalog);
    if rows.is_empty() && screen.query.trim().is_empty() {
        ui.add(EmptyState::new(t!("records.archive-empty")).message(t!("records.archive-message"))).fill();
        return;
    }
    let columns = [
        Column::new(t!("records.col-name")).min(8),
        Column::new(t!("records.col-kind")).width(ColumnWidth::Fit),
        Column::new(t!("records.col-category")).min(8),
    ];
    let table_rows: Arc<[TableRow]> = rows
        .iter()
        .map(|row| match row {
            Row::Category(id) => {
                let name = catalog.categories.iter().find(|c| c.id == *id).map(|c| c.name.clone()).unwrap_or_default();
                TableRow::new([name, t!("records.kind-category"), String::new()])
            }
            Row::Focus(id) => {
                let (category, focus) = names(catalog, *id);
                TableRow::new([focus, t!("records.kind-focus"), category])
            }
            Row::AddFocus(_) | Row::AddCategory => TableRow::new([String::new(), String::new(), String::new()]),
        })
        .collect();
    let selected = screen.selected.filter(|index| *index < rows.len());
    ui.add(
        Table::new(columns, table_rows)
            .selected(selected)
            .empty_text(t!("records.no-match"))
            .on_select(|index| M::from(Msg::Select(index)))
            .on_activate(|index| M::from(Msg::Activate(index))),
    )
    .id(TABLE)
    .fill();
    ui.add(
        Button::new(t!("records.unarchive"))
            .disabled(!can_edit || selected.is_none())
            .on_press(M::from(Msg::Activate(selected.unwrap_or(0)))),
    );
}

/// The problems: every diagnostic with where it is, the counts that are not about one line,
/// and the sessions whose focus is gone, each waiting to be attached to a focus that exists.
fn problems<M: From<Msg> + Clone + Send + 'static>(
    screen: &Records,
    store: &Store,
    units: &Units<'_>,
    can_edit: bool,
    ui: &mut View<'_, M>,
) {
    let orphans = u32::try_from(store.orphans.len()).unwrap_or(u32::MAX);
    let forks = u32::try_from(store.forks.len()).unwrap_or(u32::MAX);
    let unknown = u32::try_from(store.unknown_version).unwrap_or(u32::MAX);
    let lost = screen.sessions(&store.sessions, &store.voided, &store.tree);
    let lost_count = u32::try_from(lost.len()).unwrap_or(u32::MAX);
    if store.diagnostics.is_empty() && orphans == 0 && forks == 0 && unknown == 0 && lost.is_empty() {
        ui.add(EmptyState::new(t!("records.no-problems")).message(t!("records.problems-message"))).fill();
        return;
    }
    let mark = ui.env().icons().glyph("warning").into_owned();
    ui.column(|ui| {
        let lines = store
            .diagnostics
            .iter()
            .map(ToString::to_string)
            .chain((orphans > 0).then(|| t!("records.orphans", n = orphans)))
            .chain((forks > 0).then(|| t!("records.forks", n = forks)))
            .chain((unknown > 0).then(|| t!("records.unknown-version", n = unknown)))
            .chain((lost_count > 0).then(|| t!("records.lost-focus", n = lost_count)));
        for line in lines {
            // A path and a message are long; they wrap rather than lose their end.
            ui.add(Text::rich([
                qframe::widgets::Span::new(format!("{mark} ")).color("warning"),
                qframe::widgets::Span::new(line).role("secondary"),
            ]))
            .fill_width();
        }
    })
    .gap(1)
    .padding(Padding::symmetric(0, 2));
    if lost.is_empty() {
        ui.spacer().fill();
        return;
    }
    // The table stands beside the list's, under the same parent, so the keyboard that was on
    // one is on the other when the view changes.
    session_table(screen, &lost, &store.tree, units, t!("records.no-match"), ui);
    let none = screen.selected.is_none_or(|index| index >= lost.len());
    ui.add(Button::new(t!("records.attach")).disabled(!can_edit || none).on_press(M::from(Msg::Correct)));
}

/// The clock time of `seconds` (since the Unix epoch) in the offset given.
fn time_of(seconds: i64, offset_minutes: i16) -> qframe::date::TimeOfDay {
    DateTime::from_unix(seconds, offset_minutes).time
}

/// The spans of `session` as blocks of its own timeline: work, breaks and idle time each in the
/// tone of its kind, whether measured awake or through a sleep, and a gap left as a hole. Every block is named by its kind, so the strip reads
/// in words as well as in tone.
fn session_blocks(session: &Session) -> Vec<TimeBlock> {
    session
        .spans
        .iter()
        .filter_map(|span| {
            let tone = DRAWN_KINDS.iter().find(|(kind, _)| *kind == span.kind).map(|(_, tone)| *tone)?;
            let start = session.started.saturating_add(i64::from(span.offset));
            let end = start.saturating_add(i64::from(span.seconds));
            let block = TimeBlock::new(
                span_word(span),
                time_of(start, session.offset_minutes),
                time_of(end, session.offset_minutes),
            );
            Some(block.tone(tone))
        })
        .collect()
}

/// The dialog laying out the spans of `session`: the session as its own timeline with a legend
/// of the kinds, then every span as a row: what, from when, for how long, by which clock.
fn dump<M: From<Msg> + Clone + Send + 'static>(
    screen: &Records,
    session: &Session,
    catalog: &Catalog,
    units: &Units<'_>,
    ui: &mut View<'_, M>,
) {
    let (_, focus) = names(catalog, session.focus);
    let title = t!("records.dump-title", focus = focus, date = date_of(session.started, session.offset_minutes));
    let blocks = session_blocks(session);
    // The strip runs from the session's start to its end across the whole width, so a ten
    // minute break inside an hour is wide enough to see; the day turns at the start, so a
    // session across midnight stays whole.
    let start = time_of(session.started, session.offset_minutes);
    let end = time_of(session.ended, session.offset_minutes);
    let timeline = Timeline::new(blocks.clone())
        .day_starts_at(start)
        .range(start, end)
        .axis()
        .readout()
        .selected(screen.dump_block.filter(|index| *index < blocks.len()))
        .on_select(|index| M::from(Msg::DumpBlock(index)));
    let kinds: Vec<(SpanKind, usize)> =
        DRAWN_KINDS.into_iter().filter(|(kind, _)| session.spans.iter().any(|span| span.kind == *kind)).collect();
    let legend = Legend::new(kinds.iter().map(|(kind, _)| kind_word(*kind))).tones(kinds.iter().map(|(_, tone)| *tone));
    let columns = [
        Column::new(t!("records.col-kind")).width(ColumnWidth::Fit),
        Column::new(t!("records.col-start")).width(ColumnWidth::Fixed(8)),
        Column::new(t!("records.col-length")).width(ColumnWidth::Fit).align(Align::End),
        Column::new(t!("records.col-clock")).min(6),
    ];
    let rows: Arc<[TableRow]> = session
        .spans
        .iter()
        .map(|span: &Span| {
            let start = session.started.saturating_add(i64::from(span.offset));
            TableRow::new([
                span_word(span),
                clock_with_seconds(start, session.offset_minutes),
                short(u64::from(span.seconds), units),
                clock_word(span.clock),
            ])
        })
        .collect();
    let height = u16::try_from(session.spans.len().max(1) + 1).unwrap_or(u16::MAX);
    let summary =
        t!("records.interval", span = short(span_seconds(session), units), work = short(session.work_seconds(), units));
    let dialog = Modal::new()
        .title(title)
        .on_close(M::from(Msg::CloseDump))
        .action(Button::new(t!("recover.close")).on_press(M::from(Msg::CloseDump)));
    ui.add_with(dialog, |ui| {
        ui.add(Text::new(summary).role("secondary")).fill_width();
        if !blocks.is_empty() {
            ui.add(timeline).id(DUMP_STRIP).fill_width();
            ui.add(legend).fill_width();
        }
        ui.add(Table::new(columns, rows).empty_text(t!("records.no-spans"))).height(Length::Cells(height)).fill_width();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{Category, Focus};

    fn catalog() -> Catalog {
        Catalog {
            categories: vec![Category {
                id: Id::new(1, 1),
                name: "Work".to_owned(),
                icon: None,
                goal: None,
                archived: false,
                order: 0.0,
                focuses: vec![
                    Focus { id: Id::new(2, 2), name: "Rust".to_owned(), goal: None, archived: false, order: 0.0 },
                    Focus { id: Id::new(3, 3), name: "Review".to_owned(), goal: None, archived: true, order: 1.0 },
                ],
            }],
        }
    }

    fn session(id: u64, focus: Id, started: i64, note: &str) -> Session {
        Session {
            id: Id::new(id, 1),
            revision: 1,
            written: started + 600,
            focus,
            started,
            offset_minutes: 180,
            ended: started + 600,
            spans: vec![Span::new(SpanKind::Work, 0, 600, ClockSource::Mono)],
            source: Source::Timer,
            replaces: None,
            voids: None,
            continues: None,
            flags: Vec::new(),
            note: note.to_owned(),
        }
    }

    #[test]
    fn rows_are_newest_first_and_the_filter_reads_focus_and_note() {
        let catalog = catalog();
        let sessions = [
            session(1, Id::new(2, 2), 1_000, "morning"),
            session(2, Id::new(3, 3), 3_000, "evening"),
            session(3, Id::new(2, 2), 2_000, ""),
        ];
        let mut screen = Records::new();
        let rows = screen.sessions(&sessions, &[], &catalog);
        assert_eq!(rows.iter().map(|row| row.started).collect::<Vec<_>>(), vec![3_000, 2_000, 1_000]);
        screen.query = "REVIEW".to_owned();
        assert_eq!(screen.sessions(&sessions, &[], &catalog).len(), 1);
        screen.query = "morn".to_owned();
        assert_eq!(screen.sessions(&sessions, &[], &catalog).len(), 1);
        screen.query = "work".to_owned();
        assert_eq!(screen.sessions(&sessions, &[], &catalog).len(), 3, "the category name matches too");
    }

    #[test]
    fn delete_asks_for_the_selected_session_only_in_the_list() {
        let mut catalog = catalog();
        let sessions = [session(1, Id::new(2, 2), 1_000, ""), session(2, Id::new(2, 2), 2_000, "")];
        let mut screen = Records::new();
        let (_, request): (Command<Msg>, _) = update(&mut screen, &sessions, &[], &mut catalog, Msg::Delete);
        assert_eq!(request, None, "nothing selected");
        let _: (Command<Msg>, _) = update(&mut screen, &sessions, &[], &mut catalog, Msg::Select(0));
        let (_, request): (Command<Msg>, _) = update(&mut screen, &sessions, &[], &mut catalog, Msg::Delete);
        assert_eq!(request, Some(Request::Void(Id::new(2, 1))), "the newest is first");
        let _: (Command<Msg>, _) =
            update(&mut screen, &sessions, &[], &mut catalog, Msg::View(ViewKind::Trash.index()));
        assert_eq!(screen.selected(), None, "the selection does not carry over");
        let _: (Command<Msg>, _) = update(&mut screen, &sessions, &[], &mut catalog, Msg::Select(0));
        let (_, request): (Command<Msg>, _) = update(&mut screen, &sessions, &[], &mut catalog, Msg::Delete);
        assert_eq!(request, None, "the trash has no delete");
    }

    #[test]
    fn activating_in_the_trash_restores_and_in_the_list_corrects_while_s_opens_the_spans() {
        let mut catalog = catalog();
        let sessions = [session(1, Id::new(2, 2), 1_000, "")];
        let voided = [session(9, Id::new(2, 2), 5_000, "")];
        let mut screen = Records::new();
        let (_, request): (Command<Msg>, _) = update(&mut screen, &sessions, &voided, &mut catalog, Msg::Spans);
        assert_eq!(request, None, "nothing selected");
        assert!(!screen.is_dumping());
        let (_, request): (Command<Msg>, _) = update(&mut screen, &sessions, &voided, &mut catalog, Msg::Activate(0));
        assert_eq!(request, Some(Request::Correct(Id::new(1, 1))));
        assert!(!screen.is_dumping());
        let (_, request): (Command<Msg>, _) = update(&mut screen, &sessions, &voided, &mut catalog, Msg::Spans);
        assert_eq!(request, None);
        assert!(screen.is_dumping());
        let _: (Command<Msg>, _) = update(&mut screen, &sessions, &voided, &mut catalog, Msg::CloseDump);
        assert!(!screen.is_dumping());
        let (_, request): (Command<Msg>, _) = update(&mut screen, &sessions, &voided, &mut catalog, Msg::Add);
        assert_eq!(request, Some(Request::Add));
        let _: (Command<Msg>, _) =
            update(&mut screen, &sessions, &voided, &mut catalog, Msg::View(ViewKind::Trash.index()));
        let (_, request): (Command<Msg>, _) = update(&mut screen, &sessions, &voided, &mut catalog, Msg::Activate(0));
        assert_eq!(request, Some(Request::Restore(Id::new(9, 1))));
    }

    #[test]
    fn activating_an_archived_focus_brings_it_back() {
        let mut catalog = catalog();
        let mut screen = Records::new();
        let _: (Command<Msg>, _) = update(&mut screen, &[], &[], &mut catalog, Msg::View(ViewKind::Archive.index()));
        assert_eq!(screen.archived(&catalog), vec![Row::Focus(Id::new(3, 3))]);
        let (_, request): (Command<Msg>, _) = update(&mut screen, &[], &[], &mut catalog, Msg::Activate(0));
        assert_eq!(request, Some(Request::Changed));
        assert!(!catalog.categories[0].focuses[1].archived);
        assert!(screen.archived(&catalog).is_empty());
    }

    #[test]
    fn a_sessions_blocks_are_its_work_breaks_and_idle_time_and_sleep_is_a_hole() {
        let mut session = session(1, Id::new(2, 2), 1_000, "");
        session.spans = vec![
            Span::new(SpanKind::Work, 0, 600, ClockSource::Mono),
            Span::new(SpanKind::Gap, 600, 7_200, ClockSource::Boot),
            Span::new(SpanKind::Pause, 7_800, 300, ClockSource::Mono),
            Span::new(SpanKind::Idle, 8_100, 900, ClockSource::Mono),
        ];
        session.ended = session.started + 9_000;
        let at = |seconds: i64| time_of(session.started + seconds, session.offset_minutes);
        assert_eq!(
            session_blocks(&session),
            vec![
                TimeBlock::new(kind_word(SpanKind::Work), at(0), at(600)).tone(0),
                TimeBlock::new(kind_word(SpanKind::Pause), at(7_800), at(8_100)).tone(1),
                TimeBlock::new(kind_word(SpanKind::Idle), at(8_100), at(9_000)).tone(2),
            ]
        );
        let mut screen = Records::new();
        let mut catalog = catalog();
        let _: (Command<Msg>, _) = update(&mut screen, &[], &[], &mut catalog, Msg::DumpBlock(1));
        assert_eq!(screen.dump_block, Some(1));
        let _: (Command<Msg>, _) = update(&mut screen, &[], &[], &mut catalog, Msg::CloseDump);
        assert_eq!(screen.dump_block, None, "closing forgets the block");
    }

    #[test]
    fn a_span_counted_through_sleep_says_so_and_a_gap_says_why_it_was_not_measured() {
        let work = Span::new(SpanKind::Work, 0, 600, ClockSource::Mono);
        let slept = Span::new(SpanKind::Work, 600, 7_200, ClockSource::Boot);
        assert_eq!(span_word(&work), kind_word(SpanKind::Work));
        assert_eq!(span_word(&slept), t!("records.span-asleep", kind = kind_word(SpanKind::Work)));
        assert_ne!(span_word(&slept), span_word(&work));
        let old_sleep = Span::new(SpanKind::Gap, 0, 60, ClockSource::Boot);
        let down = Span::new(SpanKind::Gap, 0, 60, ClockSource::Wall);
        assert_eq!(span_word(&old_sleep), kind_word(SpanKind::Gap));
        assert_eq!(span_word(&down), t!("records.span-down"));
    }

    #[test]
    fn the_problems_view_lists_the_sessions_whose_focus_is_gone_and_attaches_them() {
        let mut catalog = catalog();
        let sessions = [session(1, Id::new(2, 2), 1_000, ""), session(2, Id::new(7, 7), 2_000, "")];
        let mut screen = Records::new();
        let _: (Command<Msg>, _) =
            update(&mut screen, &sessions, &[], &mut catalog, Msg::View(ViewKind::Problems.index()));
        let rows = screen.sessions(&sessions, &[], &catalog);
        assert_eq!(rows.iter().map(|row| row.id).collect::<Vec<_>>(), vec![Id::new(2, 1)]);
        let (_, request): (Command<Msg>, _) = update(&mut screen, &sessions, &[], &mut catalog, Msg::Activate(0));
        assert_eq!(request, Some(Request::Attach(Id::new(2, 1))));
        let (_, request): (Command<Msg>, _) = update(&mut screen, &sessions, &[], &mut catalog, Msg::Correct);
        assert_eq!(request, Some(Request::Attach(Id::new(2, 1))), "the button asks the same");
        let (_, request): (Command<Msg>, _) = update(&mut screen, &sessions, &[], &mut catalog, Msg::Spans);
        assert_eq!(request, None);
        assert!(!screen.is_dumping(), "the spans are laid out from the list only");
    }

    #[test]
    fn the_search_field_closes_and_the_filter_stays() {
        let mut catalog = catalog();
        let mut screen = Records::new();
        let _: (Command<Msg>, _) = update(&mut screen, &[], &[], &mut catalog, Msg::Search);
        assert!(screen.is_searching());
        let _: (Command<Msg>, _) = update(&mut screen, &[], &[], &mut catalog, Msg::Typed("rust".to_owned()));
        let _: (Command<Msg>, _) = update(&mut screen, &[], &[], &mut catalog, Msg::SearchClose);
        assert!(!screen.is_searching());
        assert_eq!(screen.query(), "rust");
    }

    #[test]
    fn columns_fold_as_the_terminal_narrows() {
        assert_eq!(Shown::at(120), Shown { end: true, source: true, note: true });
        assert_eq!(Shown::at(80), Shown { end: true, source: true, note: false });
        assert_eq!(Shown::at(60), Shown { end: true, source: false, note: false });
        assert_eq!(Shown::at(40), Shown { end: false, source: false, note: false });
    }
}
