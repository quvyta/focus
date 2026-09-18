//! The categories a person keeps and the focuses under them.
//!
//! The tree is written whole and rarely, and a person may fix it by hand, so it is TOML. A broken
//! entry never empties the file: it is skipped, it becomes a located diagnostic, and the rest is
//! kept. Reading goes through the same recoverable parser the framework uses for its own files, so
//! every problem can point at a line and a column.

use std::collections::HashSet;

use qframe::diagnostics::{Diagnostic, Location};
use toml::Spanned;
use toml::de::{DeTable, DeValue};

use crate::id::Id;

/// How often a goal starts over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    /// Every day.
    Day,
    /// Every week.
    Week,
    /// Every month.
    Month,
}

impl Period {
    /// The word written in a file.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }

    /// Reads the word written in a file.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            _ => None,
        }
    }
}

/// Time a person means to give something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Goal {
    /// Seconds in the period.
    pub amount: u32,
    /// How often it starts over.
    pub period: Period,
}

/// One thing a person works on.
#[derive(Debug, Clone, PartialEq)]
pub struct Focus {
    /// Its permanent identifier; sessions carry this, never the name.
    pub id: Id,
    /// What the person calls it.
    pub name: String,
    /// What they mean to give it, if anything.
    pub goal: Option<Goal>,
    /// Archived focuses leave the lists but stay in the history.
    pub archived: bool,
    /// Where it sits among its siblings.
    pub order: f64,
}

/// A group of focuses.
#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    /// Its permanent identifier.
    pub id: Id,
    /// What the person calls it.
    pub name: String,
    /// An icon from the icon set, if they chose one.
    pub icon: Option<String>,
    /// What they mean to give the whole category, if anything.
    pub goal: Option<Goal>,
    /// Archived categories leave the lists but stay in the history.
    pub archived: bool,
    /// Where it sits among its siblings.
    pub order: f64,
    /// The focuses under it.
    pub focuses: Vec<Focus>,
}

/// Every category a person keeps.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tree {
    /// The categories, in the order they were read.
    pub categories: Vec<Category>,
}

impl Tree {
    /// Reads a tree, keeping everything that can be read.
    ///
    /// A mistake in one entry costs that entry and nothing else. The file on disk is never
    /// rewritten here, so nothing a person typed is lost to a parse error.
    #[must_use]
    pub fn read(file: &str, text: &str) -> (Self, Vec<Diagnostic>) {
        let mut diagnostics = Vec::new();
        let (root, errors) = DeTable::parse_recoverable(text);
        for error in &errors {
            let location = error.span().map(|span| Location::from_offset(file, text, span.start));
            diagnostics.push(Diagnostic::error(location, error.message().to_owned()));
        }

        let mut seen: HashSet<Id> = HashSet::new();
        let mut categories = Vec::new();
        let entries = entry(root.get_ref(), "category").and_then(array).unwrap_or(&[]);
        for (index, item) in entries.iter().enumerate() {
            let Some(entry_table) = table(item) else {
                let location = Location::from_offset(file, text, item.span().start);
                diagnostics.push(Diagnostic::error(Some(location), format!("category {} is not a table", index + 1)));
                continue;
            };
            let Some(id) = read_id(entry_table, &mut seen, file, text, "category", index, &mut diagnostics) else {
                continue;
            };
            let mut focuses = Vec::new();
            let under = entry(entry_table, "focus").and_then(array).unwrap_or(&[]);
            for (place, child) in under.iter().enumerate() {
                let Some(child_table) = table(child) else {
                    let location = Location::from_offset(file, text, child.span().start);
                    diagnostics.push(Diagnostic::error(Some(location), format!("focus {} is not a table", place + 1)));
                    continue;
                };
                let Some(focus_id) = read_id(child_table, &mut seen, file, text, "focus", place, &mut diagnostics)
                else {
                    continue;
                };
                focuses.push(Focus {
                    id: focus_id,
                    name: string(child_table, "name").unwrap_or_default(),
                    goal: read_goal(child_table),
                    archived: boolean(child_table, "archived").unwrap_or(false),
                    order: float(child_table, "order").unwrap_or_else(|| place_of(place)),
                });
            }
            categories.push(Category {
                id,
                name: string(entry_table, "name").unwrap_or_default(),
                icon: string(entry_table, "icon"),
                goal: read_goal(entry_table),
                archived: boolean(entry_table, "archived").unwrap_or(false),
                order: float(entry_table, "order").unwrap_or_else(|| place_of(index)),
                focuses,
            });
        }
        (Self { categories }, diagnostics)
    }

    /// The tree as TOML. Values that are already the default are left out, as settings files do.
    #[must_use]
    pub fn write(&self) -> String {
        let mut out = String::new();
        for category in &self.categories {
            out.push_str("[[category]]\n");
            write_text(&mut out, "id", &category.id.to_string());
            write_text(&mut out, "name", &category.name);
            if let Some(icon) = &category.icon {
                write_text(&mut out, "icon", icon);
            }
            if category.archived {
                out.push_str("archived = true\n");
            }
            write_number(&mut out, "order", category.order);
            if let Some(goal) = category.goal {
                write_goal(&mut out, goal);
            }
            for focus in &category.focuses {
                out.push_str("\n[[category.focus]]\n");
                write_text(&mut out, "id", &focus.id.to_string());
                write_text(&mut out, "name", &focus.name);
                if focus.archived {
                    out.push_str("archived = true\n");
                }
                write_number(&mut out, "order", focus.order);
                if let Some(goal) = focus.goal {
                    write_goal(&mut out, goal);
                }
            }
            out.push('\n');
        }
        out
    }

    /// The focus with this identifier, wherever it sits.
    #[must_use]
    pub fn focus(&self, id: Id) -> Option<&Focus> {
        self.categories.iter().flat_map(|category| &category.focuses).find(|focus| focus.id == id)
    }
}

/// The place an entry without an `order` keeps: the one it was written in.
fn place_of(index: usize) -> f64 {
    u32::try_from(index).map_or(0.0, f64::from)
}

/// The value of `key` in `table`, if it has one.
fn entry<'a, 'i>(table: &'a DeTable<'i>, key: &str) -> Option<&'a Spanned<DeValue<'i>>> {
    table.iter().find(|(name, _)| name.get_ref().as_ref() == key).map(|(_, value)| value)
}

/// The items of an array value.
fn array<'a, 'i>(value: &'a Spanned<DeValue<'i>>) -> Option<&'a [Spanned<DeValue<'i>>]> {
    match value.get_ref() {
        DeValue::Array(items) => Some(&items[..]),
        _ => None,
    }
}

/// The table inside a value.
fn table<'a, 'i>(value: &'a Spanned<DeValue<'i>>) -> Option<&'a DeTable<'i>> {
    match value.get_ref() {
        DeValue::Table(inner) => Some(inner),
        _ => None,
    }
}

/// A text value.
fn string(table: &DeTable<'_>, key: &str) -> Option<String> {
    match entry(table, key)?.get_ref() {
        DeValue::String(text) => Some(text.to_string()),
        _ => None,
    }
}

/// A true or false value.
fn boolean(table: &DeTable<'_>, key: &str) -> Option<bool> {
    match entry(table, key)?.get_ref() {
        DeValue::Boolean(flag) => Some(*flag),
        _ => None,
    }
}

/// A number with a fraction. A whole number written without a point is read as one too, so a
/// hand-written `order = 2` is not thrown away.
fn float(table: &DeTable<'_>, key: &str) -> Option<f64> {
    match entry(table, key)?.get_ref() {
        DeValue::Float(number) => number.as_str().replace('_', "").parse().ok(),
        DeValue::Integer(number) => number.as_str().replace('_', "").parse().ok(),
        _ => None,
    }
}

/// A whole number.
fn integer(table: &DeTable<'_>, key: &str) -> Option<i64> {
    match entry(table, key)?.get_ref() {
        DeValue::Integer(number) => number.as_str().replace('_', "").parse().ok(),
        _ => None,
    }
}

/// The identifier of an entry, or `None` when it cannot be used.
fn read_id(
    entry_table: &DeTable<'_>,
    seen: &mut HashSet<Id>,
    file: &str,
    text: &str,
    kind: &str,
    index: usize,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Id> {
    // The value's own position: searching the text for what it says would find the same letters
    // inside a comment or a longer word.
    let location = entry(entry_table, "id").map(|value| Location::from_offset(file, text, value.span().start));
    let written = string(entry_table, "id").unwrap_or_default();
    let Some(id) = Id::parse(&written) else {
        diagnostics.push(Diagnostic::error(location, format!("{kind} {} has no readable id", index + 1)));
        return None;
    };
    if !seen.insert(id) {
        diagnostics.push(Diagnostic::error(location, format!("{id} is used twice")));
        return None;
    }
    Some(id)
}

/// The goal of an entry, when it has one that can be read.
fn read_goal(entry_table: &DeTable<'_>) -> Option<Goal> {
    let goal = table(entry(entry_table, "goal")?)?;
    let amount = u32::try_from(integer(goal, "amount")?).ok()?;
    let period = Period::parse(&string(goal, "period")?)?;
    Some(Goal { amount, period })
}

/// Writes `key = "text"`, hiding anything that would end the string.
fn write_text(out: &mut String, key: &str, text: &str) {
    out.push_str(key);
    out.push_str(" = \"");
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", other as u32)),
            other => out.push(other),
        }
    }
    out.push_str("\"\n");
}

/// Writes `key = 1.0`. The debug form always carries a point, which TOML needs for a float.
fn write_number(out: &mut String, key: &str, value: f64) {
    out.push_str(&format!("{key} = {value:?}\n"));
}

/// Writes a goal as one line.
fn write_goal(out: &mut String, goal: Goal) {
    out.push_str(&format!("goal = {{ amount = {}, period = \"{}\" }}\n", goal.amount, goal.period.as_str()));
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[[category]]
id = "00000000000000000000000001"
name = "Yazılım"
order = 1.0

[[category.focus]]
id = "00000000000000000000000002"
name = "Rust"

[[category.focus]]
id = "00000000000000000000000003"
name = "Gözden geçirme"
archived = true
goal = { amount = 21600, period = "week" }
"#;

    #[test]
    fn reads_categories_and_their_focuses() {
        let (tree, diagnostics) = Tree::read("tree.toml", SAMPLE);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(tree.categories.len(), 1);
        assert_eq!(tree.categories[0].name, "Yazılım");
        assert_eq!(tree.categories[0].focuses.len(), 2);
    }

    #[test]
    fn reads_a_goal_and_an_archive_flag() {
        let (tree, _) = Tree::read("tree.toml", SAMPLE);
        let review = &tree.categories[0].focuses[1];
        assert!(review.archived);
        assert_eq!(review.goal, Some(Goal { amount: 21_600, period: Period::Week }));
    }

    #[test]
    fn writes_what_it_read() {
        let (tree, _) = Tree::read("tree.toml", SAMPLE);
        let (again, diagnostics) = Tree::read("tree.toml", &tree.write());
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(again.categories, tree.categories);
    }

    #[test]
    fn a_broken_entry_is_skipped_and_the_rest_is_kept() {
        let text = format!("{SAMPLE}\n[[category]]\nid = \"nope\"\nname = \"Bozuk\"\n");
        let (tree, diagnostics) = Tree::read("tree.toml", &text);
        assert_eq!(tree.categories.len(), 1, "sağlam kategori kalmalı");
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn broken_toml_never_empties_the_file() {
        let (tree, diagnostics) = Tree::read("tree.toml", "[[category]\nbozuk");
        assert!(tree.categories.is_empty());
        // Kaç tanı üretileceği ayrıştırıcının işi; bizim sözümüz hiçbirinin yutulmaması ve
        // her birinin bir yeri göstermesi.
        assert!(!diagnostics.is_empty());
        assert!(diagnostics.iter().all(|found| found.location.is_some()), "{diagnostics:?}");
    }

    #[test]
    fn a_focus_can_be_found_by_its_identifier() {
        let (tree, _) = Tree::read("tree.toml", SAMPLE);
        let wanted = Id::parse("00000000000000000000000002").expect("kimlik");
        assert_eq!(tree.focus(wanted).map(|focus| focus.name.as_str()), Some("Rust"));
    }

    #[test]
    fn a_duplicate_identifier_is_reported() {
        let text = SAMPLE.replace("00000000000000000000000003", "00000000000000000000000002");
        let (_, diagnostics) = Tree::read("tree.toml", &text);
        assert_eq!(diagnostics.len(), 1);
    }
}
