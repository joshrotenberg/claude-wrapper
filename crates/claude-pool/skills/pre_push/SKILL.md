---
name: pre_push
description: "Run all checks required before pushing: format, lint, tests, docs."
---

Run the following checks in order. Stop and fix any failures before proceeding to the next step. Report the result of each step.

1. `cargo fmt --all -- --check` (formatting)
2. `cargo clippy --all-targets --all-features -- -D warnings` (lint)
3. `cargo test --lib --all-features` (unit tests)
4. `cargo test --test '*' --all-features` (integration tests)
5. `cargo doc --no-deps --all-features` (docs build)
6. `cargo test --doc --all-features` (doc tests)

If all checks pass, report success. If any fail, fix the issue and re-run that step before continuing. Summarize what was fixed, if anything.
