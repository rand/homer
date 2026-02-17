# Contributing to Homer

Thank you for your interest in contributing to Homer. This project is built in the open and welcomes contributions from everyone who shares our values and wants to make the tool better.

## Values

This project is maintained by a queer developer and exists in a community that values kindness, respect, and mutual care. We believe good software comes from environments where people feel safe, welcomed, and valued for who they are.

**We do not tolerate bigotry in any form.** Racism, sexism, homophobia, transphobia, ableism, and other forms of discrimination have no place here. This is non-negotiable. See our [Code of Conduct](CODE_OF_CONDUCT.md).

## How to Contribute

### Reporting Bugs

Open an issue at https://github.com/rand/homer/issues with:

- What you expected to happen
- What actually happened
- Steps to reproduce
- Homer version (`homer --version`) and OS

### Suggesting Features

Open an issue describing:

- The problem you're trying to solve
- How you'd like it to work
- Any alternatives you've considered

### Contributing Code

1. **Fork and clone** the repository
2. **Create a branch** for your change: `git checkout -b my-change`
3. **Make your changes** — see the development guide below
4. **Run tests**: `cargo test --workspace`
5. **Run lints**: `cargo clippy --workspace -- -D warnings`
6. **Format**: `cargo fmt --all`
7. **Commit** with a clear message describing the change
8. **Open a pull request** against `main`

### Development Setup

```bash
# Prerequisites: Rust 1.85+
rustup update stable

# Clone
git clone https://github.com/rand/homer.git
cd homer

# Build
cargo build --workspace

# Test
cargo test --workspace

# Lint
cargo clippy --workspace -- -D warnings

# Format
cargo fmt --all
```

### Code Conventions

- **Edition 2024**, MSRV 1.85
- **`unsafe` is forbidden** — the workspace forbids unsafe code
- **Clippy pedantic** is enabled with `-D warnings` in CI
- `gix` for git operations (not `git2`)
- `rusqlite` for SQLite with WAL mode
- `async_trait` for async trait definitions
- Pipeline stages return errors without aborting — individual failures are collected, not fatal
- Tests go next to the code they test (in-module `#[cfg(test)]` blocks) or in `homer-test/tests/` for integration tests

### Project Structure

```
homer-core/     Pipeline, extractors, analyzers, renderers, SQLite store
homer-graphs/   Tree-sitter heuristic extraction (6 languages)
homer-cli/      CLI binary (clap subcommands)
homer-mcp/      MCP server for AI agent integration
homer-test/     Integration test fixtures and helpers
homer-spec/     Design specification documents
docs/           User-facing documentation
```

### Writing Tests

- Unit tests belong in `#[cfg(test)]` modules within the source file
- Integration tests go in `homer-test/tests/`
- Use `tempfile` for temporary directories
- Use `insta` for snapshot testing where output format matters
- Property-based tests use `proptest`

### Commit Messages

Write clear commit messages. Use imperative mood ("Add feature" not "Added feature"). A sentence or two is fine for small changes. For larger changes, include a blank line after the summary and explain the why.

## Pull Request Process

1. PRs are reviewed before merging
2. CI must pass (tests, clippy, format check on Linux and macOS)
3. Keep PRs focused — one logical change per PR
4. If your PR adds a new feature, include tests
5. Update documentation if the user-facing behavior changes

## Questions?

Open an issue or start a discussion. There are no stupid questions.
