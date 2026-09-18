//! The Today screen: the categories and focuses as a tree, with the day's work beside each.
//!
//! The tree shows what the person means to work on, so a focus that was never worked on stays in
//! it with `—` for a time. The last row is always the way to add a category, and the category the
//! selection is in carries a row to add a focus under it, so the flow from "new" to "started"
//! never stops to ask where to go. Names are typed in place, in a field under the tree.
//!
//! Rows are dragged, or moved with the tree's keys, among their siblings: the moved row takes an
//! order between its new neighbours and nothing else is renumbered. A right click, or the menu
//! key, opens what a row can do, including moving a focus under another category, which a drag
//! never does.

use std::collections::BTreeSet;
use std::time::Duration;

use qframe::date::TimeOfDay;
use qframe::prelude::*;
use qframe::widgets::{
    ContextItem, DurationInput, EmptyState, Legend, Segmented, Span, TextInput, TimeBlock, Timeline, Toast, Tree,
    TreeMove, TreeNode,
};

use super::{GoalRow, goal_gauges};
use crate::clock;
use crate::day::{DayTotals, category_total};
use crate::duration::{Units, short};
use crate::id::Id;
use crate::stats::{self, Block, Edge};
use crate::tree::{Category, Focus, Goal, Period, Tree as Catalog};

/// The widget id of the tree, for focusing it.
pub const TREE: &str = "today-tree";

/// The widget id of the name field, for focusing it.
pub const NAME_INPUT: &str = "today-name";

/// The widget id of the day strip, for focusing it.
pub const STRIP: &str = "today-strip";

/// The widget id of the goal field's length, for focusing it.
pub const GOAL_INPUT: &str = "today-goal";

/// The widget id of the countdown field's length, for focusing it.
pub const TIMED_INPUT: &str = "today-timed";

/// Below this many columns the goal meters under the tree are left out: the tree gives up its
/// goal readout before the strip gives up its legend.
const GOALS_BELOW: u16 = 48;

/// Below this many columns the goal and countdown fields stack their parts in a column.
const FIELD_STACKS_BELOW: u16 = 72;

/// The periods a goal can run over, in the order the chooser shows them.
const PERIODS: [Period; 3] = [Period::Day, Period::Week, Period::Month];

/// Below this many columns the legend under the strip is left out; the block being read is
/// still named in the strip's own readout row.
const LEGEND_BELOW: u16 = 40;

/// Seconds in a day.
const DAY: u32 = 86_400;

/// A row of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// A category.
    Category(Id),
    /// A focus.
    Focus(Id),
    /// The row that adds a focus under a category.
    AddFocus(Id),
    /// The last row, which adds a category.
    AddCategory,
}

impl Row {
    /// The tree key of the row.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Category(id) => format!("c:{id}"),
            Self::Focus(id) => format!("f:{id}"),
            Self::AddFocus(id) => format!("+f:{id}"),
            Self::AddCategory => "+c".to_owned(),
        }
    }

    /// The row of a tree key, or `None` for a key the tree never made.
    #[must_use]
    pub fn parse(key: &str) -> Option<Self> {
        if key == "+c" {
            return Some(Self::AddCategory);
        }
        let (prefix, id) = key.split_once(':')?;
        let id = Id::parse(id)?;
        match prefix {
            "c" => Some(Self::Category(id)),
            "f" => Some(Self::Focus(id)),
            "+f" => Some(Self::AddFocus(id)),
            _ => None,
        }
    }
}

/// What the name field is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Editing {
    /// A category to add at the end.
    NewCategory,
    /// A focus to add under this category.
    NewFocus(Id),
    /// A new name for this category or focus.
    Rename(Row),
}

/// The name field while it is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// What the name is for.
    pub what: Editing,
    /// What has been typed.
    pub text: String,
    /// Whether an empty name was just refused; the reason stands beside the field until the
    /// person types.
    pub refused: bool,
}

/// The goal field while it is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalEdit {
    /// The category or focus the goal is for.
    pub row: Row,
    /// Seconds typed so far.
    pub amount: u64,
    /// How often the goal starts over.
    pub period: Period,
    /// Whether the row had a goal when the field opened, so it can be removed.
    pub had_goal: bool,
    /// Whether a zero length was just refused.
    pub refused: bool,
}

/// The countdown field while it is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedStart {
    /// The focus to start.
    pub focus: Id,
    /// Seconds typed so far.
    pub amount: u64,
    /// Whether a zero length was just refused.
    pub refused: bool,
}

/// Something that happened on the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// A row was selected.
    Select(String),
    /// A row was activated with Enter, Space or a click.
    Activate(String),
    /// A category was opened or closed.
    Expand(String, bool),
    /// The name field changed.
    Typed(String),
    /// The name field was submitted.
    Submit,
    /// The name field was closed without a name.
    Cancel,
    /// The selected category or focus was archived.
    Archive,
    /// The selected category or focus is to be renamed.
    Rename,
    /// An archived row is brought back, from the toast that said it went.
    Restore(Row),
    /// A block of the day strip was selected.
    Block(usize),
    /// The strip was zoomed to this stretch of the day; the same time twice is the whole day.
    Zoom(TimeOfDay, TimeOfDay),
    /// A row was dragged, or keyed, to another place among its siblings.
    Move(TreeMove),
    /// An entry of a row's context menu was chosen, for the row with this key.
    Menu(String, MenuAction),
    /// The goal field opens on the selected row; a row without a goal starts from these seconds.
    Goal(u32),
    /// The goal field's length changed.
    GoalAmount(Duration),
    /// The goal field's period was chosen, by position among day, week and month.
    GoalPeriod(usize),
    /// The goal field was submitted.
    GoalSave,
    /// The goal of the row is removed.
    GoalRemove,
    /// The goal field was closed without a change.
    GoalCancel,
    /// The countdown field opens on the selected focus, starting from these seconds.
    Timed(u32),
    /// The countdown field's length changed.
    TimedAmount(Duration),
    /// The countdown field was submitted.
    TimedStart,
    /// The countdown field was closed without starting.
    TimedCancel,
}

/// What the context menu of a row offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    /// Start counting the focus.
    Start,
    /// Rename the category or focus.
    Rename,
    /// Archive the category or focus.
    Archive,
    /// Add a focus under the category.
    AddFocus,
    /// Add a category at the end.
    AddCategory,
    /// Move the focus under this category.
    MoveTo(Id),
    /// Set, change or remove the goal; a row without one starts from these seconds.
    Goal(u32),
    /// Start the focus with a countdown, suggested from these seconds.
    Timed(u32),
}

/// What the screen asks the application for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Start counting this focus.
    Start(Id),
    /// Start counting this focus with a countdown of this many seconds.
    StartFor(Id, u32),
    /// The catalogue changed and wants writing.
    Changed,
}

/// The state of the screen.
#[derive(Debug, Clone, Default)]
pub struct Today {
    /// The selected row, by key.
    selected: Option<String>,
    /// Categories that are closed; every other one is open.
    collapsed: BTreeSet<Id>,
    /// The name field, while open.
    edit: Option<Edit>,
    /// The selected block of the day strip, by index into the day's blocks.
    block: Option<usize>,
    /// The stretch of the day the strip is zoomed to; the whole day when `None`.
    zoom: Option<(TimeOfDay, TimeOfDay)>,
    /// The goal field, while open.
    goal: Option<GoalEdit>,
    /// The countdown field, while open.
    timed: Option<TimedStart>,
}

impl Today {
    /// The screen with nothing selected and every category open.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The selected row.
    #[must_use]
    pub fn selected(&self) -> Option<Row> {
        self.selected.as_deref().and_then(Row::parse)
    }

    /// The name field, while open.
    #[must_use]
    pub fn edit(&self) -> Option<&Edit> {
        self.edit.as_ref()
    }

    /// The selected block of the day strip, by index into the day's blocks.
    #[must_use]
    pub fn block(&self) -> Option<usize> {
        self.block
    }

    /// The goal field, while open.
    #[must_use]
    pub fn goal_edit(&self) -> Option<&GoalEdit> {
        self.goal.as_ref()
    }

    /// The countdown field, while open.
    #[must_use]
    pub fn timed(&self) -> Option<&TimedStart> {
        self.timed.as_ref()
    }

    /// Whether any field under the tree is open: a name, a goal or a countdown.
    #[must_use]
    pub fn is_editing(&self) -> bool {
        self.edit.is_some() || self.goal.is_some() || self.timed.is_some()
    }

    /// The stretch of the day the strip is zoomed to; the whole day when `None`.
    #[must_use]
    pub fn zoom(&self) -> Option<(TimeOfDay, TimeOfDay)> {
        self.zoom
    }

    /// The category the selection is in: the selected category, or the one the selected focus
    /// or add row belongs to.
    fn selected_category(&self, catalog: &Catalog) -> Option<Id> {
        match self.selected()? {
            Row::Category(id) | Row::AddFocus(id) => Some(id),
            Row::Focus(id) => catalog
                .categories
                .iter()
                .find(|category| category.focuses.iter().any(|focus| focus.id == id))
                .map(|c| c.id),
            Row::AddCategory => None,
        }
    }

    /// Opens the name field for `what`, with `text` in it.
    fn open(&mut self, what: Editing, text: String) {
        self.edit = Some(Edit { what, text, refused: false });
    }
}

/// Applies `msg` to the screen and the catalogue, and says what the application should do.
pub fn update<M: From<Msg> + Clone + Send + 'static>(
    screen: &mut Today,
    catalog: &mut Catalog,
    msg: Msg,
) -> (Command<M>, Option<Request>) {
    match msg {
        Msg::Select(key) => {
            screen.selected = Some(key);
            (Command::none(), None)
        }
        Msg::Activate(key) => activate(screen, catalog, &key),
        Msg::Expand(key, open) => {
            if let Some(Row::Category(id)) = Row::parse(&key) {
                if open {
                    screen.collapsed.remove(&id);
                } else {
                    screen.collapsed.insert(id);
                }
            }
            (Command::none(), None)
        }
        Msg::Typed(text) => {
            if let Some(edit) = screen.edit.as_mut() {
                edit.text = text;
                edit.refused = false;
            }
            (Command::none(), None)
        }
        Msg::Submit => submit(screen, catalog),
        Msg::Cancel => {
            if screen.edit.take().is_some() {
                (Command::batch([Command::toast(Toast::info(t!("app.cancelled"))), Command::focus(TREE)]), None)
            } else {
                (Command::none(), None)
            }
        }
        Msg::Archive => archive(screen, catalog),
        Msg::Rename => {
            let name = match screen.selected() {
                Some(Row::Category(id)) => category(catalog, id).map(|category| category.name.clone()),
                Some(Row::Focus(id)) => catalog.focus(id).map(|focus| focus.name.clone()),
                _ => None,
            };
            match (name, screen.selected()) {
                (Some(name), Some(row)) => {
                    screen.open(Editing::Rename(row), name);
                    (Command::focus(NAME_INPUT), None)
                }
                _ => (Command::none(), None),
            }
        }
        Msg::Restore(row) => restore(screen, catalog, &row),
        Msg::Block(index) => {
            screen.block = Some(index);
            (Command::none(), None)
        }
        Msg::Zoom(from, to) => {
            screen.zoom = (from != to).then_some((from, to));
            (Command::none(), None)
        }
        Msg::Move(step) => moved(screen, catalog, &step),
        Msg::Menu(key, action) => {
            // The menu acts on the row it was opened on, which need not be the selected one;
            // the row becomes selected so the action and its outcome are seen where they act.
            screen.selected = Some(key.clone());
            match action {
                MenuAction::Start => activate(screen, catalog, &key),
                MenuAction::Rename => update(screen, catalog, Msg::Rename),
                MenuAction::Archive => archive(screen, catalog),
                MenuAction::AddFocus => match Row::parse(&key) {
                    Some(Row::Category(id) | Row::Focus(id) | Row::AddFocus(id)) => {
                        let category = category_of(catalog, id).unwrap_or(id);
                        activate(screen, catalog, &Row::AddFocus(category).key())
                    }
                    _ => (Command::none(), None),
                },
                MenuAction::AddCategory => activate(screen, catalog, &Row::AddCategory.key()),
                MenuAction::MoveTo(target) => match Row::parse(&key) {
                    Some(Row::Focus(focus)) => move_to(screen, catalog, focus, target),
                    _ => (Command::none(), None),
                },
                MenuAction::Goal(suggested) => update(screen, catalog, Msg::Goal(suggested)),
                MenuAction::Timed(suggested) => update(screen, catalog, Msg::Timed(suggested)),
            }
        }
        Msg::Goal(suggested) => open_goal(screen, catalog, suggested),
        Msg::GoalAmount(length) => {
            if let Some(edit) = screen.goal.as_mut() {
                edit.amount = length.as_secs();
                edit.refused = false;
            }
            (Command::none(), None)
        }
        Msg::GoalPeriod(index) => {
            if let (Some(edit), Some(period)) = (screen.goal.as_mut(), PERIODS.get(index)) {
                edit.period = *period;
            }
            (Command::none(), None)
        }
        Msg::GoalSave => save_goal(screen, catalog),
        Msg::GoalRemove => remove_goal(screen, catalog),
        Msg::GoalCancel => {
            if screen.goal.take().is_some() {
                (Command::batch([Command::toast(Toast::info(t!("app.cancelled"))), Command::focus(TREE)]), None)
            } else {
                (Command::none(), None)
            }
        }
        Msg::Timed(suggested) => match screen.selected() {
            Some(Row::Focus(focus)) if catalog.focus(focus).is_some() => {
                screen.edit = None;
                screen.goal = None;
                screen.timed = Some(TimedStart { focus, amount: u64::from(suggested), refused: false });
                (Command::focus(TIMED_INPUT), None)
            }
            _ => (Command::none(), None),
        },
        Msg::TimedAmount(length) => {
            if let Some(timed) = screen.timed.as_mut() {
                timed.amount = length.as_secs();
                timed.refused = false;
            }
            (Command::none(), None)
        }
        Msg::TimedStart => {
            let Some(timed) = screen.timed.as_mut() else { return (Command::none(), None) };
            if timed.amount == 0 {
                timed.refused = true;
                return (Command::none(), None);
            }
            let (focus, seconds) = (timed.focus, u32::try_from(timed.amount).unwrap_or(u32::MAX));
            screen.timed = None;
            (Command::focus(TREE), Some(Request::StartFor(focus, seconds)))
        }
        Msg::TimedCancel => {
            if screen.timed.take().is_some() {
                (Command::batch([Command::toast(Toast::info(t!("app.cancelled"))), Command::focus(TREE)]), None)
            } else {
                (Command::none(), None)
            }
        }
    }
}

/// The goal of the row `row`, when it is a category or a focus that has one.
fn goal_of(catalog: &Catalog, row: &Row) -> Option<Goal> {
    match row {
        Row::Category(id) => category(catalog, *id).and_then(|c| c.goal),
        Row::Focus(id) => catalog.focus(*id).and_then(|f| f.goal),
        _ => None,
    }
}

/// The name of the row `row`, for a category or a focus.
fn name_of(catalog: &Catalog, row: &Row) -> Option<String> {
    match row {
        Row::Category(id) => category(catalog, *id).map(|c| c.name.clone()),
        Row::Focus(id) => catalog.focus(*id).map(|f| f.name.clone()),
        _ => None,
    }
}

/// Opens the goal field on the selected category or focus: with its goal when it has one, else
/// with `suggested` seconds a week, which is the one place the suggestion is ever seen.
fn open_goal<M: From<Msg> + Clone + Send + 'static>(
    screen: &mut Today,
    catalog: &Catalog,
    suggested: u32,
) -> (Command<M>, Option<Request>) {
    let Some(row) = screen.selected() else { return (Command::none(), None) };
    if name_of(catalog, &row).is_none() {
        return (Command::none(), None);
    }
    let current = goal_of(catalog, &row);
    let (amount, period) =
        current.map_or((u64::from(suggested), Period::Day), |goal| (u64::from(goal.amount), goal.period));
    screen.edit = None;
    screen.timed = None;
    screen.goal = Some(GoalEdit { row, amount, period, had_goal: current.is_some(), refused: false });
    (Command::focus(GOAL_INPUT), None)
}

/// Sets the row's goal to what the field holds; a zero length is refused with the reason
/// beside the field.
fn save_goal<M: From<Msg> + Clone + Send + 'static>(
    screen: &mut Today,
    catalog: &mut Catalog,
) -> (Command<M>, Option<Request>) {
    let Some(edit) = screen.goal.as_mut() else { return (Command::none(), None) };
    if edit.amount == 0 {
        edit.refused = true;
        return (Command::none(), None);
    }
    let goal = Goal { amount: u32::try_from(edit.amount).unwrap_or(u32::MAX), period: edit.period };
    let row = edit.row.clone();
    screen.goal = None;
    let name = match &row {
        Row::Category(id) => category_mut(catalog, *id).map(|c| {
            c.goal = Some(goal);
            c.name.clone()
        }),
        Row::Focus(id) => focus_mut(catalog, *id).map(|f| {
            f.goal = Some(goal);
            f.name.clone()
        }),
        _ => None,
    };
    let Some(name) = name else { return (Command::focus(TREE), None) };
    screen.selected = Some(row.key());
    (
        Command::batch([Command::focus(TREE), Command::toast(Toast::success(t!("goal.set", name = name)))]),
        Some(Request::Changed),
    )
}

/// Takes the goal off the row the field is open on.
fn remove_goal<M: From<Msg> + Clone + Send + 'static>(
    screen: &mut Today,
    catalog: &mut Catalog,
) -> (Command<M>, Option<Request>) {
    let Some(edit) = screen.goal.take() else { return (Command::none(), None) };
    let name = match &edit.row {
        Row::Category(id) => category_mut(catalog, *id).map(|c| {
            c.goal = None;
            c.name.clone()
        }),
        Row::Focus(id) => focus_mut(catalog, *id).map(|f| {
            f.goal = None;
            f.name.clone()
        }),
        _ => None,
    };
    let Some(name) = name else { return (Command::focus(TREE), None) };
    let changed = edit.had_goal;
    screen.selected = Some(edit.row.key());
    let toast = Toast::info(t!("goal.removed", name = name));
    (Command::batch([Command::focus(TREE), Command::toast(toast)]), changed.then_some(Request::Changed))
}

/// The categories shown, in their order.
fn visible_categories(catalog: &Catalog) -> Vec<&Category> {
    let mut shown: Vec<&Category> = catalog.categories.iter().filter(|category| !category.archived).collect();
    shown.sort_by(|left, right| left.order.total_cmp(&right.order));
    shown
}

/// The focuses of `category` shown, in their order.
fn visible_focuses(category: &Category) -> Vec<&Focus> {
    let mut shown: Vec<&Focus> = category.focuses.iter().filter(|focus| !focus.archived).collect();
    shown.sort_by(|left, right| left.order.total_cmp(&right.order));
    shown
}

/// The order a row takes between the orders of its new neighbours: the middle of the two, one
/// past the only one, or unchanged when it has none.
fn between(before: Option<f64>, after: Option<f64>, current: f64) -> f64 {
    match (before, after) {
        (Some(before), Some(after)) => f64::midpoint(before, after),
        (Some(before), None) => before + 1.0,
        (None, Some(after)) => after - 1.0,
        (None, None) => current,
    }
}

/// Applies a move among siblings: the rows the tree was given are moved the same way, and the
/// moved row takes an order between its new neighbours, so every other row keeps its own.
fn moved<M: From<Msg> + Clone + Send + 'static>(
    screen: &mut Today,
    catalog: &mut Catalog,
    step: &TreeMove,
) -> (Command<M>, Option<Request>) {
    let Some(row) = Row::parse(&step.key) else { return (Command::none(), None) };
    let with_add = screen.selected_category(catalog);
    let changed = match (&row, step.parent.as_deref().and_then(Row::parse)) {
        (Row::Category(id), None) => {
            let mut rows: Vec<Row> = visible_categories(catalog).iter().map(|c| Row::Category(c.id)).collect();
            rows.push(Row::AddCategory);
            step.apply(&mut rows);
            let orders: Vec<Option<f64>> = rows
                .iter()
                .map(|row| match row {
                    Row::Category(id) => category(catalog, *id).map(|c| c.order),
                    _ => None,
                })
                .collect();
            let (before, after) = neighbours(&rows, &orders, &row);
            match category_mut(catalog, *id) {
                Some(entry) => {
                    entry.order = between(before, after, entry.order);
                    true
                }
                None => false,
            }
        }
        (Row::Focus(id), Some(Row::Category(parent))) => match category_mut(catalog, parent) {
            Some(entry) => {
                let mut rows: Vec<Row> = visible_focuses(entry).iter().map(|f| Row::Focus(f.id)).collect();
                if with_add == Some(parent) {
                    rows.push(Row::AddFocus(parent));
                }
                step.apply(&mut rows);
                let orders: Vec<Option<f64>> = rows
                    .iter()
                    .map(|row| match row {
                        Row::Focus(id) => entry.focuses.iter().find(|f| f.id == *id).map(|f| f.order),
                        _ => None,
                    })
                    .collect();
                let (before, after) = neighbours(&rows, &orders, &row);
                match entry.focuses.iter_mut().find(|f| f.id == *id) {
                    Some(focus) => {
                        focus.order = between(before, after, focus.order);
                        true
                    }
                    None => false,
                }
            }
            None => false,
        },
        _ => false,
    };
    if !changed {
        return (Command::none(), None);
    }
    screen.selected = Some(row.key());
    // A drag leaves the pointer on the tree and the keys should follow it there.
    (Command::focus(TREE), Some(Request::Changed))
}

/// The orders of the nearest rows before and after `row` in `rows` that have one; add rows have
/// none and are looked past.
fn neighbours(rows: &[Row], orders: &[Option<f64>], row: &Row) -> (Option<f64>, Option<f64>) {
    let Some(at) = rows.iter().position(|other| other == row) else { return (None, None) };
    let before = orders[..at].iter().rev().find_map(|order| *order);
    let after = orders[at + 1..].iter().find_map(|order| *order);
    (before, after)
}

/// Moves the focus `focus` under the category `target`, at its end. The focus keeps its
/// identifier, so its history stays with it.
fn move_to<M: From<Msg> + Clone + Send + 'static>(
    screen: &mut Today,
    catalog: &mut Catalog,
    focus: Id,
    target: Id,
) -> (Command<M>, Option<Request>) {
    let Some(from) = category_of(catalog, focus) else { return (Command::none(), None) };
    if from == target || category(catalog, target).is_none() {
        return (Command::none(), None);
    }
    let Some(taken) = category_mut(catalog, from).and_then(|c| {
        let at = c.focuses.iter().position(|f| f.id == focus)?;
        Some(c.focuses.remove(at))
    }) else {
        return (Command::none(), None);
    };
    let name = taken.name.clone();
    let Some(into) = category_mut(catalog, target) else { return (Command::none(), None) };
    let order = into.focuses.iter().map(|f| f.order).fold(0.0, f64::max) + 1.0;
    into.focuses.push(Focus { order, ..taken });
    let category_name = into.name.clone();
    screen.collapsed.remove(&target);
    screen.selected = Some(Row::Focus(focus).key());
    let toast = Toast::success(t!("today.moved", name = name, category = category_name));
    (Command::toast(toast), Some(Request::Changed))
}

/// What activating a row does: a focus starts, an add row opens the name field, a category is
/// opened or closed by the tree itself.
fn activate<M: From<Msg> + Clone + Send + 'static>(
    screen: &mut Today,
    catalog: &Catalog,
    key: &str,
) -> (Command<M>, Option<Request>) {
    screen.selected = Some(key.to_owned());
    match Row::parse(key) {
        Some(Row::Focus(id)) if catalog.focus(id).is_some() => (Command::none(), Some(Request::Start(id))),
        Some(Row::AddFocus(id)) => {
            screen.open(Editing::NewFocus(id), String::new());
            (Command::focus(NAME_INPUT), None)
        }
        Some(Row::AddCategory) => {
            screen.open(Editing::NewCategory, String::new());
            (Command::focus(NAME_INPUT), None)
        }
        _ => (Command::none(), None),
    }
}

/// Takes the typed name: an empty one is refused with the reason beside the field, a name a
/// sibling already has is allowed and mentioned.
fn submit<M: From<Msg> + Clone + Send + 'static>(
    screen: &mut Today,
    catalog: &mut Catalog,
) -> (Command<M>, Option<Request>) {
    let Some(edit) = screen.edit.as_mut() else { return (Command::none(), None) };
    let name = edit.text.trim().to_owned();
    if name.is_empty() {
        edit.refused = true;
        return (Command::none(), None);
    }
    let what = edit.what.clone();
    screen.edit = None;
    let taken = match &what {
        Editing::NewCategory | Editing::Rename(Row::Category(_)) => {
            let own = if let Editing::Rename(Row::Category(id)) = &what { Some(*id) } else { None };
            catalog.categories.iter().any(|c| !c.archived && Some(c.id) != own && c.name == name)
        }
        Editing::NewFocus(category) => {
            self::category(catalog, *category).is_some_and(|c| c.focuses.iter().any(|f| !f.archived && f.name == name))
        }
        Editing::Rename(Row::Focus(id)) => catalog
            .categories
            .iter()
            .find(|c| c.focuses.iter().any(|f| f.id == *id))
            .is_some_and(|c| c.focuses.iter().any(|f| !f.archived && f.id != *id && f.name == name)),
        Editing::Rename(_) => false,
    };
    let selected = match what {
        Editing::NewCategory => {
            let order = catalog.categories.iter().map(|c| c.order).fold(0.0, f64::max) + 1.0;
            let id = clock::new_id();
            catalog.categories.push(Category {
                id,
                name: name.clone(),
                icon: None,
                goal: None,
                archived: false,
                order,
                focuses: Vec::new(),
            });
            Some(Row::Category(id))
        }
        Editing::NewFocus(category) => match category_mut(catalog, category) {
            Some(parent) => {
                let order = parent.focuses.iter().map(|f| f.order).fold(0.0, f64::max) + 1.0;
                let id = clock::new_id();
                parent.focuses.push(Focus { id, name: name.clone(), goal: None, archived: false, order });
                screen.collapsed.remove(&category);
                Some(Row::Focus(id))
            }
            None => None,
        },
        Editing::Rename(Row::Category(id)) => category_mut(catalog, id).map(|c| {
            c.name = name.clone();
            Row::Category(id)
        }),
        Editing::Rename(Row::Focus(id)) => focus_mut(catalog, id).map(|f| {
            f.name = name.clone();
            Row::Focus(id)
        }),
        Editing::Rename(_) => None,
    };
    let Some(row) = selected else { return (Command::focus(TREE), None) };
    screen.selected = Some(row.key());
    let mut commands = vec![Command::focus(TREE)];
    if taken {
        commands.push(Command::toast(Toast::warning(t!("today.name-taken", name = name))));
    }
    (Command::batch(commands), Some(Request::Changed))
}

/// Archives the selected category or focus, with a toast that brings it back.
fn archive<M: From<Msg> + Clone + Send + 'static>(
    screen: &mut Today,
    catalog: &mut Catalog,
) -> (Command<M>, Option<Request>) {
    let (row, name, key) = match screen.selected() {
        Some(Row::Category(id)) => match category_mut(catalog, id) {
            Some(category) => {
                category.archived = true;
                (Row::Category(id), category.name.clone(), "today.archived-category")
            }
            None => return (Command::none(), None),
        },
        Some(Row::Focus(id)) => match focus_mut(catalog, id) {
            Some(focus) => {
                focus.archived = true;
                (Row::Focus(id), focus.name.clone(), "today.archived-focus")
            }
            None => return (Command::none(), None),
        },
        _ => return (Command::none(), None),
    };
    // The selection moves to where the row was: its category, or the add row at the end.
    screen.selected = Some(match &row {
        Row::Focus(id) => catalog
            .categories
            .iter()
            .find(|c| c.focuses.iter().any(|f| f.id == *id))
            .map_or_else(|| Row::AddCategory.key(), |c| Row::Category(c.id).key()),
        _ => Row::AddCategory.key(),
    });
    let toast = Toast::info(t!(key, name = name)).action(t!("today.undo"), M::from(Msg::Restore(row)));
    (Command::toast(toast), Some(Request::Changed))
}

/// Brings an archived row back and selects it.
fn restore<M: From<Msg> + Clone + Send + 'static>(
    screen: &mut Today,
    catalog: &mut Catalog,
    row: &Row,
) -> (Command<M>, Option<Request>) {
    let name = match row {
        Row::Category(id) => category_mut(catalog, *id).map(|c| {
            c.archived = false;
            c.name.clone()
        }),
        Row::Focus(id) => focus_mut(catalog, *id).map(|f| {
            f.archived = false;
            f.name.clone()
        }),
        _ => None,
    };
    let Some(name) = name else { return (Command::none(), None) };
    screen.selected = Some(row.key());
    (Command::toast(Toast::success(t!("today.restored", name = name))), Some(Request::Changed))
}

/// The category with this identifier.
fn category(catalog: &Catalog, id: Id) -> Option<&Category> {
    catalog.categories.iter().find(|category| category.id == id)
}

/// The category with this identifier, to change.
fn category_mut(catalog: &mut Catalog, id: Id) -> Option<&mut Category> {
    catalog.categories.iter_mut().find(|category| category.id == id)
}

/// The focus with this identifier, to change.
fn focus_mut(catalog: &mut Catalog, id: Id) -> Option<&mut Focus> {
    catalog.categories.iter_mut().flat_map(|category| category.focuses.iter_mut()).find(|focus| focus.id == id)
}

/// Whether the catalogue has anything that is not archived.
fn is_empty(catalog: &Catalog) -> bool {
    !catalog.categories.iter().any(|category| !category.archived)
}

/// Draws the screen: the tree with the day's work, or the invitation when there is nothing yet,
/// the meters of the goals under it, and the name, goal or countdown field while one is open.
/// `can_start` false mutes the focuses, for an instance that only looks. `goals` are the rows
/// that have a goal, measured; `suggested` is what the goal field offers a row without one.
pub fn view<M: From<Msg> + Clone + Send + 'static>(
    screen: &Today,
    catalog: &Catalog,
    totals: &DayTotals,
    goals: &[GoalRow],
    units: &Units<'_>,
    (can_start, suggested): (bool, u32),
    ui: &mut View<'_, M>,
) {
    ui.column(|ui| {
        if is_empty(catalog) && screen.edit.is_none() {
            let action = Button::new(t!("today.empty-action"))
                .icon("add")
                .on_press(M::from(Msg::Activate(Row::AddCategory.key())));
            ui.add(EmptyState::new(t!("today.empty-title")).message(t!("today.empty-message")).action(action)).fill();
        } else {
            let roots = nodes(screen, catalog, totals, units, can_start);
            let menu = MenuNames::of(catalog, can_start, suggested);
            ui.add(
                Tree::new(roots)
                    .selected(screen.selected.as_deref())
                    .on_select(|key| M::from(Msg::Select(key.to_owned())))
                    .on_activate(|key| M::from(Msg::Activate(key.to_owned())))
                    .on_expand(|key, open| M::from(Msg::Expand(key.to_owned(), open)))
                    .reorderable(|step| M::from(Msg::Move(step)))
                    .context_menu(move |key| menu.items(key)),
            )
            .id(TREE)
            .fill();
            if ui.size().width >= GOALS_BELOW {
                ui.column(|ui| goal_gauges(goals, units, ui)).padding(Padding::symmetric(0, 1)).fill_width();
            }
        }
        if let Some(edit) = &screen.edit {
            name_field(edit, catalog, ui);
        }
        if let Some(edit) = &screen.goal {
            goal_field(edit, catalog, ui);
        }
        if let Some(timed) = &screen.timed {
            timed_field(timed, catalog, ui);
        }
    })
    .fill();
}

/// The goal field under the tree: what it is for, the length, the period, and the buttons that
/// keep or remove the goal. Narrow, its parts stack.
fn goal_field<M: From<Msg> + Clone + Send + 'static>(edit: &GoalEdit, catalog: &Catalog, ui: &mut View<'_, M>) {
    let label = t!("goal.for", name = name_of(catalog, &edit.row).unwrap_or_default());
    let periods = PERIODS.map(|period| t!(&format!("goal.{}", period.as_str())));
    let chosen = PERIODS.iter().position(|period| *period == edit.period).unwrap_or(0);
    let stacked = ui.size().width < FIELD_STACKS_BELOW;
    let parts = |ui: &mut View<'_, M>| {
        ui.add(Text::new(label.clone()).role("secondary").no_wrap());
        ui.add(
            DurationInput::new(Duration::from_secs(edit.amount))
                .invalid(edit.refused)
                .on_change(|length| M::from(Msg::GoalAmount(length))),
        )
        .id(GOAL_INPUT);
        ui.add(Segmented::new(periods.clone()).selected(chosen).on_select(|index| M::from(Msg::GoalPeriod(index))));
        ui.row(|ui| {
            ui.add(Button::new(t!("goal.save")).variant("primary").on_press(M::from(Msg::GoalSave)));
            if edit.had_goal {
                ui.add(Button::new(t!("goal.remove")).on_press(M::from(Msg::GoalRemove)));
            }
        })
        .gap(2);
    };
    field(stacked, edit.refused.then(|| t!("goal.zero")), parts, ui);
}

/// Lays a field's `parts` out under the tree, in a row or, when `stacked`, one under another,
/// with `refused` as the reason on its own line beneath them.
fn field<M: 'static>(
    stacked: bool,
    refused: Option<String>,
    parts: impl FnOnce(&mut View<'_, M>),
    ui: &mut View<'_, M>,
) {
    ui.column(|ui| {
        if stacked {
            ui.column(parts).gap(0).fill_width();
        } else {
            ui.row(parts).gap(2).fill_width();
        }
        if let Some(reason) = refused {
            refusal(reason, ui);
        }
    })
    .gap(0)
    .padding(Padding::symmetric(0, 1))
    .fill_width();
}

/// The countdown field under the tree: the focus it starts, the length and the start button.
fn timed_field<M: From<Msg> + Clone + Send + 'static>(timed: &TimedStart, catalog: &Catalog, ui: &mut View<'_, M>) {
    let name = catalog.focus(timed.focus).map(|focus| focus.name.clone()).unwrap_or_default();
    let label = t!("timed.for", name = name);
    let stacked = ui.size().width < FIELD_STACKS_BELOW;
    let parts = |ui: &mut View<'_, M>| {
        ui.add(Text::new(label.clone()).role("secondary").no_wrap());
        ui.add(
            DurationInput::new(Duration::from_secs(timed.amount))
                .invalid(timed.refused)
                .on_change(|length| M::from(Msg::TimedAmount(length))),
        )
        .id(TIMED_INPUT);
        ui.add(Button::new(t!("timed.start")).variant("primary").on_press(M::from(Msg::TimedStart)));
    };
    field(stacked, timed.refused.then(|| t!("timed.zero")), parts, ui);
}

/// A reason a field refused what it holds: a mark and the words.
fn refusal<M: 'static>(reason: String, ui: &mut View<'_, M>) {
    let mark = ui.env().icons().glyph("warning").into_owned();
    ui.add(Text::rich([Span::new(format!("{mark} ")).color("warning"), Span::new(reason)]).no_wrap());
}

/// Draws the day strip with its legend: the blocks of `blocks` on a timeline that runs the day
/// from `rollover`, each in the tone of its category, and under it the legend naming those
/// tones. The strip's own readout row names the block being read. A day with no blocks draws
/// nothing at all, so an empty day is the header and the tree, not a bare track.
pub fn strip_view<M: From<Msg> + Clone + Send + 'static>(
    screen: &Today,
    catalog: &Catalog,
    blocks: &[Block],
    rollover: TimeOfDay,
    ui: &mut View<'_, M>,
) {
    if blocks.is_empty() {
        return;
    }
    let owners: Vec<Option<Id>> = blocks.iter().map(|block| category_of(catalog, block.focus)).collect();
    let time_blocks = blocks.iter().zip(&owners).map(|(block, owner)| {
        let name = catalog.focus(block.focus).map_or_else(|| t!("records.unknown-focus"), |focus| focus.name.clone());
        let time_block = TimeBlock::new(name, at(rollover, block.from), at(rollover, block.from + block.seconds))
            .tone(stats::tone_of(catalog, *owner));
        // The edge says how the block sits in the day, and the readout words it: a session
        // that goes on past the day's end and the running counter fade out at their end, the
        // rest of a session from the day before fades in and stands fainter, as its whole is
        // on that day's strip.
        match block.edge {
            Edge::Whole => time_block,
            Edge::PastEnd | Edge::Running => time_block.open_end(),
            Edge::Continued => time_block.open_start().faint(),
        }
    });
    let mut timeline = Timeline::new(time_blocks)
        .day_starts_at(rollover)
        .axis()
        .readout()
        .selected(screen.block.filter(|index| *index < blocks.len()))
        .on_select(|index| M::from(Msg::Block(index)))
        .on_zoom(|from, to| M::from(Msg::Zoom(from, to)));
    if let Some((from, to)) = screen.zoom {
        timeline = timeline.range(from, to);
    }
    ui.column(|ui| {
        ui.add(timeline).id(STRIP).fill_width();
        if ui.size().width >= LEGEND_BELOW {
            let mut shown: Vec<Option<Id>> = owners;
            shown.sort_by_key(|owner| (stats::tone_of(catalog, *owner), *owner));
            shown.dedup();
            let names = shown.iter().map(|owner| category_name(catalog, *owner));
            let tones = shown.iter().map(|owner| stats::tone_of(catalog, *owner));
            ui.add(Legend::new(names).tones(tones)).fill_width();
        }
    })
    .padding(Padding::symmetric(0, 1))
    .fill_width();
}

/// The clock time `seconds` after the start of a day that turns at `rollover`. A block that
/// fills the whole day would end where it starts, which the timeline reads as a moment, so it
/// is held a second short.
fn at(rollover: TimeOfDay, seconds: u32) -> TimeOfDay {
    TimeOfDay::from_seconds_since_midnight((rollover.seconds_since_midnight() + seconds.min(DAY - 1)) % DAY)
}

/// The category the focus `id` is in, or `None` for a focus that is in none.
fn category_of(catalog: &Catalog, id: Id) -> Option<Id> {
    catalog.categories.iter().find(|category| category.focuses.iter().any(|focus| focus.id == id)).map(|c| c.id)
}

/// The name of the category `id`, or the words for sessions whose focus is in no category.
fn category_name(catalog: &Catalog, id: Option<Id>) -> String {
    id.and_then(|id| category(catalog, id)).map_or_else(|| t!("charts.unknown-category"), |c| c.name.clone())
}

/// The rows of the tree: open categories with their focuses, the add-focus row under the
/// category the selection is in, and the add-category row at the end.
fn nodes(screen: &Today, catalog: &Catalog, totals: &DayTotals, units: &Units<'_>, can_start: bool) -> Vec<TreeNode> {
    let none = t!("today.none");
    let detail = |seconds: u64| if seconds == 0 { none.clone() } else { short(seconds, units) };
    let with_add = screen.selected_category(catalog);
    let mut roots: Vec<TreeNode> = visible_categories(catalog)
        .into_iter()
        .map(|category| {
            let mut children: Vec<TreeNode> = visible_focuses(category)
                .into_iter()
                .map(|focus| {
                    let seconds = totals.by_focus.get(&focus.id).copied().unwrap_or(0);
                    TreeNode::new(Row::Focus(focus.id).key(), &focus.name).detail(detail(seconds)).faint(!can_start)
                })
                .collect();
            if with_add == Some(category.id) {
                children.push(
                    TreeNode::new(Row::AddFocus(category.id).key(), t!("today.add-focus"))
                        .icon("add", None)
                        .faint(true),
                );
            }
            TreeNode::new(Row::Category(category.id).key(), &category.name)
                .detail(detail(category_total(totals, category)))
                .children(children)
                .expanded(!screen.collapsed.contains(&category.id))
        })
        .collect();
    roots.push(TreeNode::new(Row::AddCategory.key(), t!("today.add-category")).icon("add", None).faint(true));
    roots
}

/// What the context menu of a row needs to know about the catalogue, owned, so the menu can be
/// built for any row after the view has been declared.
struct MenuNames {
    /// The categories shown, with their names, for the "move to" entries.
    categories: Vec<(Id, String)>,
    /// The category each shown focus is in.
    owners: Vec<(Id, Id)>,
    /// Whether a focus may be started here.
    can_start: bool,
    /// What the goal and countdown fields suggest, in seconds.
    suggested: u32,
}

impl MenuNames {
    fn of(catalog: &Catalog, can_start: bool, suggested: u32) -> Self {
        let shown = visible_categories(catalog);
        let categories = shown.iter().map(|c| (c.id, c.name.clone())).collect();
        let owners = shown.iter().flat_map(|c| visible_focuses(c).into_iter().map(move |f| (f.id, c.id))).collect();
        Self { categories, owners, can_start, suggested }
    }

    /// The entries of the menu of the row with `key`: what the row can do, then where it can
    /// go, then what archives it, marked as the one entry that takes it out of the list.
    fn items<M: From<Msg> + 'static>(&self, key: &str) -> Vec<ContextItem<M>> {
        let send = |action: MenuAction| M::from(Msg::Menu(key.to_owned(), action));
        let archive = ContextItem::new(t!("today.menu-archive"), send(MenuAction::Archive))
            .icon("warning")
            .shortcut("del")
            .danger(true);
        match Row::parse(key) {
            Some(Row::Focus(id)) => {
                let owner = self.owners.iter().find(|(focus, _)| *focus == id).map(|(_, owner)| *owner);
                let elsewhere: Vec<ContextItem<M>> = self
                    .categories
                    .iter()
                    .filter(|(category, _)| Some(*category) != owner)
                    .map(|(category, name)| ContextItem::new(name.clone(), send(MenuAction::MoveTo(*category))))
                    .collect();
                let nowhere = elsewhere.is_empty();
                vec![
                    ContextItem::new(t!("today.menu-start"), send(MenuAction::Start))
                        .shortcut("enter")
                        .disabled(!self.can_start),
                    ContextItem::new(t!("today.menu-timed"), send(MenuAction::Timed(self.suggested)))
                        .shortcut("t")
                        .disabled(!self.can_start),
                    ContextItem::new(t!("today.menu-rename"), send(MenuAction::Rename)).shortcut("f2"),
                    ContextItem::new(t!("today.menu-goal"), send(MenuAction::Goal(self.suggested))).shortcut("g"),
                    ContextItem::gap(),
                    ContextItem::submenu(t!("today.menu-move"), elsewhere).disabled(nowhere),
                    ContextItem::gap(),
                    archive,
                ]
            }
            Some(Row::Category(_)) => vec![
                ContextItem::new(t!("today.menu-rename"), send(MenuAction::Rename)).shortcut("f2"),
                ContextItem::new(t!("today.menu-goal"), send(MenuAction::Goal(self.suggested))).shortcut("g"),
                ContextItem::new(t!("today.menu-add-focus"), send(MenuAction::AddFocus)).icon("add"),
                ContextItem::gap(),
                archive,
            ],
            Some(Row::AddFocus(_)) => {
                vec![ContextItem::new(t!("today.menu-add-focus"), send(MenuAction::AddFocus)).icon("add")]
            }
            Some(Row::AddCategory) => {
                vec![ContextItem::new(t!("today.menu-add-category"), send(MenuAction::AddCategory)).icon("add")]
            }
            None => Vec::new(),
        }
    }
}

/// The name field under the tree, with what it is for on the left and, when an empty name was
/// refused, why on the right.
fn name_field<M: From<Msg> + Clone + Send + 'static>(edit: &Edit, catalog: &Catalog, ui: &mut View<'_, M>) {
    let label = match &edit.what {
        Editing::NewCategory => t!("today.new-category"),
        Editing::NewFocus(id) => {
            let name = category(catalog, *id).map(|c| c.name.clone()).unwrap_or_default();
            t!("today.new-focus", category = name)
        }
        Editing::Rename(Row::Category(id)) => {
            t!("today.rename", name = category(catalog, *id).map(|c| c.name.clone()).unwrap_or_default())
        }
        Editing::Rename(Row::Focus(id)) => {
            t!("today.rename", name = catalog.focus(*id).map(|f| f.name.clone()).unwrap_or_default())
        }
        Editing::Rename(_) => String::new(),
    };
    ui.row(|ui| {
        ui.add(Text::new(label).role("secondary").no_wrap());
        ui.add(
            TextInput::new(&edit.text)
                .placeholder(t!("today.name-placeholder"))
                .invalid(edit.refused)
                .on_change(|text| M::from(Msg::Typed(text)))
                .on_submit(|_| M::from(Msg::Submit)),
        )
        .id(NAME_INPUT)
        .width(Length::Fill(1));
        if edit.refused {
            refusal(t!("today.name-empty"), ui);
        }
    })
    .gap(1)
    .padding(Padding::symmetric(0, 1))
    .fill_width();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_with(names: &[(&str, &[&str])]) -> Catalog {
        let mut catalog = Catalog { categories: Vec::new() };
        for (index, (name, focuses)) in names.iter().enumerate() {
            let focuses = focuses
                .iter()
                .enumerate()
                .map(|(place, focus)| Focus {
                    id: Id::new(1_000 + u64::try_from(place).unwrap_or(0), u128::try_from(index).unwrap_or(0)),
                    name: (*focus).to_owned(),
                    goal: None,
                    archived: false,
                    order: 0.0,
                })
                .collect();
            catalog.categories.push(Category {
                id: Id::new(100 + u64::try_from(index).unwrap_or(0), 0),
                name: (*name).to_owned(),
                icon: None,
                goal: None,
                archived: false,
                order: 0.0,
                focuses,
            });
        }
        catalog
    }

    #[test]
    fn keys_round_trip() {
        let id = Id::new(5, 7);
        for row in [Row::Category(id), Row::Focus(id), Row::AddFocus(id), Row::AddCategory] {
            assert_eq!(Row::parse(&row.key()), Some(row));
        }
        assert_eq!(Row::parse("x:nothing"), None);
    }

    #[test]
    fn an_empty_name_is_refused_and_the_field_stays_open() {
        let mut screen = Today::new();
        let mut catalog = catalog_with(&[]);
        let _: (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Activate(Row::AddCategory.key()));
        let _: (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Typed("   ".to_owned()));
        let (_, request): (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Submit);
        assert_eq!(request, None);
        assert!(screen.edit().is_some_and(|edit| edit.refused));
        assert!(catalog.categories.is_empty());
    }

    #[test]
    fn a_new_focus_is_selected_and_the_catalogue_changes() {
        let mut screen = Today::new();
        let mut catalog = catalog_with(&[("Work", &[])]);
        let category = catalog.categories[0].id;
        let _: (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Activate(Row::AddFocus(category).key()));
        let _: (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Typed("Rust".to_owned()));
        let (_, request): (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Submit);
        assert_eq!(request, Some(Request::Changed));
        let focus = catalog.categories[0].focuses[0].id;
        assert_eq!(screen.selected(), Some(Row::Focus(focus)));
        assert_eq!(catalog.categories[0].focuses[0].name, "Rust");
    }

    #[test]
    fn a_focus_starts_and_an_archived_one_comes_back() {
        let mut screen = Today::new();
        let mut catalog = catalog_with(&[("Work", &["Rust"])]);
        let focus = catalog.categories[0].focuses[0].id;
        let (_, request): (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Activate(Row::Focus(focus).key()));
        assert_eq!(request, Some(Request::Start(focus)));
        let (_, request): (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Archive);
        assert_eq!(request, Some(Request::Changed));
        assert!(catalog.categories[0].focuses[0].archived);
        assert_eq!(screen.selected(), Some(Row::Category(catalog.categories[0].id)));
        let _: (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Restore(Row::Focus(focus)));
        assert!(!catalog.categories[0].focuses[0].archived);
        assert_eq!(screen.selected(), Some(Row::Focus(focus)));
    }

    #[test]
    fn a_block_is_selected_and_the_same_time_twice_is_the_whole_day() {
        let mut screen = Today::new();
        let mut catalog = catalog_with(&[]);
        let _: (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Block(2));
        assert_eq!(screen.block(), Some(2));
        let (from, to) = (TimeOfDay::new(9, 0, 0), TimeOfDay::new(12, 0, 0));
        let _: (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Zoom(from, to));
        assert_eq!(screen.zoom(), Some((from, to)));
        let _: (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Zoom(from, from));
        assert_eq!(screen.zoom(), None);
    }

    fn step(key: &str, parent: Option<&str>, from: usize, to: usize) -> TreeMove {
        TreeMove { key: key.to_owned(), parent: parent.map(str::to_owned), from, to }
    }

    #[test]
    fn a_move_among_siblings_writes_an_order_between_the_neighbours() {
        let mut screen = Today::new();
        let mut catalog = catalog_with(&[("Work", &["Rust", "Review", "Docs"]), ("Life", &[]), ("Study", &[])]);
        for (index, category) in catalog.categories.iter_mut().enumerate() {
            category.order = f64::from(u8::try_from(index).unwrap_or(0));
            for (place, focus) in category.focuses.iter_mut().enumerate() {
                focus.order = f64::from(u8::try_from(place).unwrap_or(0));
            }
        }
        let work = catalog.categories[0].id;
        let study = catalog.categories[2].id;
        // The last category dragged to the top lands before the first one's order.
        let (_, request): (Command<Msg>, _) =
            update(&mut screen, &mut catalog, Msg::Move(step(&Row::Category(study).key(), None, 2, 0)));
        assert_eq!(request, Some(Request::Changed));
        assert_eq!(catalog.categories[2].order, -1.0);
        assert_eq!(screen.selected(), Some(Row::Category(study)));
        let names: Vec<&str> = visible_categories(&catalog).iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Study", "Work", "Life"]);
        // A focus dragged between two others takes the middle of their orders; nothing else moves.
        let rust = catalog.categories[0].focuses[0].id;
        let parent = Row::Category(work).key();
        let _: (Command<Msg>, _) =
            update(&mut screen, &mut catalog, Msg::Move(step(&Row::Focus(rust).key(), Some(&parent), 0, 1)));
        let orders: Vec<f64> = catalog.categories[0].focuses.iter().map(|f| f.order).collect();
        assert_eq!(orders, [1.5, 1.0, 2.0]);
        let names: Vec<&str> = visible_focuses(&catalog.categories[0]).iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["Review", "Rust", "Docs"]);
        // Dropped past the add row at the end, a focus becomes the last one.
        let _: (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Select(Row::Category(work).key()));
        let _: (Command<Msg>, _) =
            update(&mut screen, &mut catalog, Msg::Move(step(&Row::Focus(rust).key(), Some(&parent), 1, 3)));
        assert_eq!(catalog.categories[0].focuses[0].order, 3.0);
        // The add rows themselves never move.
        let (_, request): (Command<Msg>, _) =
            update(&mut screen, &mut catalog, Msg::Move(step(&Row::AddCategory.key(), None, 3, 0)));
        assert_eq!(request, None);
    }

    #[test]
    fn a_focus_moves_to_another_category_and_keeps_its_identifier() {
        let mut screen = Today::new();
        let mut catalog = catalog_with(&[("Work", &["Rust"]), ("Life", &["Reading"])]);
        let rust = catalog.categories[0].focuses[0].id;
        let life = catalog.categories[1].id;
        let key = Row::Focus(rust).key();
        let (_, request): (Command<Msg>, _) =
            update(&mut screen, &mut catalog, Msg::Menu(key, MenuAction::MoveTo(life)));
        assert_eq!(request, Some(Request::Changed));
        assert!(catalog.categories[0].focuses.is_empty());
        assert_eq!(catalog.categories[1].focuses.len(), 2);
        assert_eq!(catalog.categories[1].focuses[1].id, rust);
        assert!(catalog.categories[1].focuses[1].order > catalog.categories[1].focuses[0].order);
        assert_eq!(screen.selected(), Some(Row::Focus(rust)));
        // Moving to where it already is changes nothing.
        let key = Row::Focus(rust).key();
        let (_, request): (Command<Msg>, _) =
            update(&mut screen, &mut catalog, Msg::Menu(key, MenuAction::MoveTo(life)));
        assert_eq!(request, None);
    }

    #[test]
    fn the_menu_acts_on_the_row_it_was_opened_on() {
        let mut screen = Today::new();
        let mut catalog = catalog_with(&[("Work", &["Rust", "Review"])]);
        let work = catalog.categories[0].id;
        let review = catalog.categories[0].focuses[1].id;
        let _: (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Select(Row::Category(work).key()));
        let (_, request): (Command<Msg>, _) =
            update(&mut screen, &mut catalog, Msg::Menu(Row::Focus(review).key(), MenuAction::Archive));
        assert_eq!(request, Some(Request::Changed));
        assert!(catalog.categories[0].focuses[1].archived);
        assert!(!catalog.categories[0].focuses[0].archived);
        let (_, request): (Command<Msg>, _) =
            update(&mut screen, &mut catalog, Msg::Menu(Row::Focus(review).key(), MenuAction::AddFocus));
        assert_eq!(request, None);
        assert_eq!(screen.edit().map(|edit| edit.what.clone()), Some(Editing::NewFocus(work)));
        let names = MenuNames::of(&catalog, true, 3_600);
        let items: Vec<ContextItem<Msg>> = names.items(&Row::Focus(review).key());
        assert_eq!(items.len(), 8, "start, timed, rename, goal, gap, move to, gap, archive");
        let items: Vec<ContextItem<Msg>> = names.items(&Row::AddCategory.key());
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn renaming_keeps_the_row_selected() {
        let mut screen = Today::new();
        let mut catalog = catalog_with(&[("Work", &["Rust"])]);
        let category = catalog.categories[0].id;
        let _: (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Select(Row::Category(category).key()));
        let _: (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Rename);
        assert_eq!(screen.edit().map(|edit| edit.text.as_str()), Some("Work"));
        let _: (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Typed("Craft".to_owned()));
        let (_, request): (Command<Msg>, _) = update(&mut screen, &mut catalog, Msg::Submit);
        assert_eq!(request, Some(Request::Changed));
        assert_eq!(catalog.categories[0].name, "Craft");
        assert_eq!(screen.selected(), Some(Row::Category(category)));
    }
}
