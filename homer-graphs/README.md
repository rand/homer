# homer-graphs

Tree-sitter-based source code extraction engine for Homer. Parses 13 languages to build scope graphs, call graphs, and import graphs.

## Supported Languages

| Language | Module | Tier | Notes |
|----------|--------|------|-------|
| Rust | `languages/rust.rs` | Precise | Full scope graph, `crate::`/`super::` resolution |
| Python | `languages/python.rs` | Precise | Module imports, class methods |
| TypeScript | `languages/typescript.rs` | Precise | Shares ECMAScript scope walker |
| JavaScript | `languages/javascript.rs` | Precise | Shares ECMAScript scope walker |
| Go | `languages/go.rs` | Precise | Package imports |
| Java | `languages/java.rs` | Precise | Package/class imports |
| Ruby | `languages/ruby.rs` | Precise | `def`/`class`/`module` scope gates, `require` imports |
| Swift | `languages/swift.rs` | Precise | Structs, classes, protocols, access control |
| Kotlin | `languages/kotlin.rs` | Precise | Classes, objects, companion objects, KDoc |
| C# | `languages/csharp.rs` | Precise | Namespaces (block + file-scoped), XML docs |
| PHP | `languages/php.rs` | Precise | Namespaces, traits, PHPDoc |
| Zig | `languages/zig.rs` | Precise | `pub` export control, structs/enums/unions, `@import` |
| Lean 4 | `languages/lean.rs` | Precise | Flat namespace/end markers, `private` suppression, `/-- -/` docs |

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
  ruby.rs         — Ruby support
  swift.rs        — Swift support
  kotlin.rs       — Kotlin support
  csharp.rs       — C# support
  php.rs          — PHP support
  zig.rs          — Zig support
  lean.rs         — Lean 4 support
  fallback.rs     — Fallback for unsupported languages
  ecma_scope.rs   — Shared ECMAScript scope graph walker (TS + JS)
```

## Adding a Language

See [docs/extending.md](../docs/extending.md) for a step-by-step guide.

## Tests

204 unit tests covering extraction, scope resolution, and cross-language consistency.
