// Benchmark tree-sitter parsing and heuristic extraction throughput.

use std::path::Path;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rayon::prelude::*;

use homer_graphs::LanguageRegistry;

fn generate_rust_source(functions: usize) -> String {
    use std::fmt::Write;
    let mut src = String::new();
    for i in 0..functions {
        let _ = write!(
            src,
            "/// Doc comment for function {i}.\nfn func_{i}(x: i32) -> i32 {{\n    helper_{i}(x + 1)\n}}\n\n"
        );
    }
    src
}

fn generate_python_source(functions: usize) -> String {
    use std::fmt::Write;
    let mut src = String::new();
    for i in 0..functions {
        let _ = write!(
            src,
            "def func_{i}(x):\n    \"\"\"Doc for func_{i}.\"\"\"\n    return helper_{i}(x + 1)\n\n"
        );
    }
    src
}

fn bench_parse_single_file(c: &mut Criterion) {
    let registry = LanguageRegistry::new();
    let rust_lang = registry.get("rust").unwrap();

    let mut group = c.benchmark_group("parse_single_file");

    for func_count in [10, 50, 200] {
        let source = generate_rust_source(func_count);

        group.bench_with_input(
            BenchmarkId::new("rust_functions", func_count),
            &source,
            |b, src| {
                b.iter(|| {
                    let mut parser = tree_sitter::Parser::new();
                    parser
                        .set_language(&rust_lang.tree_sitter_language())
                        .unwrap();
                    let tree = parser.parse(src, None).unwrap();
                    rust_lang
                        .extract_heuristic(&tree, src, Path::new("bench.rs"))
                        .unwrap();
                });
            },
        );
    }
    group.finish();
}

fn bench_parse_parallel(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_parallel");

    // Generate a set of source files
    let file_count = 100;
    let files: Vec<(String, String)> = (0..file_count)
        .map(|i| {
            if i % 2 == 0 {
                (format!("file_{i}.rs"), generate_rust_source(20))
            } else {
                (format!("file_{i}.py"), generate_python_source(20))
            }
        })
        .collect();

    group.bench_function("100_files_sequential", |b| {
        let registry = LanguageRegistry::new();
        b.iter(|| {
            for (name, src) in &files {
                let path = Path::new(name);
                if let Some(lang) = registry.for_file(path) {
                    let mut parser = tree_sitter::Parser::new();
                    parser.set_language(&lang.tree_sitter_language()).unwrap();
                    let tree = parser.parse(src, None).unwrap();
                    let _ = lang.extract_heuristic(&tree, src, path);
                }
            }
        });
    });

    group.bench_function("100_files_rayon", |b| {
        let registry = LanguageRegistry::new();
        b.iter(|| {
            files
                .par_iter()
                .filter_map(|(name, src)| {
                    let path = Path::new(name);
                    let lang = registry.for_file(path)?;
                    let mut parser = tree_sitter::Parser::new();
                    parser.set_language(&lang.tree_sitter_language()).ok()?;
                    let tree = parser.parse(src, None)?;
                    lang.extract_heuristic(&tree, src, path).ok()
                })
                .collect::<Vec<_>>()
        });
    });

    group.finish();
}

fn bench_extract_heuristic(c: &mut Criterion) {
    let registry = LanguageRegistry::new();
    let mut group = c.benchmark_group("extract_heuristic");

    let languages = [
        ("rust", generate_rust_source(50)),
        ("python", generate_python_source(50)),
    ];

    for (lang_id, source) in &languages {
        let lang = registry.get(lang_id).unwrap();
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&lang.tree_sitter_language()).unwrap();
        let tree = parser.parse(source, None).unwrap();

        group.bench_with_input(
            BenchmarkId::new("language", lang_id),
            &(&tree, source.as_str()),
            |b, (tree, src)| {
                b.iter(|| {
                    lang.extract_heuristic(tree, src, Path::new("bench"))
                        .unwrap();
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_parse_single_file,
    bench_parse_parallel,
    bench_extract_heuristic,
);
criterion_main!(benches);
