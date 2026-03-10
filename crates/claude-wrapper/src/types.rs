use serde::{Deserialize, Serialize};

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
    /// Minimal effort, fastest responses.
    Min,
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
            Self::Min => "min",
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

/// A message in the query JSON output.
#[cfg(feature = "json")]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QueryMessage {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: serde_json::Value,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

/// Result from a query with `--output-format json`.
#[cfg(feature = "json")]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QueryResult {
    #[serde(default)]
    pub result: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}
