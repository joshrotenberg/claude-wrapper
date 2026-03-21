# Plan: Fix #442 — Desugar chains before checking file overlaps

## Problem

`cmd_run()` in `crates/claudes/src/main.rs` calls `manifest.check_file_overlaps()` at three
points, all **before** `desugar_chains()` ever runs. `desugar_chains()` only executes inside
`runner::run()` at `crates/claudes/src/runner.rs:177`. As a result, chain-declared dependencies
are invisible to the overlap checker and false-positive warnings are emitted for tasks that are
correctly sequenced via `chains`.

## Root cause trace

| Location | What happens |
|---|---|
| `main.rs:81` | `manifest.check_file_overlaps()` — chains **not** desugared yet |
| `main.rs:157` | `manifest.check_file_overlaps()` — chains **not** desugared yet |
| `main.rs:215` | `manifest.check_file_overlaps()` — chains **not** desugared yet |
| `runner.rs:177` | `manifest.desugar_chains()` — **too late** for overlap check |

## Fix: two changes

### Change 1 — `crates/claudes/src/main.rs`

Add `manifest.desugar_chains()` immediately before each of the three
`manifest.check_file_overlaps()` calls. The manifest is already `mut` at all three sites.

**Site 1 — `--manifest` path (lines 81–83):**

Current:
```rust
        for warning in manifest.check_file_overlaps() {
            eprintln!("warning: {warning}");
        }
```

Replace with:
```rust
        manifest.desugar_chains();
        for warning in manifest.check_file_overlaps() {
            eprintln!("warning: {warning}");
        }
```

**Site 2 — auto-discover path (lines 157–159):**

Current:
```rust
            for warning in manifest.check_file_overlaps() {
                eprintln!("warning: {warning}");
            }
```

Replace with:
```rust
            manifest.desugar_chains();
            for warning in manifest.check_file_overlaps() {
                eprintln!("warning: {warning}");
            }
```

**Site 3 — `-p` prompts path (lines 215–217):**

Current:
```rust
    for warning in manifest.check_file_overlaps() {
        eprintln!("warning: {warning}");
    }
```

Replace with:
```rust
    manifest.desugar_chains();
    for warning in manifest.check_file_overlaps() {
        eprintln!("warning: {warning}");
    }
```

### Change 2 — `crates/claudes/src/runner.rs` (avoid double-desugaring)

After the fix above, `desugar_chains()` will have already been called on the manifest by the time
`runner::run()` sees it. `desugar_chains()` is idempotent — it `take()`s `self.chains` on the
first call, so a second call is a no-op — but the call in runner is redundant for the paths that
go through `cmd_run`.

However, `runner::run()` is a public API and callers outside `cmd_run` (e.g. library users, tests)
may pass a manifest with `chains` that has not been desugared. The safest approach is to **keep
the call in runner.rs as-is**. It is already guarded by the early `return` if `chains` is `None`,
so the cost of the redundant call is negligible.

**Decision: no change to `runner.rs`.**

## Change 3 — new test in `crates/claudes/src/manifest.rs`

Add a test inside the `#[cfg(test)] mod tests` block (after line 2503, in the
`check_file_overlaps` section) that verifies the fix at the manifest level:

```rust
#[test]
fn check_file_overlaps_suppressed_when_chained() {
    // tasks a and b both reference src/lib.rs, but are sequenced via chains.
    // After desugar_chains(), b depends on a, so no overlap warning should fire.
    let mut manifest = Manifest::new(vec![
        Task::new("a", "Fix the bug in src/lib.rs"),
        Task::new("b", "Add tests in src/lib.rs"),
    ]);
    manifest.chains = Some(vec![vec![
        ChainStep::Single("a".into()),
        ChainStep::Single("b".into()),
    ]]);

    manifest.desugar_chains();
    let warnings = manifest.check_file_overlaps();
    assert!(
        warnings.is_empty(),
        "expected no warnings for chain-sequenced tasks, got: {warnings:?}"
    );
}
```

Place it after the existing `check_file_overlaps_warns_for_unsequenced_pair_among_sequenced` test
at line ~2503 so it sits with the rest of the overlap tests.

## Files modified

| File | Change |
|---|---|
| `crates/claudes/src/main.rs` | Add `manifest.desugar_chains();` before each of the 3 `check_file_overlaps()` calls (lines 81, 157, 215) |
| `crates/claudes/src/manifest.rs` | Add 1 new test `check_file_overlaps_suppressed_when_chained` after line 2503 |

`crates/claudes/src/runner.rs` — **no change needed.**

## Pre-commit checklist

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --lib -p claudes
cargo test --test fake_claude -p claudes -- --ignored
```

The new test runs under `cargo test --lib -p claudes`.

## Why `desugar_chains()` is safe to call before `execute_manifest`

`desugar_chains()` mutates the manifest in-place by moving `chains` into `depends_on` fields on
tasks and setting `self.chains = None`. The manifest is not serialised and sent to the runner;
`execute_manifest` receives an immutable reference and the runner clones it internally
(`runner.rs:176: let mut manifest = manifest.clone()`). Calling `desugar_chains()` before the
clone means the runner's clone already has `depends_on` populated and `chains = None`, so the
runner's own `desugar_chains()` call becomes a cheap no-op.

For the `dry_run` branch (which serialises the manifest to JSON before running), the desugared
manifest is printed. This is arguably more informative — it shows the fully-resolved
`depends_on` fields — but if the intent is to preserve the compact `chains` syntax in dry-run
output, the `desugar_chains()` call could be moved to after the `dry_run` check. Given that the
issue says nothing about dry-run output, keeping it before is simpler and consistent.
