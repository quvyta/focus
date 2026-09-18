//! Sessions as CSV and JSON, for a spreadsheet or another program.
//!
//! Both are written by hand: the shapes are flat and small, and a note is the only field that
//! can carry anything awkward. CSV follows RFC 4180, so a note with a comma, a quote or a line
//! break comes out as one quoted field; JSON escapes what the standard says it must. Times are
//! written in the local time each session was lived in, with its offset beside them, so nothing
//! has to be worked out again on the other side.

use qframe::date::DateTime;

use crate::session::Session;
use crate::span::{ClockSource, Span, SpanKind};
use crate::tree::Tree;

/// The header line of the CSV, one name per column.
const COLUMNS: [&str; 12] = [
    "id",
    "category",
    "focus",
    "started",
    "ended",
    "offset_minutes",
    "work_seconds",
    "span_seconds",
    "source",
    "flags",
    "note",
    "spans",
];

/// `sessions` as CSV: a header naming the columns, then one line per session.
///
/// Spans are written in one cell as `kind:offset:seconds:clock` pairs separated by spaces, so a
/// spreadsheet keeps one row per session. Lines end in `\r\n` as the standard asks.
#[must_use]
pub fn csv(sessions: &[Session], tree: &Tree) -> String {
    let mut out = String::new();
    out.push_str(&COLUMNS.join(","));
    out.push_str("\r\n");
    for session in sessions {
        let (category, focus) = names(tree, session);
        let fields = [
            session.id.to_string(),
            category,
            focus,
            local(session.started, session.offset_minutes),
            local(session.ended, session.offset_minutes),
            session.offset_minutes.to_string(),
            session.work_seconds().to_string(),
            span_seconds(session).to_string(),
            session.source.as_str().to_owned(),
            session.flags.iter().map(|flag| flag.as_str()).collect::<Vec<_>>().join(" "),
            session.note.clone(),
            session.spans.iter().map(span_word).collect::<Vec<_>>().join(" "),
        ];
        let line: Vec<String> = fields.iter().map(|field| csv_field(field)).collect();
        out.push_str(&line.join(","));
        out.push_str("\r\n");
    }
    out
}

/// `sessions` as a JSON array of objects, one per session, with the spans as an array inside.
#[must_use]
pub fn json(sessions: &[Session], tree: &Tree) -> String {
    if sessions.is_empty() {
        return "[]\n".to_owned();
    }
    let mut out = String::from("[\n");
    for (index, session) in sessions.iter().enumerate() {
        if index > 0 {
            out.push_str(",\n");
        }
        let (category, focus) = names(tree, session);
        let flags: Vec<String> = session.flags.iter().map(|flag| json_string(flag.as_str())).collect();
        let spans: Vec<String> = session
            .spans
            .iter()
            .map(|span| {
                format!(
                    "{{\"kind\": {}, \"offset\": {}, \"seconds\": {}, \"clock\": {}}}",
                    json_string(kind_word(span)),
                    span.offset,
                    span.seconds,
                    json_string(clock_word(span))
                )
            })
            .collect();
        out.push_str("  {\n");
        let pairs = [
            ("id", json_string(&session.id.to_string())),
            ("category", json_string(&category)),
            ("focus", json_string(&focus)),
            ("started", json_string(&local(session.started, session.offset_minutes))),
            ("ended", json_string(&local(session.ended, session.offset_minutes))),
            ("started_unix", session.started.to_string()),
            ("ended_unix", session.ended.to_string()),
            ("offset_minutes", session.offset_minutes.to_string()),
            ("work_seconds", session.work_seconds().to_string()),
            ("span_seconds", span_seconds(session).to_string()),
            ("source", json_string(session.source.as_str())),
            ("flags", format!("[{}]", flags.join(", "))),
            ("note", json_string(&session.note)),
            ("spans", format!("[{}]", spans.join(", "))),
        ];
        let lines: Vec<String> = pairs.iter().map(|(key, value)| format!("    \"{key}\": {value}")).collect();
        out.push_str(&lines.join(",\n"));
        out.push_str("\n  }");
    }
    out.push_str("\n]\n");
    out
}

/// The category and focus names of `session`, empty when the catalogue no longer has them.
fn names(tree: &Tree, session: &Session) -> (String, String) {
    tree.categories
        .iter()
        .find_map(|category| {
            category
                .focuses
                .iter()
                .find(|focus| focus.id == session.focus)
                .map(|focus| (category.name.clone(), focus.name.clone()))
        })
        .unwrap_or_default()
}

/// Seconds between the start and the end of `session`.
fn span_seconds(session: &Session) -> i64 {
    session.ended.saturating_sub(session.started)
}

/// A moment as `YYYY-MM-DD HH:MM:SS` in the offset given.
fn local(seconds: i64, offset_minutes: i16) -> String {
    let moment = DateTime::from_unix(seconds, offset_minutes);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        moment.date.year(),
        moment.date.month(),
        moment.date.day(),
        moment.time.hour,
        moment.time.minute,
        moment.time.second
    )
}

/// One span as `kind:offset:seconds:clock`.
fn span_word(span: &Span) -> String {
    format!("{}:{}:{}:{}", kind_word(span), span.offset, span.seconds, clock_word(span))
}

/// The word for what the person was doing.
fn kind_word(span: &Span) -> &'static str {
    match span.kind {
        SpanKind::Work => "work",
        SpanKind::Pause => "pause",
        SpanKind::Gap => "gap",
        SpanKind::Idle => "idle",
    }
}

/// The word for the clock a span was measured with.
fn clock_word(span: &Span) -> &'static str {
    match span.clock {
        ClockSource::Mono => "mono",
        ClockSource::Boot => "boot",
        ClockSource::Wall => "wall",
    }
}

/// One CSV field: quoted, with quotes doubled, whenever it holds a comma, a quote or a line
/// break; as it is otherwise.
fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) { format!("\"{}\"", text.replace('"', "\"\"")) } else { text.to_owned() }
}

/// `text` as a JSON string with its quotes.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", other as u32)),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::Id;
    use crate::session::{Flag, Source};
    use crate::tree::{Category, Focus};

    fn tree() -> Tree {
        Tree {
            categories: vec![Category {
                id: Id::new(1, 1),
                name: "Work".to_owned(),
                icon: None,
                goal: None,
                archived: false,
                order: 0.0,
                focuses: vec![Focus {
                    id: Id::new(2, 2),
                    name: "Rust".to_owned(),
                    goal: None,
                    archived: false,
                    order: 0.0,
                }],
            }],
        }
    }

    fn session(note: &str) -> Session {
        // 2026-09-18 09:00 in Istanbul.
        let started = 1_789_711_200;
        Session {
            id: Id::new(3, 3),
            revision: 1,
            written: started + 2_400,
            focus: Id::new(2, 2),
            started,
            offset_minutes: 180,
            ended: started + 2_400,
            spans: vec![
                Span::new(SpanKind::Work, 0, 1_500, ClockSource::Mono),
                Span::new(SpanKind::Pause, 1_500, 300, ClockSource::Mono),
                Span::new(SpanKind::Work, 1_800, 600, ClockSource::Mono),
            ],
            source: Source::Timer,
            replaces: None,
            voids: None,
            continues: None,
            flags: vec![Flag::SuspectClock],
            note: note.to_owned(),
        }
    }

    #[test]
    fn csv_has_a_header_and_one_line_per_session_with_local_times() {
        let text = csv(&[session("plain"), session("")], &tree());
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(lines.len(), 4, "{text:?}");
        assert_eq!(lines[0], COLUMNS.join(","));
        assert_eq!(
            lines[1],
            format!(
                "{},Work,Rust,2026-09-18 09:00:00,2026-09-18 09:40:00,180,2100,2400,timer,suspect-clock,plain,\
                 work:0:1500:mono pause:1500:300:mono work:1800:600:mono",
                Id::new(3, 3)
            )
        );
        assert_eq!(lines[3], "");
    }

    #[test]
    fn csv_quotes_a_note_with_a_comma_a_quote_and_a_line_break() {
        let text = csv(&[session("read \"Rust\", twice\nthen wrote")], &tree());
        assert!(text.contains(",\"read \"\"Rust\"\", twice\nthen wrote\","), "{text}");
        // The line break inside the quotes is the only one besides the record ends.
        assert_eq!(text.matches("\r\n").count(), 2);
    }

    #[test]
    fn an_unknown_focus_leaves_the_names_empty() {
        let mut orphan = session("");
        orphan.focus = Id::new(9, 9);
        let text = csv(&[orphan], &tree());
        assert!(text.contains(&format!("{},,,2026-09-18", Id::new(3, 3))), "{text}");
    }

    #[test]
    fn json_is_an_array_of_objects_with_spans_and_escaped_notes() {
        let text = json(&[session("tab\there \"q\" \\ done")], &tree());
        assert!(text.starts_with("[\n  {\n"), "{text}");
        assert!(text.ends_with("\n  }\n]\n"), "{text}");
        assert!(text.contains("\"note\": \"tab\\there \\\"q\\\" \\\\ done\""), "{text}");
        assert!(text.contains("\"flags\": [\"suspect-clock\"]"), "{text}");
        assert!(
            text.contains("{\"kind\": \"pause\", \"offset\": 1500, \"seconds\": 300, \"clock\": \"mono\"}"),
            "{text}"
        );
        assert!(text.contains("\"work_seconds\": 2100"), "{text}");
        assert!(text.contains("\"started\": \"2026-09-18 09:00:00\""), "{text}");
        assert!(text.contains("\"started_unix\": 1789711200"), "{text}");
    }

    #[test]
    fn empty_lists_are_still_well_formed() {
        assert_eq!(csv(&[], &tree()), format!("{}\r\n", COLUMNS.join(",")));
        assert_eq!(json(&[], &tree()), "[]\n");
    }
}
