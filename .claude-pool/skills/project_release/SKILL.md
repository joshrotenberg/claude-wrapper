---
name: project_release
description: "Release readiness checks for all 3 crates in dependency order."
---

Check release readiness for all 3 crates. Test in dependency order:

1. claude-wrapper (core crate)
2. claude-pool (depends on claude-wrapper)
3. claude-pool-server (depends on claude-pool)

For EACH crate in order:

a) Run all pre-commit checks:
   - `cargo fmt --all -- --check`
   - `cargo clippy --all-targets --all-features -- -D warnings`
   - `cargo test --lib --all-features`
   - `cargo test --test '*' --all-features`

b) Run release-specific checks:
   - `cargo doc --no-deps --all-features` (docs build without warnings)
   - `cargo test --doc --all-features` (doc tests pass)
   - `cargo publish --dry-run -p {crate}` (package builds)

Stop on first failure. Fix and re-run that crate, then continue.

Report:
- Crate-by-crate status
- Any failures with fixes applied
- Final readiness verdict (ready / blocked)
