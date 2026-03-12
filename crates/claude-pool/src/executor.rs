//! Task execution helpers — core Claude CLI invocation logic.
//!
//! These functions handle the low-level work of building commands, managing
//! per-slot MCP config files, and invoking the Claude CLI. They operate on
//! `&PoolInner<S>` directly so they can be called from pool.rs without going
//! through the public `Pool<S>` API.

use std::path::Path;

use claude_wrapper::types::OutputFormat;

use crate::cli_parsing::detect_permission_prompt;
use crate::config::ResolvedConfig;
use crate::error::Result;
use crate::pool::PoolInner;
use crate::store::PoolStore;
use crate::types::*;

/// Drain pending messages for a slot and prepend them to a prompt.
///
/// If the slot has pending messages, they are consumed (removed from inbox)
/// and formatted as a preamble before the actual task prompt. This provides
/// automatic message delivery at task boundaries without requiring polling.
pub(crate) fn prepend_messages<S: PoolStore>(
    inner: &PoolInner<S>,
    slot_id: &SlotId,
    prompt: &str,
) -> String {
    let messages = inner.message_bus.read(slot_id);
    if messages.is_empty() {
        return prompt.to_string();
    }

    let mut preamble = String::from("## Messages received while idle\n\n");
    for msg in &messages {
        preamble.push_str(&format!(
            "**From {}** ({}): {}\n\n",
            msg.from.0,
            msg.timestamp.format("%H:%M:%S"),
            msg.content
        ));
    }
    preamble.push_str("---\n\n");
    preamble.push_str(prompt);
    preamble
}

/// Build the system prompt by combining slot identity, resolved config and context.
pub(crate) fn build_system_prompt<S: PoolStore>(
    inner: &PoolInner<S>,
    resolved: &ResolvedConfig,
    slot_config: &SlotConfig,
) -> Option<String> {
    let context_entries: Vec<_> = inner
        .context
        .iter()
        .map(|r| (r.key().clone(), r.value().clone()))
        .collect();

    // Check if there's any content to include
    let has_identity = slot_config.name.is_some()
        || slot_config.role.is_some()
        || slot_config.description.is_some();

    if resolved.system_prompt.is_none() && context_entries.is_empty() && !has_identity {
        return None;
    }

    let mut parts = Vec::new();

    // Inject slot identity
    if has_identity {
        let mut identity = String::new();
        identity.push_str("You are ");

        if let Some(ref name) = slot_config.name {
            identity.push_str(name);
        } else {
            identity.push_str("a slot");
        }

        if let Some(ref role) = slot_config.role {
            identity.push_str(", a ");
            identity.push_str(role);
        }

        if let Some(ref description) = slot_config.description {
            identity.push_str(". ");
            identity.push_str(description);
        } else if slot_config.role.is_some() {
            identity.push('.');
        }

        parts.push(identity);
    }

    if let Some(ref sp) = resolved.system_prompt {
        parts.push(sp.clone());
    }

    if !context_entries.is_empty() {
        parts.push("\n\n## Shared Context\n".to_string());
        for (key, value) in &context_entries {
            parts.push(format!("- **{key}**: {value}"));
        }
    }

    Some(parts.join("\n"))
}

/// Ensure a per-slot MCP config temp file exists. Writes a new one on first
/// call for a given slot, then reuses the stored path on subsequent calls.
///
/// Returns the path to the temp file.
pub(crate) async fn ensure_slot_mcp_config<S: PoolStore>(
    inner: &PoolInner<S>,
    slot_id: &SlotId,
    servers: &std::collections::HashMap<String, serde_json::Value>,
) -> Result<std::path::PathBuf> {
    // Check if this slot already has a temp file.
    if let Some(slot) = inner.store.get_slot(slot_id).await?
        && let Some(ref path) = slot.mcp_config_path
    {
        // Re-write the file in case servers changed (task-level overrides).
        let json = serde_json::to_string_pretty(&serde_json::json!({
            "mcpServers": servers
        }))?;
        std::fs::write(path, json)?;
        return Ok(path.clone());
    }

    // First call for this slot — create a new temp file.
    use std::io::Write as _;
    let json = serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": servers
    }))?;
    let mut file = tempfile::Builder::new()
        .prefix(&format!("claude-pool-{}-", slot_id.0))
        .suffix(".mcp.json")
        .tempfile()?;
    file.write_all(json.as_bytes())?;

    // Persist the temp file (prevents deletion on drop) and store the path.
    let path = file
        .into_temp_path()
        .keep()
        .map_err(std::io::Error::other)?
        .to_path_buf();

    if let Some(mut slot) = inner.store.get_slot(slot_id).await? {
        slot.mcp_config_path = Some(path.clone());
        inner.store.put_slot(slot).await?;
    }

    tracing::debug!(
        slot_id = %slot_id.0,
        path = %path.display(),
        servers = servers.len(),
        "created slot MCP config"
    );

    Ok(path)
}

/// Execute a task on a specific slot by invoking the Claude CLI.
pub(crate) async fn execute_task<S: PoolStore + 'static>(
    inner: &PoolInner<S>,
    task_id: &TaskId,
    prompt: &str,
    slot_id: &SlotId,
    slot_config: &SlotConfig,
    override_working_dir: Option<&Path>,
) -> Result<TaskResult> {
    let task_record = inner.store.get_task(task_id).await?;
    let task_cfg = task_record.as_ref().and_then(|t| t.config.as_ref());

    let resolved = ResolvedConfig::resolve(&inner.config, slot_config, task_cfg);

    // Build the system prompt with identity and context injection.
    let system_prompt = build_system_prompt(inner, &resolved, slot_config);

    // Auto-deliver pending messages by prepending them to the prompt.
    let effective_prompt = prepend_messages(inner, slot_id, prompt);

    // Build and execute the query.
    let mut cmd = claude_wrapper::QueryCommand::new(&effective_prompt)
        .output_format(OutputFormat::Json)
        .permission_mode(resolved.permission_mode);

    if resolved.permission_mode == PermissionMode::BypassPermissions {
        cmd = cmd.dangerously_skip_permissions();
    }

    if let Some(ref model) = resolved.model {
        cmd = cmd.model(model);
    }
    if let Some(max_turns) = resolved.max_turns {
        cmd = cmd.max_turns(max_turns);
    }
    if let Some(ref sp) = system_prompt {
        cmd = cmd.system_prompt(sp);
    }
    if let Some(effort) = resolved.effort {
        cmd = cmd.effort(effort);
    }
    if let Some(ref schema) = resolved.json_schema {
        cmd = cmd.json_schema(schema.to_string());
    }
    if !resolved.allowed_tools.is_empty() {
        cmd = cmd.allowed_tools(&resolved.allowed_tools);
    }

    // Write per-slot MCP config if servers are configured.
    // Reuses an existing temp file if the slot already has one; otherwise
    // writes a new one and stores the path on the slot record.
    if !resolved.mcp_servers.is_empty() {
        let mcp_path = ensure_slot_mcp_config(inner, slot_id, &resolved.mcp_servers).await?;
        cmd = cmd.mcp_config(mcp_path.to_string_lossy());
        if resolved.strict_mcp_config {
            cmd = cmd.strict_mcp_config();
        }
    }

    // Use override working dir, slot worktree, or default.
    let claude_instance = if let Some(slot) = inner.store.get_slot(slot_id).await? {
        // Resume session if the slot has one, but NOT when using an
        // override working dir (clone isolation) — the session belongs
        // to the original working directory and won't exist in the clone.
        if override_working_dir.is_none()
            && let Some(ref session_id) = slot.session_id
        {
            cmd = cmd.resume(session_id);
        }

        if let Some(dir) = override_working_dir {
            inner.claude.with_working_dir(dir)
        } else if let Some(ref wt_path) = slot.worktree_path {
            inner.claude.with_working_dir(wt_path)
        } else {
            inner.claude.clone()
        }
    } else {
        inner.claude.clone()
    };

    tracing::debug!(
        slot_id = %slot_id.0,
        model = ?resolved.model,
        effort = ?resolved.effort,
        mcp_servers = resolved.mcp_servers.len(),
        "executing task"
    );

    let start = std::time::Instant::now();

    let query_result = match cmd.execute_json(&claude_instance).await {
        Ok(r) => r,
        Err(e) if inner.config.detect_permission_prompts => {
            if let Some(detected) = detect_permission_prompt(&e, &slot_id.0) {
                return Err(detected);
            }
            return Err(e.into());
        }
        Err(e) => return Err(e.into()),
    };

    let elapsed_ms = start.elapsed().as_millis() as u64;

    let cost_microdollars = query_result
        .cost_usd
        .map(|c| (c * 1_000_000.0) as u64)
        .unwrap_or(0);

    let mut result = TaskResult::success(
        query_result.result,
        cost_microdollars,
        query_result.num_turns.unwrap_or(0),
    )
    .with_elapsed_ms(elapsed_ms)
    .with_session_id(query_result.session_id);

    if query_result.is_error {
        result.success = false;
    }

    if let Some(ref model) = resolved.model {
        result = result.with_model(model);
    }

    Ok(result)
}

/// Execute a task with optional streaming output.
///
/// When `on_output` is `Some`, uses streaming execution (`stream-json` format)
/// and calls the callback with each text chunk as it arrives. When `None`,
/// falls back to the standard `execute_task` path.
pub(crate) async fn execute_task_streaming<S: PoolStore + 'static>(
    inner: &PoolInner<S>,
    task_id: &TaskId,
    prompt: &str,
    slot_id: &SlotId,
    slot_config: &SlotConfig,
    on_output: Option<crate::chain::OnOutputChunk>,
    override_working_dir: Option<&Path>,
) -> Result<TaskResult> {
    // If no streaming callback, use the standard non-streaming path.
    let on_output = match on_output {
        Some(cb) => cb,
        None => {
            return execute_task(
                inner,
                task_id,
                prompt,
                slot_id,
                slot_config,
                override_working_dir,
            )
            .await;
        }
    };

    let task_record = inner.store.get_task(task_id).await?;
    let task_cfg = task_record.as_ref().and_then(|t| t.config.as_ref());
    let resolved = ResolvedConfig::resolve(&inner.config, slot_config, task_cfg);

    let system_prompt = build_system_prompt(inner, &resolved, slot_config);

    // Auto-deliver pending messages by prepending them to the prompt.
    let effective_prompt = prepend_messages(inner, slot_id, prompt);

    // Build the query with StreamJson format for streaming.
    let mut cmd = claude_wrapper::QueryCommand::new(&effective_prompt)
        .output_format(OutputFormat::StreamJson)
        .permission_mode(resolved.permission_mode);

    if resolved.permission_mode == PermissionMode::BypassPermissions {
        cmd = cmd.dangerously_skip_permissions();
    }
    if let Some(ref model) = resolved.model {
        cmd = cmd.model(model);
    }
    if let Some(max_turns) = resolved.max_turns {
        cmd = cmd.max_turns(max_turns);
    }
    if let Some(ref sp) = system_prompt {
        cmd = cmd.system_prompt(sp);
    }
    if let Some(effort) = resolved.effort {
        cmd = cmd.effort(effort);
    }
    if !resolved.allowed_tools.is_empty() {
        cmd = cmd.allowed_tools(&resolved.allowed_tools);
    }

    if !resolved.mcp_servers.is_empty() {
        let mcp_path = ensure_slot_mcp_config(inner, slot_id, &resolved.mcp_servers).await?;
        cmd = cmd.mcp_config(mcp_path.to_string_lossy());
        if resolved.strict_mcp_config {
            cmd = cmd.strict_mcp_config();
        }
    }

    // Use override working dir (chain worktree) > slot worktree > default.
    let claude_instance = if let Some(slot) = inner.store.get_slot(slot_id).await? {
        // Skip session resumption in clone isolation — the session
        // belongs to the original directory and won't exist in the clone.
        if override_working_dir.is_none()
            && let Some(ref session_id) = slot.session_id
        {
            cmd = cmd.resume(session_id);
        }
        if let Some(dir) = override_working_dir {
            inner.claude.with_working_dir(dir)
        } else if let Some(ref wt_path) = slot.worktree_path {
            inner.claude.with_working_dir(wt_path)
        } else {
            inner.claude.clone()
        }
    } else {
        inner.claude.clone()
    };

    tracing::debug!(
        slot_id = %slot_id.0,
        model = ?resolved.model,
        effort = ?resolved.effort,
        mcp_servers = resolved.mcp_servers.len(),
        "executing task (streaming)"
    );

    let start = std::time::Instant::now();

    // Collect the final result from stream events.
    let mut result_text = String::new();
    let mut session_id = String::new();
    let mut cost_usd: Option<f64> = None;
    let mut is_error = false;

    let stream_result = claude_wrapper::streaming::stream_query(
        &claude_instance,
        &cmd,
        |event: claude_wrapper::streaming::StreamEvent| {
            match event.event_type() {
                Some("result") => {
                    if let Some(text) = event.result_text() {
                        result_text = text.to_string();
                    }
                    if let Some(sid) = event.session_id() {
                        session_id = sid.to_string();
                    }
                    cost_usd = event.cost_usd();
                    is_error = event
                        .data
                        .get("is_error")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                }
                Some("assistant") => {
                    // Extract text from content blocks in assistant messages.
                    // Content may be at top level or nested under message.
                    let content_sources = [
                        event.data.get("content"),
                        event.data.get("message").and_then(|m| m.get("content")),
                    ];
                    for content in content_sources.into_iter().flatten() {
                        for block in content.as_array().into_iter().flatten() {
                            if block.get("type").and_then(|t| t.as_str()) == Some("text")
                                && let Some(text) = block.get("text").and_then(|t| t.as_str())
                            {
                                on_output(text);
                            }
                        }
                    }
                }
                Some("content_block_delta") => {
                    // Handle incremental text deltas.
                    if let Some(delta) = event.data.get("delta")
                        && delta.get("type").and_then(|t| t.as_str()) == Some("text_delta")
                        && let Some(text) = delta.get("text").and_then(|t| t.as_str())
                    {
                        on_output(text);
                    }
                }
                _ => {}
            }
        },
    )
    .await;

    match stream_result {
        Ok(_) => {}
        Err(e) if inner.config.detect_permission_prompts => {
            if let Some(detected) = detect_permission_prompt(&e, &slot_id.0) {
                return Err(detected);
            }
            return Err(e.into());
        }
        Err(e) => return Err(e.into()),
    }

    let elapsed_ms = start.elapsed().as_millis() as u64;
    let cost_microdollars = cost_usd.map(|c| (c * 1_000_000.0) as u64).unwrap_or(0);

    let mut result = if is_error {
        TaskResult::failure(&result_text)
    } else {
        TaskResult::success(result_text, cost_microdollars, 0)
    }
    .with_elapsed_ms(elapsed_ms)
    .with_session_id(session_id);

    if is_error {
        // failure() sets cost to 0; preserve the actual cost if known.
        result.cost_microdollars = cost_microdollars;
    }

    if let Some(ref model) = resolved.model {
        result = result.with_model(model);
    }

    Ok(result)
}
