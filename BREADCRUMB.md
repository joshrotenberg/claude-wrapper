# Fix: chain-declared dependencies don't suppress overlap warnings (issue #442)

## Problem

`check_file_overlaps()` is called in `crates/claudes/src/main.rs` at three locations (lines 81, 157, and 215) **before** `desugar_chains()` runs. `desugar_chains()` is only called later inside `runner::run()` (`crates/claudes/src/runner.rs:177`).

Because `check_file_overlaps()` checks `depends_on` to suppress warnings for sequenced tasks, chains-declared dependencies are invisible to it at the time it runs — resulting in spurious overlap warnings for tasks that are actually sequenced via `chains`.

## Fix

### 1. `crates/claudes/src/main.rs` — 3 call sites

Call `manifest.desugar_chains()` immediately before each `manifest.check_file_overlaps()` call.

**Site 1 — line 81** (manifest path via `--manifest` flag):
```rust
// Before:
for warning in manifest.check_file_overlaps() {

// After:
manifest.desugar_chains();
for warning in manifest.check_file_overlaps() {
```

**Site 2 — line 157** (auto-discovered manifest path):
```rust
// Before:
for warning in manifest.check_file_overlaps() {

// After:
manifest.desugar_chains();
for warning in manifest.check_file_overlaps() {
```

**Site 3 — line 215** (CLI prompts path via `-p`):
```rust
// Before:
for warning in manifest.check_file_overlaps() {

// After:
manifest.desugar_chains();
for warning in manifest.check_file_overlaps() {
```

**Why this is safe:** `desugar_chains()` is idempotent — it uses `.take()` on `self.chains` (`manifest.rs:117`), so the field becomes `None` after the first call. The later call in `runner::run()` is a no-op.

### 2. `crates/claudes/src/manifest.rs` — add unit test

Add a test in the existing test section (near the other `check_file_overlaps_*` tests around line 2447) that verifies chains suppress overlap warnings after desugaring:

```rust
#[test]
fn check_file_overlaps_suppressed_when_chains_sequence_tasks() {
    // Two tasks touching the same file, but chained so they run sequentially.
    let mut manifest = Manifest::new(vec![
        Task::new("task-a", "Fix the bug in src/lib.rs"),
        Task::new("task-b", "Add tests in src/lib.rs"),
    ]);
    manifest.chains = Some(vec![vec![
        ChainStep::Single("task-a".into()),
        ChainStep::Single("task-b".into()),
    ]]);
    // Desugar chains first (mirrors what main.rs must do before check_file_overlaps).
    manifest.desugar_chains();
    let warnings = manifest.check_file_overlaps();
    assert!(
        warnings.is_empty(),
        "expected no warnings for chain-sequenced tasks, got: {warnings:?}"
    );
}
```

## Files to modify

| File | Change |
|------|--------|
| `crates/claudes/src/main.rs` | Add `manifest.desugar_chains();` before each of the 3 `check_file_overlaps()` calls |
| `crates/claudes/src/manifest.rs` | Add `check_file_overlaps_suppressed_when_chains_sequence_tasks` test |

## Verification

```bash
cargo test --lib -p claudes        # new test must pass
cargo clippy -p claudes            # no warnings
cargo fmt --all -- --check         # formatting
```
