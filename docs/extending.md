# Extending Homer

Step-by-step guides for adding languages, analyzers, renderers, and extractors to Homer.

## Adding a Language

Homer's graph extraction engine supports 6 languages. Each language implements the `LanguageSupport` trait in `homer-graphs/src/languages/`. To add a new one:

### 1. Add the tree-sitter grammar

Add the grammar crate to `homer-graphs/Cargo.toml`:

```toml
[dependencies]
tree-sitter-ruby = "0.23"
```

### 2. Create the language module

Create `homer-graphs/src/languages/ruby.rs`. Use `rust.rs` as the canonical reference — it demonstrates every feature.

```rust
use crate::{ResolutionTier, Result};
use crate::scope_graph::FileScopeGraph;
use super::LanguageSupport;
use super::helpers::ScopeGraphBuilder;

#[derive(Debug)]
pub struct RubySupport;

impl LanguageSupport for RubySupport {
    fn id(&self) -> &'static str { "ruby" }

    fn extensions(&self) -> &'static [&'static str] { &["rb"] }

    fn tier(&self) -> ResolutionTier { ResolutionTier::Precise }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_ruby::LANGUAGE.into()
    }

    fn build_scope_graph(
        &self,
        tree: &tree_sitter::Tree,
        source: &str,
        path: &std::path::Path,
    ) -> Result<Option<FileScopeGraph>> {
        let mut builder = ScopeGraphBuilder::new(path);
        // Walk the AST and build the scope graph
        // See rust.rs for the full pattern
        Ok(Some(builder.build()))
    }

    fn extract_heuristic(
        &self,
        tree: &tree_sitter::Tree,
        source: &str,
        path: &std::path::Path,
    ) -> Result<crate::HeuristicGraph> {
        // Extract function definitions, call sites, imports
        // See rust.rs for the pattern
        todo!()
    }
}
```

### 3. Register in the language dispatcher

Edit `homer-graphs/src/languages/mod.rs` to add the new language:

```rust
pub mod ruby;

// In the language_for_extension function:
"rb" => Some(Box::new(ruby::RubySupport)),
```

### 4. Add file patterns to config defaults

Edit `homer-core/src/config.rs` to include the new extension in the default `include_patterns`:

```rust
include_patterns: vec![
    // ... existing patterns ...
    "**/*.rb".into(),
],
```

### 5. Write tests

Add tests in `ruby.rs` covering:
- Function definition extraction
- Call site detection
- Import resolution
- Doc comment extraction
- Scope graph construction

Use the pattern from existing language tests: parse a small source snippet, extract, and assert on the results.

### Key Concepts

- **Scope Graph Builder**: `ScopeGraphBuilder` in `helpers.rs` provides common operations (create scope, add definition, add reference). All languages use this.
- **Resolution Tier**: All current languages use `ResolutionTier::Precise`. Use `Heuristic` if you can't build scope graphs.
- **ECMAScript sharing**: TypeScript and JavaScript share `ecma_scope.rs`. If your language is similar to an existing one, consider sharing.

## Adding an Analyzer

Analyzers compute derived insights from the hypergraph. Homer has 7 analyzers. To add a new one:

### 1. Define the AnalysisKinds

Edit `homer-core/src/types.rs` to add new variants to `AnalysisKind`:

```rust
pub enum AnalysisKind {
    // ... existing variants ...

    // Your new analyzer
    SecurityRisk,
    DependencyFreshness,
}
```

This is an exhaustive enum — the compiler will tell you every match arm that needs updating (in `as_str()`, serde tests, proptest arbitraries, etc.). Fix them all.

### 2. Create the analyzer module

Create `homer-core/src/analyze/security.rs`. Use `behavioral.rs` as a reference for the pattern.

```rust
use crate::analyze::traits::Analyzer;
use crate::analyze::AnalyzeStats;
use crate::config::HomerConfig;
use crate::store::HomerStore;
use crate::types::AnalysisKind;

#[derive(Debug)]
pub struct SecurityAnalyzer;

#[async_trait::async_trait]
impl Analyzer for SecurityAnalyzer {
    fn name(&self) -> &'static str { "security" }

    fn produces(&self) -> &'static [AnalysisKind] {
        &[AnalysisKind::SecurityRisk, AnalysisKind::DependencyFreshness]
    }

    fn requires(&self) -> &'static [AnalysisKind] {
        // Declare what must exist before this analyzer runs
        &[AnalysisKind::CompositeSalience]
    }

    async fn needs_rerun(&self, store: &dyn HomerStore) -> crate::error::Result<bool> {
        // Optional: check if inputs have changed
        Ok(true)
    }

    async fn analyze(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<AnalyzeStats> {
        let mut stats = AnalyzeStats::default();

        // Query the store for the data you need
        // Compute your analysis
        // Store results via store.store_analysis()

        Ok(stats)
    }
}
```

### 3. Register in the pipeline

Edit `homer-core/src/pipeline.rs` to add the analyzer to `build_analyzer_list()`:

```rust
fn build_analyzer_list(&self, config: &HomerConfig) -> Vec<Box<dyn Analyzer>> {
    let mut analyzers: Vec<Box<dyn Analyzer>> = vec![
        // ... existing analyzers ...
        Box::new(SecurityAnalyzer),
    ];
    analyzers
}
```

The topological sort will automatically place it after its dependencies (whatever produces `CompositeSalience`).

### 4. Write tests

Add `#[cfg(test)]` module in your analyzer file. Key tests:
- Analyzer produces expected `AnalysisKind`s
- Analyzer handles empty store gracefully
- Analyzer produces correct results for known input

### Key Concepts

- **`produces()` / `requires()`**: These are the dependency declaration system. Get them right and the topological sort handles execution order automatically.
- **`needs_rerun()`**: Optional optimization. Return `false` to skip recomputation when inputs haven't changed. Default is `true` (always run).
- **Error collection**: Use `stats.errors.push(...)` for non-fatal errors. Only return `Err(...)` for truly fatal failures.

## Adding a Renderer

Renderers produce output artifacts. Homer has 6 renderers. To add a new one:

### 1. Create the renderer module

Create `homer-core/src/render/my_format.rs`. Use `agents_md.rs` as a reference.

```rust
use std::path::Path;
use crate::config::HomerConfig;
use crate::render::traits::Renderer;
use crate::store::HomerStore;

#[derive(Debug)]
pub struct MyFormatRenderer;

#[async_trait::async_trait]
impl Renderer for MyFormatRenderer {
    fn name(&self) -> &'static str { "my-format" }

    fn output_path(&self) -> &'static str { "homer-output.txt" }

    async fn render(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<String> {
        // Query the store for data
        // Format it into your output
        Ok("rendered content".to_string())
    }

    // The default `write()` implementation handles:
    // - Creating parent directories
    // - Merging with <!-- homer:preserve --> blocks
    // Override only if you need different behavior
}
```

### 2. Add config (optional)

If your renderer needs configuration, add a config struct in `homer-core/src/config.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MyFormatConfig {
    pub output_path: String,
}

impl Default for MyFormatConfig {
    fn default() -> Self {
        Self { output_path: "homer-output.txt".to_string() }
    }
}
```

Add it to `RenderersSection`:

```rust
pub struct RenderersSection {
    // ... existing fields ...
    #[serde(default, rename = "my-format")]
    pub my_format: MyFormatConfig,
}
```

### 3. Register in the pipeline

Edit `homer-core/src/pipeline.rs`:

1. Add to `ALL_RENDERER_NAMES`:
   ```rust
   pub const ALL_RENDERER_NAMES: &[&str] = &[
       // ... existing names ...
       "my-format",
   ];
   ```

2. Add to `build_renderer()`:
   ```rust
   "my-format" => Some(Box::new(MyFormatRenderer)),
   ```

3. Add to `run_selected_renderers()` match arm:
   ```rust
   "my-format" => ("render:my_format".into(), Box::new(MyFormatRenderer)),
   ```

### 4. Write tests

Test that your renderer:
- Produces non-empty output for a populated store
- Handles an empty store gracefully
- Output is valid for its format (parseable JSON, valid Markdown, etc.)

## Adding an Extractor

Extractors populate the hypergraph from external sources. Homer has 7 extractors. To add a new one:

### 1. Create the extractor module

Create `homer-core/src/extract/my_source.rs`. Use `git.rs` as a reference.

```rust
use crate::config::HomerConfig;
use crate::extract::traits::{ExtractStats, Extractor};
use crate::store::HomerStore;

#[derive(Debug)]
pub struct MySourceExtractor {
    repo_path: std::path::PathBuf,
}

impl MySourceExtractor {
    pub fn new(repo_path: &std::path::Path) -> Self {
        Self { repo_path: repo_path.to_path_buf() }
    }
}

#[async_trait::async_trait(?Send)]
impl Extractor for MySourceExtractor {
    fn name(&self) -> &'static str { "my-source" }

    async fn has_work(&self, store: &dyn HomerStore) -> crate::error::Result<bool> {
        // Check if there's new data to process
        // Use checkpoints to track what was already processed
        Ok(true)
    }

    async fn extract(
        &self,
        store: &dyn HomerStore,
        config: &HomerConfig,
    ) -> crate::error::Result<ExtractStats> {
        let mut stats = ExtractStats::default();

        // Create nodes via store.upsert_node()
        // Create edges via store.upsert_hyperedge()
        // Track progress in stats

        Ok(stats)
    }
}
```

### 2. Register in the pipeline

Edit `homer-core/src/pipeline.rs` in `run_extraction()`:

```rust
extractors.push(Box::new(MySourceExtractor::new(&self.repo_path)));
```

### 3. Add node/edge kinds if needed

If your extractor creates new entity types, add variants to `NodeKind` and `HyperedgeKind` in `types.rs`. The compiler will guide you through all the match arms that need updating.

### Key Concepts

- **`?Send` trait**: The `Extractor` trait uses `async_trait(?Send)` because some extractors (like git via `gix`) use `RefCell` types that aren't `Send`. This is fine since extractors run sequentially.
- **`has_work()`**: Implement this to skip unnecessary work. The git extractor checks if there are commits after `git_last_sha`. Return `true` if unsure.
- **Checkpoints**: Use `store.set_checkpoint()` / `store.get_checkpoint()` for incrementality.
- **Batch operations**: For high-throughput extraction, use `store.upsert_nodes_batch()` instead of individual `upsert_node()` calls.

## Testing Patterns

### Unit Tests

Place `#[cfg(test)]` modules in each source file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn my_analyzer_handles_empty_store() {
        let store = SqliteStore::in_memory().unwrap();
        let config = HomerConfig::default();
        let analyzer = MyAnalyzer;

        let stats = analyzer.analyze(&store, &config).await.unwrap();
        assert_eq!(stats.results_stored, 0);
    }
}
```

### Integration Tests

Add end-to-end tests in `homer-test/tests/pipeline.rs`:

```rust
#[tokio::test]
async fn my_feature_integration() {
    let tmp = tempfile::tempdir().unwrap();
    // Set up test repo
    // Run pipeline
    // Assert results
}
```

### Property Tests

Use `proptest` for serde round-trips and invariant checking:

```rust
proptest! {
    #[test]
    fn my_type_serde_roundtrip(val in arb_my_type()) {
        let json = serde_json::to_string(&val).unwrap();
        let back: MyType = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(back, val);
    }
}
```

## Conventions

- **Edition 2024**, MSRV 1.85, `unsafe` forbidden
- **Clippy pedantic** with `-D warnings` — `--all-targets` in CI catches test code lints
- **`async_trait`** on all async traits (Analyzer, Renderer, HomerStore)
- **Error collection** over error propagation in pipeline stages
- **Exhaustive enums** for type safety — the compiler is your guide

## Next Steps

- [Internals](internals.md) — Architecture deep dive
- [Concepts](concepts.md) — User-facing explanation
- [Contributing](../CONTRIBUTING.md) — General contribution guidelines
