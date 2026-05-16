//! Polished `claude-server` MCP binary.
//!
//! The library (`claude_server`) is the product; this binary is the
//! sharp default front door. Run it bare with `claude-server serve`
//! for stdio MCP, `claude-server serve-http` for HTTP, or one of the
//! diagnostic subcommands (`tools`, `doctor`, `config`,
//! `install-mcp-config`) to inspect / wire up the server.
//!
//! For the truly-minimal copy-paste integration recipe, see
//! `examples/server.rs` -- the bare bones from which this binary
//! grew.
//!
//! All surfaces are compiled in by default (the `[[bin]]` section in
//! `Cargo.toml` requires `features = ["full"]`). Use the
//! `[surfaces]` block in `ServerConfig` to disable individual
//! surfaces at runtime without recompiling.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use claude_server::{
    ServerConfig, build_router_with_notification_sender, notification_channel, registered_tools,
};
use tower::Layer;
use tower_mcp::HttpTransport;
use tower_mcp::middleware::McpTracingLayer;
use tower_mcp::transport::stdio::GenericStdioTransport;

/// Polished MCP server CLI for `claude-server`.
#[derive(Debug, Parser)]
#[command(name = "claude-server", version, about, long_about = None)]
struct Cli {
    /// Path to a TOML ServerConfig. Defaults are used if omitted.
    #[arg(long, short = 'c', global = true)]
    config: Option<PathBuf>,

    /// Log level for the embedded tracing-subscriber. Overrides the
    /// `RUST_LOG` env var. Accepts `error`, `warn`, `info`, `debug`,
    /// `trace`, or a full env-filter expression like
    /// `info,claude_server=debug`.
    #[arg(long, global = true)]
    log_level: Option<String>,

    /// Log output format. `text` is the default human-readable
    /// shape; `json` emits structured JSON one event per line,
    /// suitable for log shippers.
    #[arg(long, global = true, value_enum, default_value_t = LogFormat::Text)]
    log_format: LogFormat,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum LogFormat {
    Text,
    Json,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Serve over stdio (default).
    Serve,
    /// Serve over HTTP via axum. Localhost-only by default.
    ServeHttp {
        /// Bind address. Defaults to 127.0.0.1:7800.
        #[arg(long, default_value = "127.0.0.1:7800")]
        bind: SocketAddr,
        /// Optional bearer token. When set, every request must
        /// include `Authorization: Bearer <token>` or it gets a 401.
        #[arg(long)]
        bearer: Option<String>,
        /// Allow binding to non-loopback addresses. Required to
        /// bind anything other than `127.0.0.1` / `::1`.
        /// Refuses by default so an accidental `--bind 0.0.0.0`
        /// doesn't quietly expose the server publicly.
        #[arg(long)]
        allow_public: bool,
    },
    /// Print the registered tool surface as JSON and exit.
    Tools,
    /// Run a pre-flight check: is `claude` on PATH, what CLI
    /// version, what auth strategy, what tested-against range
    /// status, are the configured roots present.
    Doctor,
    /// Print the effective ServerConfig (post-load, post-defaults)
    /// as JSON. Useful for "why is this not picking up my setting".
    Config,
    /// Emit an MCP client config snippet pointing at this binary.
    /// Stdout by default; pass `--write` to write to the target's
    /// canonical path.
    InstallMcpConfig {
        /// Which MCP client to target.
        #[arg(long, value_enum, default_value_t = McpTarget::Stdout)]
        target: McpTarget,
        /// Override the server name registered in the client (the
        /// key the client uses to refer to this server). Defaults to
        /// `claude-server`.
        #[arg(long, default_value = "claude-server")]
        name: String,
        /// Override the path to the binary embedded in the snippet.
        /// Defaults to the absolute path of the currently-running
        /// `claude-server` executable.
        #[arg(long)]
        binary: Option<PathBuf>,
        /// Write the snippet to the target's canonical config path
        /// (with a confirmation prompt if the file already exists)
        /// instead of printing to stdout. Currently honored for
        /// `claude-desktop`; other targets print and let the user
        /// pipe.
        #[arg(long)]
        write: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum McpTarget {
    /// Print a generic stdio MCP config blob to stdout. Caller
    /// pipes / pastes into whatever client they want.
    Stdout,
    /// Claude Desktop. Canonical path on macOS:
    /// `~/Library/Application Support/Claude/claude_desktop_config.json`.
    ClaudeDesktop,
    /// Claude Code CLI -- emit a `claude mcp add-json` command
    /// the user can run to register this server with their CLI.
    ClaudeCode,
    /// VS Code MCP -- emit the user-settings snippet. Note: the
    /// VS Code MCP path is project-local most of the time; user
    /// pastes wherever appropriate.
    Vscode,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.log_level.as_deref(), cli.log_format);

    let cfg = load_config(cli.config.clone()).await?;

    match cli.command.unwrap_or(Cmd::Serve) {
        Cmd::Serve => serve_stdio(cfg).await,
        Cmd::ServeHttp {
            bind,
            bearer,
            allow_public,
        } => serve_http(cfg, bind, bearer, allow_public).await,
        Cmd::Tools => cmd_tools(cfg),
        Cmd::Doctor => cmd_doctor(cfg).await,
        Cmd::Config => cmd_config(cfg),
        Cmd::InstallMcpConfig {
            target,
            name,
            binary,
            write,
        } => cmd_install_mcp_config(target, name, binary, write),
    }
}

// -- serve over stdio ----------------------------------------------

async fn serve_stdio(cfg: ServerConfig) -> Result<()> {
    let (notif_tx, notif_rx) = notification_channel(256);
    let router = build_router_with_notification_sender(cfg, notif_tx)
        .context("build_router_with_notification_sender")?;
    let service = McpTracingLayer::new().layer(router);
    let mut transport = GenericStdioTransport::with_notifications(service, notif_rx);
    transport.run().await.context("stdio transport")
}

// -- serve over HTTP -----------------------------------------------

async fn serve_http(
    cfg: ServerConfig,
    bind: SocketAddr,
    bearer: Option<String>,
    allow_public: bool,
) -> Result<()> {
    if !bind.ip().is_loopback() && !allow_public {
        anyhow::bail!(
            "refusing to bind {bind}: address is not loopback. \
             Pass `--allow-public` if you really mean to expose this \
             server beyond localhost. Note: an unauthenticated MCP \
             endpoint can drive `claude` arbitrarily; consider \
             `--bearer <token>` as well."
        );
    }
    if !bind.ip().is_loopback() {
        tracing::warn!(
            %bind,
            bearer_set = bearer.is_some(),
            "binding to non-loopback address; ensure your firewall is appropriate"
        );
    }

    let (notif_tx, notif_rx) = notification_channel(256);
    let router = build_router_with_notification_sender(cfg, notif_tx)
        .context("build_router_with_notification_sender")?;
    let mut app = HttpTransport::with_notifications(router, notif_rx)
        .layer(McpTracingLayer::new())
        .into_router();
    if let Some(token) = bearer {
        use axum::middleware;
        app = app.layer(middleware::from_fn(move |req, next| {
            let token = token.clone();
            bearer_guard(req, next, token)
        }));
        tracing::info!("bearer auth enabled");
    }
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!(%bind, "claude-server listening on http");
    axum::serve(listener, app).await.context("axum::serve")
}

// -- tools subcommand ----------------------------------------------

fn cmd_tools(cfg: ServerConfig) -> Result<()> {
    let tools = registered_tools(cfg).context("registered_tools")?;
    let body: Vec<_> = tools
        .into_iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description,
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&body)?);
    Ok(())
}

// -- doctor subcommand ---------------------------------------------

async fn cmd_doctor(cfg: ServerConfig) -> Result<()> {
    use claude_wrapper::{Claude, auth};

    println!("claude-server doctor");
    println!("====================");

    // 1. claude binary on PATH
    let claude_builder_result = Claude::builder()
        .binary(
            cfg.claude
                .binary
                .clone()
                .unwrap_or_else(|| PathBuf::from("claude")),
        )
        .build();
    match &claude_builder_result {
        Ok(_) => println!("[ok ]   `claude` binary found"),
        Err(e) => println!("[FAIL]  `claude` binary: {e}"),
    }

    // 2. live CLI version + tested-range status
    if let Ok(claude) = Claude::builder()
        .tested_cli_version_range(
            claude_wrapper::CliVersion {
                major: 2,
                minor: 1,
                patch: 0,
            },
            claude_wrapper::CliVersion {
                major: 2,
                minor: 1,
                patch: 143,
            },
        )
        .build()
    {
        match claude.cli_version().await {
            Ok(v) => println!("[ok ]   CLI version: {v}"),
            Err(e) => println!("[warn]  CLI version fetch failed: {e}"),
        }
        match claude.cli_version_status().await {
            Ok(status) => match status {
                claude_wrapper::CliVersionStatus::Tested => {
                    println!("[ok ]   tested-against range: tested");
                }
                claude_wrapper::CliVersionStatus::NewerUntested { found, tested_max } => {
                    println!(
                        "[warn]  tested-against range: newer than tested (found {found}, tested max {tested_max})"
                    );
                }
                claude_wrapper::CliVersionStatus::OlderThanMinimum { found, minimum } => {
                    println!(
                        "[FAIL]  tested-against range: below declared minimum (found {found}, minimum {minimum})"
                    );
                }
            },
            Err(e) => println!("[warn]  range classification failed: {e}"),
        }
    }

    // 3. auth strategy (env-derived)
    let auth_summary = auth::detect();
    println!("[info]  auth strategy: {}", auth_summary.strategy.as_str());

    // 4. configured roots (only relevant when feature is on)
    if let Some(p) = cfg.history_root.as_ref() {
        println!(
            "[{}]  history_root: {}",
            if p.exists() { "ok " } else { "warn" },
            p.display()
        );
    }
    if let Some(p) = cfg.agents_root.as_ref() {
        println!(
            "[{}]  agents_root: {}",
            if p.exists() { "ok " } else { "warn" },
            p.display()
        );
    }
    if let Some(p) = cfg.jobs_root.as_ref() {
        println!(
            "[{}]  jobs_root: {}",
            if p.exists() { "ok " } else { "warn" },
            p.display()
        );
    }
    if let Some(p) = cfg.worktrees_root.as_ref() {
        println!(
            "[{}]  worktrees_root: {}",
            if p.exists() { "ok " } else { "warn" },
            p.display()
        );
    }

    // 5. surface counts
    let tools = registered_tools(cfg).context("registered_tools for doctor")?;
    println!("[info]  registered tools: {}", tools.len());

    Ok(())
}

// -- config subcommand ---------------------------------------------

fn cmd_config(cfg: ServerConfig) -> Result<()> {
    let json = serde_json::to_string_pretty(&cfg).context("serialize ServerConfig")?;
    println!("{json}");
    Ok(())
}

// -- install-mcp-config subcommand ---------------------------------

fn cmd_install_mcp_config(
    target: McpTarget,
    name: String,
    binary: Option<PathBuf>,
    write: bool,
) -> Result<()> {
    let binary_path = match binary {
        Some(p) => p,
        None => std::env::current_exe().context("resolving current executable path")?,
    };
    let binary_str = binary_path.to_string_lossy().to_string();

    match target {
        McpTarget::Stdout => {
            let blob = serde_json::json!({
                name.as_str(): {
                    "command": binary_str,
                    "args": ["serve"]
                }
            });
            println!("{}", serde_json::to_string_pretty(&blob)?);
        }
        McpTarget::ClaudeDesktop => {
            let blob = serde_json::json!({
                "mcpServers": {
                    name.as_str(): {
                        "command": binary_str,
                        "args": ["serve"]
                    }
                }
            });
            if write {
                let path =
                    claude_desktop_config_path().context("resolving Claude Desktop config path")?;
                anyhow::bail!(
                    "--write would overwrite {} -- merging into existing \
                     config is not implemented yet. Print to stdout and \
                     edit manually instead.",
                    path.display()
                );
            }
            println!(
                "// Paste the `mcpServers` entry below into Claude Desktop's config:\n\
                 // macOS:   ~/Library/Application Support/Claude/claude_desktop_config.json\n\
                 // Windows: %APPDATA%\\Claude\\claude_desktop_config.json"
            );
            println!("{}", serde_json::to_string_pretty(&blob)?);
        }
        McpTarget::ClaudeCode => {
            let json_blob = serde_json::json!({
                "command": binary_str,
                "args": ["serve"]
            });
            let json_str = serde_json::to_string(&json_blob)?;
            println!(
                "# Run this to register {name} with your `claude` CLI:\n\
                 claude mcp add-json {name} '{json_str}'"
            );
        }
        McpTarget::Vscode => {
            let blob = serde_json::json!({
                "servers": {
                    name.as_str(): {
                        "type": "stdio",
                        "command": binary_str,
                        "args": ["serve"]
                    }
                }
            });
            println!(
                "// Paste the entry below into your VS Code MCP config\n\
                 // (project: .vscode/mcp.json; user: settings.json under `mcp`):"
            );
            println!("{}", serde_json::to_string_pretty(&blob)?);
        }
    }
    Ok(())
}

fn claude_desktop_config_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME not set")?;
    #[cfg(target_os = "macos")]
    let p = home
        .join("Library")
        .join("Application Support")
        .join("Claude")
        .join("claude_desktop_config.json");
    #[cfg(not(target_os = "macos"))]
    let p = home
        .join(".config")
        .join("Claude")
        .join("claude_desktop_config.json");
    Ok(p)
}

// -- helpers --------------------------------------------------------

async fn load_config(path: Option<PathBuf>) -> Result<ServerConfig> {
    let Some(p) = path else {
        return Ok(ServerConfig::default());
    };
    let text = tokio::fs::read_to_string(&p)
        .await
        .with_context(|| format!("reading {}", p.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", p.display()))
}

fn init_tracing(level_override: Option<&str>, format: LogFormat) {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = if let Some(spec) = level_override {
        EnvFilter::try_new(spec).unwrap_or_else(|_| EnvFilter::new("info"))
    } else {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info,claude_server=info,tower_mcp=info"))
    };
    let builder = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_ansi(false);
    let _ = match format {
        LogFormat::Text => builder.try_init(),
        LogFormat::Json => builder.json().try_init(),
    };
}

async fn bearer_guard(
    req: axum::extract::Request,
    next: axum::middleware::Next,
    expected: String,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    let header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    match header {
        Some(t) if t == expected => Ok(next.run(req).await),
        _ => Err(axum::http::StatusCode::UNAUTHORIZED),
    }
}
