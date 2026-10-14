//! Whole Hounsfield units are read back exactly: what `probe`, a threshold
//! at a whole unit and `segment metrics` report is what the file says
//! (Stage 18 field test: a voxel of 55 HU was read as 55.01, a minimum of
//! -25 as -24.9866).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use ferrum_agent::Agent;
use ferrum_domain::{Dims3, Volume};
use glam::Vec3;
use serde_json::{json, Value};

const DIMS: (usize, usize, usize) = (20, 20, 12);

/// Whole-unit value of the block voxel (i, j, k).
fn block_value(i: usize, j: usize, k: usize) -> f32 {
    ((i * 7 + j * 3 + k * 5) % 101) as f32 + 40.0
}

fn in_block(i: usize, j: usize, k: usize) -> bool {
    (5..15).contains(&i) && (5..15).contains(&j) && (3..9).contains(&k)
}

/// A CT-like image: air at -1000 HU, a block of tissue of 40 to 140 HU, and one 1500 HU voxel in a corner so
/// that the range of the data is as wide as in a real study.
fn write_ct(path: &std::path::Path) {
    let mut values = vec![-1000.0f32; DIMS.0 * DIMS.1 * DIMS.2];
    for k in 0..DIMS.2 {
        for j in 0..DIMS.1 {
            for i in 0..DIMS.0 {
                if in_block(i, j, k) {
                    values[i + DIMS.0 * (j + DIMS.1 * k)] = block_value(i, j, k);
                }
            }
        }
    }
    values[0] = 1500.0;
    let dims = Dims3::new(DIMS.0 as u32, DIMS.1 as u32, DIMS.2 as u32);
    ferrum_io::write_nifti(&Volume::from_physical(dims, Vec3::new(0.8, 0.8, 2.5), &values).unwrap(), path).unwrap();
}

struct Fixture {
    _dir: tempfile::TempDir,
    ws: String,
    agent: Agent,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let nifti = dir.path().join("ct.nii.gz");
        write_ct(&nifti);
        let ws = dir.path().join("ws/ct").to_string_lossy().into_owned();
        let f = Self { _dir: dir, ws, agent: Agent::default() };
        f.ok("study open", json!({ "path": nifti, "modality": "CT" }));
        f
    }

    fn ok(&self, command: &str, extra: Value) -> Value {
        let mut p = json!({ "workspace": self.ws });
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let e = self.agent.run(command, &p);
        assert_eq!(e["ok"], true, "{command}: {e:#}");
        e["data"].clone()
    }
}

// covers 18.13-d
#[test]
fn probe_returns_the_whole_unit_the_file_holds() {
    let f = Fixture::new();
    for (i, j, k) in [(5, 5, 3), (7, 6, 4), (14, 14, 8), (9, 11, 5)] {
        let probe = f.ok("probe", json!({ "point": { "voxel": [i, j, k] } }));
        assert_eq!(probe["value"].as_f64(), Some(f64::from(block_value(i, j, k))), "voxel ({i},{j},{k})");
    }
    let air = f.ok("probe", json!({ "point": { "voxel": [19, 19, 11] } }));
    assert_eq!(air["value"].as_f64(), Some(-1000.0));
    let bright = f.ok("probe", json!({ "point": { "voxel": [0, 0, 0] } }));
    assert_eq!(bright["value"].as_f64(), Some(1500.0));
}

// covers 18.13-e
#[test]
fn a_threshold_at_a_whole_unit_includes_the_voxels_at_exactly_that_unit() {
    let f = Fixture::new();
    // the voxel (5, 5, 3) holds `block_value`; a range of exactly that unit must accept the seed
    let seed_value = block_value(5, 5, 3);
    let made = f.ok(
        "segment threshold",
        json!({ "seed": { "voxel": [5, 5, 3] }, "min": seed_value, "max": seed_value, "name": "One unit" }),
    );
    let one = made["segment"]["voxels"].as_u64().unwrap();
    assert!(one >= 1, "{made:#}");
    // ... a range from a whole unit up: the seed sits exactly on the lower edge
    let edge_value = block_value(14, 14, 8);
    let edge = f.ok(
        "segment threshold",
        json!({ "seed": { "voxel": [14, 14, 8] }, "min": edge_value, "max": 1e6, "name": "Edge" }),
    );
    assert!(edge["segment"]["voxels"].as_u64().unwrap() >= 1, "{edge:#}");
    // ... and a range between two whole units counts at most the voxels of the block that lie in it
    let (lo, hi) = (block_value(7, 6, 4) - 30.0, block_value(7, 6, 4) + 30.0);
    let want = (0..DIMS.2)
        .flat_map(|k| (0..DIMS.1).flat_map(move |j| (0..DIMS.0).map(move |i| (i, j, k))))
        .filter(|&(i, j, k)| in_block(i, j, k) && (lo..=hi).contains(&block_value(i, j, k)))
        .count();
    let all =
        f.ok("segment threshold", json!({ "seed": { "voxel": [7, 6, 4] }, "min": lo, "max": hi, "name": "Range" }));
    // the region is 6-connected and the others took their voxels: never more than `want`
    let got = all["segment"]["voxels"].as_u64().unwrap() as usize;
    assert!(got >= 1 && got <= want, "{got} of {want}");
}

// covers 18.13-d
#[test]
fn metrics_of_the_block_are_the_numbers_of_the_file() {
    let f = Fixture::new();
    let made =
        f.ok("segment threshold", json!({ "seed": { "voxel": [5, 5, 3] }, "min": 39, "max": 141, "name": "Block" }));
    let label = made["segment"]["label"].as_u64().unwrap();
    let m = f.ok("segment metrics", json!({ "segment": label, "profile": "clinical" }));
    let values: Vec<f64> = (0..DIMS.2)
        .flat_map(|k| (0..DIMS.1).flat_map(move |j| (0..DIMS.0).map(move |i| (i, j, k))))
        .filter(|&(i, j, k)| in_block(i, j, k))
        .map(|(i, j, k)| f64::from(block_value(i, j, k)))
        .collect();
    let feature =
        |id: &str| m["features"][id]["value"].as_f64().unwrap_or_else(|| panic!("{id}: {:#}", m["features"][id]));
    assert_eq!(feature("size.voxels"), values.len() as f64);
    // whole units: the minimum, maximum and the sum of squares are not off by a fraction of a unit
    assert_eq!(feature("intensity.min"), values.iter().copied().fold(f64::MAX, f64::min));
    assert_eq!(feature("intensity.max"), values.iter().copied().fold(f64::MIN, f64::max));
    let energy: f64 = values.iter().map(|v| v * v).sum();
    assert!(
        (feature("intensity.energy") - energy).abs() <= energy * 1e-6,
        "{} vs {energy}",
        feature("intensity.energy")
    );
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    assert!((feature("intensity.mean") - mean).abs() <= mean * 1e-6);
    // the median of whole units is a whole unit
    assert_eq!(feature("intensity.p50").fract(), 0.0);
}
