//! One sweep that drives every screen of the tour through the conditions a terminal may set:
//! ASCII glyphs at forty columns and at a wide size, and sixteen colours.
//!
//! The narrow terminal in Unicode glyphs is the languages test's (`languages.rs`); this sweep
//! adds what that one does not look at. Every check gathers findings instead of stopping at the
//! first, so a run names every screen and condition that misses at once. Findings that are
//! known and not fixed here sit in [`EXCEPTIONS`], each saying whether it is a defect or an
//! honest limit and why; a finding that no longer happens fails the sweep as well, so the list
//! shrinks and never rots. [`EXCEPTION_COUNT`] fixes its length, so another line cannot slip in
//! unnoticed.

use std::collections::BTreeSet;

use qframe::color::Rgb;
use toml::de::{DeTable, DeValue};

use super::*;

/// The wide size the sweep draws at besides forty columns.
const WIDE: u16 = 120;

/// The checks, named as they appear in a failure.
mod check {
    /// A character outside ASCII drawn in ASCII glyph mode: in English any at all, in the other
    /// languages a glyph, mark, box or cut, since their words are text and not glyphs.
    pub const NON_ASCII: &str = "ascii-non-ascii";
    /// A shape the aesthetics constitution forbids in every mode.
    pub const SHAPE: &str = "ascii-shape";
    /// Text that stops reading against what is behind it once colours are reduced to sixteen.
    pub const CONTRAST: &str = "colour-contrast";
    /// Background tones that all collapse into one colour once reduced to sixteen.
    pub const FLAT: &str = "colour-flat";
}

/// What an accepted finding is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A real bug in a framework component qfocus draws: it belongs on the framework's work
    /// list, and is not worked around here.
    Defect,
    /// An honest limit: the condition cannot hold the shape.
    Limit,
}

/// A finding the sweep tolerates for now.
struct Exception {
    /// The screen of the tour it belongs to, or `*` for every screen.
    screen: &'static str,
    check: &'static str,
    /// Text the finding must contain, so an exception covers one thing and not a whole check.
    detail: &'static str,
    kind: Kind,
    /// Why it is tolerated, in one line.
    reason: &'static str,
}

/// Findings that are known. Every defect here is a framework component's and is filed with the
/// framework rather than worked around.
const EXCEPTIONS: &[Exception] = &[
    // Defects: a framework component draws a mark ASCII does not have.
    Exception {
        screen: "records 0",
        check: check::NON_ASCII,
        detail: "draws `…`",
        kind: Kind::Defect,
        reason: "the table cuts a name that does not fit with `…` in ASCII mode too; the framework's cut has one mark for every mode",
    },
    // Defects: in sixteen colours the page behind a dialog disappears into its own ground.
    Exception {
        screen: "quit dialog",
        check: check::CONTRAST,
        detail: "keeps only 1.00:1",
        kind: Kind::Defect,
        reason: "the framework dims the page behind a dialog to a tone that reduces to the same colour as the ground",
    },
    Exception {
        screen: "spans",
        check: check::CONTRAST,
        detail: "keeps only 1.00:1",
        kind: Kind::Defect,
        reason: "the framework dims the page behind a dialog to a tone that reduces to the same colour as the ground",
    },
    Exception {
        screen: "record form",
        check: check::CONTRAST,
        detail: "keeps only 1.00:1",
        kind: Kind::Defect,
        reason: "the framework dims the page behind a dialog to a tone that reduces to the same colour as the ground",
    },
    Exception {
        screen: "purge dialog",
        check: check::CONTRAST,
        detail: "keeps only 1.00:1",
        kind: Kind::Defect,
        reason: "the framework dims the page behind a dialog to a tone that reduces to the same colour as the ground",
    },
    // Defect: the theme's surfaces all reduce to black.
    Exception {
        screen: "today, empty",
        check: check::FLAT,
        detail: "4 background tones",
        kind: Kind::Defect,
        reason: "the ground, the tab bar and the lifted tab all reduce to black, so a screen with no selection has no shape left",
    },
    // Limits: the glyphs are what the step shows.
    Exception {
        screen: "wizard, appearance",
        check: check::NON_ASCII,
        detail: "in `Nerd Font",
        kind: Kind::Limit,
        reason: "the appearance step shows its icons in all three glyph modes so the person chooses by eye; the Nerd Font row is a sample",
    },
    Exception {
        screen: "wizard, appearance",
        check: check::NON_ASCII,
        detail: "in `Unicode ",
        kind: Kind::Limit,
        reason: "the appearance step shows its icons in all three glyph modes so the person chooses by eye; the Unicode row is a sample",
    },
];

/// How many exceptions the list is allowed to hold. Adding one means changing this number, which
/// makes it a decision rather than an accident.
const EXCEPTION_COUNT: usize = 8;

/// One missed promise.
struct Finding {
    screen: String,
    check: &'static str,
    /// The render it was seen in, such as `ja, ascii at 40 columns`.
    when: String,
    detail: String,
}

/// Collects findings, keeping the first one per screen, check, render and thing found.
#[derive(Default)]
struct Findings {
    seen: BTreeSet<(String, &'static str, String, String)>,
    items: Vec<Finding>,
}

impl Findings {
    /// Adds a finding unless the same `what` (the character, the shape) was already found on this
    /// screen in this render: one per thing, so an excepted glyph never hides another one.
    fn add(&mut self, screen: &str, check: &'static str, when: &str, what: &str, detail: String) {
        if self.seen.insert((screen.to_owned(), check, when.to_owned(), what.to_owned())) {
            self.items.push(Finding { screen: screen.to_owned(), check, when: when.to_owned(), detail });
        }
    }
}

impl Exception {
    fn covers(&self, finding: &Finding) -> bool {
        (self.screen == "*" || self.screen == finding.screen)
            && self.check == finding.check
            && finding.detail.contains(self.detail)
    }
}

/// Shapes the aesthetics constitution forbids in every mode, ASCII included: `[ ] ( ) { }`
/// wrapping, `|` separating, and the drawn arrows and rules.
fn forbidden_shape(line: &str) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    for (index, c) in chars.iter().enumerate() {
        if "[](){}".contains(*c) {
            return Some(format!("`{c}` wraps text"));
        }
        if *c == '|' {
            let before = index.checked_sub(1).and_then(|i| chars.get(i)).copied().unwrap_or(' ');
            let after = chars.get(index + 1).copied().unwrap_or(' ');
            if !before.is_alphanumeric() && !after.is_alphanumeric() {
                return Some("`|` separates".to_owned());
            }
        }
    }
    ["===", "-->", "<--", "+--", "--+"].iter().find(|d| line.contains(**d)).map(|d| format!("`{d}` decorates"))
}

/// Whether `c` is a drawn glyph rather than a letter of a language: marks, cuts, arrows,
/// boxes, blocks, shapes, braille, dingbats and the icon font's private characters.
fn is_drawn_glyph(c: char) -> bool {
    matches!(u32::from(c),
        0x00B7 // · middle dot
        | 0x2022 // • bullet
        | 0x2026 // … the cut
        | 0x2190..=0x21FF // arrows
        | 0x2300..=0x23FF // technical marks
        | 0x2500..=0x27BF // boxes, blocks, shapes, symbols, dingbats
        | 0x27F0..=0x27FF // supplemental arrows
        | 0x2800..=0x28FF // braille
        | 0x2900..=0x297F // more arrows
        | 0x2B00..=0x2BFF // symbols and arrows
        | 0xE000..=0xF8FF // private use: the icon font
        | 0xF0000..)
}

/// Every value a language file writes, one after another: the words themselves, without the
/// keys and comments around them.
fn values(text: &str) -> String {
    fn gather(value: &DeValue<'_>, out: &mut String) {
        match value {
            DeValue::String(text) => {
                out.push_str(text);
                out.push('\n');
            }
            DeValue::Table(table) => table.values().for_each(|value| gather(value.get_ref(), out)),
            DeValue::Array(items) => items.iter().for_each(|value| gather(value.get_ref(), out)),
            _ => {}
        }
    }
    let root = DeTable::parse(text).expect("a language file parses");
    let mut out = String::new();
    root.get_ref().values().for_each(|value| gather(value.get_ref(), &mut out));
    out
}

/// The non-ASCII characters `code`'s own file writes, which are words of the language wherever
/// they are drawn: a guillemet, a full-width colon or an ideographic comma is text, not a glyph.
fn written_by(code: &str) -> BTreeSet<char> {
    crate::locales()
        .iter()
        .find(|(file, _)| file.trim_end_matches(".toml") == code)
        .map(|(_, text)| values(text).chars().filter(|c| !c.is_ascii()).collect())
        .unwrap_or_default()
}

/// ASCII glyph mode in every language at forty columns, and in English at the wide size too.
///
/// English is held to the whole promise: nothing outside ASCII at all. The other languages write
/// their words in their own letters, so what is held against them is a drawn glyph (see
/// [`is_drawn_glyph`]), or a mark outside ASCII their own file never writes.
fn sweep_ascii(found: &mut Findings) {
    for &(file, _) in crate::locales() {
        let code = file.trim_end_matches(".toml");
        let own = written_by(code);
        let widths: &[u16] = if code == "en" { &[40, WIDE] } else { &[40] };
        for &width in widths {
            let when = format!("{code}, ascii at {width} columns");
            let look = Look { width, glyphs: GlyphMode::Ascii };
            tour(code, &format!("sweep-{code}-{width}"), look, &mut |name, h| {
                for (row, line) in h.screen().lines().enumerate() {
                    let held =
                        |c: char| code == "en" || is_drawn_glyph(c) || (!c.is_alphanumeric() && !own.contains(&c));
                    for bad in line.chars().filter(|c| !c.is_ascii() && held(*c)) {
                        let detail = format!("row {row} draws `{bad}` in `{}`", line.trim());
                        found.add(name, check::NON_ASCII, &when, &bad.to_string(), detail);
                    }
                    if let Some(shape) = forbidden_shape(line) {
                        found.add(name, check::SHAPE, &when, &shape, format!("row {row} {shape} in `{}`", line.trim()));
                    }
                }
            });
        }
    }
}

/// The sixteen standard terminal colours, the xterm defaults [`Rgb::to_ansi16`] snaps a colour
/// to. A real terminal may be themed differently; these are what a reduction can count on.
const ANSI16: [Rgb; 16] = [
    Rgb::new(0, 0, 0),
    Rgb::new(205, 0, 0),
    Rgb::new(0, 205, 0),
    Rgb::new(205, 205, 0),
    Rgb::new(0, 0, 238),
    Rgb::new(205, 0, 205),
    Rgb::new(0, 205, 205),
    Rgb::new(229, 229, 229),
    Rgb::new(127, 127, 127),
    Rgb::new(255, 0, 0),
    Rgb::new(0, 255, 0),
    Rgb::new(255, 255, 0),
    Rgb::new(92, 92, 255),
    Rgb::new(255, 0, 255),
    Rgb::new(0, 255, 255),
    Rgb::new(255, 255, 255),
];

/// The colour a sixteen-colour terminal shows in place of `color`.
fn reduced(color: Rgb) -> Rgb {
    ANSI16[usize::from(color.to_ansi16())]
}

/// The contrast text must keep against its background after the reduction: the framework's
/// sweep's bar, "still a different colour to the eye", since no reduction to sixteen colours
/// survives the 4.5 the theme asks of true-colour body text.
const MIN_CONTRAST: f64 = 1.6;

/// Whether a glyph is text rather than a fill. Blocks, shades and rails carry their meaning in
/// their colour and say nothing about readability.
fn is_text(glyph: char) -> bool {
    glyph.is_alphanumeric() || (glyph.is_ascii_punctuation() && glyph != '_')
}

/// Sixteen colours, in English, at forty columns and at the wide size: text keeps reading, and
/// the tones a screen leans on stay apart.
fn sweep_sixteen_colours(found: &mut Findings) {
    for width in [40, WIDE] {
        let when = format!("sixteen colours at {width} columns");
        let look = Look { width, glyphs: GlyphMode::Unicode };
        tour("en", &format!("sweep-colour-{width}"), look, &mut |name, h| {
            let area = h.buffer().area;
            let mut grounds: BTreeSet<[u8; 3]> = BTreeSet::new();
            let mut reduced_grounds: BTreeSet<u8> = BTreeSet::new();
            let mut worst: Option<(f64, char, u16, u16)> = None;
            for y in 0..area.height {
                for x in 0..area.width {
                    let Some(bg) = h.bg(x, y) else { continue };
                    grounds.insert([bg.r, bg.g, bg.b]);
                    reduced_grounds.insert(bg.to_ansi16());
                    let glyph = h.buffer()[(x, y)].symbol().chars().next().filter(|c| is_text(*c));
                    let (Some(glyph), Some(fg)) = (glyph, h.fg(x, y)) else { continue };
                    let ratio = reduced(fg).contrast_ratio(reduced(bg));
                    if worst.is_none_or(|(low, ..)| ratio < low) {
                        worst = Some((ratio, glyph, x, y));
                    }
                }
            }
            if let Some((ratio, glyph, x, y)) = worst.filter(|(ratio, ..)| *ratio < MIN_CONTRAST) {
                let row = h.screen().lines().nth(usize::from(y)).unwrap_or_default().trim().to_owned();
                found.add(
                    name,
                    check::CONTRAST,
                    &when,
                    "",
                    format!("`{glyph}` at {x},{y} keeps only {ratio:.2}:1 in `{row}`"),
                );
            }
            if grounds.len() >= 3 && reduced_grounds.len() < 2 {
                found.add(
                    name,
                    check::FLAT,
                    &when,
                    "",
                    format!("{} background tones collapse into one of the sixteen", grounds.len()),
                );
            }
        });
    }
}

/// Every screen of the tour in ASCII glyphs, at forty columns and wide, and in sixteen colours.
/// What is not kept yet is in [`EXCEPTIONS`] with its reason.
#[test]
fn every_screen_keeps_to_ascii_and_sixteen_colours() {
    assert_eq!(EXCEPTIONS.len(), EXCEPTION_COUNT, "the exception list changed length; say so on purpose");

    let mut found = Findings::default();
    sweep_ascii(&mut found);
    sweep_sixteen_colours(&mut found);

    let mut report = String::new();
    for finding in &found.items {
        if !EXCEPTIONS.iter().any(|allowed| allowed.covers(finding)) {
            report.push_str(&format!(
                "\n  {} · {} · {}: {}",
                finding.screen, finding.check, finding.when, finding.detail
            ));
        }
    }
    for allowed in EXCEPTIONS {
        if !found.items.iter().any(|finding| allowed.covers(finding)) {
            let kind = if allowed.kind == Kind::Defect { "defect" } else { "limit" };
            report.push_str(&format!(
                "\n  {} · {}: the {kind} `{}` is gone, so drop the exception ({})",
                allowed.screen, allowed.check, allowed.detail, allowed.reason
            ));
        }
    }
    assert!(report.is_empty(), "a screen misses ASCII or sixteen colours:{report}\n");
}

/// Every value of every language file can be drawn by the fonts a picture of qfocus is made
/// with, so a README picture can be taken in any of the nine languages, not only the ones that
/// happen to have been drawn.
#[test]
fn every_language_file_can_be_drawn() {
    for &(file, text) in crate::locales() {
        let missing = qshots::missing_in(&values(text));
        assert!(missing.is_empty(), "{file} writes characters the fonts cannot draw: {missing:?}");
    }
}
