//! Orchestrator — the main pipeline that processes a single issue end-to-end.

use std::path::Path;

use tracing::{error, info, warn};

use crate::config::RunnerConfig;
use crate::error::{Error, Result};
use crate::executor::{AgentAdapter, ClaudeAdapter, StageResult};
use crate::github;
use crate::isolation;
use crate::planner;
use crate::policy::RepoPolicy;
use crate::state::{self, RunRecord, RunStatus, StageStatus};
use crate::triage::{self, TriageDecision};
use crate::workflow::{self, StageKind};

/// Process a single issue through the full pipeline using the new config model.
pub async fn process_issue_with_config(
    number: u64,
    config: &RunnerConfig,
    state_dir: &Path,
    repo_dir: &Path,
) -> Result<RunRecord> {
    let repo = &config.global.repo;
    info!(repo = repo, issue = number, "processing issue");

    // 1. Fetch the issue.
    let issue = github::fetch_issue(repo, number).await?;
    info!(
        issue = number,
        title = issue.title,
        labels = ?issue.labels,
        "fetched issue"
    );

    // 2. Match route.
    let (route_name, route) = config
        .match_route(&issue)
        .ok_or_else(|| Error::Triage("no route matches this issue's labels".into()))?;
    info!(issue = number, route = route_name, "matched route");

    // 3. Acquire lease.
    let _ = github::add_label(repo, number, &config.global.in_progress_label).await;
    if let Some(ref gate) = config.global.gate_label {
        let _ = github::remove_label(repo, number, gate).await;
    }

    // 4. Resolve workflow template.
    let template = config
        .resolve_workflow(&route.workflow)
        .ok_or_else(|| Error::Policy(format!("unknown workflow: {}", route.workflow)))?;
    let branch = config.branch_for_issue(&issue);
    info!(
        issue = number,
        route = route_name,
        workflow = template.name,
        branch = branch,
        stages = template.stages.len(),
        "selected workflow"
    );

    // 5. Create work plan.
    let plan = planner::create_plan_with_config(
        &issue,
        &template,
        &branch,
        None,
        Some((config, repo_dir)),
    );

    // 6. Set up isolation.
    let worktree_dir = isolation::create_worktree(repo_dir, &branch, ".worktrees").await?;
    info!(issue = number, worktree = %worktree_dir.display(), "created worktree");

    // 7. Execute stages.
    let mut adapter = ClaudeAdapter::new();
    if let Some(model) = config.effective_model(route) {
        adapter = adapter.model(model);
    }
    let validation = config.effective_validation(route);

    let mut record = RunRecord::new(repo, number, &template.name, &branch);
    let mut all_succeeded = true;

    for planned_stage in &plan.stages {
        if planned_stage.kind == StageKind::OpenPr {
            match handle_open_pr(repo, &branch, &issue.title, number, &worktree_dir).await {
                Ok(pr) => {
                    record.pr_number = Some(pr.number);
                    info!(
                        issue = number,
                        pr = pr.number,
                        url = pr.html_url,
                        "created PR"
                    );
                    record.record_stage(
                        planned_stage.kind,
                        StageStatus::Succeeded,
                        StageResult {
                            stage: "open_pr".into(),
                            success: true,
                            duration_secs: 0.0,
                            cost_usd: None,
                            output: pr.html_url,
                            files_modified: None,
                        },
                    );
                }
                Err(e) => {
                    error!(issue = number, error = %e, "failed to create PR");
                    record.record_stage(
                        planned_stage.kind,
                        StageStatus::Failed,
                        StageResult {
                            stage: "open_pr".into(),
                            success: false,
                            duration_secs: 0.0,
                            cost_usd: None,
                            output: e.to_string(),
                            files_modified: None,
                        },
                    );
                    all_succeeded = false;
                    if !planned_stage.optional {
                        break;
                    }
                }
            }
            continue;
        }

        // Skip optional stages if condition says so.
        if planned_stage.optional
            && let Some(ref condition) = planned_stage.condition
            && let Ok(o) = tokio::process::Command::new("sh")
                .args(["-c", condition])
                .current_dir(&worktree_dir)
                .output()
                .await
            && o.status.success()
        {
            info!(
                issue = number,
                stage = planned_stage.kind_name(),
                "skipping optional stage (condition met)"
            );
            continue;
        }

        info!(
            issue = number,
            stage = planned_stage.kind_name(),
            "running stage"
        );

        match adapter.execute_stage(planned_stage, &worktree_dir).await {
            Ok(result) => {
                let status = if result.success {
                    StageStatus::Succeeded
                } else {
                    StageStatus::Failed
                };
                info!(
                    issue = number,
                    stage = planned_stage.kind_name(),
                    success = result.success,
                    duration = format!("{:.1}s", result.duration_secs),
                    cost = result.cost_usd,
                    "stage complete"
                );
                let failed = !result.success;
                record.record_stage(planned_stage.kind, status, result);
                if failed {
                    all_succeeded = false;
                    if !planned_stage.optional {
                        break;
                    }
                }
            }
            Err(e) => {
                error!(issue = number, stage = planned_stage.kind_name(), error = %e, "stage error");
                record.record_stage(
                    planned_stage.kind,
                    StageStatus::Failed,
                    StageResult {
                        stage: planned_stage.kind_name().to_string(),
                        success: false,
                        duration_secs: 0.0,
                        cost_usd: None,
                        output: e.to_string(),
                        files_modified: None,
                    },
                );
                all_succeeded = false;
                if !planned_stage.optional {
                    break;
                }
            }
        }

        // Run validation.
        if !validation.is_empty() {
            run_validation(validation, &worktree_dir).await;
        }
    }

    // 8. Finish.
    let final_status = if all_succeeded {
        RunStatus::Succeeded
    } else {
        RunStatus::Failed
    };
    record.finish(final_status);
    state::save_run(&record, state_dir)?;
    info!(issue = number, run_id = record.run_id, status = ?final_status, cost = record.total_cost_usd, "run complete");

    // 9. Update lease labels.
    let _ = github::remove_label(repo, number, &config.global.in_progress_label).await;
    if all_succeeded {
        let _ = github::add_label(repo, number, &config.global.complete_label).await;
    } else {
        let _ = github::add_label(repo, number, &config.global.failed_label).await;
    }

    // 10. Cleanup.
    if all_succeeded
        && let Err(e) = isolation::remove_worktree(repo_dir, &worktree_dir, false).await
    {
        warn!(error = %e, "failed to clean worktree (non-fatal)");
    }

    Ok(record)
}

/// Process all eligible issues for a config using route matching.
pub async fn process_batch_with_config(
    config: &RunnerConfig,
    state_dir: &Path,
    repo_dir: &Path,
) -> Vec<RunRecord> {
    let repo = &config.global.repo;

    // Fetch issues with the gate label if configured.
    let labels = config
        .global
        .gate_label
        .as_deref()
        .map(|l| vec![l.to_string()])
        .unwrap_or_default();

    let issues = match github::fetch_eligible_issues(repo, &labels, 10).await {
        Ok(issues) => issues,
        Err(e) => {
            error!(error = %e, "failed to fetch issues");
            return vec![];
        }
    };

    info!(repo = repo, count = issues.len(), "found eligible issues");

    let mut records = Vec::new();
    for issue in issues {
        // Only process if a route matches.
        if config.match_route(&issue).is_none() {
            tracing::debug!(issue = issue.number, "no route match, skipping");
            continue;
        }
        match process_issue_with_config(issue.number, config, state_dir, repo_dir).await {
            Ok(record) => records.push(record),
            Err(e) => warn!(issue = issue.number, error = %e, "failed to process issue"),
        }
    }

    records
}

/// Process a single issue through the full pipeline (legacy API).
///
/// Returns the run record with per-stage results.
pub async fn process_issue(
    repo: &str,
    number: u64,
    policy: &RepoPolicy,
    state_dir: &Path,
    repo_dir: &Path,
) -> Result<RunRecord> {
    info!(repo = repo, issue = number, "processing issue");

    // 1. Fetch the issue.
    let issue = github::fetch_issue(repo, number).await?;
    info!(
        issue = number,
        title = issue.title,
        labels = ?issue.labels,
        "fetched issue"
    );

    // 2. Acquire lease (add runner:in-progress, remove runner:ready).
    let _ = github::add_label(repo, number, "runner:in-progress").await;
    let _ = github::remove_label(repo, number, "runner:ready").await;

    // 3. Triage.
    let decision = triage::triage(&issue, policy);
    let needs_clarification = match &decision {
        TriageDecision::Ready => {
            info!(issue = number, "issue is ready for automation");
            false
        }
        TriageDecision::NeedsClarification(questions) => {
            warn!(
                issue = number,
                ?questions,
                "issue needs clarification, injecting clarify stage"
            );
            true
        }
        TriageDecision::OutOfScope(reason) => {
            info!(issue = number, reason = reason, "issue out of scope");
            return Err(Error::Triage(format!("out of scope: {reason}")));
        }
        TriageDecision::AlreadyInProgress => {
            info!(issue = number, "issue already in progress");
            return Err(Error::Triage("already in progress".into()));
        }
        TriageDecision::Blocked(reason) => {
            info!(issue = number, reason = reason, "issue blocked");
            return Err(Error::Triage(format!("blocked: {reason}")));
        }
        TriageDecision::Duplicate(other) => {
            info!(issue = number, duplicate_of = other, "duplicate issue");
            return Err(Error::Triage(format!("duplicate of #{other}")));
        }
    };

    // 3. Select workflow and generate branch.
    let mut template = workflow::select_workflow(&issue, policy);

    // Inject Clarify stage before Plan when triage identified gaps.
    if needs_clarification {
        let clarify_stage = workflow::Stage {
            kind: StageKind::Clarify,
            optional: false,
            max_retries: 1,
            timeout_secs: None,
            condition: None,
        };
        let plan_pos = template
            .stages
            .iter()
            .position(|s| s.kind == StageKind::Plan);
        if let Some(pos) = plan_pos {
            template.stages.insert(pos, clarify_stage);
        } else {
            template.stages.insert(0, clarify_stage);
        }
        info!(
            issue = number,
            workflow = template.name,
            "injected clarify stage before plan"
        );
    }
    let branch = policy.branch_for_issue(&issue);
    info!(
        issue = number,
        workflow = template.name,
        branch = branch,
        stages = template.stages.len(),
        "selected workflow"
    );

    // 4. Create work plan.
    let plan = planner::create_plan(&issue, &template, &branch, Some(policy));

    // 5. Set up isolation.
    let worktree_dir = isolation::create_worktree(repo_dir, &branch, ".worktrees").await?;
    info!(
        issue = number,
        worktree = %worktree_dir.display(),
        "created worktree"
    );

    // 6. Execute stages.
    let adapter = build_adapter(policy);
    let mut record = RunRecord::new(repo, number, &template.name, &branch);

    let mut all_succeeded = true;

    for planned_stage in &plan.stages {
        // Evaluate condition if set — skip stage if condition exits non-zero.
        if let Some(ref condition) = planned_stage.condition
            && eval_stage_condition(condition, &issue, &worktree_dir).await
        {
            info!(
                issue = number,
                stage = planned_stage.kind_name(),
                condition = condition,
                "stage condition not met, skipping"
            );
            record.record_stage_skipped(planned_stage.kind);
            continue;
        }

        // Handle OpenPr specially — it's a platform operation.
        if planned_stage.kind == StageKind::OpenPr {
            match handle_open_pr(repo, &branch, &issue.title, number, &worktree_dir).await {
                Ok(pr) => {
                    record.pr_number = Some(pr.number);
                    info!(
                        issue = number,
                        pr = pr.number,
                        url = pr.html_url,
                        "created PR"
                    );
                    record.record_stage(
                        planned_stage.kind,
                        StageStatus::Succeeded,
                        StageResult {
                            stage: "open_pr".into(),
                            success: true,
                            duration_secs: 0.0,
                            cost_usd: None,
                            output: pr.html_url,
                            files_modified: None,
                        },
                    );
                }
                Err(e) => {
                    error!(issue = number, error = %e, "failed to create PR");
                    record.record_stage(
                        planned_stage.kind,
                        StageStatus::Failed,
                        StageResult {
                            stage: "open_pr".into(),
                            success: false,
                            duration_secs: 0.0,
                            cost_usd: None,
                            output: e.to_string(),
                            files_modified: None,
                        },
                    );
                    all_succeeded = false;
                    if !planned_stage.optional {
                        break;
                    }
                }
            }
            continue;
        }

        // Execute agent stage.
        info!(
            issue = number,
            stage = planned_stage.kind_name(),
            "running stage"
        );

        match adapter.execute_stage(planned_stage, &worktree_dir).await {
            Ok(result) => {
                let status = if result.success {
                    StageStatus::Succeeded
                } else {
                    StageStatus::Failed
                };
                info!(
                    issue = number,
                    stage = planned_stage.kind_name(),
                    success = result.success,
                    duration = format!("{:.1}s", result.duration_secs),
                    cost = result.cost_usd,
                    "stage complete"
                );
                let failed = !result.success;
                record.record_stage(planned_stage.kind, status, result);

                if failed {
                    all_succeeded = false;
                    if !planned_stage.optional {
                        break;
                    }
                }
            }
            Err(e) => {
                error!(
                    issue = number,
                    stage = planned_stage.kind_name(),
                    error = %e,
                    "stage execution error"
                );
                record.record_stage(
                    planned_stage.kind,
                    StageStatus::Failed,
                    StageResult {
                        stage: planned_stage.kind_name().to_string(),
                        success: false,
                        duration_secs: 0.0,
                        cost_usd: None,
                        output: e.to_string(),
                        files_modified: None,
                    },
                );
                all_succeeded = false;
                if !planned_stage.optional {
                    break;
                }
            }
        }

        // Run validation commands between stages if configured.
        if !policy.validation_commands.is_empty() {
            run_validation(&policy.validation_commands, &worktree_dir).await;
        }
    }

    // 7. Finish run.
    let final_status = if all_succeeded {
        RunStatus::Succeeded
    } else {
        RunStatus::Failed
    };
    record.finish(final_status);

    // 8. Save state.
    state::save_run(&record, state_dir)?;
    info!(
        issue = number,
        run_id = record.run_id,
        status = ?final_status,
        cost = record.total_cost_usd,
        "run complete"
    );

    // 9. Update lease labels.
    let _ = github::remove_label(repo, number, "runner:in-progress").await;
    if all_succeeded {
        let _ = github::add_label(repo, number, "runner:complete").await;
    } else {
        let _ = github::add_label(repo, number, "runner:failed").await;
    }

    // 10. Cleanup on success.
    if all_succeeded {
        if let Err(e) = isolation::remove_worktree(repo_dir, &worktree_dir, false).await {
            warn!(error = %e, "failed to clean worktree (non-fatal)");
        }
    } else {
        info!(
            worktree = %worktree_dir.display(),
            "keeping worktree for inspection (run failed)"
        );
    }

    Ok(record)
}

async fn handle_open_pr(
    repo: &str,
    branch: &str,
    issue_title: &str,
    issue_number: u64,
    work_dir: &Path,
) -> Result<github::PullRequest> {
    // Commit any uncommitted changes (tracked files only — skip breadcrumbs/temp files).
    let status = tokio::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(work_dir)
        .output()
        .await?;
    let has_changes = !String::from_utf8_lossy(&status.stdout).trim().is_empty();

    if has_changes {
        let _ = tokio::process::Command::new("git")
            .args(["add", "-u"])
            .current_dir(work_dir)
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .args([
                "commit",
                "-m",
                &format!("automation: implement changes for #{issue_number}"),
            ])
            .current_dir(work_dir)
            .output()
            .await;
    }

    // Rebase on latest origin/main to avoid conflicts with recently merged PRs.
    let _ = tokio::process::Command::new("git")
        .args(["fetch", "origin"])
        .current_dir(work_dir)
        .output()
        .await;
    let rebase = tokio::process::Command::new("git")
        .args(["rebase", "origin/main"])
        .current_dir(work_dir)
        .output()
        .await;
    if let Ok(ref o) = rebase
        && !o.status.success()
    {
        // Rebase failed — abort and try pushing anyway.
        let _ = tokio::process::Command::new("git")
            .args(["rebase", "--abort"])
            .current_dir(work_dir)
            .output()
            .await;
        tracing::warn!(
            issue = issue_number,
            "rebase on origin/main failed, pushing as-is"
        );
    }

    // Push.
    github::push_branch(work_dir, branch).await?;

    // Create PR.
    let body = format!(
        "## Summary\n\n\
         Automated implementation for #{issue_number}.\n\n\
         ## Test plan\n\n\
         - [ ] CI passes\n\
         - [ ] Manual review\n\n\
         Closes #{issue_number}"
    );

    github::create_pull_request(repo, branch, issue_title, &body, work_dir).await
}

/// Evaluate a stage condition shell command. Returns `true` if the stage should be skipped.
///
/// The following env vars are available to the condition script:
/// - `RUNNER_ISSUE_NUMBER`
/// - `RUNNER_ISSUE_TITLE`
/// - `RUNNER_ISSUE_BODY`
/// - `RUNNER_ISSUE_LABELS` (comma-separated)
async fn eval_stage_condition(
    condition: &str,
    issue: &github::IssueCandidate,
    work_dir: &Path,
) -> bool {
    let output = tokio::process::Command::new("sh")
        .args(["-c", condition])
        .current_dir(work_dir)
        .env("RUNNER_ISSUE_NUMBER", issue.number.to_string())
        .env("RUNNER_ISSUE_TITLE", &issue.title)
        .env("RUNNER_ISSUE_BODY", &issue.body)
        .env("RUNNER_ISSUE_LABELS", issue.labels.join(","))
        .output()
        .await;

    match output {
        Ok(o) => !o.status.success(),
        Err(e) => {
            warn!(
                condition = condition,
                error = %e,
                "condition command failed to run; not skipping stage"
            );
            false
        }
    }
}

fn build_adapter(policy: &RepoPolicy) -> ClaudeAdapter {
    let mut adapter = ClaudeAdapter::new();
    if let Some(ref model) = policy.model {
        adapter = adapter.model(model);
    }
    adapter
}

async fn run_validation(commands: &[String], work_dir: &Path) {
    for cmd in commands {
        let output = tokio::process::Command::new("sh")
            .args(["-c", cmd])
            .current_dir(work_dir)
            .output()
            .await;
        match output {
            Ok(o) if o.status.success() => {
                tracing::debug!(command = cmd, "validation passed");
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                warn!(command = cmd, stderr = %stderr, "validation failed");
            }
            Err(e) => {
                warn!(command = cmd, error = %e, "validation command failed to run");
            }
        }
    }
}

/// Process multiple issues from a repo poll.
///
/// Respects both the global `max_concurrency` limit and per-workflow limits
/// from the `concurrency` map. Active runs are identified by the
/// `runner:in-progress` label on GitHub issues.
pub async fn process_batch(
    repo: &str,
    policy: &RepoPolicy,
    state_dir: &Path,
    repo_dir: &Path,
) -> Vec<RunRecord> {
    let labels = if policy.eligible_labels.is_empty() {
        vec![]
    } else {
        policy.eligible_labels.clone()
    };

    let issues = match github::fetch_eligible_issues(repo, &labels, 10).await {
        Ok(issues) => issues,
        Err(e) => {
            error!(error = %e, "failed to fetch issues");
            return vec![];
        }
    };

    info!(repo = repo, count = issues.len(), "found eligible issues");

    // Fetch currently active runs via the runner:in-progress lease label.
    let active_issues = match github::fetch_issues_with_label(repo, "runner:in-progress").await {
        Ok(issues) => issues,
        Err(e) => {
            warn!(error = %e, "failed to fetch in-progress issues, skipping batch");
            return vec![];
        }
    };

    let mut global_active = active_issues.len();
    info!(
        repo = repo,
        active = global_active,
        max = policy.max_concurrency,
        "concurrency check"
    );

    // Count active runs per workflow type.
    let mut workflow_active: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for active in &active_issues {
        let wf = workflow::select_workflow(active, policy);
        *workflow_active.entry(wf.name).or_insert(0) += 1;
    }

    let mut records = Vec::new();
    for issue in issues {
        // Check global concurrency limit.
        if global_active >= policy.max_concurrency {
            info!(
                active = global_active,
                max = policy.max_concurrency,
                "global concurrency limit reached, stopping batch"
            );
            break;
        }

        // Check per-workflow concurrency limit.
        let wf = workflow::select_workflow(&issue, policy);
        if let Some(&limit) = policy.concurrency.get(&wf.name)
            && limit > 0
        {
            let wf_count = workflow_active.get(&wf.name).copied().unwrap_or(0);
            if wf_count >= limit {
                info!(
                    issue = issue.number,
                    workflow = wf.name,
                    active = wf_count,
                    limit = limit,
                    "per-workflow concurrency limit reached, skipping issue"
                );
                continue;
            }
        }

        // Update local tracking before we start (optimistic — avoids
        // starting two issues of the same workflow in the same batch).
        global_active += 1;
        *workflow_active.entry(wf.name.clone()).or_insert(0) += 1;

        match process_issue(repo, issue.number, policy, state_dir, repo_dir).await {
            Ok(record) => records.push(record),
            Err(e) => {
                warn!(issue = issue.number, error = %e, "failed to process issue");
                // Roll back the optimistic increment on error (the lease
                // may not have been acquired).
                global_active = global_active.saturating_sub(1);
                if let Some(c) = workflow_active.get_mut(&wf.name) {
                    *c = c.saturating_sub(1);
                }
            }
        }
    }

    records
}
