# Implementation Plan: Conditional Task Execution (#444)

## Overview

Add a `condition` field to tasks that runs a shell command before spawning the Claude session. If the command exits 0, the task is skipped (condition met = nothing to do). If it exits non-zero, the task runs normally. Skipped tasks cost zero API calls.

## Design Decisions

1. **New `TaskStatus::ConditionSkipped` variant** rather than reusing `Skipped`. The existing `Skipped` means "dependency failed" and is treated as a failure (propagates to dependents). `ConditionSkipped` is a success-like status: the task determined it had nothing to do, so dependents should still run.

2. **Condition runs before isolation setup**. The condition checks whether the task needs to run at all. Running it in the project root (before worktree creation) avoids the cost of creating a worktree for a task that won't execute. The condition command sees the project directory, not a worktree.

3. **Condition runs before pre_hooks**. Ordering: condition -> isolation setup -> pre_hooks -> session -> post_hooks -> finally_hooks. If condition skips, none of the subsequent steps run.

4. **`condition` is NOT inheritable from shared/profiles**. Conditions are task-specific by nature ("only run if this file changed"). Adding it to `Shared` would be confusing. It lives only on `Task`.

5. **Exit code semantics**: exit 0 = "condition met, skip" (the thing you'd check for is already done). Exit non-zero = "condition not met, run the task". This matches the issue's design.

6. **Condition-skipped tasks count as successful** for dependency purposes. If task B depends on task A, and A is condition-skipped, B still runs. This is different from `Skipped` (dependency failure), which blocks dependents.

## Files to Modify

### 1. `crates/claudes/src/manifest.rs`

**Add `condition` field to `Task` struct** (after `finally_hooks`, before `depends_on`):

```rust
/// Shell command to evaluate before running this task.
/// Exit 0 = condition met (skip the task). Exit non-zero = run the task.
/// Runs in the project directory before isolation setup.
#[serde(skip_serializing_if = "Option::is_none")]
pub condition: Option<String>,
```

**Update `Task::new()`** to include `condition: None`.

**Update `resolve()`** to pass through the condition field (no shared/profile inheritance):

```rust
condition: task.condition.clone(),
```

**Update `TaskBuilder`** (if it exists) to include a `condition()` method.

**No changes to `Shared`** -- condition is task-only.

### 2. `crates/claudes/src/state.rs`

**Add `ConditionSkipped` variant to `TaskStatus`**:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Success,
    Failed,
    Timeout,
    Skipped,
    /// Task was skipped because its condition command exited 0.
    ConditionSkipped,
}
```

The `#[serde(rename_all = "lowercase")]` will serialize it as `"conditionskipped"`. Alternatively, use `#[serde(rename = "condition_skipped")]` on the variant for a cleaner JSON key. Decision: add `#[serde(rename = "condition_skipped")]` to the variant.

**Update `build_state()`** to detect condition-skipped tasks. The runner will mark them with a specific stderr sentinel (like `"skipped: dependency failed"` for dep-skipped). Use `"condition: skipped"` as the sentinel:

```rust
let status = if t.success {
    TaskStatus::Success
} else if t.stderr == "condition: skipped" {
    TaskStatus::ConditionSkipped
} else if is_timeout(&t.stdout, &t.stderr) {
    TaskStatus::Timeout
} else if t.stderr.starts_with("skipped: dependency failed") {
    TaskStatus::Skipped
} else {
    TaskStatus::Failed
};
```

Note: condition-skipped tasks should have `success: true` in `TaskResult` so they count toward `success_count()` and don't block dependents. This means the detection in `build_state` should use a different mechanism. Better approach: add a `condition_skipped: bool` field to `TaskResult`, and check it in `build_state`.

**Revised approach**: Add `condition_skipped: bool` to `TaskResult` (runner.rs). Then in `build_state`:

```rust
let status = if t.condition_skipped {
    TaskStatus::ConditionSkipped
} else if t.success {
    TaskStatus::Success
} else if is_timeout(&t.stdout, &t.stderr) {
    TaskStatus::Timeout
} else if t.stderr.starts_with("skipped:") {
    TaskStatus::Skipped
} else {
    TaskStatus::Failed
};
```

**Update `print_status()`** to display `ConditionSkipped`:

```rust
TaskStatus::ConditionSkipped => "COND_SKIP",
```

With color: same grey as `Skipped`.

**Update `RunSummary`** to add a `condition_skipped` count field (or fold it into `succeeded` since it's a success-like outcome). Decision: add `#[serde(default)] pub condition_skipped: usize` to `RunSummary`, and adjust `build_state` to count them. They are *not* counted in `succeeded` or `failed` -- they're their own category.

**Update `compute_metrics()`** to include condition_skipped count in `RunMetrics`.

### 3. `crates/claudes/src/runner.rs`

**Add `condition_skipped: bool` to `TaskResult`**:

```rust
pub struct TaskResult {
    // ... existing fields ...
    /// Whether this task was skipped due to its condition command.
    pub condition_skipped: bool,
}
```

Update all `TaskResult` construction sites to include `condition_skipped: false` (or `true` when appropriate).

**Add `run_condition()` function** (modeled on `run_hook()`):

```rust
/// Evaluate a task's condition command. Returns true if the task should be skipped.
async fn run_condition(
    task_name: &str,
    condition: &str,
    project_dir: &Path,
    event_sender: Option<&mpsc::UnboundedSender<TaskEvent>>,
) -> bool {
    info!(task = task_name, condition = condition, "evaluating condition");
    if let Some(sender) = event_sender {
        let _ = sender.send(TaskEvent {
            task_name: task_name.to_owned(),
            event: StreamEvent {
                data: serde_json::json!({
                    "type": "claudes_condition_start",
                    "task_name": task_name,
                    "command": condition,
                }),
            },
        });
    }
    let start = std::time::Instant::now();
    let result = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(condition)
        .current_dir(project_dir)
        .output()
        .await;
    let duration_ms = start.elapsed().as_millis();

    let (should_skip, exit_code) = match result {
        Ok(output) => (output.status.success(), output.status.code().unwrap_or(-1)),
        Err(e) => {
            warn!(task = task_name, error = %e, "condition command failed to spawn, running task");
            (false, -1)
        }
    };

    if let Some(sender) = event_sender {
        let _ = sender.send(TaskEvent {
            task_name: task_name.to_owned(),
            event: StreamEvent {
                data: serde_json::json!({
                    "type": "claudes_condition_complete",
                    "task_name": task_name,
                    "exit_code": exit_code,
                    "skip": should_skip,
                    "duration_ms": duration_ms,
                }),
            },
        });
    }

    if should_skip {
        info!(task = task_name, "condition met, skipping task");
    } else {
        info!(task = task_name, exit_code = exit_code, "condition not met, running task");
    }

    should_skip
}
```

**Modify `run_task_impl()`** to check condition before isolation setup:

```rust
async fn run_task_impl(task: &Task, options: &RunOptions) -> TaskResult {
    let start = std::time::Instant::now();
    let task_name = task.name.clone();

    // Check condition before doing anything expensive.
    if let Some(condition) = &task.condition {
        if run_condition(&task_name, condition, &options.project_dir, options.event_sender.as_ref()).await {
            return TaskResult {
                name: task_name,
                success: true,
                stdout: String::new(),
                stderr: String::new(),
                duration: start.elapsed(),
                work_dir: options.project_dir.clone(),
                cost_usd: None,
                files_modified: None,
                lines_changed: None,
                condition_skipped: true,
            };
        }
    }

    // ... existing isolation + execution logic ...
}
```

**Important**: The condition check happens inside `run_task_impl`, which is called from `run_task`. This means:
- In the dependency-graph path: the dependency-skip check happens first (in `run()`), then condition check happens inside `run_task_impl`. This is correct — if a dependency failed, we skip without even checking the condition.
- `finally_hooks` should NOT run if condition-skipped (no work was started). Currently `run_task_impl` runs finally_hooks. Since the condition-skipped return is before the `run_task_inner` call, finally_hooks won't run. But we need to restructure slightly because `run_task_impl` currently wraps `run_task_inner` and always runs finally_hooks. Move the condition check before the inner call but after the span setup.

Actually, looking more carefully at `run_task_impl`: it calls `run_task_inner` and then runs `finally_hooks`. The condition check should be placed at the top of `run_task_impl`, before `run_task_inner` is called. The `finally_hooks` block checks `task.finally_hooks` and runs them. If we return early from condition, `finally_hooks` won't run — which is correct since no work was started.

**Update the dependency-graph path in `run()`** to emit a condition-skipped event:

In the dependency-graph path, after condition-skipped tasks complete, they need to emit the right event. Since `run_task` returns a `TaskResult` with `condition_skipped: true`, the caller in `run()` needs to emit `claudes_task_condition_skipped`. But currently `run_task` is spawned into a `JoinSet` and the event handling is done inside the task via `event_sender`. So the condition-skip event emission should happen inside `run_task_impl` (via the `run_condition` function above), not in the main `run()` loop.

**Handle condition-skipped in dependency tracking**: In the `has_dependencies` branch of `run()`, when a task result comes back with `condition_skipped: true`, it should NOT be added to `failed_tasks`. Since `success` is `true`, it won't be. Dependents will proceed normally. This works with the existing logic.

### 4. `crates/claudes/src/output.rs`

**Update `render_progress()`** to handle new event types:

Add handling for `"claudes_condition_start"` and `"claudes_condition_complete"`:

```rust
"claudes_condition_start" => {
    if let Some(pb) = bars.get(task.as_str()) {
        let command = data.get("command").and_then(|c| c.as_str()).unwrap_or("");
        pb.set_message(format!("condition: {}", truncate(command, 50)));
    }
}
"claudes_condition_complete" => {
    let skip = data.get("skip").and_then(|s| s.as_bool()).unwrap_or(false);
    if skip {
        if let Some(pb) = bars.get(task.as_str()) {
            let finish = if use_color {
                format!(
                    "  {}  {:<22} {}",
                    "~".with(Color::DarkGrey),
                    task_prefix.with(Color::DarkGrey),
                    "condition met".with(Color::DarkGrey)
                )
            } else {
                format!("  ~  {task_prefix:<22} condition met")
            };
            pb.finish_with_message("");
            pb.set_style(ProgressStyle::with_template("{msg}").unwrap());
            pb.finish_with_message(finish);
        }
        completed_tasks += 1;
        // ... update footer ...
    }
}
```

**Update `print_run_complete_progress()`** to recognize condition-skipped tasks. Currently it detects skipped tasks by checking `task.stderr.contains("skipped: dependency failed")`. Add detection for condition-skipped via `TaskResult.condition_skipped`:

Actually, `print_run_complete_progress` operates on `RunResult` which contains `TaskResult`. Add condition_skipped detection:

```rust
let is_condition_skipped = task.condition_skipped;
let status = if task.success && !is_condition_skipped {
    "ok"
} else if is_condition_skipped {
    "COND_SKIP"
} else if is_skipped {
    "SKIPPED"
} else if is_timeout(...) {
    "TIMEOUT"
} else {
    "FAILED"
};
```

**Update `render_ndjson()`** to pass through condition events.

### 5. `crates/claudes/src/main.rs`

**Update the `fix` subcommand** to handle `ConditionSkipped` tasks. Currently `fix` re-runs tasks with `TaskStatus::Failed | TaskStatus::Timeout`. Condition-skipped tasks should NOT be re-run by `fix` (they succeeded, just had nothing to do).

No changes needed if `ConditionSkipped` is already excluded from the match. Verify the match arm in `main.rs:449`.

### 6. `crates/claudes/src/mcp.rs`

**Update `fix_tasks` handler** similarly — verify `ConditionSkipped` is not included in the failed/timeout filter at line 318.

### 7. `crates/claudes/src/planner.rs`

**Update plan output** to include `condition` field when generating manifests. The planner's `PlanOptions` and manifest generation should support `condition` in the generated output if provided via CLI. For the initial implementation, this is optional — users can add conditions manually to generated manifests.

### 8. `crates/claudes/src/cli.rs`

No CLI flag needed for condition (it's a manifest-only field). No changes required.

## Serde Support

The `condition` field on `Task` is `Option<String>` with `#[serde(skip_serializing_if = "Option::is_none")]`. Both JSON and TOML will work automatically via serde.

JSON example:
```json
{
  "tasks": [{
    "name": "fix-types",
    "condition": "git diff --quiet HEAD -- src/types.rs",
    "prompt": "Fix the types..."
  }]
}
```

TOML example:
```toml
[[tasks]]
name = "fix-types"
condition = "git diff --quiet HEAD -- src/types.rs"
prompt = "Fix the types..."
```

The `ConditionSkipped` variant in `TaskStatus` needs `#[serde(rename = "condition_skipped")]` since the default `rename_all = "lowercase"` would produce `"conditionskipped"`.

## Edge Cases

### Condition + chained tasks (depends_on)

If task A is condition-skipped and task B depends on A:
- B should still run (condition-skip is success-like)
- B will NOT receive breadcrumbs from A (A didn't produce any)
- This is correct behavior — A determined its work was already done

### Condition + pre_hooks ordering

Condition runs first, before isolation and pre_hooks. If condition skips, pre_hooks never run. This is intentional — conditions are cheaper than hooks and prevent unnecessary work.

### Condition with worktree isolation

Condition runs in the **project root**, not in a worktree. The worktree hasn't been created yet when the condition runs. This means condition commands should reference the main repo state, not worktree state. This is the correct behavior for the primary use cases ("has this file changed?", "are tests passing?").

### Condition command failure (spawn error)

If the condition command fails to spawn (e.g., command not found), the task runs normally. This is the safe default — better to run an unnecessary task than to skip a needed one.

### Condition + `claudes fix`

`fix` re-runs failed/timed-out tasks. Condition-skipped tasks are not re-run (they're not failures). This is correct.

### Condition + `claudes generate`

The AI-assisted manifest generator should be aware of the `condition` field. Add it to the schema description in the generate prompt. Low priority — can be a follow-up.

### Condition in Shared block

Not supported. Conditions are inherently task-specific. If a user wants the same condition on multiple tasks, they must specify it on each task.

### Condition evaluation timeout

Initially, no timeout on condition commands. They should be fast shell commands. If this becomes an issue, add a `condition_timeout_secs` field later. For now, rely on the overall task timeout.

## Test Plan

### Unit Tests (in `crates/claudes/src/`)

#### `manifest.rs`
- `condition_field_serde_json`: Roundtrip a Task with `condition` set through JSON serialization
- `condition_field_serde_toml`: Roundtrip through TOML
- `condition_field_none_by_default`: `Task::new()` has `condition: None`
- `condition_not_inherited_from_shared`: `resolve()` doesn't pull condition from shared
- `condition_preserved_through_resolve`: Task's condition survives resolve()

#### `state.rs`
- `condition_skipped_status_serde`: `TaskStatus::ConditionSkipped` roundtrips as `"condition_skipped"`
- `build_state_condition_skipped`: `build_state` correctly identifies condition-skipped tasks
- `condition_skipped_counted_in_summary`: `RunSummary` correctly counts condition-skipped tasks
- `condition_skipped_not_counted_as_failed`: Condition-skipped doesn't inflate failed count

#### `runner.rs`
- `condition_exit_0_skips_task`: Mock condition "exit 0" -> task skipped
- `condition_exit_1_runs_task`: Mock condition "exit 1" -> task runs
- `condition_none_runs_task`: No condition -> task runs normally
- `condition_spawn_failure_runs_task`: Invalid command -> task runs (fail-safe)
- `condition_skipped_emits_event`: Verify the event sender receives condition events
- `condition_runs_in_project_dir`: Verify CWD of condition command

### Integration Tests (in `crates/claudes/tests/fake_claude.rs`)

- `condition_skips_task`: Task with `condition = "exit 0"` is skipped, fake-claude never runs
- `condition_runs_task`: Task with `condition = "exit 1"` runs normally
- `condition_skipped_with_dependencies`: Task A (condition-skipped) -> Task B runs
- `condition_skipped_no_worktree_created`: Verify no worktree is created when condition skips
- `condition_skipped_no_pre_hooks_run`: Pre-hooks don't run when condition skips
- `condition_skipped_state_recorded`: Run state file records `condition_skipped` status
- `condition_mixed_tasks`: Manifest with some condition-skipped and some normal tasks

## Implementation Order

1. Add `condition` field to `Task` struct and `Task::new()` (`manifest.rs`)
2. Add `ConditionSkipped` to `TaskStatus` (`state.rs`)
3. Add `condition_skipped` to `TaskResult` and `RunSummary` (`runner.rs`, `state.rs`)
4. Implement `run_condition()` function (`runner.rs`)
5. Add condition check in `run_task_impl()` (`runner.rs`)
6. Update `build_state()` to detect condition-skipped (`state.rs`)
7. Update progress/output display (`output.rs`)
8. Update status display (`state.rs`)
9. Add unit tests
10. Add integration tests
11. Verify `fix` and MCP handlers exclude `ConditionSkipped` (`main.rs`, `mcp.rs`)
