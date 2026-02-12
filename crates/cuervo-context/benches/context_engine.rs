//! Benchmarks for the context engine hot paths.
//!
//! Run: cargo bench -p cuervo-context

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

use chrono::Utc;
use cuervo_context::assembler::estimate_tokens;
use cuervo_context::cold_store::ColdStore;
use cuervo_context::compression::{compress, decompress, delta_decode, delta_encode};
use cuervo_context::pipeline::{ContextPipeline, ContextPipelineConfig};
use cuervo_context::segment::{extract_segment_from_message, ContextSegment};
use cuervo_context::cold_archive::ColdArchive;
use cuervo_context::repo_map::{self, RepoMap};
use cuervo_context::semantic_store::SemanticStore;
use cuervo_context::sliding_window::SlidingWindow;
use cuervo_core::types::{ChatMessage, MessageContent, Role};

fn make_segment(start: u32, end: u32, summary: &str) -> ContextSegment {
    ContextSegment {
        round_start: start,
        round_end: end,
        summary: summary.to_string(),
        decisions: vec![],
        files_modified: vec![],
        tools_used: vec![],
        token_estimate: estimate_tokens(summary) as u32,
        created_at: Utc::now(),
    }
}

fn text_msg(role: Role, text: &str) -> ChatMessage {
    ChatMessage {
        role,
        content: MessageContent::Text(text.to_string()),
    }
}

fn repetitive_text(n: usize) -> String {
    "Discussing Rust async patterns, error handling with thiserror, and tokio runtime. "
        .repeat(n)
}

// --- Compression benchmarks ---

fn bench_compress(c: &mut Criterion) {
    let mut group = c.benchmark_group("compression");

    for size in [1, 5, 20, 100] {
        let text = repetitive_text(size);
        group.bench_with_input(
            BenchmarkId::new("zstd_compress", format!("{}_repeats", size)),
            &text,
            |b, text| b.iter(|| compress(black_box(text))),
        );
    }

    // Decompress benchmark (needs pre-compressed block).
    let text = repetitive_text(20);
    let block = compress(&text).unwrap();
    group.bench_function("zstd_decompress_20_repeats", |b| {
        b.iter(|| decompress(black_box(&block)))
    });

    group.finish();
}

fn bench_delta(c: &mut Criterion) {
    let mut group = c.benchmark_group("delta_encoding");

    let base = repetitive_text(20);
    let target_similar = format!("{} Additional insight about error handling.", &base[..base.len() - 1]);
    let target_different = "Completely different content about web development with React and Next.js. ".repeat(20);

    group.bench_function("encode_similar", |b| {
        b.iter(|| delta_encode(black_box(&base), black_box(&target_similar)))
    });

    group.bench_function("encode_different", |b| {
        b.iter(|| delta_encode(black_box(&base), black_box(&target_different)))
    });

    let delta = delta_encode(&base, &target_similar);
    group.bench_function("decode_similar", |b| {
        b.iter(|| delta_decode(black_box(&base), black_box(&delta)))
    });

    group.finish();
}

// --- ColdStore benchmarks ---

fn bench_cold_store(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_store");

    // Store benchmark.
    let large_summary = repetitive_text(10);
    let segment = make_segment(1, 5, &large_summary);
    group.bench_function("store_single", |b| {
        let mut store = ColdStore::new(1000);
        b.iter(|| store.store(black_box(&segment)))
    });

    // Store + retrieve cycle.
    group.bench_function("store_retrieve_100", |b| {
        b.iter(|| {
            let mut store = ColdStore::new(200);
            for i in 0..100 {
                let seg = make_segment(i, i + 1, &repetitive_text(5));
                store.store(&seg);
            }
            store.retrieve(black_box(50_000))
        })
    });

    // Retrieve from populated store.
    let mut store = ColdStore::new(200);
    for i in 0..100 {
        store.store(&make_segment(i, i + 1, &repetitive_text(5)));
    }
    group.bench_function("retrieve_100_entries", |b| {
        b.iter(|| store.retrieve(black_box(50_000)))
    });

    group.finish();
}

// --- Pipeline benchmarks ---

fn bench_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline");

    for msg_count in [10, 100, 500] {
        group.bench_with_input(
            BenchmarkId::new("add_message", msg_count),
            &msg_count,
            |b, &count| {
                b.iter(|| {
                    let config = ContextPipelineConfig {
                        max_context_tokens: 50_000,
                        hot_buffer_capacity: 8,
                        default_tool_output_budget: 500,
                        l1_merge_threshold: 1000,
                        max_cold_entries: 100,
                        ..Default::default()
                    };
                    let mut pipeline = ContextPipeline::new(&config);
                    for i in 0..count {
                        pipeline.add_message(text_msg(
                            if i % 2 == 0 { Role::User } else { Role::Assistant },
                            &format!(
                                "Message {i}: discussing Rust async patterns and error handling \
                                 with thiserror for library code and anyhow for applications"
                            ),
                        ));
                    }
                })
            },
        );
    }

    // Build messages from populated pipeline.
    let config = ContextPipelineConfig {
        max_context_tokens: 10_000,
        hot_buffer_capacity: 8,
        default_tool_output_budget: 500,
        l1_merge_threshold: 500,
        max_cold_entries: 50,
        ..Default::default()
    };
    let mut pipeline = ContextPipeline::new(&config);
    for i in 0..200 {
        pipeline.add_message(text_msg(
            if i % 2 == 0 { Role::User } else { Role::Assistant },
            &format!("Message {i}: detailed content about code patterns"),
        ));
    }
    group.bench_function("build_messages_200", |b| {
        b.iter(|| pipeline.build_messages())
    });

    // Build messages with L4 archive loaded (cross-session scenario).
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("bench_l4.bin");
    {
        let mut setup = ColdArchive::with_path(500, path.clone());
        for i in 0..50 {
            setup.store(&make_segment(
                i as u32,
                i as u32 + 1,
                &format!("Archived segment {i}: Rust async patterns and error handling discussion"),
            ));
        }
        setup.flush_to_disk();
    }
    let mut pipeline_with_l4 = ContextPipeline::new(&config);
    pipeline_with_l4.load_l4_archive(&path);
    for i in 0..50 {
        pipeline_with_l4.add_message(text_msg(
            if i % 2 == 0 { Role::User } else { Role::Assistant },
            &format!("Message {i}: current session content about code patterns"),
        ));
    }
    group.bench_function("build_messages_50_with_l4", |b| {
        b.iter(|| pipeline_with_l4.build_messages())
    });

    // Instruction cache refresh benchmark (mtime check, no change).
    let instr_dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        instr_dir.path().join("CUERVO.md"),
        "# Project Instructions\nUse Rust. Follow SOTA patterns. Write comprehensive tests.",
    )
    .unwrap();
    let mut refresh_pipeline = ContextPipeline::new(&config);
    refresh_pipeline.initialize("system prompt", instr_dir.path());
    group.bench_function("refresh_instructions_no_change", |b| {
        b.iter(|| black_box(refresh_pipeline.refresh_instructions(instr_dir.path())))
    });

    group.finish();
}

// --- Segment extraction benchmarks ---

fn bench_segment_extraction(c: &mut Criterion) {
    let mut group = c.benchmark_group("segment_extraction");

    let text_msg_content = "We decided to use Rust for the project. \
        Modified src/main.rs and tests/lib.rs to add the new module. \
        Also updated Cargo.toml with dependencies. \
        We chose SQLite for storage after evaluating options.";

    let msg = ChatMessage {
        role: Role::User,
        content: MessageContent::Text(text_msg_content.to_string()),
    };
    group.bench_function("extract_from_text_message", |b| {
        b.iter(|| extract_segment_from_message(black_box(&msg), 5))
    });

    let blocks_msg = ChatMessage {
        role: Role::Assistant,
        content: MessageContent::Blocks(vec![
            cuervo_core::types::ContentBlock::Text {
                text: "Let me run the tests and check the results.".to_string(),
            },
            cuervo_core::types::ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "cargo test --workspace"}),
            },
        ]),
    };
    group.bench_function("extract_from_blocks_message", |b| {
        b.iter(|| extract_segment_from_message(black_box(&blocks_msg), 10))
    });

    group.finish();
}

// --- Sliding window benchmarks ---

fn bench_sliding_window(c: &mut Criterion) {
    let mut group = c.benchmark_group("sliding_window");

    // Merge adjacent.
    group.bench_function("merge_adjacent_50_segments", |b| {
        b.iter(|| {
            let mut window = SlidingWindow::new();
            for i in 0..50 {
                window.push(make_segment(i, i, &format!("Segment {i} summary content")));
            }
            window.merge_adjacent(black_box(500))
        })
    });

    // Retrieve.
    let mut window = SlidingWindow::new();
    for i in 0..50 {
        window.push(make_segment(i, i, &format!("Segment {i} with detailed context")));
    }
    group.bench_function("retrieve_50_segments", |b| {
        b.iter(|| window.retrieve(black_box(50_000)))
    });

    group.finish();
}

// --- Token estimation ---

fn bench_token_estimation(c: &mut Criterion) {
    let mut group = c.benchmark_group("token_estimation");

    for size in [100, 1000, 10_000] {
        let text = "a".repeat(size);
        group.bench_with_input(
            BenchmarkId::new("estimate_tokens", size),
            &text,
            |b, text| b.iter(|| estimate_tokens(black_box(text))),
        );
    }

    group.finish();
}

// --- Semantic store benchmarks ---

fn bench_semantic_store(c: &mut Criterion) {
    let mut group = c.benchmark_group("semantic_store");

    // Store benchmark.
    let segment = make_segment(1, 5, &repetitive_text(10));
    group.bench_function("store_single", |b| {
        let mut store = SemanticStore::new(1000);
        b.iter(|| store.store(black_box(&segment)))
    });

    // Populate store for retrieval benchmarks.
    let topics = [
        "Rust async patterns with tokio runtime and error handling strategies",
        "Python Flask web server with REST API endpoints and JSON serialization",
        "SQLite database configuration with WAL mode and connection pooling",
        "React frontend components with TypeScript and state management",
        "Docker container orchestration with Kubernetes and service mesh",
        "Machine learning model training with PyTorch and GPU acceleration",
        "GraphQL API design with schema stitching and federation patterns",
        "WebAssembly compilation targets with wasm-bindgen and web-sys",
        "CI/CD pipeline configuration with GitHub Actions and deployment",
        "Cryptographic protocols with TLS certificates and key management",
    ];

    // Retrieve benchmark (100 entries, varied topics).
    let mut store = SemanticStore::new(200);
    for i in 0..100 {
        let topic = topics[i % topics.len()];
        store.store(&make_segment(
            i as u32,
            i as u32 + 1,
            &format!("Segment {i}: {topic}. Additional detail about implementation."),
        ));
    }
    group.bench_function("retrieve_100_entries", |b| {
        b.iter(|| store.retrieve(black_box("Rust async tokio error handling"), black_box(50_000)))
    });

    // Retrieve with tight budget.
    group.bench_function("retrieve_100_tight_budget", |b| {
        b.iter(|| store.retrieve(black_box("Rust async tokio"), black_box(100)))
    });

    // Store + retrieve cycle.
    group.bench_function("store_retrieve_cycle_50", |b| {
        b.iter(|| {
            let mut store = SemanticStore::new(100);
            for i in 0..50 {
                let topic = topics[i % topics.len()];
                store.store(&make_segment(i as u32, i as u32, topic));
            }
            store.retrieve("Rust async patterns", 10_000)
        })
    });

    group.finish();
}

// --- Cold archive benchmarks ---

fn bench_cold_archive(c: &mut Criterion) {
    let mut group = c.benchmark_group("cold_archive");

    let segment = make_segment(1, 5, &repetitive_text(10));
    group.bench_function("store_single", |b| {
        let mut archive = ColdArchive::new(1000);
        b.iter(|| archive.store(black_box(&segment)))
    });

    // Populate for retrieval.
    let mut archive = ColdArchive::new(200);
    for i in 0..100 {
        archive.store(&make_segment(
            i as u32,
            i as u32 + 1,
            &format!("Segment {i}: discussing Rust async patterns and error handling"),
        ));
    }

    group.bench_function("retrieve_100_with_query", |b| {
        b.iter(|| archive.retrieve(Some(black_box("Rust async")), black_box(50_000)))
    });

    group.bench_function("retrieve_100_no_query", |b| {
        b.iter(|| archive.retrieve(black_box(None), black_box(50_000)))
    });

    // Serialize/deserialize.
    group.bench_function("serialize_100", |b| {
        b.iter(|| archive.serialize())
    });

    let data = archive.serialize();
    group.bench_function("deserialize_100", |b| {
        b.iter(|| ColdArchive::deserialize(black_box(&data), 200))
    });

    // Flush/load cycle to disk.
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("bench_archive.bin");
    let mut disk_archive = ColdArchive::with_path(200, path.clone());
    for i in 0..100 {
        disk_archive.store(&make_segment(
            i as u32,
            i as u32 + 1,
            &format!("Segment {i}: benchmark content for disk persistence"),
        ));
    }
    disk_archive.flush_to_disk();

    group.bench_function("flush_to_disk_100", |b| {
        b.iter(|| disk_archive.flush_to_disk())
    });

    group.bench_function("load_from_disk_100", |b| {
        b.iter(|| ColdArchive::load_from_disk(black_box(&path), 200))
    });

    group.finish();
}

// --- Repo map benchmarks ---

fn bench_repo_map(c: &mut Criterion) {
    let mut group = c.benchmark_group("repo_map");

    // Generate synthetic source files.
    let rust_code = r#"pub struct Config {
    pub name: String,
    pub timeout: u64,
}

pub enum Mode {
    Fast,
    Slow,
}

pub trait Handler {
    fn handle(&self, req: &str) -> String;
}

impl Handler for Config {
    fn handle(&self, req: &str) -> String {
        format!("{}: {}", self.name, req)
    }
}

pub fn create_config(name: &str) -> Config {
    Config { name: name.to_string(), timeout: 30 }
}

pub async fn fetch_data(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(format!("data from {}", url))
}

pub mod utils;
pub mod helpers;
"#;

    // Extract symbols from a single file.
    group.bench_function("extract_symbols_rust", |b| {
        b.iter(|| repo_map::extract_symbols(black_box(rust_code), "lib.rs", "rs"))
    });

    // Build repo map from 50 files.
    let files: Vec<(String, String)> = (0..50)
        .map(|i| {
            (
                format!("src/mod_{}.rs", i),
                format!(
                    "pub struct Type{i} {{}}\n\
                     pub fn func_{i}(x: &str) -> String {{ x.to_string() }}\n\
                     impl Type{i} {{\n\
                         pub fn method_{i}(&self) -> u32 {{ {i} }}\n\
                     }}\n"
                ),
            )
        })
        .collect();
    let file_refs: Vec<(&str, &str)> = files.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();

    group.bench_function("build_map_50_files", |b| {
        b.iter(|| RepoMap::build(black_box("/project"), black_box(&file_refs)))
    });

    // Render with budget.
    let map = RepoMap::build("/project", &file_refs);
    group.bench_function("render_50_files_10k_budget", |b| {
        b.iter(|| map.render(black_box(10_000)))
    });

    // Search.
    group.bench_function("search_50_files", |b| {
        b.iter(|| map.search(black_box("func_25")))
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_compress,
    bench_delta,
    bench_cold_store,
    bench_pipeline,
    bench_segment_extraction,
    bench_sliding_window,
    bench_token_estimation,
    bench_semantic_store,
    bench_cold_archive,
    bench_repo_map,
);
criterion_main!(benches);
