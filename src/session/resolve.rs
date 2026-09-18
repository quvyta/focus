//! Working out which version of a session is the one to show.
//!
//! Nothing is ever taken out of a file: a correction, a removal and an undo are all new lines. The
//! rule is one sentence — for a chain of identifiers the highest revision is the one that counts,
//! and a line that voids the chain takes it out of the lists.

use std::collections::{HashMap, HashSet};

use qframe::diagnostics::Diagnostic;

use super::Session;
use crate::id::Id;

/// What the records in a file add up to.
#[derive(Debug, Clone, Default)]
pub struct Resolution {
    /// The sessions to show, one per chain.
    pub sessions: Vec<Session>,
    /// Sessions taken out of the lists, each as its last version before the removal: what the
    /// trash shows and what an undo brings back.
    pub voided: Vec<Session>,
    /// Corrections of records that are not here, kept so a person's work is never dropped in
    /// silence.
    pub orphans: Vec<Session>,
    /// Chains two records claim to be the newest version of, for a person to sort out.
    pub forks: Vec<Id>,
    /// Everything worth telling the person about.
    pub diagnostics: Vec<Diagnostic>,
}

/// Works out the current version of every session in `records`.
///
/// The order records arrive in does not matter: a correction written a month later, in another
/// file, resolves the same way.
#[must_use]
pub fn resolve(records: Vec<Session>) -> Resolution {
    let mut resolution = Resolution::default();
    let parents = parents_of(&records);
    let known: HashSet<Id> = records.iter().map(|record| record.id).collect();

    let mut chains: HashMap<Id, Vec<Session>> = HashMap::new();
    for record in records {
        let root = root_of(&record, &parents);
        if known.contains(&root) {
            chains.entry(root).or_default().push(record);
        } else {
            let message = format!("correction {} replaces a record that is not here", record.id);
            resolution.diagnostics.push(Diagnostic::warning(None, message));
            resolution.orphans.push(record);
        }
    }

    let mut roots: Vec<Id> = chains.keys().copied().collect();
    roots.sort_unstable();
    for root in roots {
        let Some(chain) = chains.remove(&root) else { continue };
        let Some(winner) = newest(&chain) else { continue };
        if chain.iter().filter(|record| record.revision == winner.revision).count() > 1 {
            resolution.forks.push(root);
            let message = format!("two records claim to be version {} of {root}", winner.revision);
            resolution.diagnostics.push(Diagnostic::warning(None, message));
        }
        if winner.voids.is_some() {
            // The removal itself carries no session worth showing; the newest version that was
            // a session is what the trash holds.
            let kept: Vec<Session> = chain.into_iter().filter(|record| record.voids.is_none()).collect();
            if let Some(last) = newest(&kept) {
                resolution.voided.push(last);
            }
            continue;
        }
        resolution.sessions.push(winner);
    }
    resolution
}

/// The highest revision any record of the chain `id` belongs to carries, or zero when no record
/// carries `id`. A record that is to win the chain needs a revision above this.
#[must_use]
pub fn newest_revision(records: &[Session], id: Id) -> u32 {
    let parents = parents_of(records);
    let Some(member) = records.iter().find(|record| record.id == id) else { return 0 };
    let root = root_of(member, &parents);
    records.iter().filter(|record| root_of(record, &parents) == root).map(|record| record.revision).max().unwrap_or(0)
}

/// The first identifier of the chain each record belongs to, keyed by the record's own
/// identifier: the same grouping [`resolve`] makes, for a caller that has to find every line of a
/// chain.
#[must_use]
pub fn roots(records: &[Session]) -> HashMap<Id, Id> {
    let parents = parents_of(records);
    records.iter().map(|record| (record.id, root_of(record, &parents))).collect()
}

/// Which record each identifier is a version of.
fn parents_of(records: &[Session]) -> HashMap<Id, Id> {
    let mut parents = HashMap::new();
    for record in records {
        if let Some(target) = record.replaces.or(record.voids) {
            parents.insert(record.id, target);
        }
    }
    parents
}

/// The first identifier of the chain `record` belongs to.
///
/// When the chain points at a record that is not in the file, the identifier it points at comes
/// back; the caller sees that no record carries it and keeps the correction as an orphan.
fn root_of(record: &Session, parents: &HashMap<Id, Id>) -> Id {
    let Some(mut current) = record.replaces.or(record.voids) else {
        return record.id;
    };
    // A chain cannot be longer than the links that made it, so this walk always ends, even when a
    // damaged file points two records at each other.
    for _ in 0..=parents.len() {
        match parents.get(&current) {
            Some(next) => current = *next,
            None => return current,
        }
    }
    current
}

/// The record of `chain` that counts: the highest revision, then the latest write, then the
/// largest identifier so two machines always agree.
fn newest(chain: &[Session]) -> Option<Session> {
    chain
        .iter()
        .max_by(|left, right| {
            left.revision.cmp(&right.revision).then(left.written.cmp(&right.written)).then(left.id.cmp(&right.id))
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Source;
    use crate::span::{ClockSource, Span, SpanKind};

    fn record(id: Id, revision: u32, written: i64, replaces: Option<Id>, voids: Option<Id>) -> Session {
        Session {
            id,
            revision,
            written,
            focus: Id::new(1, 1),
            started: 1_758_124_800,
            offset_minutes: 0,
            ended: 1_758_124_900,
            spans: vec![Span::new(SpanKind::Work, 0, 100, ClockSource::Mono)],
            source: Source::Timer,
            replaces,
            voids,
            continues: None,
            flags: Vec::new(),
            note: String::new(),
        }
    }

    fn id(n: u128) -> Id {
        Id::new(1_000 + u64::try_from(n).unwrap_or(0), n)
    }

    #[test]
    fn a_single_record_survives() {
        let resolution = resolve(vec![record(id(1), 1, 10, None, None)]);
        assert_eq!(resolution.sessions.len(), 1);
        assert!(resolution.diagnostics.is_empty());
    }

    #[test]
    fn the_highest_revision_wins() {
        let first = record(id(1), 1, 10, None, None);
        let second = record(id(2), 2, 20, Some(id(1)), None);
        let resolution = resolve(vec![first, second.clone()]);
        assert_eq!(resolution.sessions, vec![second]);
    }

    #[test]
    fn order_in_the_file_does_not_matter() {
        let first = record(id(1), 1, 10, None, None);
        let second = record(id(2), 2, 20, Some(id(1)), None);
        let forwards = resolve(vec![first.clone(), second.clone()]);
        let backwards = resolve(vec![second, first]);
        assert_eq!(forwards.sessions, backwards.sessions);
    }

    #[test]
    fn a_later_write_breaks_a_tie_in_revision() {
        let first = record(id(1), 1, 10, None, None);
        let rival = record(id(2), 1, 30, Some(id(1)), None);
        let resolution = resolve(vec![first, rival.clone()]);
        assert_eq!(resolution.sessions, vec![rival]);
        assert_eq!(resolution.forks, vec![id(1)]);
    }

    #[test]
    fn voiding_takes_the_chain_out() {
        let first = record(id(1), 1, 10, None, None);
        let removal = record(id(2), 2, 20, None, Some(id(1)));
        let resolution = resolve(vec![first, removal]);
        assert!(resolution.sessions.is_empty());
    }

    #[test]
    fn an_older_correction_cannot_revive_a_voided_chain() {
        let first = record(id(1), 1, 10, None, None);
        let removal = record(id(3), 3, 30, None, Some(id(1)));
        let stale = record(id(2), 2, 20, Some(id(1)), None);
        let resolution = resolve(vec![first, removal, stale]);
        assert!(resolution.sessions.is_empty(), "{:?}", resolution.sessions);
    }

    #[test]
    fn an_undo_with_a_higher_revision_brings_it_back() {
        let first = record(id(1), 1, 10, None, None);
        let removal = record(id(2), 2, 20, None, Some(id(1)));
        let undo = record(id(3), 3, 30, Some(id(1)), None);
        let resolution = resolve(vec![first, removal, undo.clone()]);
        assert_eq!(resolution.sessions, vec![undo]);
    }

    #[test]
    fn a_voided_chain_keeps_its_last_version_for_the_trash() {
        let first = record(id(1), 1, 10, None, None);
        let fixed = record(id(2), 2, 20, Some(id(1)), None);
        let removal = record(id(3), 3, 30, None, Some(id(2)));
        let resolution = resolve(vec![first, fixed.clone(), removal]);
        assert!(resolution.sessions.is_empty());
        assert_eq!(resolution.voided, vec![fixed]);
    }

    #[test]
    fn a_revived_chain_is_not_in_the_trash() {
        let first = record(id(1), 1, 10, None, None);
        let removal = record(id(2), 2, 20, None, Some(id(1)));
        let undo = record(id(3), 3, 30, Some(id(1)), None);
        let resolution = resolve(vec![first, removal, undo]);
        assert!(resolution.voided.is_empty());
        assert_eq!(resolution.sessions.len(), 1);
    }

    #[test]
    fn the_newest_revision_is_counted_over_the_whole_chain() {
        let first = record(id(1), 1, 10, None, None);
        let removal = record(id(2), 2, 20, None, Some(id(1)));
        let undo = record(id(3), 3, 30, Some(id(1)), None);
        let records = vec![first, removal, undo];
        assert_eq!(newest_revision(&records, id(1)), 3);
        assert_eq!(newest_revision(&records, id(2)), 3);
        assert_eq!(newest_revision(&records, id(3)), 3);
        assert_eq!(newest_revision(&records, id(404)), 0);
    }

    #[test]
    fn every_record_of_a_chain_has_the_same_root() {
        let first = record(id(1), 1, 10, None, None);
        let fixed = record(id(2), 2, 20, Some(id(1)), None);
        let removal = record(id(3), 3, 30, None, Some(id(2)));
        let other = record(id(4), 1, 10, None, None);
        let stray = record(id(5), 2, 20, Some(id(404)), None);
        let roots = roots(&[first, fixed, removal, other, stray]);
        assert_eq!(roots.get(&id(1)), Some(&id(1)));
        assert_eq!(roots.get(&id(2)), Some(&id(1)));
        assert_eq!(roots.get(&id(3)), Some(&id(1)));
        assert_eq!(roots.get(&id(4)), Some(&id(4)));
        assert_eq!(roots.get(&id(5)), Some(&id(404)), "an orphan's root is the record that is not here");
    }

    #[test]
    fn a_correction_of_nothing_is_kept_as_an_orphan() {
        let stray = record(id(9), 2, 20, Some(id(404)), None);
        let resolution = resolve(vec![stray.clone()]);
        assert!(resolution.sessions.is_empty());
        assert_eq!(resolution.orphans, vec![stray]);
        assert_eq!(resolution.diagnostics.len(), 1);
    }
}
