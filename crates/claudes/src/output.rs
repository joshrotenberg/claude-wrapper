//! Output formatting for task execution results.

use std::io::Write;

use crate::runner::{RunResult, TaskResult};

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
    let status = if task.success { "complete" } else { "FAILED" };
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
