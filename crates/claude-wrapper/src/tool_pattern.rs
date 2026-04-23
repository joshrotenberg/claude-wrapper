//! Tool permission patterns for `--allowed-tools` / `--disallowed-tools`.
//!
//! The Claude CLI accepts three pattern shapes in its tool lists:
//!
//! - Bare name: `Bash`, `Read`, `Write`.
//! - Name with argument glob: `Bash(git log:*)`, `Write(src/*.rs)`.
//! - MCP pattern: `mcp__server__tool` or `mcp__server__*`.
//!
//! [`ToolPattern`] models all three. The typed constructors
//! ([`ToolPattern::tool`], [`ToolPattern::tool_with_args`],
//! [`ToolPattern::all`], [`ToolPattern::mcp`]) always produce
//! well-formed output. [`ToolPattern::parse`] validates shape of a
//! raw string and returns [`PatternError`] on malformed input.
//!
//! For back-compat, `From<&str>` / `From<String>` accept any string
//! and store it verbatim -- callers passing raw CLI strings through
//! [`QueryCommand::allowed_tool`](crate::QueryCommand::allowed_tool)
//! keep working without changes. Use [`ToolPattern::parse`] directly
//! when you want to catch typos before the CLI invocation.
//!
//! # Example
//!
//! ```
//! use claude_wrapper::ToolPattern;
//!
//! let p = ToolPattern::tool_with_args("Bash", "git log:*");
//! assert_eq!(p.as_str(), "Bash(git log:*)");
//!
//! let p = ToolPattern::all("Write");
//! assert_eq!(p.as_str(), "Write(*)");
//!
//! let p = ToolPattern::mcp("my-server", "*");
//! assert_eq!(p.as_str(), "mcp__my-server__*");
//! ```

use std::fmt;

/// A tool permission pattern, ready to be emitted in a comma-joined
/// `--allowed-tools` / `--disallowed-tools` value.
///
/// See the [module docs](crate::tool_pattern) for the accepted shapes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolPattern {
    repr: String,
}

/// Errors from parsing a raw string with [`ToolPattern::parse`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PatternError {
    /// Input was empty or all whitespace.
    #[error("tool pattern must not be empty")]
    Empty,

    /// The tool name part was empty (e.g. `(args)` with nothing before).
    #[error("tool pattern is missing a name before '('")]
    MissingName,

    /// Parentheses were unbalanced or appeared out of order.
    #[error("tool pattern has unbalanced parentheses: {0:?}")]
    UnbalancedParens(String),

    /// Contained a character that the CLI disallows in pattern values
    /// (comma splits the argv, control chars can break the shell).
    #[error("tool pattern contains an illegal character: {0:?}")]
    IllegalChar(String),
}

impl ToolPattern {
    /// A bare tool name, e.g. `ToolPattern::tool("Bash")` -> `Bash`.
    ///
    /// No validation beyond trimming whitespace; the CLI is the
    /// source of truth for which tool names exist.
    pub fn tool(name: impl Into<String>) -> Self {
        Self {
            repr: name.into().trim().to_string(),
        }
    }

    /// A tool with an argument glob, rendered `Name(args)`.
    ///
    /// ```
    /// # use claude_wrapper::ToolPattern;
    /// assert_eq!(
    ///     ToolPattern::tool_with_args("Bash", "git log:*").as_str(),
    ///     "Bash(git log:*)"
    /// );
    /// ```
    pub fn tool_with_args(name: impl Into<String>, args: impl Into<String>) -> Self {
        Self {
            repr: format!("{}({})", name.into().trim(), args.into()),
        }
    }

    /// Shorthand for [`ToolPattern::tool_with_args`] with `*` as the
    /// argument pattern -- "any args to this tool."
    ///
    /// ```
    /// # use claude_wrapper::ToolPattern;
    /// assert_eq!(ToolPattern::all("Write").as_str(), "Write(*)");
    /// ```
    pub fn all(name: impl Into<String>) -> Self {
        Self::tool_with_args(name, "*")
    }

    /// An MCP pattern: `mcp__{server}__{tool}`. Pass `"*"` as the tool
    /// to match any tool from the server.
    ///
    /// ```
    /// # use claude_wrapper::ToolPattern;
    /// assert_eq!(
    ///     ToolPattern::mcp("my-server", "do_thing").as_str(),
    ///     "mcp__my-server__do_thing"
    /// );
    /// assert_eq!(
    ///     ToolPattern::mcp("my-server", "*").as_str(),
    ///     "mcp__my-server__*"
    /// );
    /// ```
    pub fn mcp(server: impl Into<String>, tool: impl Into<String>) -> Self {
        Self {
            repr: format!("mcp__{}__{}", server.into(), tool.into()),
        }
    }

    /// Parse and validate a raw CLI-format pattern string.
    ///
    /// Validation is shape-level only (non-empty, balanced parens, no
    /// comma or control chars). Tool names are not checked against any
    /// allowlist because the CLI's tool inventory evolves independently.
    pub fn parse(s: impl AsRef<str>) -> Result<Self, PatternError> {
        let trimmed = s.as_ref().trim();
        if trimmed.is_empty() {
            return Err(PatternError::Empty);
        }

        for ch in trimmed.chars() {
            if ch == ',' || ch.is_control() {
                return Err(PatternError::IllegalChar(trimmed.to_string()));
            }
        }

        if let Some(open) = trimmed.find('(') {
            if !trimmed.ends_with(')') {
                return Err(PatternError::UnbalancedParens(trimmed.to_string()));
            }
            // Exactly one '(' and one ')'.
            if trimmed.matches('(').count() != 1 || trimmed.matches(')').count() != 1 {
                return Err(PatternError::UnbalancedParens(trimmed.to_string()));
            }
            if open == 0 {
                return Err(PatternError::MissingName);
            }
        } else if trimmed.contains(')') {
            return Err(PatternError::UnbalancedParens(trimmed.to_string()));
        }

        Ok(Self {
            repr: trimmed.to_string(),
        })
    }

    /// The rendered pattern string, as it will appear in the CLI arg.
    pub fn as_str(&self) -> &str {
        &self.repr
    }
}

impl fmt::Display for ToolPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.repr)
    }
}

impl AsRef<str> for ToolPattern {
    fn as_ref(&self) -> &str {
        &self.repr
    }
}

impl From<&str> for ToolPattern {
    fn from(s: &str) -> Self {
        Self {
            repr: s.trim().to_string(),
        }
    }
}

impl From<String> for ToolPattern {
    fn from(s: String) -> Self {
        let trimmed = s.trim();
        if trimmed.len() == s.len() {
            Self { repr: s }
        } else {
            Self {
                repr: trimmed.to_string(),
            }
        }
    }
}

impl From<&String> for ToolPattern {
    fn from(s: &String) -> Self {
        Self::from(s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_strips_whitespace() {
        assert_eq!(ToolPattern::tool("  Bash  ").as_str(), "Bash");
    }

    #[test]
    fn tool_with_args_renders_parens() {
        let p = ToolPattern::tool_with_args("Bash", "git log:*");
        assert_eq!(p.as_str(), "Bash(git log:*)");
    }

    #[test]
    fn all_wildcards_args() {
        assert_eq!(ToolPattern::all("Write").as_str(), "Write(*)");
    }

    #[test]
    fn mcp_patterns() {
        assert_eq!(ToolPattern::mcp("srv", "do_it").as_str(), "mcp__srv__do_it");
        assert_eq!(ToolPattern::mcp("srv", "*").as_str(), "mcp__srv__*");
    }

    #[test]
    fn parse_accepts_bare_name() {
        assert_eq!(ToolPattern::parse("Bash").unwrap().as_str(), "Bash");
    }

    #[test]
    fn parse_accepts_name_with_args() {
        assert_eq!(
            ToolPattern::parse("Bash(git log:*)").unwrap().as_str(),
            "Bash(git log:*)"
        );
    }

    #[test]
    fn parse_accepts_mcp() {
        assert_eq!(
            ToolPattern::parse("mcp__srv__*").unwrap().as_str(),
            "mcp__srv__*"
        );
    }

    #[test]
    fn parse_trims_whitespace() {
        assert_eq!(ToolPattern::parse("  Read  ").unwrap().as_str(), "Read");
    }

    #[test]
    fn parse_rejects_empty() {
        assert_eq!(ToolPattern::parse("").unwrap_err(), PatternError::Empty);
        assert_eq!(ToolPattern::parse("   ").unwrap_err(), PatternError::Empty);
    }

    #[test]
    fn parse_rejects_unbalanced_parens() {
        assert!(matches!(
            ToolPattern::parse("Bash(git log"),
            Err(PatternError::UnbalancedParens(_))
        ));
        assert!(matches!(
            ToolPattern::parse("Bashgit log)"),
            Err(PatternError::UnbalancedParens(_))
        ));
        assert!(matches!(
            ToolPattern::parse("Bash((nested))"),
            Err(PatternError::UnbalancedParens(_))
        ));
    }

    #[test]
    fn parse_rejects_missing_name() {
        assert_eq!(
            ToolPattern::parse("(args)").unwrap_err(),
            PatternError::MissingName
        );
    }

    #[test]
    fn parse_rejects_comma() {
        assert!(matches!(
            ToolPattern::parse("Bash,Read"),
            Err(PatternError::IllegalChar(_))
        ));
    }

    #[test]
    fn parse_rejects_control_chars() {
        // Must be mid-string: leading/trailing whitespace is trimmed.
        assert!(matches!(
            ToolPattern::parse("Ba\nsh"),
            Err(PatternError::IllegalChar(_))
        ));
    }

    #[test]
    fn from_str_is_loose() {
        // Skips validation so back-compat callers keep working.
        let p: ToolPattern = "anything goes".into();
        assert_eq!(p.as_str(), "anything goes");
    }

    #[test]
    fn display_matches_as_str() {
        let p = ToolPattern::tool_with_args("Bash", "ls");
        assert_eq!(format!("{p}"), p.as_str());
    }
}
