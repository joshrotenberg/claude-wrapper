//! claude-pool MCP server binary.
//!
//! Manages a pool of Claude CLI slots, exposed as an MCP server
//! over stdio transport.

mod prompts;
mod resources;
mod tools;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use claude_pool::{InMemoryStore, Pool, PoolConfig, PoolStore, SkillRegistry, WorkflowRegistry};
use tokio::sync::RwLock;
use tower_mcp::{McpRouter, StdioTransport};

/// Shared state accessible by all tool/resource handlers.
pub struct State<S: PoolStore> {
    /// The pool instance.
    pub pool: Pool<S>,
    /// Thread-safe skill registry (mutated by skill management tools).
    pub skills: Arc<RwLock<SkillRegistry>>,
    /// Workflow registry.
    pub workflows: WorkflowRegistry,
    /// Directory for persisting project-local skills.
    pub skills_dir: PathBuf,
}

/// MCP server for managing a pool of Claude CLI slots.
#[derive(Parser)]
#[command(name = "claude-pool-server", version)]
struct Cli {
    /// Number of slots to spawn.
    #[arg(short = 'n', long, default_value = "2")]
    slots: usize,

    /// Default model for all slots (e.g. "claude-haiku-4-5-20251001").
    #[arg(short, long)]
    model: Option<String>,

    /// Default effort level (min, low, medium, high, max).
    #[arg(short, long)]
    effort: Option<String>,

    /// Total budget cap in USD (e.g. 5.00).
    #[arg(short, long)]
    budget_usd: Option<f64>,

    /// System prompt for all slots.
    #[arg(short, long)]
    system_prompt: Option<String>,

    /// Permission mode for slots (default, acceptEdits, bypassPermissions, plan, auto).
    #[arg(short, long, default_value = "plan")]
    permission_mode: String,

    /// Minimum number of slots (floor for scale-down). Default: 1.
    #[arg(long, default_value = "1")]
    min_slots: usize,

    /// Maximum number of slots (ceiling for scale-up). Default: 16.
    #[arg(long, default_value = "16")]
    max_slots: usize,

    /// Enable git worktree isolation for slots.
    #[arg(short = 'w', long)]
    worktree: bool,

    /// Disable built-in skills.
    #[arg(long)]
    no_builtins: bool,

    /// Directory to load project-local skill definitions from.
    #[arg(long, default_value = ".claude-pool/skills")]
    skills_dir: PathBuf,

    /// Skip loading project-local skills.
    #[arg(long)]
    no_project_skills: bool,

    /// Disable specific skills by name (comma-separated).
    #[arg(long, value_delimiter = ',')]
    disable_skill: Vec<String>,
}

fn parse_permission_mode(s: &str) -> claude_pool::PermissionMode {
    match s.to_lowercase().as_str() {
        "default" => claude_pool::PermissionMode::Default,
        "acceptedits" => claude_pool::PermissionMode::AcceptEdits,
        "bypasspermissions" => claude_pool::PermissionMode::BypassPermissions,
        "dontask" => claude_pool::PermissionMode::DontAsk,
        "auto" => claude_pool::PermissionMode::Auto,
        _ => claude_pool::PermissionMode::Plan,
    }
}

fn parse_effort(s: &str) -> Option<claude_pool::Effort> {
    match s.to_lowercase().as_str() {
        "min" | "low" => Some(claude_pool::Effort::Low),
        "medium" => Some(claude_pool::Effort::Medium),
        "high" => Some(claude_pool::Effort::High),
        "max" => Some(claude_pool::Effort::Max),
        _ => None,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("claude_pool=info".parse()?)
                .add_directive("claude_pool_server=info".parse()?),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let config = PoolConfig {
        model: cli.model,
        effort: cli.effort.and_then(|e| parse_effort(&e)),
        budget_microdollars: cli.budget_usd.map(|b| (b * 1_000_000.0) as u64),
        system_prompt: cli.system_prompt,
        permission_mode: Some(parse_permission_mode(&cli.permission_mode)),
        worktree_isolation: cli.worktree,
        scaling: claude_pool::ScalingConfig {
            min_slots: cli.min_slots,
            max_slots: cli.max_slots,
        },
        ..Default::default()
    };

    let claude = claude_wrapper::Claude::builder().build()?;

    let pool = Pool::builder_with_store(claude, InMemoryStore::new())
        .slots(cli.slots)
        .config(config)
        .build()
        .await
        .map_err(|e| format!("failed to build pool: {e}"))?;

    let mut skills = if cli.no_builtins {
        SkillRegistry::new()
    } else {
        SkillRegistry::with_builtins()
    };

    if !cli.no_project_skills {
        let count = skills.load_from_dir(&cli.skills_dir)?;
        if count > 0 {
            tracing::info!(count, dir = %cli.skills_dir.display(), "loaded project skills");
        }
    }

    if !cli.disable_skill.is_empty() {
        let names: Vec<&str> = cli.disable_skill.iter().map(|s| s.as_str()).collect();
        skills.remove_many(&names);
        tracing::info!(disabled = ?cli.disable_skill, "disabled skills");
    }

    // Build prompts before wrapping skills in RwLock (prompts are static at startup).
    let prompt_list = prompts::skill_prompts(&skills);

    let workflows = WorkflowRegistry::with_builtins();

    let state = Arc::new(State {
        pool,
        skills: Arc::new(RwLock::new(skills)),
        workflows,
        skills_dir: cli.skills_dir,
    });

    let tool_list = tools::all_tools(&state);
    let resource_list = resources::all_resources(&state);
    let template_list = resources::all_templates(&state);

    let mut router = McpRouter::new()
        .server_info("claude-pool", env!("CARGO_PKG_VERSION"))
        .instructions(
            "Execution modes: use inline for decisions and interactive work; pool_run for simple \
             administrative tasks; pool_submit_chain for multi-step workflows (plan/code/review/PR) \
             to keep conversations responsive; pool_fan_out for parallel tasks; Agent tool for \
             research requiring MCP tools (GitHub, crates.io). Pool slots have CLI tools (git, \
             cargo, gh) but not MCP access. Default to inline when uncertain; user can say \
             \"slotize it.\" \
             \
             Task sizing: Single task (pool_run) = one clear action with one clear output; if using \
             \"and\" more than once, use a chain instead. Chain = workflow where steps feed into each \
             other; natural unit is a deliverable (PR, report, resolved issue); each step should be \
             independently verifiable (can't describe success of step N without referencing N+1 = \
             steps too coupled). Fan-out = N independent instances of same work; use if items don't \
             depend on each other; use chain if they do. \
             \
             Tools: pool_run (synchronous), pool_submit/pool_result (async), pool_fan_out (parallel), \
             pool_chain (synchronous pipeline), pool_submit_chain/pool_chain_result (async pipeline \
             with per-step progress), context_set/get/list (shared state), pool_configure_slot. \
             Both effort and model can be overridden per-task (pool_run config) and per-chain-step \
             (step config) to fine-tune cost and quality. \
             \
             Model guidance: default to the pool's configured model; override per-task/step with \
             the model field when needed. Haiku: bounded single tasks (create issue, run checks, \
             rebase, label), template-driven work, high-volume fan-outs where speed matters. \
             Sonnet: code review needing subtlety, multi-file changes with dependencies, planning \
             steps where quality matters. Opus: large mechanical refactors where one mistake breaks \
             compilation, complex architectural reasoning, tasks where you would want a senior \
             engineer. Rule of thumb: how much does the task benefit from deeper thinking? \
             \
             Skills available as prompts: code_review, implement, write_tests, refactor, summarize. \
             Skills management: pool_skill_list (discover), pool_skill_get (inspect), \
             pool_skill_add (register ephemeral), pool_skill_remove (unregister), \
             pool_skill_save (persist to disk). \
             \
             Scheduling: use Claude Code's /loop to run tasks on a recurring interval \
             (e.g. `/loop 30m check pool status`). /loop fires while idle (session-only). For \
             unattended scheduling, use cron or systemd timers calling `claude -p` directly. \
             The pool server is stateless and reactive; scheduling is handled by the client.",
        )
        .tools(tool_list)
        .resources(resource_list)
        .prompts(prompt_list);

    for template in template_list {
        router = router.resource_template(template);
    }

    tracing::info!(slots = cli.slots, "claude-pool-server starting");

    let mut transport = StdioTransport::new(router);
    transport.run().await?;

    Ok(())
}
