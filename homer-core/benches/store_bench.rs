// Benchmark SQLite store operations: bulk insert, lookup, analysis storage.

use std::collections::HashMap;

use chrono::Utc;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

use homer_core::store::HomerStore;
use homer_core::store::sqlite::SqliteStore;
use homer_core::types::{AnalysisKind, AnalysisResult, AnalysisResultId, Node, NodeId, NodeKind};

fn bench_bulk_insert_nodes(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("store_insert_nodes");

    for count in [100, 1_000, 5_000] {
        group.bench_with_input(BenchmarkId::new("count", count), &count, |b, &n| {
            b.iter(|| {
                rt.block_on(async {
                    let store = SqliteStore::in_memory().unwrap();
                    let now = Utc::now();
                    for i in 0..n {
                        store
                            .upsert_node(&Node {
                                id: NodeId(0),
                                kind: NodeKind::File,
                                name: format!("src/file_{i}.rs"),
                                content_hash: Some(i as u64),
                                last_extracted: now,
                                metadata: HashMap::new(),
                            })
                            .await
                            .unwrap();
                    }
                });
            });
        });
    }
    group.finish();
}

fn bench_node_lookup(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Pre-populate store
    let store = rt.block_on(async {
        let store = SqliteStore::in_memory().unwrap();
        let now = Utc::now();
        for i in 0..1_000 {
            store
                .upsert_node(&Node {
                    id: NodeId(0),
                    kind: NodeKind::Function,
                    name: format!("src/mod_{}.rs::func_{}", i / 10, i),
                    content_hash: Some(i as u64),
                    last_extracted: now,
                    metadata: HashMap::new(),
                })
                .await
                .unwrap();
        }
        store
    });

    c.bench_function("store_lookup_by_name", |b| {
        b.iter(|| {
            rt.block_on(async {
                store
                    .get_node_by_name(NodeKind::Function, "src/mod_50.rs::func_500")
                    .await
                    .unwrap();
            });
        });
    });
}

fn bench_analysis_storage(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("store_analysis_roundtrip", |b| {
        b.iter(|| {
            rt.block_on(async {
                let store = SqliteStore::in_memory().unwrap();
                let now = Utc::now();

                let node_id = store
                    .upsert_node(&Node {
                        id: NodeId(0),
                        kind: NodeKind::File,
                        name: "src/main.rs".to_string(),
                        content_hash: None,
                        last_extracted: now,
                        metadata: HashMap::new(),
                    })
                    .await
                    .unwrap();

                store
                    .store_analysis(&AnalysisResult {
                        id: AnalysisResultId(0),
                        node_id,
                        kind: AnalysisKind::ChangeFrequency,
                        data: serde_json::json!({
                            "total": 42,
                            "last_30d": 5,
                            "last_90d": 15,
                            "percentile": 0.85,
                        }),
                        input_hash: 12345,
                        computed_at: now,
                    })
                    .await
                    .unwrap();

                store
                    .get_analysis(node_id, AnalysisKind::ChangeFrequency)
                    .await
                    .unwrap();
            });
        });
    });
}

criterion_group!(
    benches,
    bench_bulk_insert_nodes,
    bench_node_lookup,
    bench_analysis_storage,
);
criterion_main!(benches);
