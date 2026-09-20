//! The records page and the form behind it: listing, searching, deleting, correcting, exporting.

use super::*;

use crate::ui::records;
use crate::ui::records::clock_of;

fn records_at(dir: &Path, clock: &FakeClock, width: u16, height: u16) -> Harness<QFocus> {
    let mut h = harness(app_at(dir, clock), width, height);
    h.press("3");
    assert_eq!(h.app().page(), Page::Records);
    h
}

#[test]
fn without_a_data_folder_records_stay_in_memory() {
    let dir = temp("memory");
    let clock = FakeClock::new();
    let store = Store::open_read_only(Paths::at(&dir, "test"));
    // The settings live apart from the records here: the data folder must stay untouched, and
    // the appearance writes its own files where it is told to.
    let settings_dir = temp("memory-settings");
    let app = QFocus::new(store, false, None, clock.reader(), Settings::in_memory(), appearance(&settings_dir));
    let mut h = harness(app, 80, 20);
    let screen = h.screen();
    assert!(screen.contains("stays in memory"), "{screen}");
    assert!(screen.contains("time zone is unknown"), "{screen}");
    h.click_text("Add a category").type_text("Work").press("enter");
    h.click_text("Add a focus").type_text("Rust").press("enter");
    h.press("enter");
    assert!(h.app().timer().is_some(), "{}", h.screen());
    clock.pass(60);
    h.advance(Duration::from_secs(1));
    h.press("space");
    assert_eq!(h.app().pending().len(), 1);
    assert!(h.screen().contains("1 min"), "{}", h.screen());
    assert!(!dir.exists());
    done(&settings_dir);
}

#[test]
fn the_records_tab_lists_sessions_newest_first_and_lays_out_the_spans() {
    let dir = temp("records");
    let (_, focus) = seeded(&dir);
    two_sessions(&dir, focus);
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 100, 20);
    let screen = h.screen();
    assert!(screen.contains("Work › Rust"), "{screen}");
    assert!(screen.contains("meeting notes"), "{screen}");
    assert!(screen.contains("50 min ◆"), "{screen}");
    let (_, ten) = h.find("10 min").expect("the short session");
    let (_, fifty) = h.find("50 min").expect("the long session");
    assert!(ten < fifty, "the newest session comes first:\n{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    // Down selects the first row; the second row explains its mark and Enter lays it out.
    h.press("down").press("down");
    assert_eq!(h.app().records().selected(), Some(1));
    assert!(h.screen().contains("1 h span, 50 min work"), "{}", h.screen());
    h.press("s");
    let screen = h.screen();
    assert!(h.app().records().is_dumping());
    assert!(screen.contains("Rust · 2026-09-18"), "{screen}");
    assert!(screen.contains("break"), "{screen}");
    assert!(screen.contains("12:30:00"), "{screen}");
    assert!(screen.contains("monotonic"), "{screen}");
    // The session's own strip runs from its start to its end, with the hours between.
    assert!(screen.contains("12:15") && screen.contains("12:45"), "{screen}");
    let (_, work) = h.find("work").expect("the legend names the kinds");
    let (_, table) = h.find("Kind").expect("the table follows");
    assert!(work < table, "the legend stands over the table:\n{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    assert!(h.is_focused(records::DUMP_STRIP), "the strip takes the keys when the dialog opens:\n{screen}");
    h.press("right").press("right");
    assert_eq!(h.app().records().selected(), Some(1), "the table's row is untouched");
    assert!(h.screen().contains("break  12:30–12:40  10 min"), "{}", h.screen());
    h.press("esc");
    assert!(!h.app().records().is_dumping());
    h.press("1");
    assert_eq!(h.app().page(), Page::Today);
    assert!(h.screen().contains("Add a category"), "{}", h.screen());
    done(&dir);
}

#[test]
fn an_empty_records_tab_says_so() {
    let dir = temp("records-empty");
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 80, 20);
    let screen = h.screen();
    assert!(screen.contains("No records yet"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.click_text("Trash");
    assert!(h.screen().contains("The trash is empty"), "{}", h.screen());
    h.click_text("Archive");
    assert!(h.screen().contains("Nothing is archived"), "{}", h.screen());
    h.click_text("Problems");
    assert!(h.screen().contains("No problems"), "{}", h.screen());
    done(&dir);
}

#[test]
fn search_filters_by_note_and_focus_and_the_filter_outlives_the_field() {
    let dir = temp("records-search");
    let (_, focus) = seeded(&dir);
    two_sessions(&dir, focus);
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 100, 20);
    h.press("/");
    assert!(h.is_focused(records::SEARCH_INPUT), "{}", h.screen());
    h.type_text("MEETING").press("enter");
    assert!(!h.app().records().is_searching());
    let screen = h.screen();
    assert!(screen.contains("Filtered by MEETING"), "{screen}");
    assert!(screen.contains("50 min"), "{screen}");
    assert!(!screen.contains("10 min"), "{screen}");
    h.press("/").type_text("x").press("esc");
    assert!(h.screen().contains("No matching records"), "{}", h.screen());
    assert_eq!(h.app().records().query(), "MEETINGx");
    done(&dir);
}

#[test]
fn delete_moves_a_session_to_the_trash_and_ctrl_z_brings_it_back() {
    let dir = temp("records-delete");
    let (_, focus) = seeded(&dir);
    let (_, short) = two_sessions(&dir, focus);
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 100, 20);
    h.press("down").press("delete");
    assert!(h.screen().contains("Rust: session moved to the trash"), "{}", h.screen());
    assert_eq!(h.app().store().sessions.len(), 1);
    assert_eq!(h.app().store().voided.first().map(|s| s.id), Some(short.id));
    assert!(h.app().store().paths.trash_file(2026, 9).exists());
    h.press("ctrl+z");
    assert!(h.screen().contains("Rust: session is back"), "{}", h.screen());
    assert_eq!(h.app().store().sessions.len(), 2);
    assert!(h.app().store().voided.is_empty());
    h.press("ctrl+z");
    assert!(h.screen().contains("Nothing to undo"), "{}", h.screen());
    done(&dir);
}

#[test]
fn the_trash_view_brings_a_session_back_with_enter() {
    let dir = temp("records-trash");
    let (_, focus) = seeded(&dir);
    two_sessions(&dir, focus);
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 100, 20);
    h.press("down").press("delete");
    h.click_text("Trash");
    assert_eq!(h.app().records().view_kind(), records::ViewKind::Trash);
    let screen = h.screen();
    assert!(screen.contains("10 min"), "{screen}");
    assert!(!screen.contains("50 min ◆"), "the long session is not in the trash:\n{screen}");
    h.press("down").press("enter");
    assert!(h.app().store().voided.is_empty());
    assert!(h.screen().contains("The trash is empty"), "{}", h.screen());
    done(&dir);
}

#[test]
fn the_archive_view_brings_a_focus_back_and_saves_the_tree() {
    let dir = temp("records-archive");
    seeded(&dir);
    let clock = FakeClock::new();
    let mut h = harness(app_at(&dir, &clock), 80, 20);
    h.click_text("Rust");
    // A click starts the counter; stop it and archive the row instead.
    h.press("space");
    h.press("delete");
    assert!(h.screen().contains("Rust archived"), "{}", h.screen());
    h.press("3").click_text("Archive");
    let screen = h.screen();
    assert!(screen.contains("Rust"), "{screen}");
    assert!(screen.contains("Focus"), "{screen}");
    h.press("down").press("enter");
    assert!(h.screen().contains("Rust is back"), "{}", h.screen());
    assert!(!h.app().store().tree.categories[0].focuses[0].archived);
    let tree = fs::read_to_string(h.app().store().paths.tree_file()).expect("tree");
    assert!(!tree.contains("archived"), "{tree}");
    // The toasts of the start, the stop, the archive and the return stand over the page.
    h.advance(Duration::from_secs(10));
    assert!(h.screen().contains("Nothing is archived"), "{}", h.screen());
    done(&dir);
}

#[test]
fn the_problems_view_lists_what_could_not_be_read() {
    let dir = temp("records-problems");
    let (_, focus) = seeded(&dir);
    two_sessions(&dir, focus);
    let month = Paths::at(&dir, "test").month_file(2026, 9);
    let mut text = fs::read_to_string(&month).expect("month");
    text.push_str("qf1;not a record\n");
    fs::write(&month, text).expect("month");
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 100, 20);
    assert!(h.screen().contains("1 problem was found"), "{}", h.screen());
    h.click_text("Problems");
    let screen = h.screen();
    assert!(screen.contains("2026-09.log:3:1"), "{screen}");
    assert!(screen.contains("fields"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    done(&dir);
}

#[test]
fn a_narrow_records_page_drops_columns_and_stays_clean() {
    let dir = temp("records-narrow");
    let (_, focus) = seeded(&dir);
    two_sessions(&dir, focus);
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 40, 20);
    let screen = h.screen();
    assert!(screen.contains("Date"), "{screen}");
    assert!(screen.contains("Work"), "{screen}");
    assert!(!screen.contains("Note"), "{screen}");
    assert!(!screen.contains("Source"), "{screen}");
    assert!(!screen.contains("End"), "{screen}");
    assert!(screen.contains("50 min ◆"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.resize(100, 20);
    let screen = h.screen();
    assert!(screen.contains("Note"), "{screen}");
    assert!(screen.contains("Source"), "{screen}");
    done(&dir);
}

#[test]
fn ascii_and_turkish_records_keep_clean() {
    let dir = temp("records-ascii");
    let (_, focus) = seeded(&dir);
    two_sessions(&dir, focus);
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 100, 20);
    h.set_glyph_mode(GlyphMode::Ascii).set_locale("tr");
    let screen = h.screen();
    assert!(screen.contains("Kayıtlar"), "{screen}");
    assert!(screen.contains("Hepsi"), "{screen}");
    assert!(screen.contains("Work > Rust"), "{screen}");
    assert!(screen.contains("50 dk *"), "{screen}");
    assert!(!screen.contains('◆'), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.press("down").press("down").press("s");
    let screen = h.screen();
    assert!(screen.contains("mola"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.press("esc").press("enter");
    let screen = h.screen();
    assert!(screen.contains("Kaydı düzelt"), "{screen}");
    assert!(screen.contains("Work > Rust"), "{screen}");
    assert!(screen.contains("Bitiş 12:50"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.press("esc");
    assert!(h.app().form().is_none());
    h.press("a");
    let screen = h.screen();
    assert!(screen.contains("Elle kayıt ekle"), "{screen}");
    assert!(screen.contains("Bir odak seç"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.click_text("Kaydet");
    let screen = h.screen();
    assert!(screen.contains("Oturum için bir odak seç."), "{screen}");
    assert!(screen.contains("Süre sıfır olamaz."), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    done(&dir);
}

#[test]
fn a_session_is_added_by_hand_through_the_form() {
    let dir = temp("records-add");
    let (_, focus) = seeded(&dir);
    two_sessions(&dir, focus);
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 100, 24);
    h.press("a");
    let screen = h.screen();
    assert!(h.app().form().is_some(), "{screen}");
    assert!(screen.contains("Add a session by hand"), "{screen}");
    assert!(h.is_focused(record_form::FOCUS_SELECT), "{screen}");
    // Saving with nothing typed refuses and writes the reasons beside the fields.
    h.click_text("Save");
    let screen = h.screen();
    assert!(h.app().form().is_some(), "{screen}");
    assert!(screen.contains("Pick a focus for the session."), "{screen}");
    assert!(screen.contains("The duration cannot be zero."), "{screen}");
    assert!(!screen.contains("in the future"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    assert_eq!(h.app().store().sessions.len(), 2, "nothing was written");
    // The focus is picked from the list, the start typed, the duration typed as minutes.
    h.press("enter").press("enter");
    assert!(h.screen().contains("Work › Rust"), "{}", h.screen());
    h.press("tab").press("tab").type_text("1000");
    h.press("tab").type_text("0045");
    let screen = h.screen();
    assert!(screen.contains("Ends at 10:45"), "{screen}");
    assert!(!screen.contains("cannot be zero"), "the reason goes once the field changes:\n{screen}");
    h.press("tab").type_text("typed in").press("enter");
    assert!(h.app().form().is_none(), "{}", h.screen());
    assert!(h.screen().contains("Rust: 45 min added by hand"), "{}", h.screen());
    assert!(h.is_focused(records::TABLE), "{}", h.screen());
    let sessions = &h.app().store().sessions;
    assert_eq!(sessions.len(), 3);
    let added = sessions.iter().find(|session| session.note == "typed in").expect("the new session");
    assert_eq!(added.source, Source::Manual);
    assert_eq!(added.work_seconds(), 2_700);
    assert_eq!(added.spans, vec![Span::new(SpanKind::Work, 0, 2_700, ClockSource::Wall)]);
    assert_eq!(clock_of(added.started, 180), "10:00");
    let lines = fs::read_to_string(h.app().store().paths.month_file(2026, 9)).expect("month file");
    assert_eq!(lines.lines().count(), 3, "{lines}");
    assert!(lines.contains(";manual;"), "{lines}");
    let screen = h.screen();
    assert!(screen.contains("by hand"), "the source column says so:\n{screen}");
    assert!(screen.contains("typed in"), "{screen}");
    done(&dir);
}

#[test]
fn a_correction_replaces_the_session_in_its_month_and_ctrl_z_takes_it_back() {
    let dir = temp("records-correct");
    let (_, focus) = seeded(&dir);
    let (_, short_session) = two_sessions(&dir, focus);
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 100, 24);
    h.press("down").press("enter");
    let screen = h.screen();
    assert!(h.app().form().is_some(), "{screen}");
    assert!(screen.contains("Correct the session"), "{screen}");
    assert!(screen.contains("Work › Rust"), "{screen}");
    assert!(screen.contains("13 : 00"), "the start is filled in:\n{screen}");
    assert!(screen.contains("0 h 10 min"), "and the work:\n{screen}");
    assert!(screen.contains("Ends at 13:10"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    // Half an hour instead of ten minutes, and a note.
    h.click_text("0 h 10 min").press("right").type_text("30");
    assert!(h.screen().contains("Ends at 13:30"), "{}", h.screen());
    h.press("tab").type_text("fixed").press("enter");
    assert!(h.app().form().is_none(), "{}", h.screen());
    assert!(h.screen().contains("Rust: session corrected"), "{}", h.screen());
    let sessions = h.app().store().sessions.clone();
    assert_eq!(sessions.len(), 2, "the correction replaces, it does not add");
    let corrected = sessions.iter().find(|session| session.note == "fixed").expect("the correction");
    assert_eq!(corrected.source, Source::Edited);
    assert_eq!(corrected.revision, 2);
    assert_eq!(corrected.replaces, Some(short_session.id));
    assert_eq!(corrected.work_seconds(), 1_800);
    assert!(corrected.spans.iter().all(|span| span.clock == ClockSource::Wall));
    let month = h.app().store().paths.month_file(2026, 9);
    assert_eq!(fs::read_to_string(&month).expect("month").lines().count(), 3);
    let screen = h.screen();
    assert!(screen.contains("30 min"), "{screen}");
    assert!(screen.contains("edited"), "{screen}");
    assert!(!screen.contains("10 min"), "{screen}");
    // Ctrl+Z writes the old values back as a newer line; the file only grows.
    h.press("ctrl+z");
    assert!(h.screen().contains("Rust: correction taken back"), "{}", h.screen());
    let sessions = &h.app().store().sessions;
    assert_eq!(sessions.len(), 2);
    let back = sessions.iter().find(|session| session.replaces == Some(corrected.id)).expect("the revert");
    assert_eq!(back.revision, 3);
    assert_eq!(back.work_seconds(), 600);
    assert_eq!(back.note, "");
    assert_eq!(fs::read_to_string(&month).expect("month").lines().count(), 4);
    assert!(h.screen().contains("10 min"), "{}", h.screen());
    h.press("ctrl+z");
    assert!(h.screen().contains("Nothing to undo"), "{}", h.screen());
    done(&dir);
}

#[test]
fn a_session_of_a_lost_focus_is_attached_from_the_problems_view() {
    let dir = temp("records-lost");
    let (_, focus) = seeded(&dir);
    two_sessions(&dir, focus);
    let lost =
        recorded(&dir, 30, Id::new(99, 99), 5 * 3_600, vec![Span::new(SpanKind::Work, 0, 900, ClockSource::Mono)], "");
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 100, 24);
    assert!(h.screen().contains("unknown focus"), "{}", h.screen());
    h.click_text("Problems");
    let screen = h.screen();
    assert!(screen.contains("1 session belongs to a focus that is not in the list"), "{screen}");
    assert!(screen.contains("15 min"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    assert!(h.is_focused(records::TABLE), "{screen}");
    h.press("down");
    assert_eq!(h.app().records().selected(), Some(0), "{}", h.screen());
    h.press("enter");
    let screen = h.screen();
    assert!(screen.contains("Attach to a focus"), "{screen}");
    assert!(screen.contains("Pick a focus"), "the lost focus is not on offer:\n{screen}");
    h.press("enter").press("enter");
    h.click_text("Save");
    assert!(h.app().form().is_none(), "{}", h.screen());
    assert!(h.screen().contains("Rust: session attached"), "{}", h.screen());
    let attached = h.app().store().sessions.iter().find(|session| session.replaces == Some(lost.id)).expect("attached");
    assert_eq!(attached.focus, focus);
    assert_eq!(attached.spans, lost.spans, "only the focus changed; the measured spans stay");
    assert_eq!(attached.source, Source::Edited);
    assert!(h.screen().contains("No problems"), "{}", h.screen());
    done(&dir);
}

#[test]
fn an_empty_records_tab_offers_to_add_by_hand() {
    let dir = temp("records-empty-add");
    seeded(&dir);
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 80, 20);
    h.click_text("Add by hand");
    assert!(h.app().form().is_some(), "{}", h.screen());
    h.press("esc");
    assert!(h.app().form().is_none());
    assert!(h.screen().contains("Cancelled"), "{}", h.screen());
    done(&dir);
}

#[test]
fn the_form_folds_to_one_column_at_forty_columns() {
    let dir = temp("records-form-narrow");
    let (_, focus) = seeded(&dir);
    two_sessions(&dir, focus);
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 40, 24);
    assert!(h.screen().contains("Add by hand"), "{}", h.screen());
    assert!(h.screen().contains("Export"), "the second row of buttons is there:\n{}", h.screen());
    h.press("down").press("enter");
    let screen = h.screen();
    assert!(h.app().form().is_some(), "{screen}");
    assert!(screen.contains("Focus"), "{screen}");
    assert!(screen.contains("Duration"), "{screen}");
    assert!(screen.contains("Save"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.set_glyph_mode(GlyphMode::Ascii).set_locale("tr");
    let screen = h.screen();
    assert!(screen.contains("Süre"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    done(&dir);
}

#[test]
fn an_instance_that_only_looks_cannot_add_or_correct() {
    let dir = temp("records-readonly-form");
    let (_, focus) = seeded(&dir);
    two_sessions(&dir, focus);
    let first = Store::open(Paths::at(&dir, "test"));
    assert_eq!(first.access, Access::Writer);
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 100, 20);
    h.press("a");
    assert!(h.app().form().is_none());
    assert!(h.screen().contains("only looks"), "{}", h.screen());
    h.press("down").press("f2");
    assert!(h.app().form().is_none());
    drop(first);
    done(&dir);
}

#[test]
fn an_instance_that_only_looks_cannot_remove_records() {
    let dir = temp("records-readonly");
    let (_, focus) = seeded(&dir);
    two_sessions(&dir, focus);
    let first = Store::open(Paths::at(&dir, "test"));
    assert_eq!(first.access, Access::Writer);
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 100, 20);
    h.press("down").press("delete");
    assert!(h.screen().contains("only looks"), "{}", h.screen());
    assert_eq!(h.app().store().sessions.len(), 2);
    assert!(!h.app().store().paths.trash_dir().exists());
    drop(first);
    done(&dir);
}

#[test]
fn export_writes_csv_and_json_next_to_the_records() {
    let dir = temp("records-export");
    let (_, focus) = seeded(&dir);
    two_sessions(&dir, focus);
    let clock = FakeClock::new();
    let mut h = records_at(&dir, &clock, 100, 20);
    h.press("e");
    assert!(h.screen().contains("Exported to"), "{}", h.screen());
    let folder = h.app().store().paths.export_dir();
    let csv = fs::read_to_string(folder.join("qfocus-2026-09-18.csv")).expect("csv");
    assert_eq!(csv.lines().count(), 3, "{csv}");
    assert!(csv.contains("Work,Rust,2026-09-18 12:00:00"), "{csv}");
    let json = fs::read_to_string(folder.join("qfocus-2026-09-18.json")).expect("json");
    assert!(json.contains("\"note\": \"meeting notes\""), "{json}");
    done(&dir);
}
