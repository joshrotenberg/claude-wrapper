//! Supervised worker pattern: restart a Claude agent on failure.
//!
//! Demonstrates how to build a restart loop with session resumption
//! and budget tracking -- the building blocks for long-running agent
//! orchestration. This is a pattern example, not a library feature;
//! the goal is to explore where friction exists before deciding
//! whether to abstract this into the library.
//!
//! ```sh
//! cargo run --example supervised_worker
//! ```

#![allow(dead_code, unused_variables, unused_mut)]

use std::time::Duration;

use claude_wrapper::{
    Claude, Error, McpConfigBuilder, OutputFormat, PermissionMode, QueryCommand, RetryPolicy,
};

/// Configuration for a supervised worker.
struct WorkerConfig {
    prompt: String,
    max_restarts: u32,
    total_budget_usd: f64,
    restart_delay: Duration,
    resume_session: bool,
}

/// Tracks state across worker restarts.
struct WorkerState {
    restarts: u32,
    total_cost: f64,
    last_session_id: Option<String>,
}

impl WorkerState {
    fn new() -> Self {
        Self {
            restarts: 0,
            total_cost: 0.0,
            last_session_id: None,
        }
    }
}

/// Run a supervised worker loop.
///
/// This is the core pattern: run a query, track costs, and restart
/// on failure with optional session resumption. The loop exits when:
/// - The query succeeds
/// - Max restarts exceeded
/// - Total budget exceeded
async fn run_supervised(
    claude: &Claude,
    config: &WorkerConfig,
    state: &mut WorkerState,
) -> claude_wrapper::Result<()> {
    loop {
        // Build the command, optionally resuming the previous session
        let mut cmd = QueryCommand::new(&config.prompt)
            .output_format(OutputFormat::Json)
            .permission_mode(PermissionMode::Auto)
            .max_turns(20)
            .no_session_persistence();

        if config.resume_session {
            if let Some(ref session_id) = state.last_session_id {
                cmd = cmd.resume(session_id);
            }
        }

        println!(
            "[supervisor] starting worker (attempt {}/{}), total cost: ${:.4}",
            state.restarts + 1,
            config.max_restarts + 1,
            state.total_cost,
        );

        match cmd.execute_json(claude).await {
            Ok(result) => {
                // Track cost
                if let Some(cost) = result.cost_usd {
                    state.total_cost += cost;
                }
                state.last_session_id = Some(result.session_id.clone());

                if result.is_error {
                    println!(
                        "[supervisor] worker returned error result: {}",
                        result.result
                    );
                    // Fall through to restart logic
                } else {
                    println!("[supervisor] worker completed: {}", result.result);
                    println!("[supervisor] total cost: ${:.4}", state.total_cost);
                    return Ok(());
                }
            }
            Err(Error::Timeout { timeout_seconds }) => {
                println!("[supervisor] worker timed out after {timeout_seconds}s");
            }
            Err(Error::CommandFailed {
                exit_code, stderr, ..
            }) => {
                println!("[supervisor] worker failed (exit {exit_code}): {stderr}");
            }
            Err(e) => {
                // Non-retryable error (NotFound, etc.)
                return Err(e);
            }
        }

        // Check restart limits
        state.restarts += 1;
        if state.restarts > config.max_restarts {
            println!(
                "[supervisor] max restarts ({}) exceeded",
                config.max_restarts
            );
            return Err(Error::CommandFailed {
                command: "supervised_worker".into(),
                exit_code: 1,
                stdout: String::new(),
                stderr: format!("max restarts ({}) exceeded", config.max_restarts),
            });
        }

        // Check budget
        if state.total_cost >= config.total_budget_usd {
            println!(
                "[supervisor] budget exhausted (${:.4} >= ${:.4})",
                state.total_cost, config.total_budget_usd
            );
            return Err(Error::CommandFailed {
                command: "supervised_worker".into(),
                exit_code: 1,
                stdout: String::new(),
                stderr: format!(
                    "budget exhausted: ${:.4} >= ${:.4}",
                    state.total_cost, config.total_budget_usd
                ),
            });
        }

        println!(
            "[supervisor] restarting in {}s...",
            config.restart_delay.as_secs()
        );
        tokio::time::sleep(config.restart_delay).await;
    }
}

#[tokio::main]
async fn main() -> claude_wrapper::Result<()> {
    // Build a client with per-attempt retry for transient failures
    // (this handles flaky CLI invocations, distinct from the restart loop
    // which handles task-level failures)
    let claude = Claude::builder()
        .retry(
            RetryPolicy::new()
                .max_attempts(2)
                .initial_backoff(Duration::from_secs(1))
                .retry_on_timeout(true),
        )
        .timeout_secs(300)
        .build()?;

    // Optionally set up MCP connectivity
    let _config = McpConfigBuilder::new()
        .http_server("hub", "http://127.0.0.1:9090")
        .build_temp()?;

    let worker_config = WorkerConfig {
        prompt: "You are a worker agent. Process the next item from the queue.".into(),
        max_restarts: 5,
        total_budget_usd: 2.00,
        restart_delay: Duration::from_secs(5),
        resume_session: true,
    };

    let mut state = WorkerState::new();

    // Print what this would do (uncomment run_supervised to actually execute)
    println!("[supervisor] worker config:");
    println!("  prompt: {:?}", worker_config.prompt);
    println!("  max_restarts: {}", worker_config.max_restarts);
    println!("  total_budget: ${:.2}", worker_config.total_budget_usd);
    println!("  restart_delay: {:?}", worker_config.restart_delay);
    println!("  resume_session: {}", worker_config.resume_session);
    println!();
    println!("[supervisor] two layers of resilience:");
    println!(
        "  1. RetryPolicy on Claude client: retries transient CLI failures (timeouts, flaky exits)"
    );
    println!(
        "  2. Supervisor loop: restarts on task-level failures with session resume + budget tracking"
    );
    println!();
    println!("Uncomment run_supervised() to execute against a real Claude instance.");

    // Uncomment to run single worker:
    // run_supervised(&claude, &worker_config, &mut state).await?;

    // --- Multi-worker pool pattern ---
    // Spawn N workers concurrently, each with its own session and budget slice.
    // This is what a library-level WorkerPool would encapsulate.

    println!();
    println!("[pool] multi-worker pattern (N=4):");

    let num_workers = 4;
    let per_worker_budget = 2.00;

    let mut handles = Vec::new();
    for id in 0..num_workers {
        let claude = claude.clone();
        let config = McpConfigBuilder::new()
            .http_server("hub", "http://127.0.0.1:9090")
            .build_temp()?;

        handles.push(tokio::spawn(async move {
            let mut worker_state = WorkerState::new();
            let worker_config = WorkerConfig {
                prompt: format!(
                    "You are worker-{id}. Call hub/claim to get a task, \
                     implement it, then call hub/complete when done."
                ),
                max_restarts: 3,
                total_budget_usd: per_worker_budget,
                restart_delay: Duration::from_secs(5),
                resume_session: true,
            };

            println!("[pool] worker-{id} starting");
            let result = run_supervised(&claude, &worker_config, &mut worker_state).await;
            println!(
                "[pool] worker-{id} finished: cost=${:.4}, restarts={}",
                worker_state.total_cost, worker_state.restarts
            );
            (id, result, worker_state.total_cost)
        }));
    }

    println!("[pool] {} workers would run concurrently", handles.len());
    println!(
        "[pool] total budget cap: ${:.2}",
        num_workers as f64 * per_worker_budget
    );
    println!();
    println!("Uncomment the JoinSet loop to actually execute.");

    // Uncomment to run all workers:
    // let mut total_cost = 0.0;
    // for handle in handles {
    //     let (id, result, cost) = handle.await.unwrap();
    //     total_cost += cost;
    //     match result {
    //         Ok(()) => println!("[pool] worker-{id} succeeded"),
    //         Err(e) => println!("[pool] worker-{id} failed: {e}"),
    //     }
    // }
    // println!("[pool] all workers done, total cost: ${total_cost:.4}");

    Ok(())
}
