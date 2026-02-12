//! Benchmarks for file intelligence operations.

use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn setup_temp_files() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().unwrap();

    // Text file
    std::fs::write(
        dir.path().join("code.rs"),
        "fn main() {\n    println!(\"hello\");\n}\n".repeat(100),
    )
    .unwrap();

    // JSON file
    let json_arr: Vec<serde_json::Value> = (0..200)
        .map(|i| {
            serde_json::json!({
                "id": i,
                "name": format!("item_{i}"),
                "value": i as f64 * 1.5,
                "active": i % 2 == 0,
            })
        })
        .collect();
    std::fs::write(
        dir.path().join("data.json"),
        serde_json::to_string_pretty(&json_arr).unwrap(),
    )
    .unwrap();

    // Plain text
    std::fs::write(dir.path().join("log.txt"), "a]".repeat(5000)).unwrap();

    // YAML
    std::fs::write(
        dir.path().join("config.yaml"),
        "database:\n  host: localhost\n  port: 5432\n  name: mydb\nserver:\n  port: 8080\n  workers: 4\n",
    )
    .unwrap();

    // CSV
    let mut csv_content = String::from("id,name,score,grade\n");
    for i in 0..500 {
        csv_content.push_str(&format!("{i},student_{i},{:.1},{}\n", i as f64 * 0.2, if i % 2 == 0 { "A" } else { "B" }));
    }
    std::fs::write(dir.path().join("grades.csv"), &csv_content).unwrap();

    // Markdown
    let mut md = String::new();
    for i in 0..20 {
        md.push_str(&format!("## Section {i}\n\nSome content for section {i}.\n\n"));
        md.push_str(&format!("[Link {i}](https://example.com/{i})\n\n"));
        md.push_str(&format!("```rust\nfn section_{i}() {{}}\n```\n\n"));
    }
    std::fs::write(dir.path().join("doc.md"), &md).unwrap();

    // XML
    let mut xml = String::from("<?xml version=\"1.0\"?>\n<catalog>\n");
    for i in 0..100 {
        xml.push_str(&format!(
            "  <item id=\"{i}\"><name>Item {i}</name><price>{:.2}</price></item>\n",
            i as f64 * 9.99
        ));
    }
    xml.push_str("</catalog>\n");
    std::fs::write(dir.path().join("catalog.xml"), &xml).unwrap();

    dir
}

fn bench_detect(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = setup_temp_files();

    let mut group = c.benchmark_group("detect");

    for (name, file) in [
        ("rust_source", "code.rs"),
        ("json", "data.json"),
        ("plain_text", "log.txt"),
        ("yaml", "config.yaml"),
        ("csv", "grades.csv"),
        ("markdown", "doc.md"),
        ("xml", "catalog.xml"),
    ] {
        let path = dir.path().join(file);
        group.bench_function(name, |b| {
            b.iter(|| {
                rt.block_on(async {
                    cuervo_files::detect::detect(black_box(&path)).await.unwrap()
                })
            })
        });
    }

    group.finish();
}

fn bench_inspect(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = setup_temp_files();
    let inspector = cuervo_files::FileInspector::new();

    let mut group = c.benchmark_group("inspect");

    for (name, file, budget) in [
        ("rust_source_2k", "code.rs", 2000),
        ("json_2k", "data.json", 2000),
        ("plain_text_2k", "log.txt", 2000),
        ("json_500", "data.json", 500),
        ("plain_text_100", "log.txt", 100),
    ] {
        let path = dir.path().join(file);
        group.bench_function(name, |b| {
            b.iter(|| {
                rt.block_on(async {
                    inspector
                        .inspect(black_box(&path), black_box(budget))
                        .await
                        .unwrap()
                })
            })
        });
    }

    // Feature-gated benchmarks
    #[cfg(feature = "csv")]
    {
        let path = dir.path().join("grades.csv");
        group.bench_function("csv_2k", |b| {
            b.iter(|| {
                rt.block_on(async {
                    inspector.inspect(black_box(&path), black_box(2000)).await.unwrap()
                })
            })
        });
    }

    #[cfg(feature = "markdown")]
    {
        let path = dir.path().join("doc.md");
        group.bench_function("markdown_2k", |b| {
            b.iter(|| {
                rt.block_on(async {
                    inspector.inspect(black_box(&path), black_box(2000)).await.unwrap()
                })
            })
        });
    }

    #[cfg(feature = "xml")]
    {
        let path = dir.path().join("catalog.xml");
        group.bench_function("xml_2k", |b| {
            b.iter(|| {
                rt.block_on(async {
                    inspector.inspect(black_box(&path), black_box(2000)).await.unwrap()
                })
            })
        });
    }

    group.finish();
}

fn bench_estimate_tokens(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = setup_temp_files();
    let inspector = cuervo_files::FileInspector::new();

    let mut group = c.benchmark_group("estimate_tokens");

    for (name, file) in [
        ("rust_source", "code.rs"),
        ("json", "data.json"),
        ("plain_text", "log.txt"),
    ] {
        let path = dir.path().join(file);
        group.bench_function(name, |b| {
            b.iter(|| {
                rt.block_on(async {
                    inspector.estimate_tokens(black_box(&path)).await.unwrap()
                })
            })
        });
    }

    group.finish();
}

fn bench_handler_helpers(c: &mut Criterion) {
    let mut group = c.benchmark_group("handler_helpers");

    let text_1k = "a".repeat(1000);
    let text_100k = "a".repeat(100_000);

    group.bench_function("estimate_tokens_1k", |b| {
        b.iter(|| cuervo_files::handler::estimate_tokens_from_text(black_box(&text_1k)))
    });

    group.bench_function("estimate_tokens_100k", |b| {
        b.iter(|| cuervo_files::handler::estimate_tokens_from_text(black_box(&text_100k)))
    });

    group.bench_function("truncate_no_truncation", |b| {
        b.iter(|| cuervo_files::handler::truncate_to_budget(black_box(&text_1k), black_box(1000)))
    });

    group.bench_function("truncate_with_truncation", |b| {
        b.iter(|| cuervo_files::handler::truncate_to_budget(black_box(&text_100k), black_box(100)))
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_detect,
    bench_inspect,
    bench_estimate_tokens,
    bench_handler_helpers,
);
criterion_main!(benches);
