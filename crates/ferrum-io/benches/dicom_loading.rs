//! End-to-end DICOM loading throughput on a synthetic 128-slice 256×256
//! series written to a temporary directory.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

#[path = "../tests/common/mod.rs"]
mod common;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use ferrum_domain::{NoProgress, VolumeRepository};
use ferrum_io::DicomRepository;

fn bench(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("tempdir");
    let (rows, cols, n) = (256u16, 256u16, 128u32);
    common::write_series(dir.path(), rows, cols, n, 1.0, |_| {});
    let paths = vec![dir.path().to_path_buf()];
    let repo = DicomRepository;
    let series = repo.scan(&paths, &NoProgress).expect("scan");

    let mut g = c.benchmark_group("dicom_256x256x128");
    g.sample_size(10);
    g.throughput(Throughput::Elements(u64::from(rows) * u64::from(cols) * u64::from(n)));
    g.bench_function("scan_headers", |b| b.iter(|| repo.scan(&paths, &NoProgress).expect("scan")));
    g.bench_function("load_volume", |b| b.iter(|| repo.load(&series[0], &NoProgress).expect("load")));
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
