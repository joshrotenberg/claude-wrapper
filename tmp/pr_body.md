## Summary
- Add skills field to Shared and Task for system prompt injection
- Skills are markdown files resolved relative to manifest directory
- Shared skills are prepended to task skills (like hooks)
- Add --skill CLI flag for ad-hoc skill injection
- Unit tests for skill resolution and error cases

Closes #380

## Test plan
- cargo fmt check passes
- cargo clippy clean
- cargo test --lib -p claudes (125 tests pass, 7 new skill tests)
- New unit tests cover skill resolution, error cases, and hook-like merging
