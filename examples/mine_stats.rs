//! Mine Claude Code transcripts for prompt attribution and recurrence stats.
//!
//! Spike for the extraction + stats stage of a transcript mining engine.
//! Walks every session JSONL under the history root, extracts user-role
//! prompts, and reports how the prompt stream splits between human-typed
//! and machine-generated input (`promptSource` / `isMeta` / `isSidechain`
//! attribution), plus recurrence signals: exact-duplicate templates and
//! prefix-template coverage.
//!
//! The parse is line-level and independent of `HistoryRoot::read_session`
//! because `HistoryEntry` currently drops the attribution fields (see
//! issue #721); `HistoryRoot` is used for root resolution and the
//! project inventory.
//!
//! Transcripts are pruned on a retention window, so the aggregates this
//! prints are exactly the numbers worth persisting: pass `--digest` to
//! write them as JSON.
//!
//! ```sh
//! cargo run --example mine_stats --features json
//! cargo run --example mine_stats --features json -- \
//!     --root /path/to/projects --digest digest.json --top 10
//! ```

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use claude_wrapper::history::HistoryRoot;
use serde::Deserialize;
use serde_json::{Value, json};

/// The transcript fields the miner needs from each JSONL record.
#[derive(Deserialize)]
struct Record {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, rename = "isMeta")]
    is_meta: bool,
    #[serde(default, rename = "isSidechain")]
    is_sidechain: bool,
    #[serde(rename = "promptSource")]
    prompt_source: Option<String>,
    entrypoint: Option<String>,
    timestamp: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    content: Option<Value>,
}

/// Human-visible text of a user message: the string form directly, or
/// the concatenated `text` items of the array form. `None` means the
/// record is a pure tool-result turn with nothing the user authored.
fn text_of(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            let parts: Vec<&str> = items
                .iter()
                .filter(|it| it.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|it| it.get("text").and_then(Value::as_str))
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        _ => None,
    }
}

/// Attribution bucket for one extracted prompt, in precedence order.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Bucket {
    /// Harness-injected context (`isMeta`).
    Meta,
    /// Slash-command invocation (`<command-name>` payload).
    Command,
    /// Human-typed: `typed`, `queued`, or `suggestion_accepted`.
    Human,
    /// Machine-generated: `promptSource: sdk` (dispatch, routines,
    /// task notifications, scheduled tasks).
    Sdk,
    /// Harness-generated: `promptSource: system`.
    System,
    /// Everything else: interrupts, command stdout, compaction
    /// continuations, unattributed records from older CLI versions.
    Noise,
}

impl Bucket {
    fn classify(rec: &Record, text: &str) -> Self {
        if rec.is_meta {
            return Bucket::Meta;
        }
        if text.contains("<command-name>") {
            return Bucket::Command;
        }
        match rec.prompt_source.as_deref() {
            Some("typed") | Some("queued") | Some("suggestion_accepted") => Bucket::Human,
            Some("sdk") => Bucket::Sdk,
            Some("system") => Bucket::System,
            _ => Bucket::Noise,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Bucket::Meta => "meta (harness context)",
            Bucket::Command => "slash command",
            Bucket::Human => "human-typed",
            Bucket::Sdk => "sdk (machine-generated)",
            Bucket::System => "system",
            Bucket::Noise => "noise / unattributed",
        }
    }
}

#[derive(Default)]
struct Stats {
    files: usize,
    bytes: u64,
    lines: usize,
    parse_errors: usize,
    prompts: usize,
    buckets: HashMap<&'static str, usize>,
    /// Full-text occurrence counts, kept separately for the human and
    /// machine streams. Texts are capped for memory; the cap only
    /// affects dedup of prompts longer than the cap, which are almost
    /// never exact repeats anyway.
    human_texts: HashMap<String, usize>,
    sdk_texts: HashMap<String, usize>,
    /// First 60 chars of each sdk prompt, for template coverage.
    sdk_prefixes: HashMap<String, usize>,
    sdk_total: usize,
    sdk_by_entrypoint: HashMap<String, usize>,
    human_by_project: HashMap<String, usize>,
    human_by_month: HashMap<String, usize>,
    human_lengths: Vec<usize>,
    first_ts: Option<String>,
    last_ts: Option<String>,
}

const TEXT_CAP: usize = 500;

fn prefix(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

impl Stats {
    fn record(&mut self, rec: &Record, text: &str, project: &str) {
        self.prompts += 1;
        if let Some(ts) = rec.timestamp.as_deref() {
            let date = &ts[..ts.len().min(10)];
            if self.first_ts.as_deref().is_none_or(|f| date < f) {
                self.first_ts = Some(date.to_string());
            }
            if self.last_ts.as_deref().is_none_or(|l| date > l) {
                self.last_ts = Some(date.to_string());
            }
        }
        let bucket = Bucket::classify(rec, text);
        *self.buckets.entry(bucket.label()).or_default() += 1;
        match bucket {
            Bucket::Human => {
                *self.human_texts.entry(prefix(text, TEXT_CAP)).or_default() += 1;
                *self
                    .human_by_project
                    .entry(project.to_string())
                    .or_default() += 1;
                if let Some(ts) = rec.timestamp.as_deref() {
                    let month = prefix(ts, 7);
                    *self.human_by_month.entry(month).or_default() += 1;
                }
                self.human_lengths.push(text.chars().count());
            }
            Bucket::Sdk => {
                self.sdk_total += 1;
                *self.sdk_texts.entry(prefix(text, TEXT_CAP)).or_default() += 1;
                *self.sdk_prefixes.entry(prefix(text, 60)).or_default() += 1;
                let entrypoint = rec.entrypoint.as_deref().unwrap_or("?");
                *self
                    .sdk_by_entrypoint
                    .entry(entrypoint.to_string())
                    .or_default() += 1;
            }
            _ => {}
        }
    }
}

/// Recursively collect `*.jsonl` files. Session files sit directly in
/// each project directory, but subagent and workflow transcripts nest
/// deeper, so a flat listing undercounts.
fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            out.push(path);
        }
    }
}

fn mine(root: &Path) -> Result<Stats> {
    let mut files = Vec::new();
    collect_jsonl(root, &mut files);

    let mut stats = Stats {
        files: files.len(),
        ..Stats::default()
    };

    for path in &files {
        let project = path
            .strip_prefix(root)
            .ok()
            .and_then(|p| p.components().next())
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_else(|| "?".to_string());
        stats.bytes += path.metadata().map(|m| m.len()).unwrap_or(0);
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        for line in BufReader::new(file).lines() {
            let line = line.unwrap_or_default();
            stats.lines += 1;
            // Fast path: only user records matter, and most lines are
            // assistant turns or tool results.
            if !line.contains("\"type\":\"user\"") && !line.contains("\"type\": \"user\"") {
                continue;
            }
            let rec: Record = match serde_json::from_str(&line) {
                Ok(rec) => rec,
                Err(_) => {
                    stats.parse_errors += 1;
                    continue;
                }
            };
            if rec.kind != "user" || rec.is_sidechain {
                continue;
            }
            let Some(text) = rec.message.as_ref().and_then(|m| m.content.as_ref()) else {
                continue;
            };
            let Some(text) = text_of(text) else {
                continue;
            };
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            stats.record(&rec, text, &project);
        }
    }
    Ok(stats)
}

fn top_n(map: &HashMap<String, usize>, n: usize) -> Vec<(&str, usize)> {
    let mut items: Vec<_> = map.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    items.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    items.truncate(n);
    items
}

fn one_line(text: &str, width: usize) -> String {
    let flat: String = prefix(text, width);
    flat.replace('\n', " ")
}

fn print_report(stats: &Stats, root: &Path, top: usize) {
    let pct = |n: usize| -> String {
        if stats.prompts == 0 {
            "0%".to_string()
        } else {
            format!("{:.0}%", 100.0 * n as f64 / stats.prompts as f64)
        }
    };

    println!("root: {}", root.display());
    println!(
        "files: {}  lines: {}  size: {:.1} MB  parse errors: {}",
        stats.files,
        stats.lines,
        stats.bytes as f64 / 1e6,
        stats.parse_errors
    );
    if let (Some(first), Some(last)) = (&stats.first_ts, &stats.last_ts) {
        println!("window: {first} .. {last}");
    }

    println!("\nprompts extracted: {}", stats.prompts);
    let mut buckets: Vec<_> = stats.buckets.iter().collect();
    buckets.sort_by(|a, b| b.1.cmp(a.1));
    for (label, count) in buckets {
        println!("  {:>5}  {:>4}  {}", count, pct(*count), label);
    }

    let human: usize = stats.human_lengths.len();
    if human > 0 {
        let mut lengths = stats.human_lengths.clone();
        lengths.sort_unstable();
        let short = lengths.iter().filter(|&&c| c <= 30).count();
        println!("\nhuman-typed corpus: {human} prompts");
        println!(
            "  median length: {} chars, {} ({}) are <= 30 chars",
            lengths[human / 2],
            short,
            format_args!("{:.0}%", 100.0 * short as f64 / human as f64),
        );
        println!("  by project:");
        for (project, count) in top_n(
            &stats
                .human_by_project
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            top,
        ) {
            println!("    {count:>5}  {project}");
        }
        let mut months: Vec<_> = stats.human_by_month.iter().collect();
        months.sort();
        println!("  by month:");
        for (month, count) in months {
            println!("    {count:>5}  {month}");
        }
        println!("  top exact repeats:");
        for (text, count) in top_n(&stats.human_texts, top) {
            println!("    {:>4}x  {}", count, one_line(text, 60));
        }
    }

    if stats.sdk_total > 0 {
        let distinct = stats.sdk_texts.len();
        let top20: usize = top_n(&stats.sdk_prefixes, 20).iter().map(|(_, c)| c).sum();
        println!(
            "\nmachine-generated (sdk) stream: {} prompts",
            stats.sdk_total
        );
        println!(
            "  distinct texts: {distinct} ({:.0}% duplication)",
            100.0 * (stats.sdk_total - distinct) as f64 / stats.sdk_total as f64
        );
        println!(
            "  top-20 prefix templates cover {top20} prompts ({:.0}%)",
            100.0 * top20 as f64 / stats.sdk_total as f64
        );
        let mut entrypoints: Vec<_> = stats.sdk_by_entrypoint.iter().collect();
        entrypoints.sort_by(|a, b| b.1.cmp(a.1));
        for (entrypoint, count) in entrypoints {
            println!("    {count:>5}  via {entrypoint}");
        }
        println!("  top exact repeats:");
        for (text, count) in top_n(&stats.sdk_texts, top) {
            println!("    {:>4}x  {}", count, one_line(text, 60));
        }
    }
}

fn digest(stats: &Stats) -> Value {
    let mut lengths = stats.human_lengths.clone();
    lengths.sort_unstable();
    json!({
        "files": stats.files,
        "lines": stats.lines,
        "bytes": stats.bytes,
        "parse_errors": stats.parse_errors,
        "window": { "first": stats.first_ts, "last": stats.last_ts },
        "prompts": stats.prompts,
        "buckets": stats.buckets,
        "human": {
            "total": lengths.len(),
            "median_chars": lengths.get(lengths.len() / 2),
            "by_project": stats.human_by_project,
            "by_month": stats.human_by_month,
            "top_repeats": top_n(&stats.human_texts, 20)
                .iter()
                .map(|(t, c)| json!({ "count": c, "text": t }))
                .collect::<Vec<_>>(),
        },
        "sdk": {
            "total": stats.sdk_total,
            "distinct_texts": stats.sdk_texts.len(),
            "by_entrypoint": stats.sdk_by_entrypoint,
            "top_repeats": top_n(&stats.sdk_texts, 20)
                .iter()
                .map(|(t, c)| json!({ "count": c, "text": t }))
                .collect::<Vec<_>>(),
        },
    })
}

fn main() -> Result<()> {
    let mut root: Option<PathBuf> = None;
    let mut digest_path: Option<PathBuf> = None;
    let mut top = 8usize;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--digest" => digest_path = args.next().map(PathBuf::from),
            "--top" => top = args.next().and_then(|v| v.parse().ok()).unwrap_or(top),
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }

    let history = match root {
        Some(path) => HistoryRoot::at(path),
        None => HistoryRoot::home()?,
    };
    println!("projects with history: {}", history.list_projects()?.len());

    let stats = mine(history.path())?;
    print_report(&stats, history.path(), top);

    if let Some(path) = digest_path {
        std::fs::write(&path, serde_json::to_string_pretty(&digest(&stats))?)
            .with_context(|| format!("write {}", path.display()))?;
        println!("\ndigest written to {}", path.display());
    }
    Ok(())
}
