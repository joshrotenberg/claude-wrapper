//! Output formatting for task execution results.

use std::io::Write;

use tokio::sync::mpsc;

use crate::runner::{RunResult, TaskEvent, TaskResult};
use crate::state::is_timeout;

/// Verbosity level for streaming output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Verbosity {
    /// No streaming output.
    Quiet,
    /// Show task start and task complete with cost (default).
    #[default]
    Default,
    /// Show tool calls with their first argument.
    Verbose,
    /// Show full event stream including assistant text.
    VeryVerbose,
    /// Show full event stream (same as VeryVerbose).
    Debug,
}

impl From<u8> for Verbosity {
    fn from(count: u8) -> Self {
        match count {
            0 => Verbosity::Default,
            1 => Verbosity::Verbose,
            2 => Verbosity::VeryVerbose,
            _ => Verbosity::Debug,
        }
    }
}

/// Format style for output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable streaming text.
    Text,
    /// Structured JSON.
    Json,
    /// Exit code only, no output.
    Quiet,
}

/// Print a summary of the run results.
pub fn print_summary(result: &RunResult, format: OutputFormat) {
    match format {
        OutputFormat::Text => print_text_summary(result),
        OutputFormat::Json => print_json_summary(result),
        OutputFormat::Quiet => {}
    }
}

fn print_text_summary(result: &RunResult) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let _ = writeln!(out);
    for task in &result.tasks {
        print_task_result(&mut out, task);
    }

    let total = result.tasks.len();
    let succeeded = result.success_count();
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{succeeded}/{total} tasks complete. Total time: {:.1}s",
        result
            .tasks
            .iter()
            .map(|t| t.duration.as_secs_f64())
            .max_by(|a, b| a.partial_cmp(b).unwrap())
            .unwrap_or(0.0)
    );
}

fn print_task_result(out: &mut impl Write, task: &TaskResult) {
    let status = if task.success {
        "complete"
    } else if is_timeout(&task.stdout, &task.stderr) {
        "TIMEOUT"
    } else {
        "FAILED"
    };
    let duration = format!("{:.0}s", task.duration.as_secs_f64());

    let _ = writeln!(
        out,
        "  {name:<30} {status:<12} {duration}",
        name = task.name,
    );

    if !task.success && !task.stderr.is_empty() {
        for line in task.stderr.lines().take(5) {
            let _ = writeln!(out, "    {line}");
        }
    }
}

fn print_json_summary(result: &RunResult) {
    let tasks: Vec<serde_json::Value> = result
        .tasks
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "success": t.success,
                "duration_secs": t.duration.as_secs_f64(),
                "work_dir": t.work_dir.to_string_lossy(),
                "stdout": t.stdout,
                "stderr": t.stderr,
            })
        })
        .collect();

    let json = serde_json::json!({
        "tasks": tasks,
        "all_succeeded": result.all_succeeded(),
        "success_count": result.success_count(),
        "total_count": result.tasks.len(),
    });

    println!("{}", serde_json::to_string_pretty(&json).unwrap());
}

/// Render streaming events from tasks as they arrive.
///
/// Runs until the channel is closed (all senders dropped).
pub async fn render_stream(mut rx: mpsc::UnboundedReceiver<TaskEvent>, verbosity: Verbosity) {
    if verbosity == Verbosity::Quiet {
        while rx.recv().await.is_some() {}
        return;
    }

    let stderr = std::io::stderr();

    while let Some(event) = rx.recv().await {
        let task = &event.task_name;
        let data = &event.event.data;
        let event_type = event.event.event_type().unwrap_or("");

        match event_type {
            "claudes_task_start" => {
                let mut out = stderr.lock();
                let _ = writeln!(out, "  | {task:<20} | starting");
            }
            "result" => {
                let status = data
                    .get("subtype")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown");
                let cost = data
                    .get("total_cost_usd")
                    .or_else(|| data.get("cost_usd"))
                    .and_then(|c| c.as_f64());
                let cost_str = cost.map(|c| format!(" ${c:.4}")).unwrap_or_default();
                let mut out = stderr.lock();
                let _ = writeln!(out, "  | {task:<20} | {status}{cost_str}");
            }
            "assistant" if verbosity >= Verbosity::Verbose => {
                if let Some(content) = data
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content {
                        let block_type = block.get("type").and_then(|t| t.as_str());
                        match block_type {
                            Some("tool_use") => {
                                let tool = block
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("unknown");
                                let first_arg = block
                                    .get("input")
                                    .and_then(|i| i.as_object())
                                    .and_then(|obj| obj.values().next())
                                    .map(|v| {
                                        let s = if let Some(s) = v.as_str() {
                                            s.to_string()
                                        } else {
                                            v.to_string()
                                        };
                                        let mut chars = s.chars();
                                        let truncated: String = chars.by_ref().take(40).collect();
                                        if chars.next().is_some() {
                                            format!("{truncated}...")
                                        } else {
                                            truncated
                                        }
                                    })
                                    .unwrap_or_default();
                                let mut out = stderr.lock();
                                if first_arg.is_empty() {
                                    let _ = writeln!(out, "  | {task:<20} | {tool}");
                                } else {
                                    let _ = writeln!(out, "  | {task:<20} | {tool}({first_arg})");
                                }
                            }
                            Some("text") if verbosity >= Verbosity::VeryVerbose => {
                                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                    let mut out = stderr.lock();
                                    let _ = writeln!(out, "  | {task:<20} | {text}");
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
