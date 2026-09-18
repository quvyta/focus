//! Reading and writing the line a session is stored as.
//!
//! One session is one line. Fields are separated by `;` and the order never changes. Only the note
//! carries free text, so it is the only field that needs escaping.

use qframe::diagnostics::{Diagnostic, Location};

use super::{Flag, Session, Source};
use crate::id::Id;
use crate::span::{ClockSource, Span, SpanKind, check};

/// The format this version writes.
pub const VERSION: &str = "qf1";

/// The line a session is stored as, without its line ending.
#[must_use]
pub fn write(session: &Session) -> String {
    let spans = session.spans.iter().map(write_span).collect::<Vec<_>>().join(",");
    let flags = session.flags.iter().map(|flag| flag.as_str()).collect::<Vec<_>>().join(",");
    let fields = [
        VERSION.to_owned(),
        session.id.to_string(),
        session.revision.to_string(),
        session.written.to_string(),
        session.focus.to_string(),
        session.started.to_string(),
        session.offset_minutes.to_string(),
        session.ended.to_string(),
        spans,
        session.source.as_str().to_owned(),
        write_link(session.replaces),
        write_link(session.voids),
        write_link(session.continues),
        flags,
        escape(&session.note),
    ];
    fields.join(";")
}

/// One span as `kind:offset:seconds:clock`. It holds no free text, so it needs no escaping.
fn write_span(span: &Span) -> String {
    let kind = match span.kind {
        SpanKind::Work => 'w',
        SpanKind::Pause => 'p',
        SpanKind::Gap => 'g',
        SpanKind::Idle => 'i',
    };
    let clock = match span.clock {
        ClockSource::Mono => 'm',
        ClockSource::Boot => 'b',
        ClockSource::Wall => 'w',
    };
    format!("{kind}:{}:{}:{clock}", span.offset, span.seconds)
}

/// An identifier, or an empty field when there is none.
fn write_link(id: Option<Id>) -> String {
    id.map(|value| value.to_string()).unwrap_or_default()
}

/// Hides the separator and the line endings inside a note.
fn escape(note: &str) -> String {
    let mut out = String::with_capacity(note.len());
    for character in note.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

/// Fields one line holds.
const FIELDS: usize = 15;

/// What came out of a file.
#[derive(Debug, Clone, Default)]
pub struct Reading {
    /// The sessions that could be read, in the order they appeared.
    pub sessions: Vec<Session>,
    /// Lines written by a newer qfocus. They are not broken; this one cannot read them yet.
    pub unknown_version: usize,
    /// Everything that went wrong, each pointing at a file, line and column.
    pub diagnostics: Vec<Diagnostic>,
}

/// Reads every session in `text`, skipping the lines it cannot read.
///
/// A broken line never stops the reading and never removes anything: it is skipped, it becomes a
/// diagnostic, and the rest of the file is still used. A line written by a newer version is not
/// broken, so it is counted instead of reported.
#[must_use]
pub fn read(file: &str, text: &str) -> Reading {
    let mut reading = Reading::default();
    let mut offset = 0usize;
    for line in text.split('\n') {
        let start = offset;
        offset += line.len() + 1;
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() {
            continue;
        }
        match read_line(line) {
            Ok(session) => {
                for problem in check(&session.spans, span_total(&session)) {
                    let location = Location::from_offset(file, text, start);
                    reading.diagnostics.push(Diagnostic::warning(Some(location), problem));
                }
                reading.sessions.push(session);
            }
            Err(Problem::UnknownVersion) => reading.unknown_version += 1,
            Err(Problem::Broken(reason)) => {
                let location = Location::from_offset(file, text, start);
                reading.diagnostics.push(Diagnostic::error(Some(location), reason));
            }
        }
    }
    reading
}

/// Seconds between the start and the end of `session`, as far as a `u32` reaches.
fn span_total(session: &Session) -> u32 {
    u32::try_from(session.ended.saturating_sub(session.started)).unwrap_or(u32::MAX)
}

/// Why a line could not become a session.
enum Problem {
    /// A newer qfocus wrote it.
    UnknownVersion,
    /// It is not a session line this version can make sense of.
    Broken(String),
}

/// One line as a session.
fn read_line(line: &str) -> Result<Session, Problem> {
    let fields = split_fields(line);
    let version = fields.first().map_or("", String::as_str);
    if version != VERSION {
        let newer = version.len() > 2 && version.starts_with("qf") && version[2..].bytes().all(|b| b.is_ascii_digit());
        return Err(if newer { Problem::UnknownVersion } else { Problem::Broken("not a session line".to_owned()) });
    }
    if fields.len() != FIELDS {
        return Err(Problem::Broken(format!("{} fields, expected {FIELDS}", fields.len())));
    }
    Ok(Session {
        id: id_field(&fields[1], "id")?,
        revision: number_field(&fields[2], "revision")?,
        written: number_field(&fields[3], "written")?,
        focus: id_field(&fields[4], "focus")?,
        started: number_field(&fields[5], "start")?,
        offset_minutes: number_field(&fields[6], "offset")?,
        ended: number_field(&fields[7], "end")?,
        spans: read_spans(&fields[8])?,
        source: Source::parse(&fields[9]).ok_or_else(|| Problem::Broken(format!("unknown source {:?}", fields[9])))?,
        replaces: link_field(&fields[10], "replaces")?,
        voids: link_field(&fields[11], "voids")?,
        continues: link_field(&fields[12], "continues")?,
        flags: read_flags(&fields[13])?,
        note: fields[14].clone(),
    })
}

/// The fields of a line, with the escapes in the note undone.
fn split_fields(line: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            if let Some(current) = fields.last_mut() {
                current.push(match character {
                    'n' => '\n',
                    'r' => '\r',
                    other => other,
                });
            }
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == ';' {
            fields.push(String::new());
        } else if let Some(current) = fields.last_mut() {
            current.push(character);
        }
    }
    fields
}

/// An identifier field.
fn id_field(text: &str, name: &str) -> Result<Id, Problem> {
    Id::parse(text).ok_or_else(|| Problem::Broken(format!("{name} is not an identifier")))
}

/// An identifier field that may be empty.
fn link_field(text: &str, name: &str) -> Result<Option<Id>, Problem> {
    if text.is_empty() { Ok(None) } else { id_field(text, name).map(Some) }
}

/// A number field.
fn number_field<T: std::str::FromStr>(text: &str, name: &str) -> Result<T, Problem> {
    text.parse().map_err(|_| Problem::Broken(format!("{name} is not a number")))
}

/// The spans field.
fn read_spans(text: &str) -> Result<Vec<Span>, Problem> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    text.split(',').map(read_span).collect()
}

/// One span of the spans field.
fn read_span(text: &str) -> Result<Span, Problem> {
    let parts: Vec<&str> = text.split(':').collect();
    let [kind, offset, seconds, clock] = parts.as_slice() else {
        return Err(Problem::Broken(format!("span {text:?} does not have four parts")));
    };
    let kind = match *kind {
        "w" => SpanKind::Work,
        "p" => SpanKind::Pause,
        "g" => SpanKind::Gap,
        "i" => SpanKind::Idle,
        other => return Err(Problem::Broken(format!("unknown span kind {other:?}"))),
    };
    let clock = match *clock {
        "m" => ClockSource::Mono,
        "b" => ClockSource::Boot,
        "w" => ClockSource::Wall,
        other => return Err(Problem::Broken(format!("unknown clock {other:?}"))),
    };
    Ok(Span::new(kind, number_field(offset, "span offset")?, number_field(seconds, "span length")?, clock))
}

/// The flags field.
fn read_flags(text: &str) -> Result<Vec<Flag>, Problem> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|word| Flag::parse(word).ok_or_else(|| Problem::Broken(format!("unknown flag {word:?}"))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::{ClockSource, Span, SpanKind};

    fn sample() -> Session {
        Session {
            id: Id::new(1_758_131_400_000, 1),
            revision: 1,
            written: 1_758_131_400,
            focus: Id::new(1_758_124_800_000, 2),
            started: 1_758_124_800,
            offset_minutes: 180,
            ended: 1_758_131_400,
            spans: vec![
                Span::new(SpanKind::Work, 0, 1500, ClockSource::Mono),
                Span::new(SpanKind::Pause, 1500, 600, ClockSource::Mono),
                Span::new(SpanKind::Work, 2100, 4500, ClockSource::Mono),
            ],
            source: Source::Timer,
            replaces: None,
            voids: None,
            continues: None,
            flags: Vec::new(),
            note: String::new(),
        }
    }

    #[test]
    fn writes_every_field_in_order() {
        let line = write(&sample());
        let fields: Vec<&str> = line.split(';').collect();
        assert_eq!(fields.len(), 15);
        assert_eq!(fields[0], "qf1");
        assert_eq!(fields[2], "1");
        assert_eq!(fields[6], "180");
        assert_eq!(fields[8], "w:0:1500:m,p:1500:600:m,w:2100:4500:m");
        assert_eq!(fields[9], "timer");
    }

    #[test]
    fn leaves_absent_links_empty() {
        let line = write(&sample());
        let fields: Vec<&str> = line.split(';').collect();
        assert_eq!(fields[10], "");
        assert_eq!(fields[11], "");
        assert_eq!(fields[12], "");
        assert_eq!(fields[13], "");
        assert_eq!(fields[14], "");
    }

    #[test]
    fn writes_links_and_flags_when_they_are_there() {
        let mut session = sample();
        session.revision = 2;
        session.source = Source::Edited;
        session.replaces = Some(Id::new(1, 1));
        session.flags = vec![Flag::SuspectClock, Flag::OverCeiling];
        let fields: Vec<String> = write(&session).split(';').map(str::to_owned).collect();
        assert_eq!(fields[9], "edited");
        assert_eq!(fields[10], Id::new(1, 1).to_string());
        assert_eq!(fields[13], "suspect-clock,over-ceiling");
    }

    /// Separators that actually separate: a `;` the note carries is escaped and does not count.
    /// A plain `split(';')` cannot tell the two apart, which is the whole reason for escaping.
    fn unescaped_separators(line: &str) -> usize {
        let mut count = 0;
        let mut escaped = false;
        for character in line.chars() {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == ';' {
                count += 1;
            }
        }
        count
    }

    #[test]
    fn escapes_the_note() {
        let mut session = sample();
        session.note = "yol C:\\temp\\; iki\nsatır".to_owned();
        let line = write(&session);
        assert!(line.ends_with("yol C:\\\\temp\\\\\\; iki\\nsatır"), "{line}");
        assert_eq!(unescaped_separators(&line), 14, "kaçırılmış ayırıcı satırı böldü");
    }

    #[test]
    fn a_line_never_carries_a_newline() {
        let mut session = sample();
        session.note = "bir\nsatır\r\nbaşka".to_owned();
        let line = write(&session);
        assert!(!line.contains('\n'));
        assert!(!line.contains('\r'));
    }

    #[test]
    fn reads_back_what_it_wrote() {
        let session = sample();
        let text = format!("{}\n", write(&session));
        let reading = read("2026-09.log", &text);
        assert!(reading.diagnostics.is_empty(), "{:?}", reading.diagnostics);
        assert_eq!(reading.sessions, vec![session]);
    }

    #[test]
    fn reads_back_a_note_with_separators_and_backslashes() {
        let mut session = sample();
        session.note = "yol C:\\temp\\; iki\nsatır".to_owned();
        let text = format!("{}\n", write(&session));
        let reading = read("2026-09.log", &text);
        assert_eq!(reading.sessions.first().map(|found| found.note.clone()), Some(session.note));
    }

    #[test]
    fn skips_a_broken_line_and_keeps_the_rest() {
        let good = write(&sample());
        let text = format!("{good}\nqf1;bozuk\n{good}\n");
        let reading = read("2026-09.log", &text);
        assert_eq!(reading.sessions.len(), 2);
        assert_eq!(reading.diagnostics.len(), 1);
        let location = reading.diagnostics[0].location.as_ref().expect("konum");
        assert_eq!(location.line, 2);
        assert_eq!(location.file, "2026-09.log");
    }

    #[test]
    fn an_unknown_version_is_counted_not_broken() {
        let text = format!("{}\nqf2;whatever;this;is\n", write(&sample()));
        let reading = read("2026-09.log", &text);
        assert_eq!(reading.sessions.len(), 1);
        assert_eq!(reading.unknown_version, 1);
        assert!(reading.diagnostics.is_empty(), "{:?}", reading.diagnostics);
    }

    #[test]
    fn a_line_that_is_not_ours_at_all_is_an_error() {
        let reading = read("2026-09.log", "merhaba dünya\n");
        assert_eq!(reading.unknown_version, 0);
        assert_eq!(reading.diagnostics.len(), 1);
    }

    #[test]
    fn empty_lines_are_ignored() {
        let text = format!("\n{}\n\n", write(&sample()));
        let reading = read("2026-09.log", &text);
        assert_eq!(reading.sessions.len(), 1);
        assert!(reading.diagnostics.is_empty());
    }

    #[test]
    fn broken_spans_are_reported_but_the_session_is_kept() {
        let mut session = sample();
        session.spans = vec![Span::new(SpanKind::Work, 10, 100, ClockSource::Mono)];
        let text = format!("{}\n", write(&session));
        let reading = read("2026-09.log", &text);
        assert_eq!(reading.sessions.len(), 1, "kayıt atılmamalı");
        assert!(!reading.diagnostics.is_empty());
    }

    #[test]
    fn a_column_points_at_characters_not_bytes() {
        let mut session = sample();
        session.note = "ğğğ".to_owned();
        let text = format!("{}\nqf1;bozuk\n", write(&session));
        let reading = read("2026-09.log", &text);
        let location = reading.diagnostics[0].location.as_ref().expect("konum");
        assert_eq!(location.line, 2);
        assert_eq!(location.column, 1);
    }
}
