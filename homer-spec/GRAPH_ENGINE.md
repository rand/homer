# Homer Graph Engine

> Stack graphs evolution, tree-sitter integration, language support tiers, and graph diffing.

**Parent**: [README.md](README.md)  
**Related**: [EXTRACTORS.md](EXTRACTORS.md) · [ANALYZERS.md](ANALYZERS.md) · [PERFORMANCE.md](PERFORMANCE.md)  
**Prior art**: [github/stack-graphs](https://github.com/github/stack-graphs) (archived Sept 2025), [tree-sitter-graph](https://github.com/tree-sitter/tree-sitter-graph), [Scope Graphs (TU Delft)](https://pl.ewi.tudelft.nl/research/projects/scope-graphs/)

---

## Design Philosophy

Homer's graph engine (`homer-graphs` crate) evolves the ideas behind GitHub's stack-graphs project into a purpose-built system for repository analysis. The key changes from the original:

1. **Call graph projection**: Stack graphs solve name resolution (reference → definition). Homer adds the projection step: resolving all calls within a function to produce a call graph.
2. **Temporal awareness**: The engine tracks graph diffs between snapshots, not just the current state.
3. **Tiered precision**: Explicit precision tiers per language, with graceful degradation.
4. **New language rules**: Rust and Go stack graph rules, which the original project never shipped.
5. **Performance focus**: Parallel construction, arena allocation, incremental updates.

### Relationship to Archived stack-graphs

The `github/stack-graphs` repository was archived on September 9, 2025. The published crates (`stack-graphs`, `tree-sitter-stack-graphs`, and language-specific crates) remain available on crates.io but are unmaintained.

**Homer's approach**: Fork the core algorithms (scope graph construction, path-stitching) into the `homer-graphs` crate rather than depending on the archived crates. This gives us:
- Control over bug fixes and improvements
- Ability to optimize for Homer's specific workload (batch analysis, not interactive IDE queries)
- Freedom to extend the TSG rule format if needed
- No dependency on unmaintained external code

The `tree-sitter-graph` crate (maintained by the tree-sitter project, not GitHub) remains a viable dependency for executing TSG rules.

**Why fork rather than pure DSL**: An alternative approach would be to skip the scope graph fork entirely and use `tree-sitter-graph` as a standalone DSL with targeted per-language resolvers. This was considered and rejected for the MVP because: (1) `tree-sitter-graph` provides graph *construction* from parse trees but not the path-stitching algorithm that resolves names across scopes — that algorithm is the core value of stack-graphs; (2) building per-language name resolvers without the scope graph framework means reimplementing scope graph semantics ad hoc for each language; (3) the forked code is ~5k lines of well-tested Rust focused specifically on the algorithms Homer needs. The cost of maintaining a focused fork is lower than the cost of reinventing cross-scope resolution per language.

---

## Core Concepts

### Scope Graph

A scope graph encodes a program's name binding structure as a graph:

- **Push symbol nodes**: Represent references (uses of a name)
- **Pop symbol nodes**: Represent definitions (declarations of a name)
- **Scope nodes**: Define visibility boundaries (blocks, modules, namespaces)
- **Edges**: Connect scopes, creating paths from references to definitions

Name resolution = finding a valid path from a push node to a pop node with matching symbols.

### Stack Graph (extension of scope graph)

Stack graphs add a stack discipline to path-finding, handling language features like qualified names (`foo.bar.baz`), scoped imports, and name shadowing. The "stack" tracks partially resolved names during traversal.

### File-Incremental Construction

Each source file produces an isolated subgraph. Subgraphs connect to each other through well-defined "export" and "import" scope nodes. This means:
- Rebuilding one file's subgraph doesn't require re-analyzing any other file
- Subgraphs can be constructed in parallel
- Only the path-stitching (resolution) step operates across file boundaries

### Call Graph Projection

Given resolved name bindings, Homer projects a call graph:

```
For each function definition F:
    For each call expression C within F's body:
        Resolve C using scope graph path-stitching
        If C resolves to function definition G:
            Add edge F → G to call graph
```

This projection is what enables centrality analysis. The accuracy of the call graph depends entirely on the accuracy of name resolution.

---

## Language Support

### Resolution Tiers

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResolutionTier {
    /// Full scope graph rules — precise cross-file resolution
    /// Languages: Python, JavaScript, TypeScript, Java, Rust, Go
    Precise,
    /// Tree-sitter heuristic — within-file + import-based guesses  
    /// Languages: C, C++, Ruby, PHP, Swift, Kotlin, and others with tree-sitter grammars
    Heuristic,
    /// Package manifest only — module-level dependencies
    /// Languages: any with a recognized package manifest
    Manifest,
    /// No analysis possible
    Unsupported,
}
```

### Language Support Trait

```rust
pub trait LanguageSupport: Send + Sync {
    /// Language identifier (e.g., "rust", "python")
    fn id(&self) -> &str;
    
    /// File extensions this language handles
    fn extensions(&self) -> &[&str];
    
    /// Resolution tier this implementation provides
    fn tier(&self) -> ResolutionTier;
    
    /// Tree-sitter language for parsing
    fn tree_sitter_language(&self) -> tree_sitter::Language;
    
    /// Build scope graph for a single file (Precise tier)
    fn build_scope_graph(
        &self,
        source: &str,
        path: &Path,
        globals: &Variables,
    ) -> Result<FileGraph>;
    
    /// Extract definitions and calls heuristically (Heuristic tier fallback)
    fn extract_heuristic(
        &self,
        tree: &Tree,
        source: &str,
        path: &Path,
    ) -> Result<HeuristicGraph>;
    
    /// Parse package manifest (Manifest tier)
    fn parse_manifest(
        &self,
        source: &str,
        path: &Path,
    ) -> Result<Vec<ManifestDependency>>;
    
    /// Tree-sitter queries for convention extraction
    fn convention_queries(&self) -> &[ConventionQuery];
}
```

### Language Registry

```rust
pub struct LanguageRegistry {
    languages: HashMap<String, Arc<dyn LanguageSupport>>,
    extension_map: HashMap<String, String>,  // ".rs" → "rust"
}

impl LanguageRegistry {
    pub fn new() -> Self {
        let mut reg = Self::default();
        reg.register(Arc::new(languages::PythonSupport::new()));
        reg.register(Arc::new(languages::TypeScriptSupport::new()));
        reg.register(Arc::new(languages::JavaScriptSupport::new()));
        reg.register(Arc::new(languages::JavaSupport::new()));
        reg.register(Arc::new(languages::RustSupport::new()));
        reg.register(Arc::new(languages::GoSupport::new()));
        reg.register(Arc::new(languages::FallbackSupport::new()));
        reg
    }
    
    pub fn for_file(&self, path: &Path) -> Option<Arc<dyn LanguageSupport>> { ... }
}
```

### Per-Language Notes

**Python** (Precise): Existing stack-graph rules from GitHub cover most patterns. Dynamic dispatch and runtime imports are inherently imprecise in static analysis — accept this limitation and annotate edges with lower confidence when resolution is ambiguous.

**JavaScript / TypeScript** (Precise): Existing rules cover ES modules, CommonJS requires, and TypeScript type resolution. Dynamic `require()` calls and computed property access are heuristic.

**Java** (Precise): Existing rules cover package imports, class hierarchy. Reflection-based calls are invisible to static analysis.

**Rust** (Precise — new): Rust's explicit module system, use declarations, and trait method resolution make it highly amenable to scope graph analysis. Key challenges:
- Macro expansion (capture common macros like `vec![]`, `println![]` but don't attempt arbitrary macro expansion)
- Trait method dispatch (resolve to trait definition, not concrete impl — which is correct for "what interface does this call?" analysis)
- `impl` blocks linking methods to types

**Go** (Precise — new): Go's package system is straightforward (one package per directory, explicit imports). Key challenges:
- Interface satisfaction is implicit (no `implements` keyword) — detect via structural matching
- Method sets and embedding
- `init()` function semantics

**C/C++** (Heuristic): Too complex for full scope graph rules (macros, headers, include paths, templates, ADL). Heuristic extraction can still identify function definitions, call sites, and `#include` relationships. Confidence scores will be lower.

---

## Graph Data Structures

### FileGraph (per-file scope graph output)

```rust
pub struct FileGraph {
    pub file_path: PathBuf,
    pub definitions: Vec<Definition>,
    pub references: Vec<Reference>,
    pub scope_nodes: Vec<ScopeNode>,
    pub edges: Vec<ScopeEdge>,
}

pub struct Definition {
    pub name: String,
    pub kind: SymbolKind,        // Function, Type, Variable, Module
    pub span: TextRange,
    pub scope_node_id: ScopeNodeId,
}

pub struct Reference {
    pub name: String,
    pub kind: SymbolKind,
    pub span: TextRange,
    pub scope_node_id: ScopeNodeId,
}
```

### HeuristicGraph (per-file heuristic output)

```rust
pub struct HeuristicGraph {
    pub file_path: PathBuf,
    pub definitions: Vec<HeuristicDef>,
    pub calls: Vec<HeuristicCall>,
    pub imports: Vec<HeuristicImport>,
}

pub struct HeuristicDef {
    pub name: String,
    pub qualified_name: String,  // module.class.method
    pub kind: SymbolKind,
    pub span: TextRange,
}

pub struct HeuristicCall {
    pub caller: String,          // Qualified name of containing function
    pub callee_name: String,     // Name at call site (may be unqualified)
    pub span: TextRange,
    pub confidence: f64,         // How confident are we in the resolution?
}

pub struct HeuristicImport {
    pub from_path: PathBuf,      // The importing file
    pub imported_name: String,   // What's imported
    pub target_path: Option<PathBuf>,  // Resolved target file (if determinable)
    pub confidence: f64,
}
```

---

## Graph Diffing

When a file is re-analyzed, Homer computes the diff between old and new subgraphs:

```rust
pub struct GraphDiff {
    pub added_definitions: Vec<Definition>,
    pub removed_definitions: Vec<Definition>,
    pub added_edges: Vec<(String, String)>,     // (caller, callee) qualified names
    pub removed_edges: Vec<(String, String)>,
    pub renamed_symbols: Vec<(String, String)>,  // (old_name, new_name)
}
```

### Diff Algorithm

1. Load previous FileGraph/HeuristicGraph for the file from store
2. Build new graph from current source
3. Match definitions by qualified name (exact match) or by span proximity (for renames)
4. Edges present in new but not old → added
5. Edges present in old but not new → removed
6. Definitions with matching span but different name → renamed

### Temporal Storage

Graph diffs are stored as analysis results with timestamps, enabling the [temporal analyzer](ANALYZERS.md#temporal-analyzer) to track how the graph evolves over time.

---

## Construction Pipeline

For a batch of files:

```
Files to process (changed since last run)
    │
    ├── Rayon parallel: parse each file with tree-sitter
    │       │
    │       ├── If Precise tier: execute TSG rules → FileGraph
    │       ├── If Heuristic tier: walk AST → HeuristicGraph
    │       └── For both tiers: extract doc comments adjacent to definitions
    │
    ├── Merge subgraphs into shared scope graph (sequential, thread-safe)
    │
    ├── Path-stitching for cross-file resolution (can parallelize per-query)
    │
    ├── Project call graph from resolved references
    │
    ├── Compute diffs against previous state
    │
    └── Write to store: nodes (Function, Type) with doc comment metadata,
        edges (Calls, Imports), diffs
```

### Doc Comment Extraction

During tree-sitter parsing, the graph engine extracts doc comments adjacent to function, type, and module definitions. This is a trivial addition to the existing AST walk — no separate pass is needed.

Extracted data (stripped text, content hash, detected doc style) is stored as metadata on the corresponding `Function`, `Type`, or `Module` node. See [STORE.md](STORE.md#doc-comment-metadata) for the data model and [EXTRACTORS.md](EXTRACTORS.md#doc-comment-extraction) for the extraction logic.

### Parallelism

- Parsing: embarrassingly parallel (one tree-sitter parser per thread, each file independent)
- Subgraph construction: parallel (each file's subgraph is isolated)
- Graph merging: sequential (writes to shared data structure)
- Path-stitching: parallelizable per-query, but queries share the graph read-only
- Call graph projection: parallel (each function's calls resolved independently once stitching is done)

See [PERFORMANCE.md](PERFORMANCE.md) for detailed parallelism strategy.
