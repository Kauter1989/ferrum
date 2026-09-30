#![allow(missing_docs, clippy::expect_used, clippy::unwrap_used)]
//! Throughput benchmarks of the processing algorithms on a synthetic
//! 256³ CT-like phantom.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use glam::{UVec3, Vec3};
use mri_domain::{Dims3, IntensityRange, Volume};
use mri_processing::{ambient_occlusion::AoParams, filters, resample, AmbientOcclusion, BrickGrid, Histogram};

fn phantom(n: u32) -> Volume {
    let dims = Dims3::new(n, n, n);
    let c = n as f32 * 0.5;
    let mut data = Vec::with_capacity(dims.voxel_count());
    for k in 0..n {
        for j in 0..n {
            for i in 0..n {
                let r = Vec3::new(i as f32 - c, j as f32 - c, k as f32 - c).length() / c;
                let v = if r < 0.5 {
                    50000
                } else if r < 0.9 {
                    20000
                } else {
                    0
                };
                data.push(v);
            }
        }
    }
    Volume::new(dims, Vec3::ONE, IntensityRange::new(-1024.0, 3071.0).expect("range"), data).expect("volume")
}

fn benches(c: &mut Criterion) {
    let v = phantom(256);
    let mut g = c.benchmark_group("processing_256");
    g.sample_size(10);
    g.throughput(Throughput::Elements(v.dims().voxel_count() as u64));
    g.bench_function("histogram_256_bins", |b| b.iter(|| Histogram::compute(black_box(&v), 256)));
    g.bench_function("brick_grid_8", |b| b.iter(|| BrickGrid::compute(black_box(&v), 8)));
    g.bench_function("ambient_occlusion_128", |b| {
        b.iter(|| AmbientOcclusion::compute(black_box(&v), AoParams::default()))
    });
    g.bench_function("downsample_2x", |b| b.iter(|| resample::downsample(black_box(&v), UVec3::splat(2))));
    g.finish();

    let small = phantom(96);
    let mut g = c.benchmark_group("filters_96");
    g.sample_size(10);
    g.throughput(Throughput::Elements(small.dims().voxel_count() as u64));
    g.bench_function("gaussian_sigma1", |b| b.iter(|| filters::gaussian_smooth(black_box(&small), 1.0)));
    g.bench_function("sobel", |b| b.iter(|| filters::sobel_magnitude(black_box(&small))));
    g.finish();
}

criterion_group!(processing, benches);
criterion_main!(processing);
