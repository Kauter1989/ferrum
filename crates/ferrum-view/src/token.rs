//! The rotating token of the view host (Stage 29, criterion 29.4-a,
//! [ADR 0019](https://github.com/Kauter1989/ferrum/blob/develop/docs/decisions/0019-view-host-rotating-token.md)).
//!
//! A token is valid for a TTL; `POST /api/token/refresh` issues the next one
//! and keeps the previous valid for a short grace window, so requests in flight
//! (and a panel mid-refresh) do not `401`. At most two tokens are live at once;
//! both are compared in constant time. The token never appears in a log line,
//! the workspace or a frame.

use std::time::{Duration, Instant};

use crate::session::ViewError;

/// How long a token is valid (15 minutes).
pub const TOKEN_TTL: Duration = Duration::from_secs(900);

/// How long the previous token stays valid after a rotation (60 seconds).
pub const TOKEN_GRACE: Duration = Duration::from_secs(60);

/// A 32-hex-digit token from the operating system's random source.
pub(crate) fn new_token() -> Result<String, ViewError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| ViewError::Internal(format!("cannot read random bytes: {e}")))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Compares two strings without stopping at the first differing byte.
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The current token and, briefly after a rotation, the previous one.
pub(crate) struct RotatingToken {
    current: String,
    current_expires: Instant,
    previous: Option<(String, Instant)>,
    ttl: Duration,
    grace: Duration,
}

impl RotatingToken {
    /// Issues the first token, valid for `ttl`; a rotation keeps the previous
    /// token valid for `grace`.
    pub fn new(ttl: Duration, grace: Duration) -> Result<Self, ViewError> {
        Ok(Self { current: new_token()?, current_expires: Instant::now() + ttl, previous: None, ttl, grace })
    }

    /// The current token (the one the page cookie carries).
    pub fn current(&self) -> &str {
        &self.current
    }

    /// Seconds until the current token expires.
    pub fn expires_in(&self) -> u64 {
        self.current_expires.saturating_duration_since(Instant::now()).as_secs()
    }

    /// `true` if `t` is the live current token or the previous one still inside
    /// its grace window. An expired token authorizes nothing.
    pub fn accepts(&self, t: &str) -> bool {
        let now = Instant::now();
        if now < self.current_expires && constant_time_eq(t, &self.current) {
            return true;
        }
        self.previous.as_ref().is_some_and(|(p, expires)| now < *expires && constant_time_eq(t, p))
    }

    /// Issues a new token; the current one becomes the previous and stays valid
    /// for the grace window (only while it was still live). Returns the new one.
    pub fn rotate(&mut self) -> Result<String, ViewError> {
        let now = Instant::now();
        let next = new_token()?;
        self.previous = (now < self.current_expires).then(|| (self.current.clone(), now + self.grace));
        self.current = next.clone();
        self.current_expires = now + self.ttl;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // covers 29.4-a
    #[test]
    fn the_current_token_is_accepted_and_a_wrong_one_is_not() {
        let token = RotatingToken::new(TOKEN_TTL, TOKEN_GRACE).unwrap();
        assert!(token.accepts(token.current()));
        assert!(!token.accepts("00000000000000000000000000000000"));
        assert!(!token.accepts(""));
        assert!(token.expires_in() > 0);
    }

    // covers 29.4-a
    #[test]
    fn rotation_keeps_the_previous_token_for_the_grace_window() {
        let mut token = RotatingToken::new(TOKEN_TTL, TOKEN_GRACE).unwrap();
        let old = token.current().to_owned();
        let new = token.rotate().unwrap();
        assert_ne!(old, new);
        assert_eq!(token.current(), new);
        assert!(token.accepts(&new), "the new token works");
        assert!(token.accepts(&old), "the old token works within the grace window");
    }

    // covers 29.4-a
    #[test]
    fn an_expired_token_authorizes_nothing() {
        let mut token = RotatingToken::new(Duration::from_millis(120), Duration::from_millis(60)).unwrap();
        let first = token.current().to_owned();
        std::thread::sleep(Duration::from_millis(250));
        assert!(!token.accepts(&first), "the current token expired");
        // a rotation after expiry does not resurrect the dead token
        let new = token.rotate().unwrap();
        assert!(token.accepts(&new));
        assert!(!token.accepts(&first), "no grace for a token that was already dead");
    }

    // covers 29.4-a
    #[test]
    fn the_grace_window_itself_expires() {
        let mut token = RotatingToken::new(Duration::from_millis(600), Duration::from_millis(120)).unwrap();
        let old = token.current().to_owned();
        token.rotate().unwrap();
        assert!(token.accepts(&old));
        std::thread::sleep(Duration::from_millis(250));
        assert!(!token.accepts(&old), "the grace window passed");
    }
}
