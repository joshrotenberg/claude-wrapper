//! Check every flag this wrapper emits against the installed `claude` CLI.
//!
//! The fake-binary suites prove the wrapper builds the argv it intends to
//! build. They say nothing about whether that argv is still valid. Upstream can
//! remove or rename a flag and CI stays green until a user reports it.
//!
//! This suite builds a maximal command from each builder, collects the flags it
//! emits, and checks them against the live binary's help. It answers one
//! question: **does the CLI still list what we emit.**
//!
//! Run it with a `claude` binary on PATH:
//!
//! ```sh
//! cargo test --test contract -- --ignored
//! ```
//!
//! `CLAUDE_CONTRACT_BIN` overrides which binary is checked, which is how the
//! supported floor is established: install several versions side by side and
//! run this suite against each, rather than maintaining a flag list by hand.
//!
//! ```sh
//! CLAUDE_CONTRACT_BIN=/tmp/v/node_modules/.bin/claude \
//!   cargo test --test contract -- --ignored
//! ```
//!
//! # How the check works
//!
//! The CLI's help (commander.js, not clap) indents each option line by exactly
//! two spaces inside an `Options:` block, and lists aliases comma-separated
//! before the value placeholder:
//!
//! ```text
//! Options:
//!   --add-dir <directories...>            Additional directories to allow
//!   --allowedTools, --allowed-tools <tools...>
//!       Comma or space-separated list of tool names to allow
//! ```
//!
//! Scoping to the `Options:` block matters: several subcommands carry an
//! `Examples:` section whose sample invocations are also two-space indented and
//! contain flags, which a naive parse would mistake for the contract.
//!
//! # Hidden flags
//!
//! A flag can vanish from help while still functioning. That is not a false
//! positive to suppress, it is the state that immediately precedes removal, so
//! each one is listed in [`KNOWN_HIDDEN`] with the date it was observed and
//! treated as a liability rather than a fact of life. A hidden flag that later
//! stops working is a silent breakage for every caller using it.
//!
//! # What this does not check
//!
//! - **Flag semantics.** A flag that still exists but means something new
//!   passes here. The wrapper's own tests cover intent, not meaning.
//! - **Flags the CLI offers that no builder wraps.** That is coverage, tracked
//!   separately; this suite is about drift in what we already emit.
//! - **Config keys.** The Claude CLI has no `-c key=value` override surface
//!   with a strict mode, so the sibling crate's sentinel-probe half has no
//!   equivalent here. If one appears, it belongs in this file.

#![cfg(feature = "async")]

use std::collections::HashSet;
use std::process::Command;

use claude_wrapper::duplex::DuplexOptions;
use claude_wrapper::{
    ClaudeCommand, Effort, McpAddCommand, McpGetCommand, McpListCommand, McpRemoveCommand,
    OutputFormat, PermissionMode, PluginInstallCommand, PluginListCommand, QueryCommand, Scope,
    Transport,
};

/// Flags this wrapper emits that the CLI still accepts but no longer lists in
/// `--help`. Each entry is a liability, not an exemption: when one stops
/// working it breaks silently for every caller.
///
/// Format: `(flag, subcommand-path, note)`.
const KNOWN_HIDDEN: &[(&str, &str, &str)] = &[(
    "--max-turns",
    "",
    "absent from `claude --help` as of 2.1.220 but still accepted \
         (verified: `claude -p ... --max-turns 1` exits 0). Used by \
         QueryCommand::max_turns and DuplexOptions::max_turns.",
)];

/// Parse the flags a `claude` help page lists.
///
/// Returns every option token, including aliases: the CLI lists
/// `--allowedTools, --allowed-tools <tools...>` and both spellings count as
/// present.
fn help_flags(subcommand: &[&str]) -> HashSet<String> {
    // `CLAUDE_CONTRACT_BIN` points the suite at a specific binary, which is how
    // a floor gets established: install several versions side by side and run
    // the real check against each rather than hand-maintaining a flag list.
    let binary = std::env::var("CLAUDE_CONTRACT_BIN").unwrap_or_else(|_| "claude".to_string());
    let mut cmd = Command::new(&binary);
    cmd.args(subcommand)
        .arg("--help")
        // Same hygiene the exec layer applies, so the child does not believe it
        // is running inside another Claude Code invocation.
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT");
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("running `{binary} {} --help`: {e}", subcommand.join(" ")));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    parse_help_flags(&text)
}

/// The parsing half of [`help_flags`], split out so it is testable without a
/// binary. See the module docs for the format.
fn parse_help_flags(text: &str) -> HashSet<String> {
    let mut flags = HashSet::new();
    let mut in_options = false;
    for line in text.lines() {
        if line.trim() == "Options:" {
            in_options = true;
            continue;
        }
        if !in_options {
            continue;
        }
        // A non-indented, non-empty line starts the next section.
        if !line.is_empty() && !line.starts_with(' ') {
            break;
        }
        // Option lines are indented exactly two spaces; anything deeper is a
        // wrapped description.
        let Some(rest) = line.strip_prefix("  ") else {
            continue;
        };
        if !rest.starts_with('-') {
            continue;
        }
        // Cut at the value placeholder or the description gap, then split the
        // remaining alias list.
        let head = rest
            .split("  ")
            .next()
            .unwrap_or(rest)
            .split(" <")
            .next()
            .unwrap_or(rest)
            .split(" [")
            .next()
            .unwrap_or(rest);
        for token in head.split(',') {
            let token = token.trim();
            if token.starts_with('-') {
                flags.insert(token.to_string());
            }
        }
    }
    flags
}

/// The flags a rendered command string carries.
///
/// `DuplexOptions` exposes its argv only through `to_command_string`, so its
/// flags are read back from there. Values may be shell-quoted; flags never are.
fn flags_in_command_string(cmd: &str) -> Vec<String> {
    let args: Vec<String> = cmd.split_whitespace().map(str::to_string).collect();
    emitted_flags(&args)
}

/// The flags an argv actually carries, in order, deduplicated.
///
/// Everything after a bare `--` is a positional (the prompt), never a flag.
fn emitted_flags(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for arg in args {
        if arg == "--" {
            break;
        }
        if arg.starts_with('-') && arg.len() > 1 && !out.contains(arg) {
            out.push(arg.clone());
        }
    }
    out
}

/// Assert every flag in `args` is listed in the help for `subcommand`.
fn assert_flags_documented(label: &str, subcommand: &[&str], args: &[String]) {
    assert_flags_listed(label, subcommand, emitted_flags(args));
}

/// As [`assert_flags_documented`], for an already-extracted flag list.
fn assert_flags_listed(label: &str, subcommand: &[&str], flags: Vec<String>) {
    let documented = help_flags(subcommand);
    assert!(
        !documented.is_empty(),
        "{label}: parsed no flags from `claude {} --help`; the help format \
         probably changed and this suite is now blind",
        subcommand.join(" ")
    );
    let sub_path = subcommand.join(" ");
    let mut missing = Vec::new();
    for flag in flags {
        if documented.contains(&flag) {
            continue;
        }
        if KNOWN_HIDDEN
            .iter()
            .any(|(f, s, _)| *f == flag && *s == sub_path)
        {
            eprintln!("{label}: {flag} is hidden but known (see KNOWN_HIDDEN)");
            continue;
        }
        missing.push(flag);
    }
    assert!(
        missing.is_empty(),
        "{label}: `claude {sub_path} --help` no longer lists {missing:?}. \
         Either the CLI dropped them (fix the builder) or they went hidden \
         (add to KNOWN_HIDDEN with the date and a verification note)."
    );
}

/// A `QueryCommand` exercising every flag-emitting setter we can combine in one
/// invocation. Conflicting options (`--continue` with `--resume`, the several
/// hermetic scopes) are covered by the separate cases below.
fn maximal_query() -> QueryCommand {
    QueryCommand::new("contract probe")
        .model("haiku")
        .fallback_model("sonnet")
        .effort(Effort::Low)
        .system_prompt("sys")
        .append_system_prompt("more")
        .agent("reviewer")
        .agents_json(r#"{"a":{"description":"d","prompt":"p"}}"#)
        .permission_mode(PermissionMode::Plan)
        .allowed_tools(["Read", "Bash(git:*)"])
        .disallowed_tools(["Write"])
        .add_dir("/tmp")
        .max_turns(3)
        .max_budget_usd(0.5)
        .json_schema(r#"{"type":"object"}"#)
        .mcp_config("/tmp/mcp.json")
        .strict_mcp_config()
        .settings("/tmp/settings.json")
        .setting_sources("user")
        .session_id("11111111-1111-1111-1111-111111111111")
        .fallback_model("sonnet")
        .output_format(OutputFormat::StreamJson)
        .include_partial_messages()
        .verbose(true)
        .tools(["Read"])
        .file("/tmp/x.txt")
        .betas("beta1")
        .plugin_dir("/tmp/plugins")
        .debug_filter("api")
        .debug_file("/tmp/debug.log")
        .no_session_persistence()
}

#[test]
#[ignore = "requires a real claude binary"]
fn query_flags_are_still_documented() {
    let cmd = maximal_query();
    assert_flags_documented("QueryCommand", &[], &cmd.args());
}

#[test]
#[ignore = "requires a real claude binary"]
fn query_session_flags_are_still_documented() {
    // `--resume`, `--continue`, and `--fork-session` conflict with each other
    // and with the session id above, so they get their own maximal commands.
    let resume = QueryCommand::new("p")
        .resume("11111111-1111-1111-1111-111111111111")
        .fork_session();
    assert_flags_documented("QueryCommand::resume", &[], &resume.args());

    let cont = QueryCommand::new("p").continue_session();
    assert_flags_documented("QueryCommand::continue", &[], &cont.args());
}

#[test]
#[ignore = "requires a real claude binary"]
fn query_isolation_flags_are_still_documented() {
    let worktree = QueryCommand::new("p").worktree_named("wt");
    assert_flags_documented("QueryCommand::worktree", &[], &worktree.args());

    // A hermetic seal is three flags emitted together; it is the combination
    // most likely to break, since it depends on --setting-sources accepting an
    // empty value.
    let hermetic = QueryCommand::new("p").hermetic();
    assert_flags_documented("QueryCommand::hermetic", &[], &hermetic.args());
}

#[test]
#[ignore = "requires a real claude binary"]
fn duplex_flags_are_still_documented() {
    // DuplexOptions shares SharedSpawnArgs with QueryCommand but adds its own
    // spawn-time flags, so it is checked independently.
    let opts = DuplexOptions::default()
        .model("haiku")
        .effort(Effort::Low)
        .permission_mode(PermissionMode::Plan)
        .allowed_tools(["Read"])
        .disallowed_tools(["Write"])
        .add_dir("/tmp")
        .max_turns(2)
        .max_budget_usd(0.25)
        .append_system_prompt("x")
        .agent("reviewer")
        .mcp_config("/tmp/mcp.json")
        .strict_mcp_config()
        .session_id("11111111-1111-1111-1111-111111111111");
    let claude = claude_wrapper::Claude::builder()
        .build()
        .expect("building a client for the command preview");
    let rendered = opts.to_command_string(&claude);
    assert_flags_listed("DuplexOptions", &[], flags_in_command_string(&rendered));
}

#[test]
#[ignore = "requires a real claude binary"]
fn mcp_family_flags_are_still_documented() {
    assert_flags_documented("mcp list", &["mcp", "list"], &McpListCommand::new().args());
    assert_flags_documented("mcp get", &["mcp", "get"], &McpGetCommand::new("n").args());

    let add = McpAddCommand::new("n", "https://example.test/mcp")
        .transport(Transport::Http)
        .scope(Scope::User)
        .env("K", "V")
        .header("Authorization: Bearer x")
        .callback_port(8080)
        .client_id("cid");
    assert_flags_documented("mcp add", &["mcp", "add"], &add.args());

    let remove = McpRemoveCommand::new("n").scope(Scope::User);
    assert_flags_documented("mcp remove", &["mcp", "remove"], &remove.args());
}

#[test]
#[ignore = "requires a real claude binary"]
fn plugin_family_flags_are_still_documented() {
    let list = PluginListCommand::new().json().available();
    assert_flags_documented("plugin list", &["plugin", "list"], &list.args());

    let install = PluginInstallCommand::new("p").scope(Scope::User);
    assert_flags_documented("plugin install", &["plugin", "install"], &install.args());
}

/// Guard against a vacuous suite.
///
/// If help parsing broke and returned everything, or the comparison were
/// inverted, every test above would pass regardless of what the CLI does. This
/// pins both directions: a flag the CLI has never had must be absent, and a
/// flag it certainly has must be present.
#[test]
#[ignore = "requires a real claude binary"]
fn the_check_can_actually_fail() {
    let documented = help_flags(&[]);
    assert!(
        documented.contains("--print"),
        "sanity: --print must be documented; got {} flags",
        documented.len()
    );
    assert!(
        !documented.contains("--definitely-not-a-real-claude-flag"),
        "the parser is returning flags the CLI does not have, so every other \
         test in this file is vacuous"
    );
}

#[cfg(test)]
mod parser_tests {
    use super::*;

    /// The parser is the part that can silently stop working, so it is tested
    /// without a binary. If the help format changes, these fail in normal CI
    /// rather than only in the ignored suite.
    const SAMPLE: &str = "\
Usage: claude [options] [command] [prompt]

Examples:
  claude mcp add --transport http name https://example.test/mcp

Options:
  --add-dir <directories...>            Additional directories
  --allowedTools, --allowed-tools <tools...>
      Comma or space-separated list
  -p, --print                           Print response and exit
  --verbose                             Override verbose mode

Commands:
  mcp                                   Configure servers
";

    #[test]
    fn parses_options_and_aliases() {
        let flags = parse_help_flags(SAMPLE);
        for expected in [
            "--add-dir",
            "--allowedTools",
            "--allowed-tools",
            "-p",
            "--print",
            "--verbose",
        ] {
            assert!(flags.contains(expected), "missing {expected} in {flags:?}");
        }
    }

    #[test]
    fn ignores_examples_and_stops_at_the_next_section() {
        let flags = parse_help_flags(SAMPLE);
        // `--transport` appears only in the Examples block, which precedes
        // Options; a parser that scanned the whole page would pick it up.
        assert!(!flags.contains("--transport"), "leaked from Examples");
        // `mcp` is a command, not an option, and comes after Options.
        assert!(!flags.contains("mcp"));
    }

    #[test]
    fn emitted_flags_skips_positionals_after_the_separator() {
        let args = vec![
            "--print".to_string(),
            "--model".to_string(),
            "haiku".to_string(),
            "--".to_string(),
            "--not-a-flag".to_string(),
        ];
        assert_eq!(emitted_flags(&args), vec!["--print", "--model"]);
    }

    #[test]
    fn emitted_flags_deduplicates() {
        let args = vec![
            "--add-dir".to_string(),
            "/a".to_string(),
            "--add-dir".to_string(),
            "/b".to_string(),
        ];
        assert_eq!(emitted_flags(&args), vec!["--add-dir"]);
    }

    #[test]
    fn known_hidden_entries_carry_a_note() {
        for (flag, _, note) in KNOWN_HIDDEN {
            assert!(
                note.len() > 40,
                "{flag}: KNOWN_HIDDEN entries must say when it was observed \
                 and how it was verified"
            );
        }
    }
}
