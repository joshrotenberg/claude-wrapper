//! Output modes for task execution.
//!
//! Three mutually exclusive modes:
//! - **Progress** (default TTY) — indicatif spinners with live status
//! - **Ndjson** (default piped) — structured NDJSON, superset of Claude events
//! - **Quiet** — exit code only

use std::collections::HashMap;
use std::io::{IsTerminal, Write};

use chrono::Utc;
use crossterm::style::{Color, Stylize};
use tokio::sync::mpsc;

use crate::manifest::Manifest;
use crate::runner::{RunResult, TaskEvent};
use crate::state::is_timeout;

const COLOR_PALETTE: &[Color] = &[
    Color::Rgb {
        r: 0,
        g: 255,
        b: 255,
    }, // cyan
    Color::Rgb {
        r: 0,
        g: 255,
        b: 128,
    }, // green
    Color::Rgb {
        r: 255,
        g: 255,
        b: 0,
    }, // yellow
    Color::Rgb {
        r: 255,
        g: 128,
        b: 255,
    }, // magenta
    Color::Rgb {
        r: 128,
        g: 128,
        b: 255,
    }, // blue
    Color::Rgb {
        r: 255,
        g: 128,
        b: 0,
    }, // orange
];

/// Output mode — mutually exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Live indicatif progress display (default when TTY).
    Progress,
    /// Structured NDJSON on stdout (default when piped).
    Ndjson,
    /// Exit code only, no output.
    Quiet,
}

impl OutputMode {
    /// Select the default mode based on TTY detection and flags.
    pub fn detect(output_flag: &str, quiet: bool) -> Self {
        match output_flag {
            "json" | "ndjson" => OutputMode::Ndjson,
            "quiet" => OutputMode::Quiet,
            _ if quiet => OutputMode::Quiet,
            _ if std::io::stderr().is_terminal() => OutputMode::Progress,
            _ => OutputMode::Ndjson,
        }
    }
}

// ============================================================================
// Bookends — manifest summary and run results
// ============================================================================

/// Print the opening bookend: what we're about to do.
pub fn print_run_start(manifest: &Manifest, mode: OutputMode, no_color: bool) {
    match mode {
        OutputMode::Progress => print_run_start_progress(manifest, no_color),
        OutputMode::Ndjson => print_run_start_ndjson(manifest),
        OutputMode::Quiet => {}
    }
}

/// Print the closing bookend: what we did.
pub fn print_run_complete(result: &RunResult, mode: OutputMode, no_color: bool) {
    match mode {
        OutputMode::Progress => print_run_complete_progress(result, no_color),
        OutputMode::Ndjson => print_run_complete_ndjson(result),
        OutputMode::Quiet => {}
    }
}

fn print_run_start_progress(manifest: &Manifest, no_color: bool) {
    let stderr = std::io::stderr();
    let use_color = !no_color && std::env::var_os("NO_COLOR").is_none() && stderr.is_terminal();
    let mut out = stderr.lock();

    let task_count = manifest.tasks.len();
    let model = manifest
        .shared
        .as_ref()
        .and_then(|s| s.model.as_deref())
        .or_else(|| manifest.tasks.first().and_then(|t| t.model.as_deref()))
        .unwrap_or("default");
    let isolation = manifest
        .shared
        .as_ref()
        .and_then(|s| s.isolation.as_ref())
        .map(|i| match i {
            crate::manifest::Isolation::Worktree { .. } => "worktree",
            crate::manifest::Isolation::Clone { .. } => "clone",
            crate::manifest::Isolation::None => "none",
        })
        .unwrap_or("worktree");

    let _ = writeln!(out);
    let header = format!("  {task_count} tasks, model: {model}, isolation: {isolation}");
    if use_color {
        let _ = writeln!(out, "{}", header.with(Color::White));
    } else {
        let _ = writeln!(out, "{header}");
    }

    for (i, task) in manifest.tasks.iter().enumerate() {
        let color = if use_color {
            Some(COLOR_PALETTE[i % COLOR_PALETTE.len()])
        } else {
            None
        };
        let name = truncate_name(&task.name, 30);
        let branch = task
            .branch
            .as_deref()
            .map(|b| format!(" ({b})"))
            .unwrap_or_default();
        let line = format!("    {name}{branch}");
        match color {
            Some(c) => {
                let _ = writeln!(out, "{}", line.with(c));
            }
            None => {
                let _ = writeln!(out, "{line}");
            }
        }
    }
    let _ = writeln!(out);
}

fn print_run_complete_progress(result: &RunResult, no_color: bool) {
    let stderr = std::io::stderr();
    let use_color = !no_color && std::env::var_os("NO_COLOR").is_none() && stderr.is_terminal();
    let mut out = stderr.lock();

    let _ = writeln!(out);

    for task in &result.tasks {
        let status = if task.success {
            "ok"
        } else if is_timeout(&task.stdout, &task.stderr) {
            "TIMEOUT"
        } else {
            "FAILED"
        };
        let duration = format!("{:.0}s", task.duration.as_secs_f64());
        let cost_str = task
            .cost_usd
            .map(|c| format!("${c:.4}"))
            .unwrap_or_default();
        let files_str = task
            .files_modified
            .map(|f| {
                let lines = task.lines_changed.unwrap_or(0);
                format!("  {f} files +{lines}")
            })
            .unwrap_or_default();

        let name = truncate_name(&task.name, 30);

        if use_color {
            let status_color = if task.success {
                Color::Green
            } else {
                Color::Red
            };
            let _ = write!(out, "  {:<30} ", name);
            let _ = write!(out, "{}", format!("{status:<12}").with(status_color));
            let _ = writeln!(out, " {duration:>6}  {cost_str:<10}{files_str}");
        } else {
            let _ = writeln!(
                out,
                "  {name:<30} {status:<12} {duration:>6}  {cost_str:<10}{files_str}",
            );
        }

        if !task.success && !task.stderr.is_empty() {
            for line in task.stderr.lines().take(5) {
                let _ = writeln!(out, "    {line}");
            }
        }
    }

    let total = result.tasks.len();
    let succeeded = result.success_count();
    let wall_time = result
        .tasks
        .iter()
        .map(|t| t.duration.as_secs_f64())
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap_or(0.0);

    let _ = writeln!(out);
    let summary = format!("{succeeded}/{total} succeeded, wall time: {wall_time:.1}s");
    if use_color {
        let color = if succeeded == total {
            Color::Green
        } else {
            Color::Yellow
        };
        let _ = write!(out, "  {}", summary.with(color));
    } else {
        let _ = write!(out, "  {summary}");
    }

    let has_cost = result.tasks.iter().any(|t| t.cost_usd.is_some());
    if has_cost {
        let total_cost: f64 = result.tasks.iter().filter_map(|t| t.cost_usd).sum();
        let _ = write!(out, ", total cost: ${total_cost:.2}");
    }
    let _ = writeln!(out);
    let _ = writeln!(out);
}

fn print_run_start_ndjson(manifest: &Manifest) {
    let task_names: Vec<&str> = manifest.tasks.iter().map(|t| t.name.as_str()).collect();
    let model = manifest
        .shared
        .as_ref()
        .and_then(|s| s.model.as_deref())
        .unwrap_or("default");
    let event = serde_json::json!({
        "type": "run_start",
        "tasks": task_names,
        "task_count": manifest.tasks.len(),
        "model": model,
        "timestamp": Utc::now().to_rfc3339(),
    });
    println!("{}", serde_json::to_string(&event).unwrap_or_default());
}

fn print_run_complete_ndjson(result: &RunResult) {
    let total_cost: Option<f64> = {
        let costs: Vec<f64> = result.tasks.iter().filter_map(|t| t.cost_usd).collect();
        if costs.is_empty() {
            None
        } else {
            Some(costs.iter().sum())
        }
    };
    let wall_time = result
        .tasks
        .iter()
        .map(|t| t.duration.as_secs_f64())
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap_or(0.0);

    let tasks: Vec<serde_json::Value> = result
        .tasks
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "success": t.success,
                "duration_secs": t.duration.as_secs_f64(),
                "cost_usd": t.cost_usd,
                "files_modified": t.files_modified,
                "lines_changed": t.lines_changed,
                "work_dir": t.work_dir.to_string_lossy(),
            })
        })
        .collect();

    let event = serde_json::json!({
        "type": "run_complete",
        "total": result.tasks.len(),
        "succeeded": result.success_count(),
        "failed": result.tasks.iter().filter(|t| !t.success).count(),
        "wall_time_secs": wall_time,
        "total_cost_usd": total_cost,
        "tasks": tasks,
        "timestamp": Utc::now().to_rfc3339(),
    });
    println!("{}", serde_json::to_string(&event).unwrap_or_default());
}

// ============================================================================
// Streaming renderers
// ============================================================================

/// Render streaming events as in-place indicatif progress bars.
///
/// Each task gets a spinner that updates in place showing elapsed time and
/// current activity (latest tool call). On completion the spinner is replaced
/// with a final status line. No scrolling — everything updates in place.
pub async fn render_progress(mut rx: mpsc::UnboundedReceiver<TaskEvent>, no_color: bool) {
    use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

    let stderr = std::io::stderr();
    let use_color = !no_color && std::env::var_os("NO_COLOR").is_none() && stderr.is_terminal();

    let mp = MultiProgress::new();
    let mut bars: HashMap<String, ProgressBar> = HashMap::new();
    let mut color_index: usize = 0;
    let mut color_map: HashMap<String, Color> = HashMap::new();
    let mut total_tasks: usize = 0;
    let mut completed_tasks: usize = 0;
    let mut total_cost: f64 = 0.0;

    let spinner_style =
        ProgressStyle::with_template("  {spinner:.cyan} {prefix:<22} {elapsed:>5}  {msg}")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏");

    // Gap between task spinners and footer.
    let gap = mp.add(ProgressBar::new_spinner());
    gap.set_style(ProgressStyle::with_template(" ").unwrap());
    gap.finish();

    // Footer bar — shows aggregate progress.
    let footer = mp.add(ProgressBar::new_spinner());

    // Spacer bar — just a blank line below the footer for breathing room.
    let spacer = mp.add(ProgressBar::new_spinner());
    spacer.set_style(ProgressStyle::with_template(" ").unwrap());
    spacer.finish();
    footer.enable_steady_tick(std::time::Duration::from_millis(500));

    while let Some(event) = rx.recv().await {
        let task = &event.task_name;
        let data = &event.event.data;
        let event_type = event.event.event_type().unwrap_or("");

        // Assign a color for this task.
        if use_color && !color_map.contains_key(task.as_str()) {
            let c = COLOR_PALETTE[color_index % COLOR_PALETTE.len()];
            color_map.insert(task.clone(), c);
            color_index += 1;
        }

        let task_prefix = truncate_name(task, 20);

        match event_type {
            "claudes_task_start" => {
                total_tasks += 1;
                let pb = mp.insert_before(&gap, ProgressBar::new_spinner());
                pb.set_style(spinner_style.clone());
                pb.set_prefix(task_prefix);
                pb.set_message("starting");
                pb.enable_steady_tick(std::time::Duration::from_millis(100));
                bars.insert(task.clone(), pb);
                footer.set_message(format!("{completed_tasks}/{total_tasks} complete"));
            }
            "result" => {
                let subtype = data
                    .get("subtype")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown");
                let cost = data
                    .get("total_cost_usd")
                    .or_else(|| data.get("cost_usd"))
                    .and_then(|c| c.as_f64());

                if let Some(pb) = bars.get(task.as_str()) {
                    let elapsed = format!("{:.0}s", pb.elapsed().as_secs_f64());
                    let cost_str = cost.map(|c| format!("  ${c:.2}")).unwrap_or_default();

                    if subtype == "success" {
                        let msg = format!("ok  {elapsed}{cost_str}");
                        let finish = if use_color {
                            format!(
                                "  {}  {:<22} {}",
                                "✓".with(Color::Green),
                                task_prefix
                                    .with(*color_map.get(task.as_str()).unwrap_or(&Color::White)),
                                msg.with(Color::Green)
                            )
                        } else {
                            format!("  ✓  {task_prefix:<22} {msg}")
                        };
                        pb.finish_with_message("");
                        pb.set_style(ProgressStyle::with_template("{msg}").unwrap());
                        pb.finish_with_message(finish);
                    } else {
                        let status = if subtype == "error_max_turns" {
                            "TIMEOUT"
                        } else {
                            "FAILED"
                        };
                        let msg = format!("{status}  {elapsed}");
                        let finish = if use_color {
                            format!(
                                "  {}  {:<22} {}",
                                "✗".with(Color::Red),
                                task_prefix
                                    .with(*color_map.get(task.as_str()).unwrap_or(&Color::White)),
                                msg.with(Color::Red)
                            )
                        } else {
                            format!("  ✗  {task_prefix:<22} {msg}")
                        };
                        pb.finish_with_message("");
                        pb.set_style(ProgressStyle::with_template("{msg}").unwrap());
                        pb.finish_with_message(finish);
                    }
                }

                completed_tasks += 1;
                if let Some(c) = cost {
                    total_cost += c;
                }
                let cost_msg = if total_cost > 0.0 {
                    format!("  ${total_cost:.2}")
                } else {
                    String::new()
                };
                footer.set_message(format!(
                    "{completed_tasks}/{total_tasks} complete{cost_msg}"
                ));
                if completed_tasks == total_tasks {
                    footer.finish_with_message("");
                }
            }
            "assistant" => {
                if let Some(content) = data
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for block in content {
                        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                            let tool = block
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("unknown");
                            let first_arg = extract_tool_arg(block);
                            let msg = if first_arg.is_empty() {
                                tool.to_string()
                            } else {
                                format!("{tool}({first_arg})")
                            };
                            if let Some(pb) = bars.get(task.as_str()) {
                                pb.set_message(msg);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Render streaming events as NDJSON on stdout.
///
/// Passes through raw Claude StreamEvents with the task name injected.
/// Adds claudes-specific events (task_start, task_complete) using distinct types.
pub async fn render_ndjson(mut rx: mpsc::UnboundedReceiver<TaskEvent>) {
    let mut start_times: HashMap<String, std::time::Instant> = HashMap::new();

    while let Some(event) = rx.recv().await {
        let task = &event.task_name;
        let data = &event.event.data;
        let event_type = event.event.event_type().unwrap_or("");

        match event_type {
            "claudes_task_start" => {
                start_times.insert(task.clone(), std::time::Instant::now());
                let ev = serde_json::json!({
                    "type": "task_start",
                    "task": task,
                    "timestamp": Utc::now().to_rfc3339(),
                });
                println!("{}", serde_json::to_string(&ev).unwrap_or_default());
            }
            "result" => {
                // Emit the raw result event with task injected.
                let mut raw = data.clone();
                if let serde_json::Value::Object(ref mut map) = raw {
                    map.insert("task".into(), serde_json::Value::String(task.clone()));
                    map.insert(
                        "timestamp".into(),
                        serde_json::Value::String(Utc::now().to_rfc3339()),
                    );
                }
                println!("{}", serde_json::to_string(&raw).unwrap_or_default());

                // Also emit a claudes task_complete event.
                let elapsed = start_times
                    .get(task.as_str())
                    .map(|t| t.elapsed().as_secs_f64())
                    .unwrap_or(0.0);
                let cost = data
                    .get("total_cost_usd")
                    .or_else(|| data.get("cost_usd"))
                    .and_then(|c| c.as_f64());
                let subtype = data
                    .get("subtype")
                    .and_then(|s| s.as_str())
                    .unwrap_or("unknown");
                let success = subtype == "success";

                let ev = serde_json::json!({
                    "type": "task_complete",
                    "task": task,
                    "success": success,
                    "duration_secs": elapsed,
                    "cost_usd": cost,
                    "timestamp": Utc::now().to_rfc3339(),
                });
                println!("{}", serde_json::to_string(&ev).unwrap_or_default());
            }
            _ => {
                // Pass through all other events with task injected.
                let mut raw = data.clone();
                if let serde_json::Value::Object(ref mut map) = raw {
                    map.insert("task".into(), serde_json::Value::String(task.clone()));
                }
                println!("{}", serde_json::to_string(&raw).unwrap_or_default());
            }
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Extract the first tool argument from a tool_use block, with path stripping.
fn extract_tool_arg(block: &serde_json::Value) -> String {
    block
        .get("input")
        .and_then(|i| i.as_object())
        .and_then(|obj| obj.values().next())
        .map(|v| {
            let s = if let Some(s) = v.as_str() {
                s.to_string()
            } else {
                v.to_string()
            };
            // Strip cwd prefix.
            let s = if let Ok(cwd) = std::env::current_dir() {
                let prefix = format!("{}/", cwd.display());
                if let Some(rest) = s.strip_prefix(&prefix) {
                    rest.to_string()
                } else {
                    s
                }
            } else {
                s
            };
            // Strip .worktrees/<task-name>/ prefix.
            let s = if let Some(rest) = s.strip_prefix(".worktrees/") {
                rest.split_once('/')
                    .map_or(s.clone(), |(_, after)| after.to_string())
            } else if let Some(idx) = s.find("/.worktrees/") {
                let rest = &s[idx + "/.worktrees/".len()..];
                rest.split_once('/')
                    .map_or(s.clone(), |(_, p)| p.to_string())
            } else {
                s
            };
            truncate(&s, 60)
        })
        .unwrap_or_default()
}

fn truncate_name(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let truncated: String = s.chars().take(max - 3).collect();
        format!("{truncated}...")
    } else {
        s.to_string()
    }
}

fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let truncated: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}
