//! Detect which auth strategy the embedded Claude Code CLI will use.
//!
//! Claude Code resolves auth at invocation time by inspecting a few
//! environment variables, falling back to credentials stored under
//! `~/.claude/` when none are set. This module mirrors that
//! precedence as a cheap, sync, env-only check so hosts can introspect
//! the active mode before spawning a turn.
//!
//! It is **not** a liveness check -- a reported [`AuthStrategy::Subscription`]
//! only means "no env auth set"; the user might not have run
//! `claude login` yet. Use the `claude auth status` CLI for that.
//!
//! # Precedence
//!
//! 1. `CLAUDE_CODE_USE_BEDROCK` truthy -> [`AuthStrategy::Bedrock`]
//! 2. `CLAUDE_CODE_USE_VERTEX` truthy -> [`AuthStrategy::Vertex`]
//! 3. `ANTHROPIC_API_KEY` non-empty -> [`AuthStrategy::ApiKey`]
//! 4. `CLAUDE_CODE_OAUTH_TOKEN` non-empty -> [`AuthStrategy::OauthToken`]
//! 5. Otherwise -> [`AuthStrategy::Subscription`]
//!
//! Cloud-provider strategies (Bedrock, Vertex) take precedence because
//! they redirect ALL traffic regardless of API key presence.
//!
//! # Example
//!
//! ```
//! use claude_wrapper::auth;
//!
//! let summary = auth::detect();
//! println!("strategy: {:?}", summary.strategy);
//! if summary.has_anthropic_api_key {
//!     println!("note: ANTHROPIC_API_KEY is set in the environment");
//! }
//! ```

use std::collections::HashMap;

use serde::Serialize;

/// Active auth strategy, as inferred from the host environment.
///
/// See module-level docs for precedence rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthStrategy {
    /// `CLAUDE_CODE_USE_BEDROCK` is truthy. Requests are routed to
    /// AWS Bedrock; AWS credentials are resolved separately by the
    /// Bedrock SDK from the host environment.
    Bedrock,
    /// `CLAUDE_CODE_USE_VERTEX` is truthy. Requests are routed to
    /// Google Vertex; GCP credentials are resolved separately.
    Vertex,
    /// `ANTHROPIC_API_KEY` is set. Direct API access, billed to that key.
    ApiKey,
    /// `CLAUDE_CODE_OAUTH_TOKEN` is set. OAuth token (typically from
    /// `claude setup-token`).
    OauthToken,
    /// No auth env var set. The CLI will look for stored credentials
    /// under `~/.claude/` (the result of an interactive `claude login`).
    /// May or may not actually be authenticated -- this strategy
    /// reports "the env doesn't pin anything," not "you are logged in."
    Subscription,
}

impl AuthStrategy {
    /// Stable string label, useful for logs and protocol payloads.
    /// Matches the `serde_json` representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bedrock => "bedrock",
            Self::Vertex => "vertex",
            Self::ApiKey => "api_key",
            Self::OauthToken => "oauth_token",
            Self::Subscription => "subscription",
        }
    }
}

/// Snapshot of auth-relevant environment state. Returned by [`detect`]
/// so callers see both the resolved strategy and the raw signals that
/// drove the decision.
#[derive(Debug, Clone, Serialize)]
pub struct AuthSummary {
    /// The strategy the CLI will pick under the current env.
    pub strategy: AuthStrategy,
    /// Whether `ANTHROPIC_API_KEY` is set and non-empty.
    pub has_anthropic_api_key: bool,
    /// Whether `CLAUDE_CODE_OAUTH_TOKEN` is set and non-empty.
    pub has_oauth_token: bool,
    /// Whether `CLAUDE_CODE_USE_BEDROCK` is truthy (`1`, `true`, etc.).
    pub bedrock_enabled: bool,
    /// Whether `CLAUDE_CODE_USE_VERTEX` is truthy.
    pub vertex_enabled: bool,
}

/// Detect the active auth strategy from the current process
/// environment. Cheap; no subprocess, no filesystem reads.
pub fn detect() -> AuthSummary {
    let env: HashMap<String, String> = std::env::vars().collect();
    detect_from(&env)
}

/// Same as [`detect`] but reads from a caller-provided env map.
/// Exposed for tests and for hosts that want to introspect a child
/// environment they're about to spawn under.
pub fn detect_from(env: &HashMap<String, String>) -> AuthSummary {
    let bedrock_enabled = is_truthy(env.get("CLAUDE_CODE_USE_BEDROCK").map(String::as_str));
    let vertex_enabled = is_truthy(env.get("CLAUDE_CODE_USE_VERTEX").map(String::as_str));
    let has_anthropic_api_key = is_set(env.get("ANTHROPIC_API_KEY").map(String::as_str));
    let has_oauth_token = is_set(env.get("CLAUDE_CODE_OAUTH_TOKEN").map(String::as_str));

    let strategy = if bedrock_enabled {
        AuthStrategy::Bedrock
    } else if vertex_enabled {
        AuthStrategy::Vertex
    } else if has_anthropic_api_key {
        AuthStrategy::ApiKey
    } else if has_oauth_token {
        AuthStrategy::OauthToken
    } else {
        AuthStrategy::Subscription
    };

    AuthSummary {
        strategy,
        has_anthropic_api_key,
        has_oauth_token,
        bedrock_enabled,
        vertex_enabled,
    }
}

/// Treat any non-empty, non-whitespace value as "set."
fn is_set(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.trim().is_empty())
}

/// Truthy env var: any non-empty value that isn't a recognized falsy
/// literal (`0`, `false`, `no`, case-insensitive). Mirrors the loose
/// convention most CLI tools follow for `XYZ_USE_FOO` toggles.
fn is_truthy(value: Option<&str>) -> bool {
    let Some(v) = value else { return false };
    let trimmed = v.trim();
    if trimmed.is_empty() {
        return false;
    }
    !matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn empty_env_is_subscription() {
        let s = detect_from(&env(&[]));
        assert_eq!(s.strategy, AuthStrategy::Subscription);
        assert!(!s.has_anthropic_api_key);
        assert!(!s.has_oauth_token);
        assert!(!s.bedrock_enabled);
        assert!(!s.vertex_enabled);
    }

    #[test]
    fn api_key_takes_precedence_over_oauth_token() {
        let s = detect_from(&env(&[
            ("ANTHROPIC_API_KEY", "sk-abc"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "tok-xyz"),
        ]));
        assert_eq!(s.strategy, AuthStrategy::ApiKey);
        assert!(s.has_anthropic_api_key);
        assert!(s.has_oauth_token);
    }

    #[test]
    fn oauth_token_alone_picks_oauth() {
        let s = detect_from(&env(&[("CLAUDE_CODE_OAUTH_TOKEN", "tok-xyz")]));
        assert_eq!(s.strategy, AuthStrategy::OauthToken);
        assert!(!s.has_anthropic_api_key);
        assert!(s.has_oauth_token);
    }

    #[test]
    fn bedrock_overrides_api_key() {
        let s = detect_from(&env(&[
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("ANTHROPIC_API_KEY", "sk-abc"),
        ]));
        assert_eq!(s.strategy, AuthStrategy::Bedrock);
        assert!(s.bedrock_enabled);
        assert!(s.has_anthropic_api_key);
    }

    #[test]
    fn vertex_overrides_oauth_token() {
        let s = detect_from(&env(&[
            ("CLAUDE_CODE_USE_VERTEX", "true"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "tok-xyz"),
        ]));
        assert_eq!(s.strategy, AuthStrategy::Vertex);
        assert!(s.vertex_enabled);
    }

    #[test]
    fn bedrock_takes_precedence_over_vertex_when_both_set() {
        let s = detect_from(&env(&[
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("CLAUDE_CODE_USE_VERTEX", "1"),
        ]));
        assert_eq!(s.strategy, AuthStrategy::Bedrock);
        assert!(s.bedrock_enabled);
        assert!(s.vertex_enabled);
    }

    #[test]
    fn empty_string_does_not_count_as_set() {
        let s = detect_from(&env(&[
            ("ANTHROPIC_API_KEY", ""),
            ("CLAUDE_CODE_OAUTH_TOKEN", "   "),
        ]));
        assert_eq!(s.strategy, AuthStrategy::Subscription);
    }

    #[test]
    fn explicit_falsy_disables_provider_flag() {
        let s = detect_from(&env(&[
            ("CLAUDE_CODE_USE_BEDROCK", "0"),
            ("CLAUDE_CODE_USE_VERTEX", "false"),
            ("ANTHROPIC_API_KEY", "sk-abc"),
        ]));
        assert_eq!(s.strategy, AuthStrategy::ApiKey);
        assert!(!s.bedrock_enabled);
        assert!(!s.vertex_enabled);
    }

    #[test]
    fn truthy_values_recognized() {
        for v in ["1", "true", "TRUE", "yes", "on", "anything"] {
            let s = detect_from(&env(&[("CLAUDE_CODE_USE_BEDROCK", v)]));
            assert_eq!(s.strategy, AuthStrategy::Bedrock, "value {v:?}");
        }
    }

    #[test]
    fn falsy_values_recognized() {
        for v in ["0", "false", "FALSE", "no", "off"] {
            let s = detect_from(&env(&[("CLAUDE_CODE_USE_BEDROCK", v)]));
            assert_eq!(s.strategy, AuthStrategy::Subscription, "value {v:?}");
            assert!(!s.bedrock_enabled, "value {v:?}");
        }
    }

    #[test]
    fn as_str_matches_serde_repr() {
        assert_eq!(AuthStrategy::Bedrock.as_str(), "bedrock");
        assert_eq!(AuthStrategy::Vertex.as_str(), "vertex");
        assert_eq!(AuthStrategy::ApiKey.as_str(), "api_key");
        assert_eq!(AuthStrategy::OauthToken.as_str(), "oauth_token");
        assert_eq!(AuthStrategy::Subscription.as_str(), "subscription");

        // serde_json serialization must match -- this is the value we
        // ship over MCP, so don't let it drift from as_str.
        for s in [
            AuthStrategy::Bedrock,
            AuthStrategy::Vertex,
            AuthStrategy::ApiKey,
            AuthStrategy::OauthToken,
            AuthStrategy::Subscription,
        ] {
            let json = serde_json::to_string(&s).expect("serialize");
            assert_eq!(json, format!("\"{}\"", s.as_str()));
        }
    }
}
