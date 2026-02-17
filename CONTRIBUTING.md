# Contributing

Homer is built in the open and contributions are welcome — whether that's fixing a bug, improving docs, adding language support, or just asking a good question in an issue.

Before you dive in, please read our [Code of Conduct](CODE_OF_CONDUCT.md). The short version: be good to people.

## Ways to Help

**Report a bug.** Open an issue with what you expected, what happened, and how to reproduce it. Include `homer --version` and your OS if relevant.

**Suggest something.** If Homer could work better for your use case, open an issue. Describe the problem you're solving, not just the solution you want — that helps us find the best approach together.

**Fix something.** Fork the repo, make your change on a branch, and open a PR. If it's more than a small fix, it's worth opening an issue first to discuss the approach.

**Add a language.** Homer's tree-sitter extraction engine (`homer-graphs/src/languages/`) is designed to be extended. Each language implements the `LanguageSupport` trait. Look at an existing one like `rust.rs` or `python.rs` for the pattern.

## Development

Rust 1.85+ required (Edition 2024).

```bash
git clone https://github.com/rand/homer.git
cd homer
cargo build --workspace
cargo test --workspace
```

Before submitting a PR:

```bash
cargo test --workspace           # All 187 tests pass
cargo clippy --workspace -- -D warnings  # Zero warnings
cargo fmt --all -- --check       # Formatted
```

CI runs these on both Linux and macOS.

## Project Layout

```
homer-core/     Pipeline, extractors, analyzers, renderers, store
homer-graphs/   Tree-sitter extraction for 6 languages
homer-cli/      The `homer` binary
homer-mcp/      MCP server for AI agent integration
homer-test/     Integration tests and fixture repos
homer-spec/     Design specification (12 documents)
docs/           User documentation
```

## Code Style

Homer uses clippy pedantic and forbids `unsafe`. A few conventions worth knowing:

- Tests live next to the code they test (`#[cfg(test)]` modules), with integration tests in `homer-test/tests/`
- Pipeline stages collect errors rather than aborting — one broken file shouldn't stop the whole analysis
- Node and edge types are exhaustive enums, not stringly-typed — if you add a new kind, the compiler tells you everywhere that needs updating
- Prefer `proptest` for anything involving round-trip correctness (store, serialization)

## Pull Requests

Keep them focused — one logical change per PR. If your change adds behavior, add a test. If it changes user-facing behavior, update the docs. CI must pass.

There's no PR template to fill out. Just explain what you changed and why.

## Questions

Open an issue. There's no minimum bar for asking — if something confused you, it probably confuses others too, and that's useful to know.
