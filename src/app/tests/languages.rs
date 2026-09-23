//! Every language on the same screens: what the words do to a narrow terminal.
//!
//! A translation that reads well in a document can still push a column off the edge, so the
//! tour walks each locale and the checks here are the ones that catch that.

use super::*;

/// Every language at forty columns: no key without text, no mark the aesthetics forbid, no
/// wide character broken, and no text cut short. One cut is let through by name: a person's
/// own focus or category name in a table column, which cannot wrap (see `may_be_cut`).
///
/// `QFOCUS_TOUR=<code>` prints that language's screens, for reading a translation where it
/// is shown.
#[test]
fn every_language_reads_whole_at_forty_columns() {
    let shown = std::env::var("QFOCUS_TOUR").ok();
    for &(file, _) in crate::locales() {
        let code = file.trim_end_matches(".toml");
        tour(code, code, Look::NARROW, &mut |name, h| {
            let screen = h.screen();
            if shown.as_deref() == Some(code) {
                println!("=== {code} · {name}\n{screen}");
            }
            assert!(!screen.contains('⟦'), "{code} · {name}: a key has no text\n{screen}");
            assert_eq!(forbidden(&screen), None, "{code} · {name}\n{screen}");
            assert_eq!(broken_wide_cell(h), None, "{code} · {name}\n{screen}");
            for cut in cut_texts(&screen) {
                assert!(may_be_cut(&cut, h), "{code} · {name}: \"{cut}…\" is cut short\n{screen}");
            }
        });
    }
}

/// The text before every `…` on `screen`: what is left of each text cut short, back to the
/// gap that opens its cell.
fn cut_texts(screen: &str) -> Vec<String> {
    screen
        .lines()
        .flat_map(|line| line.match_indices('…').map(move |(at, _)| &line[..at]))
        .map(|before| {
            let start = before.rfind("  ").map_or(0, |gap| gap + 2);
            before[start..].trim_start_matches(['❯', ' ']).to_owned()
        })
        .collect()
}

/// Whether the text left as `cut` may stand cut short: a person's own name in a table
/// column, which cannot wrap.
fn may_be_cut(cut: &str, h: &Harness<QFocus>) -> bool {
    let tree = &h.app().store().tree;
    let names = tree.categories.iter().flat_map(|c| std::iter::once(&c.name).chain(c.focuses.iter().map(|f| &f.name)));
    let own_name = names.into_iter().any(|full| full.len() > cut.len() && full.starts_with(cut));
    own_name && !cut.is_empty()
}
