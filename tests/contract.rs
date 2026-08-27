//! Check every flag this wrapper emits against the installed `claude` CLI.
//!
//! The fake-binary suites prove the wrapper builds the argv it intends to
//! build. They say nothing about whether that argv is still valid. Upstream can
//! remove or rename a flag and CI stays green until a user reports it.
//!
//! This suite builds a maximal command from each builder, collects the flags it
//! emits, and checks them against the live binary's help. It answers two
//! questions, one per direction:
//!
//! - **Does the CLI still list what we emit.** Catches a removal or a rename
//!   of something the wrapper depends on.
//! - **Does the wrapper account for everything the CLI lists.** Catches the
//!   wrapper falling behind, which is the direction that moves: across
//!   2.1.98 to 2.1.234 the CLI added eleven root flags and removed one.
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
//! # Coverage
//!
//! A flag the CLI lists is covered when a builder emits it or [`DECLINED`]
//! records why not. Anything else fails, which is what makes a new upstream
//! flag visible rather than silently unsupported.
//!
//! Alias spellings travel together: `-p, --print` is one option, so emitting
//! `--print` covers `-p`, and emitting `--allowed-tools` covers the
//! `--allowedTools` alias listed beside it. [`parse_help_options`] keeps that
//! grouping; [`parse_help_flags`] flattens it for the other direction.
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
//! - **Whether a flag the wrapper declines is one it should wrap.** The
//!   coverage direction only asks that every flag has been looked at, not that
//!   the answer was right. [`DECLINED`] carries the reasoning so the answer is
//!   reviewable.
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

/// The raw help text for `subcommand`.
fn help_text(subcommand: &[&str]) -> String {
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
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Parse the flags a `claude` help page lists.
///
/// Returns every option token, including aliases: the CLI lists
/// `--allowedTools, --allowed-tools <tools...>` and both spellings count as
/// present.
fn help_flags(subcommand: &[&str]) -> HashSet<String> {
    parse_help_flags(&help_text(subcommand))
}

/// As [`help_flags`], keeping each option's spellings together.
fn help_options(subcommand: &[&str]) -> Vec<Vec<String>> {
    parse_help_options(&help_text(subcommand))
}

/// The parsing half of [`help_flags`], split out so it is testable without a
/// binary. See the module docs for the format.
fn parse_help_flags(text: &str) -> HashSet<String> {
    parse_help_options(text).into_iter().flatten().collect()
}

/// As [`parse_help_flags`], keeping each option's spellings together.
///
/// The coverage direction needs the grouping that the flat set throws away:
/// `-p, --print` is one option, so emitting `--print` covers `-p`, and
/// emitting `--allowed-tools` covers the `--allowedTools` alias beside it.
/// Flattened, every alias reads as a separate uncovered flag.
fn parse_help_options(text: &str) -> Vec<Vec<String>> {
    let mut options = Vec::new();
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
        let spellings: Vec<String> = head
            .split(',')
            .map(str::trim)
            .filter(|token| token.starts_with('-'))
            .map(str::to_string)
            .collect();
        if !spellings.is_empty() {
            options.push(spellings);
        }
    }
    options
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

/// Flags the CLI lists that no builder emits, on purpose.
///
/// The companion to [`KNOWN_HIDDEN`], in the other direction. An entry records
/// a decision that this flag is not the wrapper's to wrap, so the coverage
/// check stays quiet about it; a flag the CLI gains that is not here fails the
/// suite until someone wraps it or decides not to.
///
/// Declaring any one spelling covers the whole option: `-p` and `--print` are
/// one entry, not two.
///
/// Format: `(flag, subcommand-path, reason)`. A subcommand-path of
/// [`ANY_SUBCOMMAND`] applies everywhere, for the options commander.js puts on
/// every command.
const DECLINED: &[(&str, &str, &str)] = &[
    // Not a wrapper's business: commander.js puts these on every command.
    (
        "--help",
        ANY_SUBCOMMAND,
        "help output, not a builder option",
    ),
    (
        "--version",
        ANY_SUBCOMMAND,
        "read by Claude::cli_version, which spawns `claude --version` \
         directly rather than through a command builder",
    ),
    // Interactive-only: these configure a terminal session, which the wrapper
    // does not start. Every builder runs headless.
    (
        "--ide",
        "",
        "connects an interactive session to a running IDE; there is no \
         session for a headless run to connect",
    ),
    (
        "--chrome",
        "",
        "Claude in Chrome integration, interactive-only",
    ),
    (
        "--no-chrome",
        "",
        "disables the Chrome integration; listed separately from --chrome \
         rather than as an alias of it",
    ),
    (
        "--ax-screen-reader",
        "",
        "renders screen-reader friendly TUI output; the wrapper parses \
         stream-json, not rendered text",
    ),
    (
        "--background",
        "",
        "starts the session as a background agent, which is the CLI owning a \
         process lifecycle the wrapper owns itself (--bg is the same option)",
    ),
    (
        "--remote-control",
        "",
        "starts an interactive session with Remote Control enabled",
    ),
    (
        "--remote-control-session-name-prefix",
        "",
        "names auto-generated Remote Control sessions; only meaningful with \
         --remote-control",
    ),
    // Applicable but not yet wrapped. These are gaps, not decisions: each one
    // works headless or governs a surface the wrapper already models. Tracked
    // in https://github.com/joshrotenberg/claude-wrapper/issues/799, and each
    // entry goes away with the change that wraps it.
    (
        "--forward-subagent-text",
        "",
        "unwrapped gap, see issue #799: works only with --print and \
         --output-format=stream-json, which is what the wrapper runs",
    ),
    (
        "--autocompact",
        "",
        "unwrapped gap, see issue #799: spawn-time context window setting, \
         where slash.rs already builds the /compact operation",
    ),
    (
        "--allow-dangerously-skip-permissions",
        "",
        "unwrapped gap, see issue #799: the weaker form of the flag \
         DangerousClient already emits, making bypass available rather than \
         active",
    ),
    (
        "--cloud",
        "",
        "unwrapped gap, see issue #799: a session source alongside --resume \
         and --from-pr",
    ),
    (
        "--environment",
        "",
        "unwrapped gap, see issue #799: selects the self-hosted environment \
         for a --cloud session",
    ),
    (
        "--teleport",
        "",
        "unwrapped gap, see issue #799: resumes a teleport session",
    ),
    (
        "--config",
        "plugin install",
        "unwrapped gap, see issue #799: sets a plugin userConfig option \
         non-interactively",
    ),
    (
        "--mcp-debug",
        "",
        "deprecated MCP debug flag present at the declared floor and removed \
         upstream by 2.1.220; never wrapped, and nothing to wrap now",
    ),
    (
        "--yes",
        "plugin install",
        "unwrapped gap, see issue #799: PluginUninstallCommand and \
         PluginPruneCommand have yes(), PluginInstallCommand does not, and \
         the CLI documents it as required when stdout is not a TTY",
    ),
];

/// A [`DECLINED`] scope matching every subcommand path.
const ANY_SUBCOMMAND: &str = "*";

/// Every flag the wrapper can emit at the root.
///
/// A union across builders rather than one command, because the flags that
/// conflict with each other (`--continue` against `--resume`, the several
/// hermetic scopes) still count as covered.
fn all_root_flags() -> HashSet<String> {
    let mut flags: HashSet<String> = HashSet::new();
    flags.extend(emitted_flags(&maximal_query().args()));
    flags.extend(emitted_flags(
        &QueryCommand::new("p")
            .resume("11111111-1111-1111-1111-111111111111")
            .fork_session()
            .args(),
    ));
    flags.extend(emitted_flags(
        &QueryCommand::new("p").continue_session().args(),
    ));
    flags.extend(emitted_flags(
        &QueryCommand::new("p").worktree_named("wt").args(),
    ));
    flags.extend(emitted_flags(&QueryCommand::new("p").hermetic().args()));
    flags.extend(emitted_flags(&post_floor_query().args()));
    for (_, cmd) in exclusive_query_commands() {
        flags.extend(emitted_flags(&cmd.args()));
    }
    flags.extend(flags_in_command_string(&maximal_duplex_command()));
    flags
}

/// Whether `flag` is a recorded decline for `sub_path`.
fn is_declined(flag: &str, sub_path: &str) -> bool {
    DECLINED.iter().any(|(declined, scope, _)| {
        *declined == flag && (*scope == sub_path || *scope == ANY_SUBCOMMAND)
    })
}

/// Assert the help for `subcommand` lists no option the wrapper neither emits
/// nor has declined.
///
/// The inverse of [`assert_flags_listed`]. That one catches the CLI dropping
/// something we depend on; this one catches the CLI gaining something we have
/// not looked at, which is the direction the wrapper falls behind in.
fn assert_help_is_covered(label: &str, subcommand: &[&str], emitted: &HashSet<String>) {
    let options = help_options(subcommand);
    assert!(
        !options.is_empty(),
        "{label}: parsed no options from `claude {} --help`; the help format \
         probably changed and this check is now blind",
        subcommand.join(" ")
    );
    let sub_path = subcommand.join(" ");
    let mut uncovered: Vec<String> = options
        .iter()
        .filter(|spellings| !spellings.iter().any(|flag| emitted.contains(flag)))
        .filter(|spellings| !spellings.iter().any(|flag| is_declined(flag, &sub_path)))
        // Name the option by its longest spelling, which is the long form.
        .filter_map(|spellings| spellings.iter().max_by_key(|s| s.len()).cloned())
        .collect();
    uncovered.sort();
    assert!(
        uncovered.is_empty(),
        "{label}: `claude {sub_path} --help` lists {uncovered:?}, which no \
         builder emits. Either wrap them or add them to DECLINED with a \
         reason."
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
        // Wrapped but previously unexercised: the coverage check surfaced
        // that no maximal builder emitted these, so neither direction of the
        // contract had ever checked them.
        .bare()
        .brief()
        .tmux()
        .disable_slash_commands()
        .include_hook_events()
        .name("contract")
        .replay_user_messages(true)
        .exclude_dynamic_system_prompt_sections()
}

/// Builder flags that the declared floor CLI does not list.
///
/// Kept out of [`maximal_query`] deliberately. The forward check runs against
/// both ends of the declared range, and `claude 2.1.98`
/// (`TESTED_CLI_VERSION_MIN`) has none of these, so including them there would
/// fail the pinned floor job.
///
/// That failure is a real finding, not a test artifact: the builders emit
/// flags the declared floor does not accept, so the declared range understates
/// what the wrapper requires. Tracked in
/// <https://github.com/joshrotenberg/claude-wrapper/issues/800>. This function
/// exists so the coverage check still counts them as wrapped while that is
/// decided; when the floor moves, fold these back into `maximal_query`.
fn post_floor_query() -> QueryCommand {
    QueryCommand::new("p")
        .plugin_url("https://example.test/plugins")
        .safe_mode()
        .prompt_suggestions(true)
}

/// The query flags that cannot share an invocation with [`maximal_query`].
///
/// `--from-pr` selects the session source, so it conflicts with the session id
/// that builder sets, and `--dangerously-skip-permissions` overrides the
/// permission mode it sets.
fn exclusive_query_commands() -> Vec<(&'static str, QueryCommand)> {
    vec![
        (
            "QueryCommand::from_pr",
            QueryCommand::new("p").from_pr("123"),
        ),
        (
            "QueryCommand::dangerously_skip_permissions",
            QueryCommand::new("p").dangerously_skip_permissions(),
        ),
    ]
}

/// `DuplexOptions` rendered as a command string.
///
/// It shares `SharedSpawnArgs` with `QueryCommand` but adds its own spawn-time
/// flags, and exposes its argv only through `to_command_string`.
fn maximal_duplex_command() -> String {
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
    // An explicit path rather than PATH resolution: `build()` only calls
    // `which` when no binary is set, and the flags this renders do not depend
    // on where the binary lives. Without this, every caller of
    // `all_root_flags` needs a real CLI installed, including the inventory
    // tests that otherwise run in ordinary CI.
    let claude = claude_wrapper::Claude::builder()
        .binary("claude")
        .build()
        .expect("building a client for the command preview");
    opts.to_command_string(&claude)
}

#[test]
#[ignore = "requires a real claude binary"]
fn query_flags_are_still_documented() {
    let cmd = maximal_query();
    assert_flags_documented("QueryCommand", &[], &cmd.args());

    for (label, cmd) in exclusive_query_commands() {
        assert_flags_documented(label, &[], &cmd.args());
    }
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
    assert_flags_listed(
        "DuplexOptions",
        &[],
        flags_in_command_string(&maximal_duplex_command()),
    );
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

#[test]
#[ignore = "requires a real claude binary"]
fn root_help_is_fully_covered() {
    assert_help_is_covered("root", &[], &all_root_flags());
}

#[test]
#[ignore = "requires a real claude binary"]
fn mcp_family_help_is_fully_covered() {
    let add: HashSet<String> = emitted_flags(
        &McpAddCommand::new("n", "https://example.test/mcp")
            .transport(Transport::Http)
            .scope(Scope::User)
            .env("K", "V")
            .header("Authorization: Bearer x")
            .callback_port(8080)
            .client_id("cid")
            .client_secret()
            .args(),
    )
    .into_iter()
    .collect();
    assert_help_is_covered("mcp add", &["mcp", "add"], &add);

    let list: HashSet<String> = emitted_flags(&McpListCommand::new().args())
        .into_iter()
        .collect();
    assert_help_is_covered("mcp list", &["mcp", "list"], &list);

    let get: HashSet<String> = emitted_flags(&McpGetCommand::new("n").args())
        .into_iter()
        .collect();
    assert_help_is_covered("mcp get", &["mcp", "get"], &get);

    let remove: HashSet<String> =
        emitted_flags(&McpRemoveCommand::new("n").scope(Scope::User).args())
            .into_iter()
            .collect();
    assert_help_is_covered("mcp remove", &["mcp", "remove"], &remove);
}

#[test]
#[ignore = "requires a real claude binary"]
fn plugin_family_help_is_fully_covered() {
    let list: HashSet<String> = emitted_flags(&PluginListCommand::new().json().available().args())
        .into_iter()
        .collect();
    assert_help_is_covered("plugin list", &["plugin", "list"], &list);

    let install: HashSet<String> =
        emitted_flags(&PluginInstallCommand::new("p").scope(Scope::User).args())
            .into_iter()
            .collect();
    assert_help_is_covered("plugin install", &["plugin", "install"], &install);
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
    fn parse_help_options_keeps_aliases_together() {
        let options = parse_help_options(SAMPLE);
        assert!(
            options.contains(&vec![
                "--allowedTools".to_string(),
                "--allowed-tools".to_string()
            ]),
            "alias spellings must stay in one group; got {options:?}"
        );
        assert!(
            options.contains(&vec!["-p".to_string(), "--print".to_string()]),
            "a short form and its long form are one option; got {options:?}"
        );
    }

    /// The flat and grouped parses must describe the same options, or the two
    /// directions of the suite disagree about what the CLI offers.
    #[test]
    fn the_two_parses_agree() {
        let flat = parse_help_flags(SAMPLE);
        let grouped: HashSet<String> = parse_help_options(SAMPLE).into_iter().flatten().collect();
        assert_eq!(flat, grouped);
    }

    /// Emitting one spelling covers the whole option. Without this, every
    /// short form and camelCase alias reads as an uncovered flag.
    #[test]
    fn covering_one_spelling_covers_its_aliases() {
        let emitted: HashSet<String> = ["--allowed-tools".to_string()].into_iter().collect();
        let uncovered: Vec<Vec<String>> = parse_help_options(SAMPLE)
            .into_iter()
            .filter(|spellings| !spellings.iter().any(|f| emitted.contains(f)))
            .collect();
        assert!(
            !uncovered
                .iter()
                .any(|group| group.iter().any(|f| f == "--allowedTools")),
            "--allowedTools should be covered by --allowed-tools; got {uncovered:?}"
        );
    }

    #[test]
    fn declined_scopes_match_exactly_or_by_wildcard() {
        assert!(
            is_declined("--help", ""),
            "wildcard scope must match the root"
        );
        assert!(
            is_declined("--help", "mcp add"),
            "wildcard scope must match a subcommand"
        );
        assert!(
            is_declined("--config", "plugin install"),
            "an exact scope must match itself"
        );
        assert!(
            !is_declined("--config", ""),
            "an exact scope must not leak to other subcommands"
        );
    }

    #[test]
    fn declined_entries_carry_a_reason() {
        for (flag, _, reason) in DECLINED {
            assert!(
                reason.len() > 20,
                "{flag}: DECLINED entries must say why, not just that"
            );
        }
    }

    /// A flag cannot be both emitted and declined: that means someone declined
    /// something the wrapper actually wraps, and the decline is now a lie.
    #[test]
    fn declined_root_flags_are_not_also_emitted() {
        let emitted = all_root_flags();
        for (flag, scope, _) in DECLINED {
            if !scope.is_empty() && *scope != ANY_SUBCOMMAND {
                continue;
            }
            assert!(
                !emitted.contains(*flag),
                "{flag} is in DECLINED but a builder emits it"
            );
        }
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
