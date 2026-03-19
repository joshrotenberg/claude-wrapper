//! Output formatting for task execution results.

use std::collections::HashMap;
use std::io::{IsTerminal, Write};

use crossterm::style::{Color, Stylize};
use tokio::sync::mpsc;

use crate::runner::{RunResult, TaskEvent, TaskResult};
use crate::state::is_timeout;

const COLOR_PALETTE: &[Color] = &[
    Color::Rgb {
        r: 0,
        g: 255,
        b: 255,
    }, // bright cyan
    Color::Rgb {
        r: 0,
        g: 255,
        b: 128,
    }, // bright green
    Color::Rgb {
        r: 255,
        g: 255,
        b: 0,
    }, // bright yellow
    Color::Rgb {
        r: 255,
        g: 128,
        b: 255,
    }, // bright magenta
    Color::Rgb {
        r: 128,
        g: 128,
        b: 255,
    }, // bright blue
    Color::Rgb {
        r: 255,
        g: 128,
        b: 0,
    }, // bright orange
];

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

    let has_cost = result.tasks.iter().any(|t| t.cost_usd.is_some());
    if has_cost {
        let total_cost: f64 = result.tasks.iter().filter_map(|t| t.cost_usd).sum();
        let _ = writeln!(out, "Total cost: ${total_cost:.2}");
    }
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
    let cost_str = task
        .cost_usd
        .map(|c| format!("  ${c:.2}"))
        .unwrap_or_default();

    let _ = writeln!(
        out,
        "  {name:<30} {status:<12} {duration}{cost_str}",
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
pub async fn render_stream(
    mut rx: mpsc::UnboundedReceiver<TaskEvent>,
    verbosity: Verbosity,
    no_color: bool,
) {
    if verbosity == Verbosity::Quiet {
        while rx.recv().await.is_some() {}
        return;
    }

    let stderr = std::io::stderr();
    let use_color = !no_color && std::env::var_os("NO_COLOR").is_none() && stderr.is_terminal();

    let mut color_map: HashMap<String, Color> = HashMap::new();
    let mut color_index: usize = 0;
    let mut start_times: HashMap<String, std::time::Instant> = HashMap::new();

    while let Some(event) = rx.recv().await {
        let task = &event.task_name;
        let data = &event.event.data;
        let event_type = event.event.event_type().unwrap_or("");

        let color_opt = if use_color {
            if !color_map.contains_key(task.as_str()) {
                let c = COLOR_PALETTE[color_index % COLOR_PALETTE.len()];
                color_map.insert(task.clone(), c);
                color_index += 1;
            }
            color_map.get(task.as_str()).copied()
        } else {
            None
        };

        let prefix = {
            let padded = format!("{task:<20}");
            match color_opt {
                Some(color) => format!("{}", padded.with(color)),
                None => padded,
            }
        };

        match event_type {
            "claudes_task_start" => {
                start_times.insert(task.clone(), std::time::Instant::now());
                let mut out = stderr.lock();
                let _ = writeln!(out, "  | {prefix} | starting");
            }
            "result" => {
                let subtype = data
                    .get("subtype")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown");
                let elapsed = start_times
                    .get(task.as_str())
                    .map(|t| t.elapsed().as_secs())
                    .unwrap_or(0);
                let cost = data
                    .get("total_cost_usd")
                    .or_else(|| data.get("cost_usd"))
                    .and_then(|c| c.as_f64());
                let line = if subtype == "success" {
                    let cost_str = cost.map(|c| format!(", ${c:.2}")).unwrap_or_default();
                    format!("  | {prefix} | complete ({elapsed}s{cost_str})")
                } else if subtype == "error_max_turns" {
                    format!("  | {prefix} | TIMEOUT ({elapsed}s)")
                } else {
                    format!("  | {prefix} | FAILED ({elapsed}s)")
                };
                let mut out = stderr.lock();
                let _ = writeln!(out, "{line}");
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
                                let cwd = std::env::current_dir()
                                    .ok()
                                    .map(|p| p.to_string_lossy().into_owned());
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
                                        let s = if let Some(ref cwd) = cwd {
                                            let prefix = format!("{cwd}/");
                                            if s.starts_with(&prefix) {
                                                s[prefix.len()..].to_string()
                                            } else {
                                                s
                                            }
                                        } else {
                                            s
                                        };
                                        // Also strip .worktrees/<task-name>/ prefix.
                                        let s = if let Some(rest) = s.strip_prefix(".worktrees/") {
                                            rest.split_once('/')
                                                .map_or(s.clone(), |(_, after)| after.to_string())
                                        } else {
                                            s
                                        };
                                        let mut chars = s.chars();
                                        let truncated: String = chars.by_ref().take(60).collect();
                                        if chars.next().is_some() {
                                            format!("{truncated}...")
                                        } else {
                                            truncated
                                        }
                                    })
                                    .unwrap_or_default();
                                let mut out = stderr.lock();
                                if first_arg.is_empty() {
                                    let _ = writeln!(out, "  | {prefix} | {tool}");
                                } else {
                                    let _ = writeln!(out, "  | {prefix} | {tool}({first_arg})");
                                }
                            }
                            Some("text") if verbosity >= Verbosity::VeryVerbose => {
                                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                    let mut out = stderr.lock();
                                    let _ = writeln!(out, "  | {prefix} | {text}");
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
