//! Who created an annotation or segment, and whether a person confirmed it.
//!
//! Results of agents and segmentation engines are *proposals* until a
//! clinician reviews them. Every annotation and segment carries a
//! [`Provenance`] so that a proposal is never mistaken for a finding
//! (see `docs/agent-skill.md` §10).

use std::fmt;

/// Seconds since the Unix epoch (UTC). Written as RFC 3339
/// (`2026-10-01T12:00:00Z`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// The current time of the system clock.
    pub fn now() -> Self {
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        Self(since.map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX)).unwrap_or(0))
    }

    /// RFC 3339 text in UTC, e.g. `2026-10-01T12:00:00Z`.
    pub fn to_rfc3339(self) -> String {
        let days = self.0.div_euclid(86_400);
        let secs = self.0.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
    }

    /// Parses RFC 3339 (`YYYY-MM-DDTHH:MM:SS[.fff](Z|±HH:MM)`); fractions of
    /// a second are dropped.
    pub fn parse_rfc3339(text: &str) -> Option<Self> {
        let b = text.as_bytes();
        if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
            return None;
        }
        if b[13] != b':' || b[16] != b':' {
            return None;
        }
        let num = |r: std::ops::Range<usize>| text.get(r)?.parse::<i64>().ok();
        let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
        let (h, mi, s) = (num(11..13)?, num(14..16)?, num(17..19)?);
        if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
            return None;
        }
        let mut rest = &text[19..];
        if let Some(frac) = rest.strip_prefix('.') {
            let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            rest = &frac[digits..];
        }
        let offset = match rest {
            "Z" | "z" => 0,
            _ if rest.len() == 6 && rest.as_bytes()[3] == b':' => {
                let sign = match rest.as_bytes()[0] {
                    b'+' => 1,
                    b'-' => -1,
                    _ => return None,
                };
                let oh = rest[1..3].parse::<i64>().ok()?;
                let om = rest[4..6].parse::<i64>().ok()?;
                sign * (oh * 3600 + om * 60)
            }
            _ => return None,
        };
        Some(Self(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s - offset))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`].
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Who created an item.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Author {
    /// A person working in the viewer.
    #[default]
    Human,
    /// An AI agent driving FERRUM through the agent interface.
    Agent {
        /// Agent or session id given by the harness, if any.
        id: Option<String>,
    },
    /// A segmentation engine (`ferrum-engine/1`).
    Engine {
        /// Engine name, e.g. `nnInteractive`.
        name: String,
        /// Engine version.
        version: String,
        /// The engine's licence restricts it to research use.
        research_only: bool,
    },
}

impl Author {
    /// `"human"`, `"agent"` or `"engine"`.
    pub fn kind(&self) -> &'static str {
        match self {
            Author::Human => "human",
            Author::Agent { .. } => "agent",
            Author::Engine { .. } => "engine",
        }
    }

    /// Short description for lists, e.g. `engine nnInteractive 2.6`.
    pub fn describe(&self) -> String {
        match self {
            Author::Human => "human".into(),
            Author::Agent { id: Some(id) } => format!("agent {id}"),
            Author::Agent { id: None } => "agent".into(),
            Author::Engine { name, version, .. } if version.is_empty() => format!("engine {name}"),
            Author::Engine { name, version, .. } => format!("engine {name} {version}"),
        }
    }
}

/// Review state of an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ReviewStatus {
    /// Created by an agent or engine, not reviewed yet.
    Proposed,
    /// Confirmed by a person (or drawn by one).
    #[default]
    Confirmed,
    /// Rejected by a person; kept for the record.
    Rejected,
}

impl ReviewStatus {
    /// `"proposed"`, `"confirmed"` or `"rejected"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewStatus::Proposed => "proposed",
            ReviewStatus::Confirmed => "confirmed",
            ReviewStatus::Rejected => "rejected",
        }
    }

    /// Inverse of [`ReviewStatus::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "proposed" => Some(ReviewStatus::Proposed),
            "confirmed" => Some(ReviewStatus::Confirmed),
            "rejected" => Some(ReviewStatus::Rejected),
            _ => None,
        }
    }
}

/// Origin and review state of an annotation or segment.
///
/// The default is an item drawn by a person in the viewer: author
/// [`Author::Human`], status [`ReviewStatus::Confirmed`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Provenance {
    /// Who created the item.
    pub author: Author,
    /// Review state.
    pub status: ReviewStatus,
    /// Creation time, if known.
    pub created: Option<Timestamp>,
    /// Who confirmed or rejected the item.
    pub reviewed_by: Option<String>,
    /// When the item was confirmed or rejected.
    pub reviewed: Option<Timestamp>,
}

impl Provenance {
    /// Drawn by a person at `created`; confirmed.
    pub fn human(created: Timestamp) -> Self {
        Self { created: Some(created), ..Self::default() }
    }

    /// Proposed by an agent at `created`.
    pub fn agent(id: Option<String>, created: Timestamp) -> Self {
        Self::proposal(Author::Agent { id }, created)
    }

    /// Proposed by a segmentation engine at `created`.
    pub fn engine(name: &str, version: &str, research_only: bool, created: Timestamp) -> Self {
        let author = Author::Engine { name: name.to_owned(), version: version.to_owned(), research_only };
        Self::proposal(author, created)
    }

    fn proposal(author: Author, created: Timestamp) -> Self {
        Self { author, status: ReviewStatus::Proposed, created: Some(created), reviewed_by: None, reviewed: None }
    }

    /// Records a review decision by `by` at `at`. A decision of
    /// [`ReviewStatus::Proposed`] reopens the item and clears the reviewer.
    pub fn review(&mut self, status: ReviewStatus, by: Option<&str>, at: Timestamp) {
        self.status = status;
        if status == ReviewStatus::Proposed {
            self.reviewed_by = None;
            self.reviewed = None;
        } else {
            self.reviewed_by = by.map(str::to_owned);
            self.reviewed = Some(at);
        }
    }

    /// `true` while the item waits for review.
    pub fn is_pending(&self) -> bool {
        self.status == ReviewStatus::Proposed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_round_trip() {
        assert_eq!(Timestamp(0).to_rfc3339(), "1970-01-01T00:00:00Z");
        let t = Timestamp::parse_rfc3339("2026-10-01T12:34:56Z").unwrap();
        assert_eq!(t.to_rfc3339(), "2026-10-01T12:34:56Z");
        assert_eq!(t.0, 1_790_858_096);
        assert_eq!(Timestamp::parse_rfc3339("2026-10-01T14:34:56.123+02:00"), Some(t));
        assert_eq!(Timestamp::parse_rfc3339("2024-02-29T00:00:00Z").unwrap().to_string(), "2024-02-29T00:00:00Z");
        assert_eq!(Timestamp(-1).to_rfc3339(), "1969-12-31T23:59:59Z");
        for bad in [
            "",
            "2026-10-01",
            "2026-13-01T00:00:00Z",
            "2026-10-01T00:00:00",
            "2026-10-01T00:00:00.Z",
            "x026-10-01T00:00:00Z",
            "2026-10-01T00:00:00*01:00",
        ] {
            assert_eq!(Timestamp::parse_rfc3339(bad), None, "{bad}");
        }
        assert!(Timestamp::now().0 > 1_700_000_000);
    }

    #[test]
    fn proposals_and_review() {
        let t = Timestamp(100);
        let human = Provenance::human(t);
        assert_eq!((human.author.kind(), human.status, human.is_pending()), ("human", ReviewStatus::Confirmed, false));
        let mut p = Provenance::engine("nnInteractive", "2.6", true, t);
        assert!(p.is_pending());
        assert_eq!(p.author.describe(), "engine nnInteractive 2.6");
        p.review(ReviewStatus::Confirmed, Some("dr.k"), Timestamp(200));
        assert_eq!(
            (p.status, p.reviewed_by.as_deref(), p.reviewed),
            (ReviewStatus::Confirmed, Some("dr.k"), Some(Timestamp(200)))
        );
        p.review(ReviewStatus::Proposed, Some("x"), Timestamp(300));
        assert_eq!((p.reviewed_by, p.reviewed), (None, None));
        let a = Provenance::agent(Some("run-7".into()), t);
        assert_eq!((a.author.kind(), a.author.describe()), ("agent", "agent run-7".to_string()));
        assert_eq!(Provenance::agent(None, t).author.describe(), "agent");
        assert_eq!(
            Author::Engine { name: "X".into(), version: String::new(), research_only: false }.describe(),
            "engine X"
        );
        for s in [ReviewStatus::Proposed, ReviewStatus::Confirmed, ReviewStatus::Rejected] {
            assert_eq!(ReviewStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(ReviewStatus::parse("maybe"), None);
    }
}
