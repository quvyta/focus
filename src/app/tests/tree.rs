//! The list of focuses: adding, moving, archiving, and what an empty store invites.

use super::*;

#[test]
fn the_keyboard_is_on_the_list_of_focuses_from_the_first_frame() {
    let dir = temp("first-focus");
    seeded(&dir);
    let clock = FakeClock::new();
    let h = harness(app_at(&dir, &clock), 80, 24);
    assert!(h.is_focused(today::TREE), "the arrows reach the list without pressing 1 first");
    done(&dir);
}

#[test]
fn an_empty_store_shows_the_invitation() {
    let dir = temp("empty");
    let clock = FakeClock::new();
    let h = harness(app_at(&dir, &clock), 80, 20);
    let screen = h.screen();
    assert!(screen.contains("Nothing to focus on yet"), "{screen}");
    assert!(screen.contains("Friday, 18 September"), "{screen}");
    assert!(screen.contains("0 s"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    done(&dir);
}

#[test]
fn adding_a_category_then_a_focus_selects_the_new_row() {
    let dir = temp("add");
    let clock = FakeClock::new();
    let mut h = harness(app_at(&dir, &clock), 80, 20);
    h.click_text("Add a category");
    assert!(h.is_focused(today::NAME_INPUT), "{}", h.screen());
    h.press("enter");
    assert!(h.screen().contains("A name cannot be empty"), "{}", h.screen());
    h.type_text("Work").press("enter");
    let category = h.app().store().tree.categories[0].id;
    assert_eq!(h.app().today().selected(), Some(Row::Category(category)));
    assert!(h.screen().contains("Add a focus"), "{}", h.screen());
    h.click_text("Add a focus");
    h.type_text("Rust").press("enter");
    let focus = h.app().store().tree.categories[0].focuses[0].id;
    assert_eq!(h.app().today().selected(), Some(Row::Focus(focus)));
    assert!(fs::read_to_string(h.app().store().paths.tree_file()).is_ok_and(|text| text.contains("Rust")));
    // A second focus with the same name is allowed and mentioned.
    h.press("down").press("enter").type_text("Rust").press("enter");
    assert!(h.screen().contains("already one called Rust"), "{}", h.screen());
    assert_eq!(h.app().store().tree.categories[0].focuses.len(), 2);
    done(&dir);
}

/// Work with Rust and Review side by side, and Life with Reading: three focuses to move.
fn seeded_three(dir: &Path) -> (Id, Id, Id) {
    let (rust, reading, review) = seeded_two(dir);
    let store = Store::open(Paths::at(dir, "test"));
    let mut tree = store.tree.clone();
    tree.categories[0].focuses[1].archived = false;
    fs::write(store.paths.tree_file(), tree.write()).expect("tree written");
    (rust, review, reading)
}

#[test]
fn dragging_a_focus_writes_a_fractional_order_and_the_keys_move_it_back() {
    let dir = temp("reorder");
    let (rust, review, _) = seeded_three(&dir);
    let clock = FakeClock::new();
    let mut h = harness(app_at(&dir, &clock), 80, 20);
    let (x, from) = h.find("Rust").expect("Rust row");
    let (_, to) = h.find("Review").expect("Review row");
    assert!(from < to, "{}", h.screen());
    h.drag((x, from), (x, to));
    let tree = &h.app().store().tree;
    let order_of = |id: Id| tree.focus(id).map(|focus| focus.order).unwrap_or_default();
    assert!(order_of(rust) > order_of(review), "{}", h.screen());
    assert_eq!(order_of(rust), 2.0, "past the last one: one more than its order");
    let (_, rust_row) = h.find("Rust").expect("Rust row");
    assert_eq!(rust_row, to, "{}", h.screen());
    let saved = fs::read_to_string(h.app().store().paths.tree_file()).expect("tree");
    assert!(saved.contains("order = 2.0"), "{saved}");
    assert_eq!(h.app().today().selected(), Some(Row::Focus(rust)));
    // The keys move the selected row one place among its siblings.
    h.press("ctrl+shift+up");
    let tree = &h.app().store().tree;
    let rust_order = tree.focus(rust).map(|focus| focus.order).unwrap_or_default();
    let review_order = tree.focus(review).map(|focus| focus.order).unwrap_or_default();
    assert!(rust_order < review_order, "{}", h.screen());
    let (_, rust_row) = h.find("Rust").expect("Rust row");
    assert_eq!(rust_row, from, "{}", h.screen());
    done(&dir);
}

#[test]
fn the_context_menu_moves_a_focus_to_another_category_and_archives_one() {
    let dir = temp("menu");
    let (rust, _, _) = seeded_three(&dir);
    let clock = FakeClock::new();
    let mut h = harness(app_at(&dir, &clock), 80, 24);
    let (x, y) = h.find("Rust").expect("Rust row");
    h.mouse(MouseKind::Down(MouseButton::Right), x, y).mouse(MouseKind::Up(MouseButton::Right), x, y);
    let screen = h.screen();
    assert!(screen.contains("Move to"), "{screen}");
    assert!(screen.contains("Archive"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    let (mx, my) = h.find("Move to").expect("move entry");
    h.hover(mx, my);
    assert!(h.screen().contains("Life"), "{}", h.screen());
    h.click_text("Life");
    assert!(h.screen().contains("Rust moved to Life"), "{}", h.screen());
    let tree = &h.app().store().tree;
    assert!(tree.categories[0].focuses.iter().all(|focus| focus.id != rust));
    assert_eq!(tree.categories[1].focuses.last().map(|focus| focus.id), Some(rust));
    let saved = fs::read_to_string(h.app().store().paths.tree_file()).expect("tree");
    let life_at = saved.find("Life").expect("Life");
    let rust_at = saved.find("Rust").expect("Rust");
    assert!(rust_at > life_at, "{saved}");
    assert_eq!(h.app().today().selected(), Some(Row::Focus(rust)));
    // The menu key opens the menu of the selected row; its last entry archives it.
    h.press("shift+f10");
    assert!(h.screen().contains("Rename"), "{}", h.screen());
    h.click_text("Archive");
    assert!(h.screen().contains("Rust archived"), "{}", h.screen());
    assert!(h.app().store().tree.focus(rust).is_some_and(|focus| focus.archived));
    done(&dir);
}

#[test]
fn the_menu_on_a_category_adds_a_focus_and_reads_clean_in_ascii_and_turkish_at_forty_columns() {
    let dir = temp("menu-ascii");
    seeded_three(&dir);
    let clock = FakeClock::new();
    let mut h = harness(app_at(&dir, &clock), 40, 20);
    h.set_glyph_mode(GlyphMode::Ascii).set_locale("tr");
    let (x, y) = h.find("Work").expect("Work row");
    h.mouse(MouseKind::Down(MouseButton::Right), x, y);
    let screen = h.screen();
    assert!(screen.contains("Odak ekle"), "{screen}");
    assert!(screen.contains("Arşivle"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.click_text("Odak ekle");
    assert!(h.app().today().edit().is_some(), "{}", h.screen());
    h.type_text("Go").press("enter");
    assert_eq!(h.app().store().tree.categories[0].focuses.len(), 3);
    assert_eq!(forbidden(&h.screen()), None, "{}", h.screen());
    done(&dir);
}
