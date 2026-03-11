//! Bearer token authentication for HTTP transport.

use std::collections::HashSet;
use std::sync::Arc;

/// Validated set of bearer tokens.
///
/// Wraps an `Arc<HashSet<String>>` for cheap cloning into middleware closures.
#[derive(Clone)]
#[cfg_attr(not(feature = "http"), allow(dead_code))]
pub struct BearerTokens {
    tokens: Arc<HashSet<String>>,
}

#[cfg_attr(not(feature = "http"), allow(dead_code))]
impl BearerTokens {
    /// Create a new token set from the given list.
    pub fn new(tokens: Vec<String>) -> Self {
        Self {
            tokens: Arc::new(tokens.into_iter().collect()),
        }
    }

    /// Check whether the given token is valid.
    pub fn validate(&self, token: &str) -> bool {
        self.tokens.contains(token)
    }

    /// Returns true if no tokens were configured (auth disabled).
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_known_token() {
        let tokens = BearerTokens::new(vec!["sk-test-123".into(), "sk-test-456".into()]);
        assert!(tokens.validate("sk-test-123"));
        assert!(tokens.validate("sk-test-456"));
    }

    #[test]
    fn reject_unknown_token() {
        let tokens = BearerTokens::new(vec!["sk-test-123".into()]);
        assert!(!tokens.validate("sk-wrong"));
        assert!(!tokens.validate(""));
    }

    #[test]
    fn empty_tokens() {
        let tokens = BearerTokens::new(vec![]);
        assert!(tokens.is_empty());
        assert!(!tokens.validate("anything"));
    }

    #[test]
    fn non_empty_tokens() {
        let tokens = BearerTokens::new(vec!["key".into()]);
        assert!(!tokens.is_empty());
    }
}
