use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Transport type for MCP server connections.
///
/// # Example
///
/// ```
/// use claude_wrapper::Transport;
/// use std::str::FromStr;
///
/// let t = Transport::from_str("stdio").unwrap();
/// assert_eq!(t, Transport::Stdio);
/// assert_eq!(t.to_string(), "stdio");
///
/// let t: Transport = "http".parse().unwrap();
/// assert_eq!(t, Transport::Http);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// Standard I/O transport — server runs as a subprocess.
    Stdio,
    /// HTTP transport — server accessible via URL.
    Http,
    /// Server-Sent Events transport — server accessible via URL with SSE.
    Sse,
}

impl fmt::Display for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdio => write!(f, "stdio"),
            Self::Http => write!(f, "http"),
            Self::Sse => write!(f, "sse"),
        }
    }
}

/// Error returned when parsing an unknown transport string.
#[derive(Debug, Clone, thiserror::Error)]
#[error("unknown transport: {0}")]
pub struct TransportParseError(pub String);

impl FromStr for Transport {
    type Err = TransportParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "stdio" => Ok(Self::Stdio),
            "http" => Ok(Self::Http),
            "sse" => Ok(Self::Sse),
            other => Err(TransportParseError(other.to_string())),
        }
    }
}

impl TryFrom<&str> for Transport {
    type Error = TransportParseError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;

    #[test]
    fn from_str_accepts_known_variants() {
        assert_eq!("stdio".parse::<Transport>().unwrap(), Transport::Stdio);
        assert_eq!("http".parse::<Transport>().unwrap(), Transport::Http);
        assert_eq!("sse".parse::<Transport>().unwrap(), Transport::Sse);
    }

    #[test]
    fn from_str_returns_error_for_unknown() {
        let err = "websocket".parse::<Transport>().unwrap_err();
        assert!(err.to_string().contains("websocket"));
    }

    #[test]
    fn try_from_str_returns_error_for_unknown() {
        assert!(Transport::try_from("bogus").is_err());
    }
}

/// Output format for `--output-format`.
#[derive(Debug, Clone, Copy, Default)]
pub enum OutputFormat {
    /// Plain text output (default).
    #[default]
    Text,
    /// Single JSON result object.
    Json,
    /// Streaming NDJSON.
    StreamJson,
}

impl OutputFormat {
    pub(crate) fn as_arg(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::StreamJson => "stream-json",
        }
    }
}

/// Permission mode for `--permission-mode`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    /// Default interactive permissions.
    #[default]
    Default,
    /// Auto-accept file edits.
    AcceptEdits,
    /// Bypass all permission checks.
    ///
    /// **Deprecated.** Reaching for this variant directly puts a
    /// bypass-mode query one keystroke away in any code path, which
    /// is exactly the footgun the variant enables. Use
    /// [`crate::dangerous::DangerousClient`] instead, which gates
    /// construction on the `CLAUDE_WRAPPER_ALLOW_DANGEROUS` env-var
    /// and makes the intent obvious at the call site. The variant
    /// itself will stay available through the current major version
    /// so existing callers keep compiling (with a deprecation
    /// warning).
    #[deprecated(
        since = "0.5.1",
        note = "use claude_wrapper::dangerous::DangerousClient instead; \
                direct BypassPermissions usage is a footgun and will go \
                away in a future major release"
    )]
    BypassPermissions,
    /// Don't ask for permissions (deny by default).
    DontAsk,
    /// Plan mode (read-only).
    Plan,
    /// Auto mode.
    Auto,
}

impl PermissionMode {
    pub(crate) fn as_arg(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            #[allow(deprecated)]
            Self::BypassPermissions => "bypassPermissions",
            Self::DontAsk => "dontAsk",
            Self::Plan => "plan",
            Self::Auto => "auto",
        }
    }
}

/// Input format for `--input-format`.
#[derive(Debug, Clone, Copy, Default)]
pub enum InputFormat {
    /// Plain text input (default).
    #[default]
    Text,
    /// Streaming JSON input.
    StreamJson,
}

impl InputFormat {
    pub(crate) fn as_arg(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::StreamJson => "stream-json",
        }
    }
}

/// Effort level for `--effort`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    /// Low effort.
    Low,
    /// Medium effort (default).
    Medium,
    /// High effort.
    High,
    /// Maximum effort, most thorough.
    Max,
}

impl Effort {
    pub(crate) fn as_arg(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }
}

/// Scope for MCP and plugin commands.
#[derive(Debug, Clone, Copy, Default)]
pub enum Scope {
    /// Local scope (current directory).
    #[default]
    Local,
    /// User scope (global).
    User,
    /// Project scope.
    Project,
}

impl Scope {
    pub(crate) fn as_arg(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// Authentication status returned by `claude auth status --json`.
///
/// # Example
///
/// ```no_run
/// # async fn example() -> claude_wrapper::Result<()> {
/// let claude = claude_wrapper::Claude::builder().build()?;
/// let status = claude_wrapper::AuthStatusCommand::new()
///     .execute_json(&claude).await?;
///
/// if status.logged_in {
///     println!("Logged in as {}", status.email.unwrap_or_default());
/// }
/// # Ok(())
/// # }
/// ```
#[cfg(feature = "json")]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    /// Whether the user is currently logged in.
    #[serde(default)]
    pub logged_in: bool,
    /// Authentication method (e.g. "claude.ai").
    #[serde(default)]
    pub auth_method: Option<String>,
    /// API provider (e.g. "firstParty").
    #[serde(default)]
    pub api_provider: Option<String>,
    /// Authenticated user's email address.
    #[serde(default)]
    pub email: Option<String>,
    /// Organization ID.
    #[serde(default)]
    pub org_id: Option<String>,
    /// Organization name.
    #[serde(default)]
    pub org_name: Option<String>,
    /// Subscription type (e.g. "pro", "max").
    #[serde(default)]
    pub subscription_type: Option<String>,
    /// Any additional fields not explicitly modeled.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

/// A message from a query result, representing one turn in the conversation.
#[cfg(feature = "json")]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QueryMessage {
    /// The role of the message sender (e.g., "user", "assistant").
    #[serde(default)]
    pub role: String,
    /// The text content of the message.
    #[serde(default)]
    pub content: serde_json::Value,
    /// Additional fields returned by the CLI not captured in typed fields.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

/// Result from a query with `--output-format json`.
#[cfg(feature = "json")]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QueryResult {
    /// The text content of the query response.
    #[serde(default)]
    pub result: String,
    /// The session ID for continuing conversations.
    #[serde(default)]
    pub session_id: String,
    /// Total cost of the query in USD.
    #[serde(default, rename = "total_cost_usd", alias = "cost_usd")]
    pub cost_usd: Option<f64>,
    /// Duration of the query in milliseconds.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Number of conversation turns in the query.
    #[serde(default)]
    pub num_turns: Option<u32>,
    /// Whether the query resulted in an error.
    #[serde(default)]
    pub is_error: bool,
    /// Additional fields returned by the CLI not captured in typed fields.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

#[cfg(all(test, feature = "json"))]
mod tests {
    use super::*;

    #[test]
    fn query_result_deserializes_total_cost_usd() {
        let json =
            r#"{"result":"hello","session_id":"s1","total_cost_usd":0.042,"is_error":false}"#;
        let qr: QueryResult = serde_json::from_str(json).unwrap();
        assert_eq!(qr.cost_usd, Some(0.042));
    }

    #[test]
    fn query_result_deserializes_cost_usd_alias() {
        let json = r#"{"result":"hello","session_id":"s1","cost_usd":0.01,"is_error":false}"#;
        let qr: QueryResult = serde_json::from_str(json).unwrap();
        assert_eq!(qr.cost_usd, Some(0.01));
    }

    #[test]
    fn query_result_missing_cost_defaults_to_none() {
        let json = r#"{"result":"hello","session_id":"s1","is_error":false}"#;
        let qr: QueryResult = serde_json::from_str(json).unwrap();
        assert_eq!(qr.cost_usd, None);
    }

    #[test]
    fn query_result_from_stream_result_event() {
        // Exact shape of a --output-format stream-json "result" event.
        // The flattened `extra` must absorb type/subtype without
        // breaking typed field parsing.
        let json = r#"{"type":"result","subtype":"success","result":"streamed","session_id":"sess-1","total_cost_usd":0.03,"num_turns":1,"is_error":false}"#;
        let qr: QueryResult = serde_json::from_str(json).unwrap();
        assert_eq!(qr.cost_usd, Some(0.03));
        assert_eq!(qr.num_turns, Some(1));
        assert_eq!(qr.session_id, "sess-1");
        assert_eq!(qr.result, "streamed");
        assert_eq!(
            qr.extra.get("type").and_then(|v| v.as_str()),
            Some("result")
        );
    }

    #[test]
    fn query_result_deserializes_num_turns() {
        let json = r#"{"result":"done","session_id":"s2","total_cost_usd":0.1,"num_turns":5,"is_error":false}"#;
        let qr: QueryResult = serde_json::from_str(json).unwrap();
        assert_eq!(qr.num_turns, Some(5));
        assert_eq!(qr.cost_usd, Some(0.1));
    }

    #[test]
    fn query_result_serializes_as_total_cost_usd() {
        let qr = QueryResult {
            result: "ok".into(),
            session_id: "s1".into(),
            cost_usd: Some(0.05),
            duration_ms: None,
            num_turns: Some(3),
            is_error: false,
            extra: Default::default(),
        };
        let json = serde_json::to_string(&qr).unwrap();
        assert!(json.contains("\"total_cost_usd\""));
        assert!(json.contains("\"num_turns\""));
    }
}
