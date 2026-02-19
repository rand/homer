# homer-graphs

Tree-sitter-based source code extraction engine for Homer. Parses 6 languages to build scope graphs, call graphs, and import graphs.

## Supported Languages

| Language | Module | Tier | Notes |
|----------|--------|------|-------|
| Rust | `languages/rust.rs` | Precise | Full scope graph, `crate::`/`super::` resolution |
| Python | `languages/python.rs` | Precise | Module imports, class methods |
| TypeScript | `languages/typescript.rs` | Precise | Shares ECMAScript scope walker |
| JavaScript | `languages/javascript.rs` | Precise | Shares ECMAScript scope walker |
| Go | `languages/go.rs` | Precise | Package imports |
| Java | `languages/java.rs` | Precise | Package/class imports |

All languages use `ResolutionTier::Precise` via scope graph construction.

## Key Types

- **`LanguageSupport`** — Trait for language-specific extraction. Provides tree-sitter grammar, scope graph builder, and heuristic fallback.
- **`FileScopeGraph`** — Per-file scope graph mapping definitions and references to scopes.
- **`HeuristicGraph`** — Extracted definitions (`HeuristicDef`), call sites (`HeuristicCall`), and imports (`HeuristicImport`).
- **`ScopeGraphBuilder`** — Helper for constructing scope graphs (in `languages/helpers.rs`).

## Architecture

```
scope_graph.rs    — FileScopeGraph type and scope resolution
call_graph.rs     — Cross-file call graph construction
import_graph.rs   — Cross-file import graph construction
diff.rs           — File-level diff utilities
languages/
  mod.rs          — Language dispatch (extension → LanguageSupport)
  helpers.rs      — ScopeGraphBuilder, AST traversal utilities
  rust.rs         — Rust support (canonical reference implementation)
  python.rs       — Python support
  typescript.rs   — TypeScript support
  javascript.rs   — JavaScript support
  go.rs           — Go support
  java.rs         — Java support
  fallback.rs     — Fallback for unsupported languages
  ecma_scope.rs   — Shared ECMAScript scope graph walker (TS + JS)
```

## Adding a Language

See [docs/extending.md](../docs/extending.md) for a step-by-step guide.

## Tests

102 unit tests covering extraction, scope resolution, and cross-language consistency.
