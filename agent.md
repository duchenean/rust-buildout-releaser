# Agent Guide for rust-buildout-releaser

## Project overview
- Rust CLI tool named `bldr` for managing `zc.buildout` package releases and changelogs.
- Entry point: `src/main.rs` wires CLI parsing to modules for buildout parsing, config, changelog generation, git operations, PyPI metadata, and version handling.
- Major modules:
  - `src/cli.rs`: clap command/flag definitions and CLI help text.
  - `src/config.rs`: config file (bldr.toml) parsing and defaults.
  - `src/buildout.rs`: buildout versions file parsing/updating.
  - `src/changelog.rs`: changelog aggregation and formatting.
  - `src/pypi.rs`: PyPI API fetching for version/metadata.
  - `src/git.rs`: git tag/commit utilities.
  - `src/version.rs`: version parsing/bumping semantics.

## Common commands
- Build: `cargo build`
- Run CLI locally: `cargo run -- <args>` (example: `cargo run -- check`)
- Tests: `cargo test`

## Files to know
- `README.md`: user-facing CLI docs and usage examples.
- `scripts/install.sh`: installer for prebuilt binaries.

## Notes for LLMs
- Prefer updating CLI behavior via `src/cli.rs` (command/flag definitions) plus the corresponding implementation in `src/main.rs`.
- When updating changelog behavior, scan `src/changelog.rs` for format-specific helpers and tests.
- Version logic (parsing, constraints, bumping) lives in `src/version.rs`—avoid duplicating that logic elsewhere.
- This repo currently uses standard `cargo test` without extra tooling (fmt/clippy not referenced in docs).
