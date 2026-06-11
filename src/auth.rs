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

/// Best-effort classification of an auth-related CLI failure.
///
/// Returned by [`classify_failure`]. Hosts can use it to surface a
/// cleaner message ("run `claude login`") instead of dumping CLI
/// stderr, or to skip retry policies on errors that won't resolve
/// on their own.
///
/// Conservative on purpose: false positives turn legitimate non-auth
/// failures into "auth error" surprises, so the classifier prefers
/// to miss an auth error than to misclassify a non-auth one. Use
/// [`AuthErrorKind::Other`] only when stronger signals (HTTP status
/// strings, the literal word "auth") fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthErrorKind {
    /// No credentials at all -- the user has not run `claude login`
    /// and has no env auth set. Fix: run `claude login` or set one
    /// of the env vars listed in [`AuthStrategy`].
    NotAuthenticated,
    /// Stored OAuth/session credentials existed but are expired.
    /// Fix: re-run `claude login`.
    Expired,
    /// Credentials were presented but rejected. Most often a wrong
    /// or revoked `ANTHROPIC_API_KEY`, or an `CLAUDE_CODE_OAUTH_TOKEN`
    /// that no longer maps to a valid session.
    InvalidCredentials,
    /// Authenticated but the request was rejected for rate limit /
    /// quota / billing reasons. Different remediation: wait, top up,
    /// or switch keys -- not "log in again."
    RateLimit,
    /// Bedrock or Vertex provider error (cloud creds missing or
    /// rejected). Distinct because the fix lives in the cloud
    /// provider's auth, not in `claude login`.
    ProviderError,
    /// Looked auth-shaped (HTTP 401/403, the word "auth", etc.) but
    /// didn't match any of the more specific patterns. Useful for
    /// callers that want "is this an auth thing?" without needing
    /// to know the exact subcategory.
    Other,
}

impl AuthErrorKind {
    /// Stable string label, useful for logs and protocol payloads.
    /// Matches the `serde_json` representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotAuthenticated => "not_authenticated",
            Self::Expired => "expired",
            Self::InvalidCredentials => "invalid_credentials",
            Self::RateLimit => "rate_limit",
            Self::ProviderError => "provider_error",
            Self::Other => "other",
        }
    }
}

/// Inspect a failed `claude` invocation and decide whether it looks
/// auth-shaped. Returns `Some(kind)` only when the patterns are
/// confident enough to risk relabeling.
///
/// `exit_code`, `stdout`, and `stderr` come from the CLI's exit; the
/// classifier matches against the lowercased concatenation. The
/// patterns are intentionally narrow:
///
/// - "may not exist" / "may not have access" / "not_found_error" /
///   "issue with the selected model" -> NOT auth (`None`): a
///   model-not-found / model-access failure is a bad `--model`, not a
///   credential problem, even when it carries a 403/404 status.
/// - "not authenticated" / "not logged in" / "claude login" /
///   "please run /login" / "run /login" / "no credentials" / "no auth"
///   -> [`AuthErrorKind::NotAuthenticated`]
/// - "expired" / "session has expired" / "token expired" -> [`AuthErrorKind::Expired`]
/// - "invalid api key" / "invalid token" / "401" / "unauthorized" / "403"
///   / "forbidden" -> [`AuthErrorKind::InvalidCredentials`]
/// - "rate limit" / "quota" / "too many requests" / "429" -> [`AuthErrorKind::RateLimit`]
/// - "bedrock" or "vertex" present alongside an auth signal -> [`AuthErrorKind::ProviderError`]
/// - bare "auth" / "credential" hit with nothing more specific -> [`AuthErrorKind::Other`]
pub fn classify_failure(_exit_code: i32, stdout: &str, stderr: &str) -> Option<AuthErrorKind> {
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();

    // Model-not-found / model-access failures are NOT auth errors,
    // even though they can carry a 403/404 HTTP status that the
    // checks below would otherwise read as `InvalidCredentials`. A
    // bad `--model` id is a typo, not a credential problem; relabeling
    // it as auth makes downstream consumers (e.g. roba, which maps
    // `Error::Auth` to a fleet-halting exit code) treat a model typo
    // like a credential failure. Bail to `None` (generic
    // `CommandFailed`) before any auth pattern can fire.
    let mentions_model_problem = combined.contains("may not exist")
        || combined.contains("may not have access")
        || combined.contains("not_found_error")
        || combined.contains("issue with the selected model");
    if mentions_model_problem {
        return None;
    }

    // Provider hits (Bedrock / Vertex) take precedence when the
    // failure mentions them alongside an auth signal -- the fix is
    // different (cloud creds, not `claude login`).
    let mentions_provider = combined.contains("bedrock") || combined.contains("vertex");
    let mentions_auth_signal = combined.contains("auth")
        || combined.contains("credential")
        || combined.contains("401")
        || combined.contains("403")
        || combined.contains("forbidden")
        || combined.contains("unauthorized");
    if mentions_provider && mentions_auth_signal {
        return Some(AuthErrorKind::ProviderError);
    }

    if combined.contains("rate limit")
        || combined.contains("too many requests")
        || combined.contains("429")
        || combined.contains("quota")
    {
        return Some(AuthErrorKind::RateLimit);
    }

    if combined.contains("expired")
        || combined.contains("session has expired")
        || combined.contains("token expired")
    {
        return Some(AuthErrorKind::Expired);
    }

    if combined.contains("invalid api key")
        || combined.contains("invalid token")
        || combined.contains("401")
        || combined.contains("unauthorized")
        || combined.contains("403")
        || combined.contains("forbidden")
    {
        return Some(AuthErrorKind::InvalidCredentials);
    }

    if combined.contains("not authenticated")
        || combined.contains("not logged in")
        || combined.contains("claude login")
        || combined.contains("please run /login")
        || combined.contains("run /login")
        || combined.contains("no credentials")
        || combined.contains("no auth")
    {
        return Some(AuthErrorKind::NotAuthenticated);
    }

    // Last-resort bucket: a bare "auth" or "credential" mention
    // without specifics. Conservative: only fires when the word is
    // present in stderr (where these errors typically land) so we
    // don't catch `--allowed-tools auth_helper` or similar.
    if stderr.to_ascii_lowercase().contains("auth")
        || stderr.to_ascii_lowercase().contains("credential")
    {
        return Some(AuthErrorKind::Other);
    }

    None
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

    // -- classify_failure ---------------------------------------------

    #[test]
    fn classify_returns_none_for_unrelated_failure() {
        assert_eq!(classify_failure(1, "no match found", ""), None);
        assert_eq!(
            classify_failure(2, "", "syntax error near unexpected token"),
            None
        );
    }

    #[test]
    fn classify_not_authenticated_from_stderr_hint() {
        assert_eq!(
            classify_failure(1, "", "Not authenticated. Run `claude login` to sign in."),
            Some(AuthErrorKind::NotAuthenticated)
        );
        assert_eq!(
            classify_failure(1, "", "no credentials configured"),
            Some(AuthErrorKind::NotAuthenticated)
        );
    }

    #[test]
    fn classify_expired_session() {
        assert_eq!(
            classify_failure(1, "", "Your session has expired. Please log in again."),
            Some(AuthErrorKind::Expired)
        );
        assert_eq!(
            classify_failure(1, "", "token expired at 2025-01-01T00:00:00Z"),
            Some(AuthErrorKind::Expired)
        );
    }

    #[test]
    fn classify_invalid_api_key() {
        assert_eq!(
            classify_failure(1, "", "Invalid API key. Check ANTHROPIC_API_KEY."),
            Some(AuthErrorKind::InvalidCredentials)
        );
        assert_eq!(
            classify_failure(1, "", "HTTP 401 Unauthorized"),
            Some(AuthErrorKind::InvalidCredentials)
        );
        assert_eq!(
            classify_failure(1, "", "403 Forbidden"),
            Some(AuthErrorKind::InvalidCredentials)
        );
    }

    #[test]
    fn classify_model_not_found_is_not_auth() {
        // A bad `--model` id comes back with a 404/403 payload but is a
        // typo, not a credential problem. It must not be relabeled as
        // auth (regression for #632: a model typo halted roba fleets).
        // Mirrors real claude `--output-format json` output.
        let bad_model = r#"{"type":"result","is_error":true,"api_error_status":404,"result":"There's an issue with the selected model (totally-not-a-model-xyz). It may not exist or you may not have access to it. Run --model to pick a different model."}"#;
        assert_eq!(classify_failure(1, bad_model, ""), None);
    }

    #[test]
    fn classify_model_access_403_is_not_auth() {
        // Even when a model-access failure carries a 403/forbidden
        // status, the model guard must win over `InvalidCredentials`.
        assert_eq!(
            classify_failure(
                1,
                "",
                "API Error: 403 Forbidden permission_error: you may not have access to model claude-x"
            ),
            None
        );
        assert_eq!(
            classify_failure(1, "", "404 not_found_error: model claude-x does not exist"),
            None
        );
    }

    #[test]
    fn classify_not_logged_in_bare_is_not_authenticated() {
        // `--bare` with no API key prints this to stdout with an empty
        // stderr; it is a genuine missing-credential failure and must
        // surface as auth (regression for #632).
        assert_eq!(
            classify_failure(1, "Not logged in · Please run /login", ""),
            Some(AuthErrorKind::NotAuthenticated)
        );
    }

    #[test]
    fn classify_rate_limit_takes_precedence_over_invalid_creds() {
        // Some API responses include "401-like" wording in their rate
        // limit messages; rate_limit should win.
        assert_eq!(
            classify_failure(1, "", "Rate limit exceeded. Please wait."),
            Some(AuthErrorKind::RateLimit)
        );
        assert_eq!(
            classify_failure(1, "", "HTTP 429 Too Many Requests"),
            Some(AuthErrorKind::RateLimit)
        );
        assert_eq!(
            classify_failure(1, "", "quota exceeded for this account"),
            Some(AuthErrorKind::RateLimit)
        );
    }

    #[test]
    fn classify_provider_error_when_bedrock_plus_auth_signal() {
        assert_eq!(
            classify_failure(
                1,
                "",
                "Bedrock auth failed: AWS credentials not found in chain"
            ),
            Some(AuthErrorKind::ProviderError)
        );
        assert_eq!(
            classify_failure(
                1,
                "",
                "Vertex unauthorized -- check GOOGLE_APPLICATION_CREDENTIALS"
            ),
            Some(AuthErrorKind::ProviderError)
        );
    }

    #[test]
    fn classify_falls_back_to_other_for_bare_auth_mention() {
        assert_eq!(
            classify_failure(1, "", "auth subsystem returned an unexpected error"),
            Some(AuthErrorKind::Other)
        );
    }

    #[test]
    fn classify_does_not_match_auth_in_stdout_only() {
        // The fallback "Other" bucket only fires on stderr -- otherwise
        // a CLI that just happened to print "--allowed-tools auth_helper"
        // would be misclassified.
        assert_eq!(
            classify_failure(0, "auth_helper enabled, all clear", ""),
            None
        );
    }

    #[test]
    fn classify_examines_stdout_for_specific_patterns() {
        // Specific patterns work in either stream -- some CLIs print
        // their auth diagnostics to stdout.
        assert_eq!(
            classify_failure(1, "Invalid API key", ""),
            Some(AuthErrorKind::InvalidCredentials)
        );
    }

    #[test]
    fn auth_error_kind_as_str_matches_serde_repr() {
        for k in [
            AuthErrorKind::NotAuthenticated,
            AuthErrorKind::Expired,
            AuthErrorKind::InvalidCredentials,
            AuthErrorKind::RateLimit,
            AuthErrorKind::ProviderError,
            AuthErrorKind::Other,
        ] {
            let json = serde_json::to_string(&k).expect("serialize");
            assert_eq!(json, format!("\"{}\"", k.as_str()));
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
