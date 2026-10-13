#![allow(missing_docs, clippy::expect_used, clippy::unwrap_used)]
//! Cost of the profiles on a 512 x 512 x 300 CT with a roundish segment of
//! about 100 000 voxels (the budget of Stage 18 is 2 s for `clinical`).
//! Single-threaded, as the library is. The parts are measured with profiles
//! that hold just their features.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use ferrum_domain::{Dims3, IntensityRange, MaskRegion, Volume};
use ferrum_radiomics::{compute, Input, Params, Profile, ValueUnit, CLINICAL, QUICK};
use glam::Vec3;

const DIMS: Dims3 = Dims3 { x: 512, y: 512, z: 300 };

/// A CT-like volume: soft tissue with noise (the voxel values are 16 bits,
/// -1024 to 3071 HU).
fn volume() -> Volume {
    let mut seed = 7u64;
    let data: Vec<u16> = (0..DIMS.voxel_count())
        .map(|_| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (16_000 + ((seed >> 40) % 3_000)) as u16
        })
        .collect();
    Volume::new(DIMS, Vec3::new(0.8, 0.8, 1.0), IntensityRange::new(-1024.0, 3071.0).expect("range"), data)
        .expect("volume")
}

/// A ball of radius 29 voxels, a little irregular, in the middle of the volume.
fn segment() -> MaskRegion {
    let c = Vec3::new(256.0, 256.0, 150.0);
    MaskRegion::from_grid(DIMS, |n| {
        let (i, j, k) = (n % 512, (n / 512) % 512, n / (512 * 512));
        let p = Vec3::new(i as f32, j as f32, k as f32) - c;
        p.length() < 29.0 + 2.0 * (p.x * 0.3).sin()
    })
    .expect("segment")
}

const fn only(features: &'static [&'static str]) -> Profile {
    Profile { name: "part", version: "0", summary: "a part of clinical", features, ivh_curve: false, hu_ranges: false }
}

const MESH: Profile = only(&["shape.surface_area_mm2", "shape.sphericity", "shape.major_axis_mm"]);
const HULL: Profile = only(&["shape.surface_area_mm2", "shape.solidity", "shape.convexity"]);
const DIAMETER: Profile = only(&["size.max_diameter_mm"]);
const TOPOLOGY: Profile = only(&["topology.components", "topology.holes", "topology.euler_number"]);
const DISTRIBUTION: Profile = only(&["intensity.skewness", "intensity.entropy", "intensity.ivh_i10"]);

fn benches(c: &mut Criterion) {
    let (v, r) = (volume(), segment());
    println!("segment: {} voxels", r.count());
    let input = Input { region: &r, volume: &v, unit: ValueUnit::Hu };
    let params = Params::default();
    let mut g = c.benchmark_group("profiles_512x512x300_ball_100k");
    g.sample_size(10);
    for (name, profile) in [
        ("clinical", &CLINICAL),
        ("quick", &QUICK),
        ("part_mesh_and_axes", &MESH),
        ("part_mesh_and_hull", &HULL),
        ("part_diameter", &DIAMETER),
        ("part_topology", &TOPOLOGY),
        ("part_distribution", &DISTRIBUTION),
    ] {
        g.bench_function(name, |b| b.iter(|| compute(black_box(&input), profile, &params).expect("metrics")));
    }
    g.finish();
}

criterion_group!(profiles, benches);
criterion_main!(profiles);
