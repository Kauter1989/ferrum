//! Holder records: the JSON file next to each holder's lock file.
//!
//! The format is flat (strings, integers, `null`), so it is written and read
//! here without a JSON library; other languages read it with theirs.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::PROTOCOL;

/// Records larger than this are not read (a live holder whose record cannot
/// be read counts as holding the whole group).
pub const MAX_RECORD_BYTES: u64 = 4096;

/// What a holder uses the card for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A process that stays on the card, such as a model server.
    Resident,
    /// One engine call of FERRUM (or of another client of the group).
    Engine,
}

impl Kind {
    /// Name in the record (`resident`, `engine`).
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Resident => "resident",
            Kind::Engine => "engine",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "resident" => Some(Kind::Resident),
            "engine" => Some(Kind::Engine),
            _ => None,
        }
    }
}

/// A live holder of a group, as its record describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HolderInfo {
    /// Name the holder gave itself (`llama-server`, `ferrum: nnInteractive`).
    pub holder: String,
    /// Process id of the holder.
    pub pid: u32,
    /// What the holder uses the card for.
    pub kind: Kind,
    /// Leased GPU memory in MiB; `None` holds the whole group.
    pub lease_mib: Option<u64>,
    /// When the lease started, in seconds since the Unix epoch.
    pub since_unix_s: u64,
}

impl HolderInfo {
    /// A record for the current process.
    pub fn now(holder: &str, kind: Kind, lease_mib: Option<u64>) -> Self {
        let since_unix_s = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Self { holder: holder.to_owned(), pid: std::process::id(), kind, lease_mib, since_unix_s }
    }

    /// The record a live holder has when its file cannot be read: it holds
    /// the whole group.
    pub fn unreadable() -> Self {
        Self { holder: "unreadable record".into(), pid: 0, kind: Kind::Resident, lease_mib: None, since_unix_s: 0 }
    }

    /// One phrase for messages: `llama-server (pid 4242) holds 6144 MiB`.
    pub fn describe(&self) -> String {
        let what = match self.lease_mib {
            Some(m) => format!("holds {m} MiB"),
            None => "holds the whole group".to_owned(),
        };
        format!("{} (pid {}) {what}", self.holder, self.pid)
    }

    /// The record as one line of JSON.
    pub fn to_json(&self) -> String {
        let lease = self.lease_mib.map_or("null".to_owned(), |m| m.to_string());
        format!(
            "{{\"protocol\":\"{PROTOCOL}\",\"holder\":{},\"pid\":{},\"kind\":\"{}\",\"lease_mib\":{lease},\"since_unix_s\":{}}}\n",
            quote(&self.holder),
            self.pid,
            self.kind.as_str(),
            self.since_unix_s
        )
    }

    /// Parses a record; `None` for anything that is not a `ferrum-gpu-lock/1`
    /// record with the required fields.
    pub fn from_json(text: &str) -> Option<Self> {
        let fields = parse_flat_object(text)?;
        let get = |k: &str| fields.iter().find(|(name, _)| name == k).map(|(_, v)| v);
        if get("protocol")? != &Value::Str(PROTOCOL.into()) {
            return None;
        }
        let holder = match get("holder")? {
            Value::Str(s) => s.clone(),
            _ => return None,
        };
        let pid = get("pid")?.int().and_then(|p| u32::try_from(p).ok())?;
        let kind = match get("kind")? {
            Value::Str(s) => Kind::parse(s)?,
            _ => return None,
        };
        let lease_mib = match get("lease_mib")? {
            Value::Null => None,
            v => Some(v.int()?),
        };
        let since_unix_s = get("since_unix_s").and_then(Value::int).unwrap_or(0);
        Some(Self { holder, pid, kind, lease_mib, since_unix_s })
    }
}

/// Text of all holders for messages, `; `-separated.
pub fn describe_holders(held: &[HolderInfo]) -> String {
    held.iter().map(HolderInfo::describe).collect::<Vec<_>>().join("; ")
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Str(String),
    Int(u64),
    Null,
}

impl Value {
    fn int(&self) -> Option<u64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }
}

/// Parses `{"key": value, …}` where values are strings, non-negative
/// integers or `null`.
fn parse_flat_object(text: &str) -> Option<Vec<(String, Value)>> {
    let mut p = Parser { chars: text.trim().chars().peekable() };
    p.expect('{')?;
    let mut fields = Vec::new();
    p.skip_ws();
    if p.chars.peek() == Some(&'}') {
        p.chars.next();
        return p.at_end().then_some(fields);
    }
    loop {
        p.skip_ws();
        let key = p.string()?;
        p.skip_ws();
        p.expect(':')?;
        p.skip_ws();
        fields.push((key, p.value()?));
        p.skip_ws();
        match p.chars.next()? {
            ',' => continue,
            '}' => return p.at_end().then_some(fields),
            _ => return None,
        }
    }
}

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.chars.next_if(|c| c.is_whitespace()).is_some() {}
    }

    fn expect(&mut self, c: char) -> Option<()> {
        (self.chars.next()? == c).then_some(())
    }

    fn at_end(&mut self) -> bool {
        self.skip_ws();
        self.chars.peek().is_none()
    }

    fn value(&mut self) -> Option<Value> {
        match *self.chars.peek()? {
            '"' => self.string().map(Value::Str),
            'n' => {
                for c in "null".chars() {
                    self.expect(c)?;
                }
                Some(Value::Null)
            }
            c if c.is_ascii_digit() => {
                let mut n: u64 = 0;
                while let Some(d) = self.chars.next_if(char::is_ascii_digit) {
                    n = n.checked_mul(10)?.checked_add(u64::from(d.to_digit(10)?))?;
                }
                Some(Value::Int(n))
            }
            _ => None,
        }
    }

    fn string(&mut self) -> Option<String> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            match self.chars.next()? {
                '"' => return Some(out),
                '\\' => out.push(self.escape()?),
                c => out.push(c),
            }
        }
    }

    fn escape(&mut self) -> Option<char> {
        Some(match self.chars.next()? {
            '"' => '"',
            '\\' => '\\',
            '/' => '/',
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'u' => {
                let hex: String = (0..4).map(|_| self.chars.next()).collect::<Option<_>>()?;
                char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
            }
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // covers 23.1-a
    #[test]
    fn record_round_trips_and_names_no_study() {
        let h = HolderInfo {
            holder: "llama \"server\"\n".into(),
            pid: 4242,
            kind: Kind::Resident,
            lease_mib: Some(6144),
            since_unix_s: 1_761_231_600,
        };
        let json = h.to_json();
        assert_eq!(
            json,
            "{\"protocol\":\"ferrum-gpu-lock/1\",\"holder\":\"llama \\\"server\\\"\\u000a\",\"pid\":4242,\"kind\":\"resident\",\"lease_mib\":6144,\"since_unix_s\":1761231600}\n"
        );
        assert_eq!(HolderInfo::from_json(&json), Some(h.clone()));
        let whole = HolderInfo { lease_mib: None, kind: Kind::Engine, ..h };
        assert_eq!(HolderInfo::from_json(&whole.to_json()), Some(whole));
    }

    #[test]
    fn foreign_or_broken_records_are_not_read() {
        for bad in [
            "",
            "{}",
            "{\"protocol\":\"ferrum-gpu-lock/2\",\"holder\":\"x\",\"pid\":1,\"kind\":\"engine\",\"lease_mib\":null}",
            "{\"protocol\":\"ferrum-gpu-lock/1\",\"holder\":\"x\",\"pid\":1,\"kind\":\"gpu\",\"lease_mib\":1}",
            "{\"protocol\":\"ferrum-gpu-lock/1\",\"holder\":\"x\",\"pid\":-1,\"kind\":\"engine\",\"lease_mib\":1}",
            "{\"protocol\":\"ferrum-gpu-lock/1\",\"holder\":\"x\",\"pid\":1,\"kind\":\"engine\",\"lease_mib\":1} trailing",
            "{\"protocol\":\"ferrum-gpu-lock/1\",\"holder\":\"x\",\"pid\":99999999999999999999999,\"kind\":\"engine\",\"lease_mib\":1}",
        ] {
            assert_eq!(HolderInfo::from_json(bad), None, "{bad}");
        }
        // a pretty-printed record by another language reads the same
        let pretty = "{\n  \"protocol\": \"ferrum-gpu-lock/1\",\n  \"holder\": \"py\",\n  \"pid\": 7,\n  \"kind\": \"resident\",\n  \"lease_mib\": 100\n}\n";
        assert_eq!(
            HolderInfo::from_json(pretty).map(|h| (h.pid, h.lease_mib, h.since_unix_s)),
            Some((7, Some(100), 0))
        );
    }

    #[test]
    fn holders_are_described_for_messages() {
        let a = HolderInfo {
            holder: "llama-server".into(),
            pid: 1,
            kind: Kind::Resident,
            lease_mib: Some(6144),
            since_unix_s: 0,
        };
        let b = HolderInfo { holder: "ferrum".into(), pid: 2, kind: Kind::Engine, lease_mib: None, since_unix_s: 0 };
        assert_eq!(
            describe_holders(&[a, b]),
            "llama-server (pid 1) holds 6144 MiB; ferrum (pid 2) holds the whole group"
        );
    }
}
