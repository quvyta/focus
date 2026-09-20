//! The charts page and the day strip: what the pictures say about the sessions behind them.

use super::*;

use crate::ui::charts;

/// The charts over `dir`, the week pinned to Monday so the drawings do not move with the
/// language's calendar.
fn charts_at(dir: &Path, clock: &FakeClock, width: u16, height: u16) -> Harness<QFocus> {
    let prefs = Prefs { week_start: Some(Weekday::Monday), ..Prefs::default() };
    let mut h = harness(app_with(dir, clock, &prefs), width, height);
    h.press("2");
    assert_eq!(h.app().page(), Page::Charts);
    h
}

#[test]
fn the_charts_tab_opens_with_2_and_the_day_reads_its_hours_and_categories() {
    let dir = temp("charts-day");
    history(&dir);
    let clock = FakeClock::new();
    let mut h = charts_at(&dir, &clock, 80, 24);
    let screen = h.screen();
    assert!(screen.contains("+1 h compared with yesterday"), "{screen}");
    assert!(screen.contains("By the hour"), "{screen}");
    assert!(screen.contains("00 01 02"), "the hours stand with their labels:\n{screen}");
    assert!(screen.contains("By category"), "{screen}");
    assert!(screen.contains("Work 1 h"), "the share reads as words:\n{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    // The keys go to the chart: End picks the last hour, Left walks back to the one worked.
    assert!(h.is_focused(charts::CHART), "{screen}");
    h.press("end");
    assert_eq!(h.app().charts().selected(), Some(23));
    for _ in 0..11 {
        h.press("left");
    }
    assert_eq!(h.app().charts().selected(), Some(12));
    assert!(h.screen().contains("12:00 to 13:00 · 50 min"), "{}", h.screen());
    // A narrower terminal lays the worked hours down with their labels and values.
    h.resize(60, 24);
    let screen = h.screen();
    assert!(screen.contains("12:00"), "{screen}");
    assert!(screen.contains("50 min"), "{screen}");
    assert!(screen.contains("13:00"), "{screen}");
    assert!(!screen.contains("00 01 02"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    // Delete belongs to the other pages; on the charts it archives nothing.
    h.press("delete");
    assert!(h.app().store().tree.categories.iter().all(|category| !category.archived), "{}", h.screen());
    h.press("1");
    assert_eq!(h.app().page(), Page::Today);
    done(&dir);
}

#[test]
fn the_week_stacks_the_categories_with_a_legend_and_names_the_picked_day() {
    let dir = temp("charts-week");
    history(&dir);
    let clock = FakeClock::new();
    let mut h = charts_at(&dir, &clock, 80, 24);
    h.click_text("Week");
    assert_eq!(h.app().charts().scale(), charts::Scale::Week);
    assert!(h.is_focused(charts::CHART), "{}", h.screen());
    let screen = h.screen();
    assert!(screen.contains("+3 h 15 min compared with last week"), "{screen}");
    assert!(screen.contains("Mon") && screen.contains("Sun"), "{screen}");
    assert!(screen.contains("2 h"), "Wednesday's stack carries its total:\n{screen}");
    let (_, legend) = h.find("Life").expect("the legend names the second category");
    let (_, labels) = h.find("Mon").expect("the day labels");
    assert!(legend > labels, "the legend stands under the chart:\n{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.press("right").press("right");
    assert_eq!(h.app().charts().selected(), Some(1));
    let screen = h.screen();
    assert!(screen.contains("Tuesday · 45 min · Work 45 min"), "the archived focus counts:\n{screen}");
    h.press("home");
    assert!(h.screen().contains("Monday · 30 min · Life 30 min"), "{}", h.screen());
    // Lying down at forty columns, every day keeps its full name.
    h.resize(40, 24);
    let screen = h.screen();
    assert!(screen.contains("Wednesday"), "{screen}");
    assert!(screen.contains("Life"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.press("down");
    assert_eq!(h.app().charts().selected(), Some(1), "the lying chart walks with up and down");
    done(&dir);
}

#[test]
fn the_month_reads_a_day_off_the_trend_and_ranks_the_focuses() {
    let dir = temp("charts-month");
    history(&dir);
    let clock = FakeClock::new();
    let mut h = charts_at(&dir, &clock, 80, 24);
    h.click_text("Month");
    let screen = h.screen();
    assert!(screen.contains("+4 h 15 min compared with last month"), "{screen}");
    assert!(screen.contains("Day by day"), "{screen}");
    let (_, hint) = h.find("Pick a bar").expect("the hint stands under the day axis");
    let axis = screen.lines().nth(usize::try_from(hint - 2).unwrap_or(0)).unwrap_or_default();
    assert_eq!(axis.trim_end(), "1  4  7  10 13 16", "the days thin under the trend:\n{screen}");
    assert!(screen.contains("Most worked on"), "{screen}");
    let (_, rust) = h.find("Rust").expect("the most worked focus");
    let (_, reading) = h.find("Reading").expect("the least worked focus");
    assert!(rust < reading, "most first:\n{screen}");
    assert!(screen.contains("45 min"), "the archived Review is ranked too:\n{screen}");
    assert!(screen.contains("6 sessions · average 52 min 30 s"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.press("end");
    assert_eq!(h.app().charts().selected(), Some(17));
    assert!(h.screen().contains("18 September · 1 h"), "{}", h.screen());
    h.press("left");
    assert!(h.screen().contains("17 September · 0 s"), "{}", h.screen());
    h.press("esc");
    assert_eq!(h.app().charts().selected(), None);
    assert!(h.screen().contains("Pick a bar"), "{}", h.screen());
    done(&dir);
}

#[test]
fn the_year_grid_ends_today_and_is_cut_to_the_newest_weeks_when_narrow() {
    let dir = temp("charts-year");
    history(&dir);
    let clock = FakeClock::new();
    let mut h = charts_at(&dir, &clock, 80, 24);
    h.click_text("Year");
    let screen = h.screen();
    assert!(screen.contains("+6 h 15 min compared with the year before"), "{screen}");
    assert!(screen.contains("The last 52 weeks"), "{screen}");
    let (_, hint) = h.find("Pick a day").expect("the hint stands under the grid");
    let axis = screen.lines().nth(usize::try_from(hint - 9).unwrap_or(0)).unwrap_or_default();
    assert_eq!(axis.trim_end(), "Sep Oct Nov Dec Jan Feb Mar Apr May Jun Jul Aug Sep", "over the weeks:\n{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.press("enter");
    assert_eq!(h.app().charts().selected(), Some(51 * 7 + 4), "Enter picks today first");
    assert!(h.screen().contains("Friday, 18 September · 1 h"), "{}", h.screen());
    h.press("left").press("enter");
    assert!(h.screen().contains("Friday, 11 September · 0 s"), "a week back:\n{}", h.screen());
    h.resize(40, 24);
    let screen = h.screen();
    assert!(screen.contains("The last 40 of 52 weeks"), "{screen}");
    assert!(screen.contains("Dec") && !screen.contains("Nov"), "the months begin with the first week shown:\n{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    done(&dir);
}

#[test]
fn empty_charts_say_so_on_every_scale_and_the_running_counter_is_named() {
    let dir = temp("charts-empty");
    seeded(&dir);
    let clock = FakeClock::new();
    let mut h = charts_at(&dir, &clock, 80, 24);
    for (scale, text) in [
        ("Day", "No records today yet."),
        ("Week", "No records this week yet."),
        ("Month", "No records this month yet."),
        ("Year", "No records in the last year."),
    ] {
        h.click_text(scale);
        let screen = h.screen();
        assert!(screen.contains(text), "{scale}:\n{screen}");
        assert!(!screen.contains("compared"), "no comparison over nothing:\n{screen}");
        assert_eq!(forbidden(&screen), None, "{screen}");
    }
    h.press("1").click_text("Rust");
    clock.pass(120);
    h.advance(Duration::from_secs(1));
    h.press("2").click_text("Day");
    let screen = h.screen();
    assert!(screen.contains("The running counter is not in the charts yet"), "{screen}");
    assert!(screen.contains("No records today yet."), "the counter is not counted:\n{screen}");
    done(&dir);
}

#[test]
fn unreadable_records_are_counted_above_the_charts() {
    let dir = temp("charts-problems");
    history(&dir);
    let month = Paths::at(&dir, "test").month_file(2026, 9);
    let mut text = fs::read_to_string(&month).expect("month");
    text.push_str("qf1;not a record\n");
    fs::write(&month, text).expect("month");
    let clock = FakeClock::new();
    let h = charts_at(&dir, &clock, 80, 24);
    let screen = h.screen();
    assert!(screen.contains("1 record could not be read; the charts are drawn without it."), "{screen}");
    assert_eq!(screen.matches("could not be read").count(), 1, "said once:\n{screen}");
    assert!(screen.contains("By the hour"), "the charts are still drawn:\n{screen}");
    done(&dir);
}

#[test]
fn ascii_and_turkish_charts_keep_clean() {
    let dir = temp("charts-ascii");
    history(&dir);
    let clock = FakeClock::new();
    let mut h = charts_at(&dir, &clock, 80, 24);
    h.set_glyph_mode(GlyphMode::Ascii).set_locale("tr");
    let screen = h.screen();
    assert!(screen.contains("Grafikler"), "{screen}");
    assert!(screen.contains("Düne göre +1 sa"), "{screen}");
    assert!(screen.contains("Saat saat"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.click_text("Hafta");
    let screen = h.screen();
    assert!(screen.contains("Geçen haftaya göre +3 sa 15 dk"), "{screen}");
    assert!(screen.contains("Pzt") && screen.contains("Paz"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    assert!(h.is_focused(charts::CHART), "the keys follow the scale:\n{screen}");
    h.press("right");
    assert!(h.screen().contains("Pazartesi · 30 dk · Life 30 dk"), "{}", h.screen());
    // "Ay" is also the start of the Ayarlar tab, so the scale is chosen by message.
    h.send(Msg::Charts(charts::Msg::Scale(charts::Scale::Month.index())));
    let screen = h.screen();
    assert!(screen.contains("6 oturum · ortalama 52 dk 30 sn"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.click_text("Yıl");
    let screen = h.screen();
    assert!(screen.contains("Son 52 hafta"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.resize(40, 24);
    for scale in ["Gün", "Hafta", "Ay", "Yıl"] {
        h.click_text(scale);
        assert_eq!(forbidden(&h.screen()), None, "{}", h.screen());
    }
    done(&dir);
}

/// The strip's row on the Today page: under the header and the tabs.
const STRIP_ROW: i32 = 2;

/// The column of the strip cell that `hour` o'clock falls in, on a strip `width` columns
/// wide with one column of padding on each side.
fn strip_x(hour: u32, width: u16) -> i32 {
    1 + i32::try_from(u64::from(hour) * 3_600 * u64::from(width - 2) / 86_400).unwrap_or(0)
}

/// The background of the legend swatch before the name `name`, wherever it stands first.
fn swatch(h: &Harness<QFocus>, name: &str) -> Option<qframe::color::Rgb> {
    let (x, y) = h.find(name).expect("named in a legend");
    h.bg(u16::try_from(x - 3).unwrap_or(0), u16::try_from(y).unwrap_or(0))
}

#[test]
fn the_day_strip_reads_its_blocks_with_the_pointer_and_the_keys_and_names_the_tones() {
    let dir = temp("strip");
    history(&dir);
    let clock = FakeClock::new();
    let mut h = harness(app_at(&dir, &clock), 80, 24);
    let screen = h.screen();
    assert!(screen.contains("12:00") && screen.contains("02:00"), "the hours run under the strip:\n{screen}");
    assert_eq!(screen.matches("Work").count(), 2, "the legend and the tree name the category:\n{screen}");
    assert_eq!(screen.matches("Life").count(), 1, "a category with nothing today is only in the tree:\n{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    // The block of noon carries the tone the legend names.
    let noon = (u16::try_from(strip_x(12, 80)).unwrap_or(0), u16::try_from(STRIP_ROW).unwrap_or(0));
    let tone = h.bg(noon.0, noon.1);
    assert!(tone.is_some());
    assert_eq!(tone, swatch(&h, "Work"), "{screen}");
    assert_ne!(tone, h.bg(u16::try_from(strip_x(3, 80)).unwrap_or(0), noon.1), "three o'clock is a hole");
    h.hover(strip_x(12, 80), STRIP_ROW);
    assert!(h.screen().contains("Rust  12:00–12:30  30 min"), "the pointer reads the block:\n{}", h.screen());
    h.click(strip_x(12, 80), STRIP_ROW);
    assert_eq!(h.app().today().block(), Some(0));
    assert!(h.is_focused(today::STRIP), "{}", h.screen());
    h.hover(0, 0);
    h.press("right");
    assert_eq!(h.app().today().block(), Some(1));
    assert!(h.screen().contains("Rust  12:40–13:00  20 min"), "the break is a hole:\n{}", h.screen());
    h.press("end");
    assert!(h.screen().contains("Rust  13:00–13:10  10 min"), "{}", h.screen());
    // The zoom keys ask for a stretch and 0 for the whole day; the strip never slides.
    h.press("+");
    assert!(h.app().today().zoom().is_some(), "{}", h.screen());
    assert_eq!(forbidden(&h.screen()), None, "{}", h.screen());
    h.press("0");
    assert_eq!(h.app().today().zoom(), None);
    // The same category has the same tone on the Week chart.
    h.press("2").click_text("Week");
    assert_eq!(swatch(&h, "Work"), tone, "{}", h.screen());
    done(&dir);
}

#[test]
fn the_running_counter_is_a_live_block_on_the_strip() {
    let dir = temp("strip-running");
    seeded(&dir);
    let clock = FakeClock::new();
    let mut h = harness(app_at(&dir, &clock), 80, 24);
    assert!(!h.screen().contains("00:00"), "an empty day draws no strip:\n{}", h.screen());
    h.click_text("Rust");
    clock.pass(120);
    h.advance(Duration::from_secs(1));
    let screen = h.screen();
    assert!(screen.contains("00:00"), "the strip stands over the counter:\n{screen}");
    assert!(screen.contains("Work"), "{screen}");
    // Noon UTC is three in the afternoon where the counter runs.
    h.click(strip_x(15, 80), STRIP_ROW);
    assert!(h.screen().contains("Rust  15:00–15:02  running  2 min"), "{}", h.screen());
    clock.pass(60);
    h.advance(Duration::from_secs(1));
    assert!(h.screen().contains("Rust  15:00–15:03  running  3 min"), "it grows:\n{}", h.screen());
    h.set_locale("tr");
    assert!(h.screen().contains("Rust  15:00–15:03  sürüyor  3 dk"), "{}", h.screen());
    h.set_locale("en");
    h.press("p");
    clock.pass(60);
    h.advance(Duration::from_secs(1));
    assert!(h.screen().contains("Rust  15:00–15:03  3 min"), "on a break the block is closed:\n{}", h.screen());
    h.set_glyph_mode(GlyphMode::Ascii).set_locale("tr");
    let screen = h.screen();
    assert!(screen.contains("Rust  15:00"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    done(&dir);
}

#[test]
fn a_narrow_strip_thins_its_hours_then_drops_the_legend() {
    let dir = temp("strip-narrow");
    history(&dir);
    let clock = FakeClock::new();
    let mut h = harness(app_at(&dir, &clock), 40, 24);
    let screen = h.screen();
    assert!(screen.contains("04:00") && !screen.contains("02:00"), "{screen}");
    assert_eq!(screen.matches("Work").count(), 2, "the legend stays at forty:\n{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.resize(30, 24);
    let screen = h.screen();
    assert_eq!(screen.matches("Work").count(), 1, "below forty the legend goes:\n{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    h.set_glyph_mode(GlyphMode::Ascii).set_locale("tr");
    h.click(strip_x(12, 30), STRIP_ROW);
    let screen = h.screen();
    // Two blocks share the cell of noon on a strip this narrow; the later one is read.
    assert!(screen.contains("Rust  12:40–13:00  20 dk"), "{screen}");
    assert_eq!(forbidden(&screen), None, "{screen}");
    done(&dir);
}
