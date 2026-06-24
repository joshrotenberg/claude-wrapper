//! Inspect Claude Code's on-disk state without spawning the CLI.
//!
//! The read-side introspection modules parse `~/.claude` directly:
//! session history, skills, agents, and merged settings. None of this
//! launches the `claude` binary, so it is cheap and safe to run in a
//! read-only context. Every accessor degrades to an empty result when
//! the underlying directory does not exist, so this runs fine on a
//! machine that has never used Claude Code.
//!
//! ```sh
//! cargo run --example inspect_state --features json
//! ```

use claude_wrapper::artifacts::AgentsRoot;
use claude_wrapper::history::HistoryRoot;
use claude_wrapper::settings::{SettingsLayer, SettingsLoader};
use claude_wrapper::skills::SkillsRoot;

fn main() -> claude_wrapper::Result<()> {
    // --- Session history -------------------------------------------------
    // Stored as line-delimited JSON under
    // `~/.claude/projects/<slug>/<session_id>.jsonl`.
    let history = HistoryRoot::home()?;
    let projects = history.list_projects()?;
    println!("projects with history: {}", projects.len());

    if let Some(project) = projects.first() {
        println!(
            "  {} ({} session(s))",
            project.decoded_path.display(),
            project.session_count
        );

        let sessions = history.list_sessions(Some(&project.slug))?;
        if let Some(session) = sessions.first() {
            println!(
                "  session {} -- {} message(s)",
                session.session_id, session.message_count
            );

            // Read and parse the full session log.
            let log = history.read_session(&session.session_id)?;
            println!("  entries in log: {}", log.entries.len());
        }
    }

    // --- Skills (`~/.claude/skills/<name>/SKILL.md`) ---------------------
    println!();
    for skill in SkillsRoot::home()?.list()? {
        println!(
            "skill: {} -- {}",
            skill.name,
            skill.description.as_deref().unwrap_or("(no description)")
        );
    }

    // --- Agents (`~/.claude/agents/<name>.md`) ---------------------------
    for agent in AgentsRoot::home()?.list()? {
        println!(
            "agent: {} (model: {})",
            agent.name,
            agent.model.as_deref().unwrap_or("default")
        );
    }

    // --- Settings, merged across the four precedence layers --------------
    let settings = SettingsLoader::home()?.load()?;
    let present = SettingsLayer::all()
        .into_iter()
        .filter(|&layer| settings.get(layer).is_some())
        .count();
    println!("\nsettings layers present: {present}/4");

    Ok(())
}
