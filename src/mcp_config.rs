use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::Result;

/// Builder for generating `.mcp.json` config files programmatically.
///
/// This is useful when you need to dynamically configure MCP servers
/// for agent processes that communicate via MCP.
///
/// # Example
///
/// ```no_run
/// use claude_wrapper::McpConfigBuilder;
///
/// # fn example() -> claude_wrapper::Result<()> {
/// let config = McpConfigBuilder::new()
///     .http_server("my-hub", "http://127.0.0.1:9090")
///     .stdio_server("my-tool", "npx", ["my-mcp-server"])
///     .write_to("/tmp/my-project/.mcp.json")?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct McpConfigBuilder {
    servers: HashMap<String, McpServerConfig>,
}

/// Configuration for a single MCP server entry.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum McpServerConfig {
    /// HTTP transport (streamable HTTP or SSE).
    #[serde(rename = "http")]
    Http {
        url: String,
        #[serde(skip_serializing_if = "HashMap::is_empty")]
        headers: HashMap<String, String>,
    },

    /// Stdio transport (subprocess).
    #[serde(rename = "stdio")]
    Stdio {
        command: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(skip_serializing_if = "HashMap::is_empty")]
        env: HashMap<String, String>,
    },
}

/// Wrapper for serializing the full config file.
#[derive(Debug, Serialize)]
struct McpConfigFile {
    #[serde(rename = "mcpServers")]
    mcp_servers: HashMap<String, McpServerConfig>,
}

impl McpConfigBuilder {
    /// Create a new empty MCP config builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an HTTP MCP server.
    #[must_use]
    pub fn http_server(mut self, name: impl Into<String>, url: impl Into<String>) -> Self {
        self.servers.insert(
            name.into(),
            McpServerConfig::Http {
                url: url.into(),
                headers: HashMap::new(),
            },
        );
        self
    }

    /// Add an HTTP MCP server with custom headers.
    #[must_use]
    pub fn http_server_with_headers(
        mut self,
        name: impl Into<String>,
        url: impl Into<String>,
        headers: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.servers.insert(
            name.into(),
            McpServerConfig::Http {
                url: url.into(),
                headers: headers
                    .into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect(),
            },
        );
        self
    }

    /// Add a stdio MCP server.
    #[must_use]
    pub fn stdio_server(
        mut self,
        name: impl Into<String>,
        command: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.servers.insert(
            name.into(),
            McpServerConfig::Stdio {
                command: command.into(),
                args: args.into_iter().map(Into::into).collect(),
                env: HashMap::new(),
            },
        );
        self
    }

    /// Add a stdio MCP server with environment variables.
    #[must_use]
    pub fn stdio_server_with_env(
        mut self,
        name: impl Into<String>,
        command: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
        env: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.servers.insert(
            name.into(),
            McpServerConfig::Stdio {
                command: command.into(),
                args: args.into_iter().map(Into::into).collect(),
                env: env.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
            },
        );
        self
    }

    /// Add a raw server config.
    #[must_use]
    pub fn server(mut self, name: impl Into<String>, config: McpServerConfig) -> Self {
        self.servers.insert(name.into(), config);
        self
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String> {
        let file = McpConfigFile {
            mcp_servers: self.servers.clone(),
        };

        #[cfg(feature = "json")]
        {
            serde_json::to_string_pretty(&file).map_err(|e| crate::error::Error::Json {
                message: "failed to serialize MCP config".to_string(),
                source: e,
            })
        }

        #[cfg(not(feature = "json"))]
        {
            let _ = file;
            Err(crate::error::Error::Io {
                message: "json feature required for MCP config serialization".to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "json feature not enabled",
                ),
            })
        }
    }

    /// Write the config to a file path, returning the path.
    pub fn write_to(&self, path: impl AsRef<Path>) -> Result<PathBuf> {
        let path = path.as_ref().to_path_buf();
        let json = self.to_json()?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| crate::error::Error::Io {
                message: format!("failed to create directory: {}", parent.display()),
                source: e,
            })?;
        }

        std::fs::write(&path, json).map_err(|e| crate::error::Error::Io {
            message: format!("failed to write MCP config to {}", path.display()),
            source: e,
        })?;

        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_server_config() {
        let config = McpConfigBuilder::new().http_server("my-hub", "http://127.0.0.1:9090");

        let json = config.to_json().unwrap();
        assert!(json.contains("my-hub"));
        assert!(json.contains("http://127.0.0.1:9090"));
        assert!(json.contains(r#""type": "http""#));
    }

    #[test]
    fn test_stdio_server_config() {
        let config = McpConfigBuilder::new().stdio_server(
            "my-tool",
            "npx",
            ["my-mcp-server", "--port", "3000"],
        );

        let json = config.to_json().unwrap();
        assert!(json.contains("my-tool"));
        assert!(json.contains("npx"));
        assert!(json.contains("my-mcp-server"));
        assert!(json.contains(r#""type": "stdio""#));
    }

    #[test]
    fn test_multiple_servers() {
        let config = McpConfigBuilder::new()
            .http_server("hub", "http://localhost:9090")
            .stdio_server("tool", "node", ["server.js"]);

        let json = config.to_json().unwrap();
        assert!(json.contains("hub"));
        assert!(json.contains("tool"));
    }
}
