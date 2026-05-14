# claude-wrapper

A type-safe Rust wrapper around the [Claude Code CLI](https://docs.claude.com/en/docs/claude-code/overview).

This repository is a Cargo workspace. The published crate lives at:

- [`crates/claude-wrapper/`](crates/claude-wrapper) — `claude-wrapper` on crates.io.
  Builder-pattern wrappers for every `claude` subcommand, async + sync APIs, typed
  outputs, long-lived stream-json sessions, and more. See its
  [README](crates/claude-wrapper/README.md) for installation, feature flags, and usage.

## Documentation

- [API docs on docs.rs](https://docs.rs/claude-wrapper)
- [Crate README](crates/claude-wrapper/README.md)
- [Changelog](crates/claude-wrapper/CHANGELOG.md)

## Layout

```
.
├── crates/
│   └── claude-wrapper/    # the published library
└── Cargo.toml             # virtual workspace
```

Additional crates may live under `crates/` over time.

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
