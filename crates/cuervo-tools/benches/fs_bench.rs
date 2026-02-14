//! FsService benchmarks — measures core filesystem operations.

use std::path::PathBuf;
use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use cuervo_tools::fs_service::FsService;

fn bench_resolve_path(c: &mut Criterion) {
    let fs = FsService::new(
        vec![PathBuf::from("/tmp"), PathBuf::from("/private/tmp")],
        vec![".env".into(), "*.pem".into()],
    );

    c.bench_function("resolve_path_relative", |b| {
        b.iter(|| fs.resolve_path(black_box("src/main.rs"), black_box("/project")))
    });

    c.bench_function("resolve_path_absolute", |b| {
        b.iter(|| fs.resolve_path(black_box("/project/src/lib.rs"), black_box("/project")))
    });
}

fn bench_read_write(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let fs = Arc::new(FsService::new(vec![], vec![]));

    // Prepare test files.
    let dir = tempfile::tempdir().unwrap();
    let file_1kb = dir.path().join("1kb.txt");
    let file_100kb = dir.path().join("100kb.txt");
    let file_1mb = dir.path().join("1mb.txt");
    std::fs::write(&file_1kb, "x".repeat(1024)).unwrap();
    std::fs::write(&file_100kb, "y".repeat(100 * 1024)).unwrap();
    std::fs::write(&file_1mb, "z".repeat(1024 * 1024)).unwrap();

    c.bench_function("read_to_string_1kb", |b| {
        let fs = fs.clone();
        let p = file_1kb.clone();
        b.iter(|| rt.block_on(async { fs.read_to_string(black_box(&p)).await.unwrap() }))
    });

    c.bench_function("read_to_string_100kb", |b| {
        let fs = fs.clone();
        let p = file_100kb.clone();
        b.iter(|| rt.block_on(async { fs.read_to_string(black_box(&p)).await.unwrap() }))
    });

    c.bench_function("read_to_string_1mb", |b| {
        let fs = fs.clone();
        let p = file_1mb.clone();
        b.iter(|| rt.block_on(async { fs.read_to_string(black_box(&p)).await.unwrap() }))
    });

    // Write benchmarks.
    let content_1kb = vec![b'a'; 1024];
    let content_100kb = vec![b'b'; 100 * 1024];
    let content_1mb = vec![b'c'; 1024 * 1024];

    let write_file = dir.path().join("write_bench.txt");

    c.bench_function("atomic_write_1kb", |b| {
        let fs = fs.clone();
        let p = write_file.clone();
        let c = content_1kb.clone();
        b.iter(|| rt.block_on(async { fs.atomic_write(black_box(&p), black_box(&c)).await.unwrap() }))
    });

    c.bench_function("atomic_write_100kb", |b| {
        let fs = fs.clone();
        let p = write_file.clone();
        let c = content_100kb.clone();
        b.iter(|| rt.block_on(async { fs.atomic_write(black_box(&p), black_box(&c)).await.unwrap() }))
    });

    c.bench_function("atomic_write_1mb", |b| {
        let fs = fs.clone();
        let p = write_file.clone();
        let c = content_1mb.clone();
        b.iter(|| rt.block_on(async { fs.atomic_write(black_box(&p), black_box(&c)).await.unwrap() }))
    });
}

fn bench_read_dir(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let fs = Arc::new(FsService::new(vec![], vec![]));

    // 10 entries.
    let dir10 = tempfile::tempdir().unwrap();
    for i in 0..10 {
        std::fs::write(dir10.path().join(format!("file_{i:03}.txt")), "x").unwrap();
    }

    c.bench_function("read_dir_10_entries", |b| {
        let fs = fs.clone();
        let p = dir10.path().to_path_buf();
        b.iter(|| rt.block_on(async { fs.read_dir_async(black_box(&p)).await.unwrap() }))
    });

    // 100 entries.
    let dir100 = tempfile::tempdir().unwrap();
    for i in 0..100 {
        std::fs::write(dir100.path().join(format!("file_{i:04}.txt")), "y").unwrap();
    }

    c.bench_function("read_dir_100_entries", |b| {
        let fs = fs.clone();
        let p = dir100.path().to_path_buf();
        b.iter(|| rt.block_on(async { fs.read_dir_async(black_box(&p)).await.unwrap() }))
    });
}

fn bench_batch_read(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let fs = Arc::new(FsService::new(vec![], vec![]));

    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for i in 0..10 {
        let p = dir.path().join(format!("batch_{i}.txt"));
        std::fs::write(&p, format!("content of file {i}")).unwrap();
        paths.push(p);
    }

    c.bench_function("batch_read_10_files", |b| {
        let fs = fs.clone();
        let ps = paths.clone();
        b.iter(|| rt.block_on(async { fs.batch_read(black_box(&ps)).await }))
    });
}

fn bench_cached_read(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let fs = Arc::new(FsService::new_with_cache(vec![], vec![], 100, 512 * 1024));

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cached_bench.txt");
    std::fs::write(&file, "x".repeat(1024)).unwrap();

    // Prime the cache.
    rt.block_on(async { fs.read_to_string(&file).await.unwrap() });

    c.bench_function("cached_read_hit", |b| {
        let fs = fs.clone();
        let p = file.clone();
        b.iter(|| rt.block_on(async { fs.read_to_string(black_box(&p)).await.unwrap() }))
    });

    // Uncached reads for comparison.
    let fs_nocache = Arc::new(FsService::new(vec![], vec![]));

    c.bench_function("cached_read_miss", |b| {
        let fs = fs_nocache.clone();
        let p = file.clone();
        b.iter(|| rt.block_on(async { fs.read_to_string(black_box(&p)).await.unwrap() }))
    });
}

criterion_group!(
    benches,
    bench_resolve_path,
    bench_read_write,
    bench_read_dir,
    bench_batch_read,
    bench_cached_read,
);
criterion_main!(benches);
