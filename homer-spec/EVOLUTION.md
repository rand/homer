# Homer Evolution & Extensibility

> Plugin system, schema versioning, language additions, technology evaluations, and future paths.

**Parent**: [README.md](README.md)  
**Related**: [ARCHITECTURE.md](ARCHITECTURE.md) · [STORE.md](STORE.md) · [GRAPH_ENGINE.md](GRAPH_ENGINE.md)

---

## Design for Change

Homer is designed to evolve across several dimensions without breaking changes:

1. **New languages**: Add graph extraction support without touching core
2. **New analyzers**: Add analysis passes without changing existing ones
3. **New renderers**: Add output formats without touching analysis
4. **Schema evolution**: Database schema migrations without data loss
5. **Store backends**: Swap SQLite for libSQL or other backends
6. **LLM providers**: Add new providers without changing analysis logic
7. **Forge support**: Add GitLab, Bitbucket alongside GitHub

---

## Plugin-Based Language Support

Languages are registered at startup, not compiled in:

```rust
pub struct LanguageRegistry {
    languages: HashMap<String, Arc<dyn LanguageSupport>>,
    extension_map: HashMap<String, String>,
}

impl LanguageRegistry {
    /// Register built-in languages
    pub fn with_defaults() -> Self {
        let mut reg = Self::new();
        reg.register(Arc::new(PythonSupport::new()));
        reg.register(Arc::new(TypeScriptSupport::new()));
        reg.register(Arc::new(JavaScriptSupport::new()));
        reg.register(Arc::new(JavaSupport::new()));
        reg.register(Arc::new(RustSupport::new()));
        reg.register(Arc::new(GoSupport::new()));
        reg
    }
    
    /// Register a custom language implementation
    pub fn register(&mut self, lang: Arc<dyn LanguageSupport>) {
        for ext in lang.extensions() {
            self.extension_map.insert(ext.to_string(), lang.id().to_string());
        }
        self.languages.insert(lang.id().to_string(), lang);
    }
}
```

### Adding a New Language

To add support for a new language (e.g., Kotlin):

1. **Heuristic tier (minimum viable)**:
   - Implement `LanguageSupport` with `tier() = Heuristic`
   - Write tree-sitter queries for function definitions, call sites, imports
   - Implement `extract_heuristic()` to walk AST and extract symbols

2. **Precise tier (full support)**:
   - Write TSG (tree-sitter-graph) rules for scope graph construction
   - Implement `build_scope_graph()` to execute rules
   - Write tests against known Kotlin codebases

3. **Register in `LanguageRegistry::with_defaults()`**

No other code changes required — extractors, analyzers, and renderers are language-agnostic.

### Future: Dynamic Language Plugins

Long-term, languages could be loaded from external files (TSG rule files + configuration) without recompilation:

```toml
# .homer/languages/kotlin.toml
[language]
id = "kotlin"
extensions = [".kt", ".kts"]
tier = "heuristic"
tree_sitter_grammar = "tree-sitter-kotlin"

[queries]
function_def = "(function_declaration name: (simple_identifier) @name) @definition"
call_expression = "(call_expression (simple_identifier) @callee)"
import_statement = "(import_header (identifier) @module)"
```

This is a stretch goal, not a launch requirement.

---

## Renderer Extensibility

Renderers are registered similarly to languages:

```rust
pub struct RendererRegistry {
    renderers: HashMap<String, Arc<dyn Renderer>>,
}

impl RendererRegistry {
    pub fn with_defaults() -> Self {
        let mut reg = Self::new();
        reg.register(Arc::new(AgentsMdRenderer::new()));
        reg.register(Arc::new(ModuleCtxRenderer::new()));
        reg.register(Arc::new(SkillsRenderer::new()));
        reg.register(Arc::new(SpecRenderer::new()));
        reg.register(Arc::new(ReportRenderer::new()));
        reg.register(Arc::new(RiskMapRenderer::new()));
        reg
    }
}
```

### Custom Renderers

Users could implement custom renderers (e.g., "Jira ticket generator", "Confluence page", "ADR document") by implementing the `Renderer` trait. For now, this requires Rust code and recompilation. A templating system (e.g., Tera templates consuming JSON from the store) could enable non-Rust custom renderers later.

---

## Schema Versioning

### Migration System

```rust
pub struct Migration {
    pub version: u32,
    pub description: &'static str,
    pub up: &'static str,    // SQL to apply migration
    pub down: &'static str,  // SQL to reverse migration (best-effort)
}

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "Initial schema",
        up: include_str!("migrations/001_initial.sql"),
        down: "DROP TABLE IF EXISTS nodes; DROP TABLE IF EXISTS hyperedges; ...",
    },
    Migration {
        version: 2,
        description: "Add graph snapshots table",
        up: include_str!("migrations/002_graph_snapshots.sql"),
        down: "DROP TABLE IF EXISTS graph_snapshots;",
    },
    // ...
];
```

### Migration Protocol

On database open:

```rust
async fn ensure_schema(conn: &Connection) -> Result<()> {
    let current_version = get_schema_version(conn)?;
    let target_version = MIGRATIONS.last().map(|m| m.version).unwrap_or(0);
    
    if current_version > target_version {
        return Err(HomerError::Config(
            format!("Database was created by a newer Homer version (schema v{}). \
                     This Homer supports up to schema v{}.", current_version, target_version)
        ));
    }
    
    for migration in MIGRATIONS.iter().filter(|m| m.version > current_version) {
        conn.execute_batch(migration.up)?;
        set_schema_version(conn, migration.version)?;
    }
    
    Ok(())
}
```

### Compatibility Rules

- **Forward compatible**: Older Homer can read databases created by newer Homer (if schema is backward compatible)
- **Backward compatible**: Newer Homer can read and upgrade databases from older Homer
- **Breaking changes**: If a migration is destructive, Homer prompts user for confirmation and suggests backing up first

---

## Store Backend Evolution

### The libSQL Path

Current: `SqliteStore` using `rusqlite` (pure SQLite).

Future option: `LibsqlStore` using the `libsql` crate. Benefits:

| Feature | SQLite | libSQL |
|---------|--------|--------|
| Embedded, zero-config | ✓ | ✓ |
| WAL mode | ✓ | ✓ |
| Full-text search (FTS5) | ✓ | ✓ |
| Vector search | Extension | Native |
| Embedded replicas | ✗ | ✓ |
| ALTER TABLE extensions | Limited | Extended |
| MVCC (concurrent writes) | ✗ | ✓ (experimental) |
| Community/governance | SQLite Consortium | Open source (MIT) |

**When to switch**: If Homer grows to need:
- **Vector search**: For semantic similarity queries ("find functions similar to this one" using embedding vectors of semantic summaries). This would require embedding generation (additional LLM calls or local model) but enable powerful approximate matching.
- **Replication**: For team/shared Homer instances where multiple developers sync their analysis.
- **Concurrent writes**: If Homer's pipeline evolves to write from multiple threads simultaneously.

**How to switch**: Implement `LibsqlStore` behind the same `HomerStore` trait. The `libsql` crate's API is similar enough to `rusqlite` that much code can be shared via a thin adapter layer.

### The Turso/Limbo Path

Turso (Rust rewrite of SQLite, formerly Limbo) is the long-term direction of the libSQL project. Once stable, it would be the ideal backend: pure Rust, MVCC, native vector search, DST-tested reliability. Monitor for production readiness.

---

## LLM Provider Extensibility

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    
    async fn complete(&self, request: &CompletionRequest) -> Result<CompletionResponse>;
    
    /// Estimated cost for a request (for budget tracking)
    fn estimate_cost(&self, input_tokens: u32, output_tokens: u32) -> f64;
}

pub struct CompletionRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    pub temperature: f64,
    pub response_format: Option<ResponseFormat>,  // JSON mode
}
```

Built-in providers:
- `AnthropicProvider`: Claude API via `https://api.anthropic.com/v1/messages`
- `OpenAiProvider`: OpenAI API via `https://api.openai.com/v1/chat/completions`
- `CustomProvider`: Configurable base URL for API-compatible endpoints

---

## Forge Extensibility

```rust
#[async_trait]
pub trait ForgeExtractor: Extractor {
    fn forge_type(&self) -> ForgeType;
    
    /// Auto-detect from git remotes
    fn detect(repo_path: &Path) -> Option<ForgeConfig>;
    
    /// Fetch pull/merge requests
    async fn fetch_pull_requests(
        &self, store: &dyn HomerStore, since: Option<u64>
    ) -> Result<Vec<PullRequest>>;
    
    /// Fetch issues
    async fn fetch_issues(
        &self, store: &dyn HomerStore, since: Option<u64>
    ) -> Result<Vec<Issue>>;
}

pub enum ForgeType {
    GitHub,
    GitLab,
    Bitbucket,
}
```

### Detection Logic

```rust
fn detect_forge(repo_path: &Path) -> Option<(ForgeType, ForgeConfig)> {
    let repo = git2::Repository::open(repo_path).ok()?;
    let remote = repo.find_remote("origin").ok()?;
    let url = remote.url()?;
    
    if url.contains("github.com") {
        Some((ForgeType::GitHub, parse_github_url(url)))
    } else if url.contains("gitlab") {
        Some((ForgeType::GitLab, parse_gitlab_url(url)))
    } else if url.contains("bitbucket") {
        Some((ForgeType::Bitbucket, parse_bitbucket_url(url)))
    } else {
        None
    }
}
```

---

## Versioned Public API

### Stability Guarantees

| Surface | Stability | Policy |
|---------|-----------|--------|
| CLI commands and flags | Stable | Semver: breaking changes = major version |
| Config file format | Stable | Backward compatible, new fields optional |
| MCP tool schemas | Stable | Additive changes only |
| `homer-core` public API | Unstable (0.x) | May change between minor versions |
| `homer-graphs` public API | Unstable (0.x) | May change between minor versions |
| Database schema | Migrated | Automatic forward migration |
| Output file formats | Versioned | Include version field, document changes |
| Risk map JSON schema | Versioned | `"version": "1.0"` field |

### Deprecation Process

1. Add deprecation warning in current version
2. Provide migration path in documentation
3. Remove deprecated feature in next major version
4. Minimum 2 minor versions between deprecation and removal

---

## Implementation Phases

### Phase 1: Foundation
- `homer-core`: Store (SQLite), git extractor, structure extractor, document extractor
- `homer-graphs`: Tree-sitter heuristic extraction (fallback tier) for top 4 languages, doc comment extraction
- `homer-cli`: `init`, `update`, `status`
- Behavioral analyzer: change frequency, co-change, bus factor, documentation coverage
- Renderer: AGENTS.md (basic version, with circularity handling)

### Phase 2: Graph Intelligence
- `homer-graphs`: Precise tier (stack graph rules) for Python and TypeScript
- Centrality analyzer: PageRank, betweenness, HITS, composite salience
- Community detection
- Temporal analyzer: graph snapshots and diffing
- Renderers: Module context, risk map
- CLI: `query`, `graph`

### Phase 3: Semantic Enrichment
- GitHub extractor: PRs, issues, reviews
- Semantic analyzer: LLM-powered summaries (doc-comment-aware), design rationale extraction
- Convention analyzer (including documentation style detection)
- Renderers: Skills, report
- CLI: `diff`, `serve` (MCP)

### Phase 4: Agent Intelligence
- Prompt extractor: Claude Code sessions, agent rule files, Cursor/Windsurf/Cline rules
- Task pattern analyzer
- Domain vocabulary extraction
- Correction hotspot analysis
- Enhanced AGENTS.md renderer: Common Tasks, Areas That Confuse Agents, Domain Vocabulary
- Enhanced skills renderer: prompt-derived skills
- Enhanced risk map: agent confusion zones, underprompted areas

### Phase 5: Full Coverage
- `homer-graphs`: Precise tier for Rust, Go, Java, JavaScript
- Renderer: Topos spec output (with ADR-derived content)
- Forge extensibility (GitLab)
- libSQL evaluation and potential migration
- facet.rs re-evaluation (if 1.0 reached)
- Performance optimization pass based on real-world profiling

---

## Technology Evaluations

### facet.rs (Deferred to Post-v1)

[facet.rs](https://facet.rs) is a Rust reflection library by Amos Wenger (fasterthanlime). From a single `#[derive(Facet)]`, you get serialization/deserialization (JSON, YAML, TOML, MessagePack), pretty-printing with sensitive field redaction, structural diffing, runtime reflection, and invariant checking.

The library has reached version 0.42+ (as of early 2026) with rapid iteration (~10 minor versions per month). It's used in Topos, creating a familiarity precedent.

**Where facet.rs would provide genuine value for Homer:**

1. **Structural diffing for incremental analysis.** Homer compares analysis results across runs to detect what changed. `facet_diff::diff(&old_metrics, &new_metrics)` would replace hand-rolled comparison logic for each analysis result type.

2. **Reflection for plugin introspection.** The plugin system (language support, renderers) could use facet's reflection to inspect configuration types at runtime — useful for generating help text, validating config files, and exposing plugin capabilities through MCP without manual schema definitions.

3. **Pretty-printing for diagnostic output.** `homer query` output and diagnostic logging benefit from colored, structured printing. `facet-pretty` handles this uniformly for all types that derive `Facet`.

4. **MCP tool schema generation from doc comments.** facet exposes Rust doc comments as const data on types at compile time. Homer's MCP tool input/output types could auto-generate schemas with human-readable descriptions pulled directly from doc comments — no separate schema definition needed.

**Why not now:**

1. **Maturity.** The project's own README says "buyer beware" and the API surface churns faster than Homer's development pace can absorb.
2. **SQLite integration.** Homer's hot path is reading/writing the hypergraph store. `rusqlite` has mature `ToSql`/`FromSql` traits. facet doesn't have `facet-sqlite` yet.
3. **serde ecosystem.** Numerous crates Homer depends on (`gix`, `octocrab`, `reqwest`, `clap`, `toml`) use serde. Homer would need both serde and facet derives on many types.
4. **Build time.** Another proc macro adds compilation time.

**Recommendation:** Use `serde` + `serde_json` + `toml` for serialization, `rusqlite`'s `ToSql`/`FromSql` for database access, `tracing` for structured logging, `Debug` derive for diagnostic output, and hand-roll diffing for the ~3-4 analysis result types that need it.

**Revisit when:** facet reaches 1.0 (or drops "buyer beware"), ships `facet-sqlite`, and structural diffing proves itself in Topos. The trait-based architecture means no internal refactoring is needed — it's just adding a derive and optionally switching diagnostic output to `facet-pretty`.

**Highest-value integration points when adopting:**
1. Replace hand-rolled diffing in the incremental invalidation layer with `facet-diff`
2. Use `facet-pretty` for all `homer query` output
3. Use reflection for auto-generating MCP tool schemas from Rust types
