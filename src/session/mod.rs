//! Sessions: what a person worked on, when, and for how long.

pub mod edit;
pub mod line;
pub mod resolve;

use crate::id::Id;
use crate::span::{Span, work_seconds};

/// Where a session came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Measured by the running timer.
    Timer,
    /// Typed in by hand after the fact.
    Manual,
    /// Brought back after the program stopped while the timer was running.
    Recovered,
    /// A correction of an earlier record.
    Edited,
}

impl Source {
    /// The word written in a file.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timer => "timer",
            Self::Manual => "manual",
            Self::Recovered => "recovered",
            Self::Edited => "edited",
        }
    }

    /// Reads the word written in a file.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "timer" => Some(Self::Timer),
            "manual" => Some(Self::Manual),
            "recovered" => Some(Self::Recovered),
            "edited" => Some(Self::Edited),
            _ => None,
        }
    }
}

/// Something worth saying about a session besides its time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    /// The system clock moved while the session ran, so its stamps may be off.
    SuspectClock,
    /// The session ran longer than the ceiling and has not been confirmed.
    OverCeiling,
    /// It holds idle time the person has not said what to do with.
    UnclaimedIdle,
}

impl Flag {
    /// The word written in a file.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SuspectClock => "suspect-clock",
            Self::OverCeiling => "over-ceiling",
            Self::UnclaimedIdle => "unclaimed-idle",
        }
    }

    /// Reads the word written in a file.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "suspect-clock" => Some(Self::SuspectClock),
            "over-ceiling" => Some(Self::OverCeiling),
            "unclaimed-idle" => Some(Self::UnclaimedIdle),
            _ => None,
        }
    }
}

/// One recorded stretch of work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Its own identifier.
    pub id: Id,
    /// Which version of this record it is; the highest one wins.
    pub revision: u32,
    /// When the record was written, in seconds since the Unix epoch.
    pub written: i64,
    /// The focus it belongs to.
    pub focus: Id,
    /// When it began, in seconds since the Unix epoch.
    pub started: i64,
    /// Minutes the local time was ahead of UTC when it began.
    pub offset_minutes: i16,
    /// When it ended, in seconds since the Unix epoch.
    pub ended: i64,
    /// What happened during it, in order.
    pub spans: Vec<Span>,
    /// Where it came from.
    pub source: Source,
    /// The record this one replaces, if any.
    pub replaces: Option<Id>,
    /// The record this one takes out of the lists, if any.
    pub voids: Option<Id>,
    /// The record this one carries on from, when a session was split.
    pub continues: Option<Id>,
    /// Anything worth saying about it.
    pub flags: Vec<Flag>,
    /// A line the person wrote.
    pub note: String,
}

impl Session {
    /// Seconds of work it holds. Breaks, sleep and unclaimed idle time are not counted.
    #[must_use]
    pub fn work_seconds(&self) -> u64 {
        work_seconds(&self.spans)
    }
}
