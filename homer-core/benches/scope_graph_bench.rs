// Benchmark scope graph construction, reference resolution, and call graph projection.

use std::path::{Path, PathBuf};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

use homer_graphs::scope_graph::{
    FileScopeGraph, ScopeEdge, ScopeEdgeId, ScopeGraph, ScopeNode, ScopeNodeId, ScopeNodeKind,
};
use homer_graphs::{LanguageRegistry, SymbolKind, TextRange};

/// Generate a synthetic Rust file with a known number of functions and cross-references.
fn generate_rust_with_calls(num_functions: usize, calls_per_fn: usize) -> String {
    use std::fmt::Write;
    let mut src = String::new();
    for i in 0..num_functions {
        let _ = writeln!(src, "fn func_{i}(x: i32) -> i32 {{");
        for j in 0..calls_per_fn {
            let target = (i + j + 1) % num_functions;
            let _ = writeln!(src, "    let _ = func_{target}(x);");
        }
        let _ = write!(src, "    x + 1\n}}\n\n");
    }
    src
}

/// Build a synthetic `FileScopeGraph` with controlled structure for benchmarking
/// resolution without tree-sitter parsing overhead.
#[allow(clippy::too_many_lines)]
fn build_synthetic_file_graph(
    num_defs: usize,
    num_refs_per_def: usize,
    file_idx: usize,
) -> FileScopeGraph {
    let file_path = PathBuf::from(format!("src/mod_{file_idx}.rs"));
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut next_node = 0u32;
    let mut next_edge = 0u32;

    // Root scope
    let root_id = ScopeNodeId(next_node);
    nodes.push(ScopeNode {
        id: root_id,
        kind: ScopeNodeKind::Root,
        file_path: file_path.clone(),
        span: Some(TextRange {
            start_byte: 0,
            end_byte: 0,
            start_row: 0,
            start_col: 0,
            end_row: (num_defs * (num_refs_per_def + 3)) + 1,
            end_col: 0,
        }),
        symbol_kind: None,
    });
    next_node += 1;

    // Create function definitions with inner scopes and references
    for d in 0..num_defs {
        let fn_start = d * (num_refs_per_def + 3);
        let fn_end = fn_start + num_refs_per_def + 2;

        // Function scope
        let scope_id = ScopeNodeId(next_node);
        nodes.push(ScopeNode {
            id: scope_id,
            kind: ScopeNodeKind::Scope,
            file_path: file_path.clone(),
            span: Some(TextRange {
                start_byte: 0,
                end_byte: 0,
                start_row: fn_start,
                start_col: 0,
                end_row: fn_end,
                end_col: 1,
            }),
            symbol_kind: None,
        });
        edges.push(ScopeEdge {
            id: ScopeEdgeId(next_edge),
            source: root_id,
            target: scope_id,
            precedence: 0,
        });
        next_node += 1;
        next_edge += 1;

        // Definition (PopSymbol)
        let def_id = ScopeNodeId(next_node);
        let name = format!("func_{d}");
        nodes.push(ScopeNode {
            id: def_id,
            kind: ScopeNodeKind::PopSymbol {
                symbol: name.clone(),
            },
            file_path: file_path.clone(),
            span: Some(TextRange {
                start_byte: 0,
                end_byte: 0,
                start_row: fn_start,
                start_col: 3,
                end_row: fn_end,
                end_col: 1,
            }),
            symbol_kind: Some(SymbolKind::Function),
        });
        edges.push(ScopeEdge {
            id: ScopeEdgeId(next_edge),
            source: scope_id,
            target: def_id,
            precedence: 0,
        });
        next_node += 1;
        next_edge += 1;

        // References (PushSymbol) to other functions
        for r in 0..num_refs_per_def {
            let target_fn = (d + r + 1) % num_defs;
            let ref_name = format!("func_{target_fn}");
            let ref_id = ScopeNodeId(next_node);
            nodes.push(ScopeNode {
                id: ref_id,
                kind: ScopeNodeKind::PushSymbol { symbol: ref_name },
                file_path: file_path.clone(),
                span: Some(TextRange {
                    start_byte: 0,
                    end_byte: 0,
                    start_row: fn_start + r + 1,
                    start_col: 14,
                    end_row: fn_start + r + 1,
                    end_col: 25,
                }),
                symbol_kind: Some(SymbolKind::Function),
            });
            // Edge from reference up to the scope (for resolution traversal)
            edges.push(ScopeEdge {
                id: ScopeEdgeId(next_edge),
                source: ref_id,
                target: scope_id,
                precedence: 0,
            });
            next_node += 1;
            next_edge += 1;
        }
    }

    // Export scope for cross-file resolution
    let export_id = ScopeNodeId(next_node);
    nodes.push(ScopeNode {
        id: export_id,
        kind: ScopeNodeKind::ExportScope,
        file_path: file_path.clone(),
        span: None,
        symbol_kind: None,
    });
    edges.push(ScopeEdge {
        id: ScopeEdgeId(next_edge),
        source: root_id,
        target: export_id,
        precedence: 1,
    });

    FileScopeGraph {
        file_path,
        nodes,
        edges,
        root_scope: root_id,
        export_nodes: vec![export_id],
        import_nodes: vec![],
    }
}

fn bench_scope_graph_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("scope_graph_build");

    // Measure adding file subgraphs to the combined ScopeGraph
    for num_files in [10, 50, 200] {
        let file_graphs: Vec<_> = (0..num_files)
            .map(|i| build_synthetic_file_graph(20, 5, i))
            .collect();

        group.bench_with_input(
            BenchmarkId::new("files", num_files),
            &file_graphs,
            |b, graphs| {
                b.iter(|| {
                    let mut sg = ScopeGraph::new();
                    for fg in graphs {
                        sg.add_file_graph(fg);
                    }
                    sg
                });
            },
        );
    }

    group.finish();
}

fn bench_resolve_all(c: &mut Criterion) {
    let mut group = c.benchmark_group("scope_graph_resolve");
    group.sample_size(10);

    // Build scope graphs of varying sizes and measure resolution
    for (num_files, defs_per_file, refs_per_def) in [(5, 20, 3), (20, 20, 5), (50, 30, 5)] {
        let mut sg = ScopeGraph::new();
        for i in 0..num_files {
            let fg = build_synthetic_file_graph(defs_per_file, refs_per_def, i);
            sg.add_file_graph(&fg);
        }

        let total_refs = num_files * defs_per_file * refs_per_def;
        let label = format!("{num_files}f_{defs_per_file}d_{refs_per_def}r_({total_refs}_refs)");

        group.bench_with_input(BenchmarkId::new("refs", label), &sg, |b, graph| {
            b.iter(|| graph.resolve_all());
        });
    }

    group.finish();
}

fn bench_enclosing_functions(c: &mut Criterion) {
    let mut group = c.benchmark_group("enclosing_functions");

    // Measure enclosing function computation (O(refs * funcs) currently)
    for (num_defs, refs_per_def) in [(20, 5), (50, 10), (100, 10)] {
        let fg = build_synthetic_file_graph(num_defs, refs_per_def, 0);
        let total_refs = num_defs * refs_per_def;
        let label = format!("{num_defs}fn_{total_refs}refs");

        group.bench_with_input(BenchmarkId::new("size", label), &fg, |b, graph| {
            b.iter(|| homer_graphs::call_graph::compute_enclosing_functions(graph));
        });
    }

    group.finish();
}

fn bench_parse_all_languages(c: &mut Criterion) {
    let registry = LanguageRegistry::new();
    let mut group = c.benchmark_group("parse_all_languages");

    // Generate synthetic source for each supported language
    let languages: Vec<(&str, String, &str)> = vec![
        ("rust", generate_rust_with_calls(20, 3), "bench.rs"),
        ("python", generate_python_source(20), "bench.py"),
        ("typescript", generate_ts_source(20), "bench.ts"),
        ("javascript", generate_js_source(20), "bench.js"),
        ("go", generate_go_source(20), "bench.go"),
        ("java", generate_java_source(20), "Bench.java"),
        ("ruby", generate_ruby_source(20), "bench.rb"),
        ("swift", generate_swift_source(20), "bench.swift"),
        ("kotlin", generate_kotlin_source(20), "bench.kt"),
        ("csharp", generate_csharp_source(20), "Bench.cs"),
        ("php", generate_php_source(20), "bench.php"),
    ];

    for (lang_id, source, filename) in &languages {
        let Some(lang) = registry.get(lang_id) else {
            continue;
        };

        group.bench_with_input(
            BenchmarkId::new("language", lang_id),
            &(source.as_str(), *filename),
            |b, (src, fname)| {
                b.iter(|| {
                    let mut parser = tree_sitter::Parser::new();
                    parser.set_language(&lang.tree_sitter_language()).unwrap();
                    let tree = parser.parse(*src, None).unwrap();
                    lang.extract_heuristic(&tree, src, Path::new(fname))
                        .unwrap();
                });
            },
        );
    }

    group.finish();
}

// ── Source generators for all 13 languages ─────────────────────────

fn generate_python_source(n: usize) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for i in 0..n {
        let _ = write!(
            s,
            "def func_{i}(x):\n    \"\"\"Doc for func_{i}.\"\"\"\n    return func_{}(x + 1)\n\n",
            (i + 1) % n
        );
    }
    s
}

fn generate_ts_source(n: usize) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for i in 0..n {
        let _ = write!(
            s,
            "function func_{i}(x: number): number {{\n  return func_{}(x + 1);\n}}\n\n",
            (i + 1) % n
        );
    }
    s
}

fn generate_js_source(n: usize) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for i in 0..n {
        let _ = write!(
            s,
            "function func_{i}(x) {{\n  return func_{}(x + 1);\n}}\n\n",
            (i + 1) % n
        );
    }
    s
}

fn generate_go_source(n: usize) -> String {
    use std::fmt::Write;
    let mut s = String::from("package main\n\n");
    for i in 0..n {
        let _ = write!(
            s,
            "func Func{i}(x int) int {{\n\treturn Func{}(x + 1)\n}}\n\n",
            (i + 1) % n
        );
    }
    s
}

fn generate_java_source(n: usize) -> String {
    use std::fmt::Write;
    let mut s = String::from("public class Bench {\n");
    for i in 0..n {
        let _ = write!(
            s,
            "    public static int func{i}(int x) {{\n        return func{}(x + 1);\n    }}\n\n",
            (i + 1) % n
        );
    }
    s.push_str("}\n");
    s
}

fn generate_ruby_source(n: usize) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for i in 0..n {
        let _ = write!(s, "def func_{i}(x)\n  func_{}(x + 1)\nend\n\n", (i + 1) % n);
    }
    s
}

fn generate_swift_source(n: usize) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for i in 0..n {
        let _ = write!(
            s,
            "func func{i}(_ x: Int) -> Int {{\n    return func{}(x + 1)\n}}\n\n",
            (i + 1) % n
        );
    }
    s
}

fn generate_kotlin_source(n: usize) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for i in 0..n {
        let _ = write!(
            s,
            "fun func{i}(x: Int): Int {{\n    return func{}(x + 1)\n}}\n\n",
            (i + 1) % n
        );
    }
    s
}

fn generate_csharp_source(n: usize) -> String {
    use std::fmt::Write;
    let mut s = String::from("public class Bench {\n");
    for i in 0..n {
        let _ = write!(
            s,
            "    public static int Func{i}(int x) {{\n        return Func{}(x + 1);\n    }}\n\n",
            (i + 1) % n
        );
    }
    s.push_str("}\n");
    s
}

fn generate_php_source(n: usize) -> String {
    use std::fmt::Write;
    let mut s = String::from("<?php\n\n");
    for i in 0..n {
        let _ = write!(
            s,
            "function func_{i}($x) {{\n    return func_{}($x + 1);\n}}\n\n",
            (i + 1) % n
        );
    }
    s
}

criterion_group!(
    benches,
    bench_scope_graph_build,
    bench_resolve_all,
    bench_enclosing_functions,
    bench_parse_all_languages,
);
criterion_main!(benches);
