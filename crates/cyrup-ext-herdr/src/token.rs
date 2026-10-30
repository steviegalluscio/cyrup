//! The run identity the bridge keys attention on — an opaque token, not a domain type.
//!
//! The bridge once lived inside `cyrup-ext-subagents` and keyed its attention map on that crate's
//! [`background::RunId`](https://example.invalid). It is now an always-on surface, attached whether
//! or not subagents delegation is installed, so it cannot name a subagents type: the identity has
//! to be whatever the feeding companion already is. A token is that, and nothing more — the bridge
//! never parses it, never orders it meaningfully, and never renders it.
//!
//! Ordering matters once, and only once: [`crate::state::StateModel`]'s attention list is
//! **insertion-ordered** (pi's JS `Map`, `herdr-status.ts:265`), while its `acknowledged` set is a
//! `BTreeSet` and therefore needs `Ord`. The derive below exists for that set alone; nothing sorts
//! runs by token.

/// One run, as the bridge is told about it.
///
/// `Ord` is for [`crate::state::StateModel`]'s acknowledgement set only — see this module's doc for
/// why the attention list must not be sorted by it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunToken(String);

impl RunToken {
    /// The token a companion knows this run by, carried verbatim.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The token's text, for a caller that has to print or compare it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for RunToken {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for RunToken {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::fmt::Display for RunToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// The one thing this type is load-bearing for: it round-trips the token a companion passed,
    /// so an acknowledgement for run A never matches run B.
    #[test]
    fn distinct_tokens_stay_distinct() {
        let a = RunToken::new("run-a");
        let b = RunToken::new("run-b");
        assert_ne!(a, b);
        assert_eq!(a, RunToken::from("run-a".to_string()));
        assert_eq!(a.as_str(), "run-a");
    }
}
