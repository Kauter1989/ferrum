//! `probe`, `stats` and `measure distance|angle|area`: numbers an agent
//! should report instead of reading them off an image.

use ferrum_domain::{Annotation, SliceAxis, Volume};
use glam::{DVec3, Vec2};
use serde_json::{json, Value};

use super::Ctx;
use crate::envelope::{AgentError, Output};
use crate::params::Params;
use crate::points::{describe, nearest_voxel, resolve, voxel_to_patient, PointInput};
use crate::study::Study;

fn round(x: f64, digits: i32) -> f64 {
    let f = 10f64.powi(digits);
    (x * f).round() / f
}

/// Resolves the point in parameter `key`.
pub fn point(study: &Study, p: &Params, key: &str) -> Result<DVec3, AgentError> {
    resolve(&study.volume, &PointInput::parse(p.req(key)?)?, &study.renders_dir())
}

/// Resolves a list of points in parameter `key`.
pub fn points(study: &Study, p: &Params, key: &str) -> Result<Vec<DVec3>, AgentError> {
    let list = p.list(key)?.ok_or_else(|| AgentError::bad_request(format!("{key} is required")))?;
    list.iter().map(|v| resolve(&study.volume, &PointInput::parse(v)?, &study.renders_dir())).collect()
}

fn value_at(v: &Volume, idx: [u32; 3]) -> f32 {
    v.physical(idx[0], idx[1], idx[2]).unwrap_or(f32::NAN)
}

/// `probe`: the value of the voxel nearest to a point.
pub fn probe(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let at = point(study, p, "point")?;
    let idx = nearest_voxel(&study.volume, at);
    let value = value_at(&study.volume, idx);
    let mut data = describe(&study.volume, at);
    data["value"] = json!(round(f64::from(value), 2));
    data["unit"] = json!(study.value_unit());
    data["method"] = json!("value of the nearest voxel");
    Ok(Output::new(data))
}

/// `stats`: statistics of the voxel values in a box, sphere, segment or
/// area annotation.
pub fn stats(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let (region, values) = region_values(study, p)?;
    if values.is_empty() {
        return Err(AgentError::bad_request("the region contains no voxels"));
    }
    let s = study.volume.spacing();
    let voxel_ml = f64::from(s.x) * f64::from(s.y) * f64::from(s.z) / 1000.0;
    let mut data = summarise(values);
    data["region"] = json!(region);
    data["volume_ml"] = json!(round(data["voxels"].as_f64().unwrap_or(0.0) * voxel_ml, 3));
    data["unit"] = json!(study.value_unit());
    Ok(Output::new(data))
}

/// Count, mean, standard deviation, extremes and percentiles.
pub fn summarise(mut values: Vec<f32>) -> Value {
    let n = values.len();
    let mean = values.iter().map(|v| f64::from(*v)).sum::<f64>() / n as f64;
    let var = values.iter().map(|v| (f64::from(*v) - mean).powi(2)).sum::<f64>() / n as f64;
    values.sort_by(f32::total_cmp);
    let pct = |q: f64| f64::from(values[((n - 1) as f64 * q).round() as usize]);
    json!({
        "voxels": n,
        "mean": round(mean, 2),
        "std": round(var.sqrt(), 2),
        "min": f64::from(values[0]),
        "max": f64::from(values[n - 1]),
        "percentiles": { "p5": pct(0.05), "p25": pct(0.25), "p50": pct(0.5), "p75": pct(0.75), "p95": pct(0.95) },
    })
}

fn region_values(study: &Study, p: &Params) -> Result<(&'static str, Vec<f32>), AgentError> {
    let v = &study.volume;
    if let Some(b) = p.get("box") {
        let bp = Params::new(b)?;
        let (a, z) = (nearest_voxel(v, point(study, &bp, "min")?), nearest_voxel(v, point(study, &bp, "max")?));
        let (lo, hi) =
            ((0..3).map(|i| a[i].min(z[i])).collect::<Vec<_>>(), (0..3).map(|i| a[i].max(z[i])).collect::<Vec<_>>());
        let mut out = Vec::new();
        for k in lo[2]..=hi[2] {
            for j in lo[1]..=hi[1] {
                for i in lo[0]..=hi[0] {
                    out.push(value_at(v, [i, j, k]));
                }
            }
        }
        return Ok(("box", out));
    }
    if let Some(s) = p.get("sphere") {
        let sp = Params::new(s)?;
        let c = voxel_to_patient(v, point(study, &sp, "center")?);
        let r = sp.req_f64("radius_mm")?;
        if r <= 0.0 {
            return Err(AgentError::bad_request("radius_mm must be positive"));
        }
        return Ok(("sphere", sphere_values(v, c, r)));
    }
    if let Some(label) = p.u64("segment")? {
        let label = u8::try_from(label).ok().filter(|l| study.segments.segment(*l).is_some());
        let label = label.ok_or_else(|| AgentError::not_found("unknown segment").hint("see segment list"))?;
        let values = study.segments.labels().data().iter().zip(v.data()).filter(|(l, _)| **l == label);
        return Ok(("segment", values.map(|(_, s)| v.range().from_storage(*s)).collect()));
    }
    if let Some(id) = p.u64("annotation")? {
        return Ok(("annotation", annotation_values(study, id)?));
    }
    Err(AgentError::bad_request("give one region: box, sphere, segment or annotation"))
}

fn sphere_values(v: &Volume, centre_mm: DVec3, r: f64) -> Vec<f32> {
    // the direction matrix is orthonormal, so |Δvoxel_a| · spacing_a ≤ r on every axis
    let c = crate::points::patient_to_voxel(v, centre_mm);
    let s = v.spacing().as_dvec3();
    let d = v.dims().as_uvec3();
    let range = |a: usize| {
        let lo = (c[a] - r / s[a]).floor().max(0.0) as u32;
        let hi = ((c[a] + r / s[a]).ceil().max(0.0) as u32).min(d[a] - 1);
        lo..=hi
    };
    let mut out = Vec::new();
    for k in range(2) {
        for j in range(1) {
            for i in range(0) {
                let mm = voxel_to_patient(v, DVec3::new(f64::from(i), f64::from(j), f64::from(k)));
                if mm.distance(centre_mm) <= r {
                    out.push(value_at(v, [i, j, k]));
                }
            }
        }
    }
    out
}

/// Values of the voxels whose centres lie inside an area or rectangle
/// annotation on its slice.
fn annotation_values(study: &Study, id: u64) -> Result<Vec<f32>, AgentError> {
    let a = study.annotations.get(id).ok_or_else(|| AgentError::not_found(format!("unknown annotation {id}")))?;
    let key =
        study.annotations.slice_of(id).ok_or_else(|| AgentError::not_found(format!("unknown annotation {id}")))?;
    let plane = key.slice_axis().ok_or_else(|| AgentError::internal("annotation without a plane"))?;
    let polygon: Vec<Vec2> = match a {
        Annotation::Polygon { points } => points.clone(),
        Annotation::Rect { a, b } => vec![*a, Vec2::new(b.x, a.y), *b, Vec2::new(a.x, b.y)],
        _ => return Err(AgentError::bad_request("only area and rectangle annotations enclose a region")),
    };
    Ok(pixels_inside(&study.volume, plane, key.index, &polygon))
}

fn pixels_inside(v: &Volume, plane: SliceAxis, index: u32, polygon: &[Vec2]) -> Vec<f32> {
    let (w, h) = plane.plane_dims(v);
    let sp = plane.plane_spacing(v);
    let mut out = Vec::new();
    for py in 0..h {
        for px in 0..w {
            let mm = (Vec2::new(px as f32, py as f32) + Vec2::splat(0.5)) * sp;
            if inside(polygon, mm) {
                if let Some(vox) = plane.voxel_at(v, index, px, py) {
                    out.push(value_at(v, [vox.x, vox.y, vox.z]));
                }
            }
        }
    }
    out
}

/// Even-odd point-in-polygon test.
fn inside(poly: &[Vec2], p: Vec2) -> bool {
    let mut c = false;
    for (i, a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            c = !c;
        }
    }
    c
}

fn measured(study: &Study, pts: &[DVec3], value: f64, unit: &str, method: &str) -> Value {
    json!({
        "value": round(value, 2),
        "unit": unit,
        "method": method,
        "points": pts.iter().map(|q| describe(&study.volume, *q)).collect::<Vec<_>>(),
    })
}

fn exactly(pts: &[DVec3], n: usize, what: &str) -> Result<(), AgentError> {
    if pts.len() == n {
        Ok(())
    } else {
        Err(AgentError::bad_request(format!("{what} needs {n} points, got {}", pts.len())))
    }
}

/// `measure distance`: between two points, in mm.
pub fn measure_distance(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let pts = points(study, p, "points")?;
    exactly(&pts, 2, "a distance")?;
    let mm: Vec<DVec3> = pts.iter().map(|q| voxel_to_patient(&study.volume, *q)).collect();
    let mut data = measured(study, &pts, mm[0].distance(mm[1]), "mm", "straight line between the two points");
    let u = f64::from(study.volume.spacing().max_element());
    data["uncertainty_mm"] = json!(round(u, 3));
    data["uncertainty"] = json!("± one voxel spacing (largest axis)");
    Ok(Output::new(data))
}

/// `measure angle`: at the middle of three points, in degrees.
pub fn measure_angle(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let pts = points(study, p, "points")?;
    exactly(&pts, 3, "an angle")?;
    let mm: Vec<DVec3> = pts.iter().map(|q| voxel_to_patient(&study.volume, *q)).collect();
    let (u, w) = (mm[0] - mm[1], mm[2] - mm[1]);
    if u.length() == 0.0 || w.length() == 0.0 {
        return Err(AgentError::bad_request("the arms of the angle must have length"));
    }
    let deg = (u.dot(w) / (u.length() * w.length())).clamp(-1.0, 1.0).acos().to_degrees();
    Ok(Output::new(measured(study, &pts, deg, "deg", "angle at the second point")))
}

/// `measure area`: of a planar polygon, in mm².
pub fn measure_area(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let pts = points(study, p, "points")?;
    if pts.len() < 3 {
        return Err(AgentError::bad_request(format!("an area needs at least 3 points, got {}", pts.len())));
    }
    let mm: Vec<DVec3> = pts.iter().map(|q| voxel_to_patient(&study.volume, *q)).collect();
    // Newell's method: half the length of the summed cross products
    let n = mm.iter().enumerate().fold(DVec3::ZERO, |acc, (i, a)| acc + a.cross(mm[(i + 1) % mm.len()]));
    Ok(Output::new(measured(study, &pts, n.length() / 2.0, "mm2", "area of the polygon through the points")))
}

/// Most samples of a profile.
pub const MAX_SAMPLES: u64 = 2000;

/// `profile`: values at evenly spaced points along a line (nearest voxel).
pub fn profile(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let (a, b) = (point(study, p, "from")?, point(study, p, "to")?);
    let v = &study.volume;
    let (ma, mb) = (voxel_to_patient(v, a), voxel_to_patient(v, b));
    let length = ma.distance(mb);
    let step = f64::from(v.spacing().min_element());
    let n = match p.u64("samples")? {
        Some(n) if !(2..=MAX_SAMPLES).contains(&n) => {
            return Err(AgentError::bad_request(format!("samples must be in 2..={MAX_SAMPLES}")))
        }
        Some(n) => n,
        None => ((length / step).ceil() as u64 + 1).clamp(2, MAX_SAMPLES),
    };
    let mut values = Vec::with_capacity(n as usize);
    let samples: Vec<Value> = (0..n)
        .map(|i| {
            let t = i as f64 / (n - 1) as f64;
            let q = a + (b - a) * t;
            let value = value_at(v, nearest_voxel(v, q));
            values.push(value);
            let mut s = describe(v, q);
            s["distance_mm"] = json!(round(length * t, 3));
            s["value"] = json!(round(f64::from(value), 2));
            s
        })
        .collect();
    let min = values.iter().copied().fold(f32::INFINITY, f32::min);
    let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    Ok(Output::new(json!({
        "length_mm": round(length, 3),
        "unit": study.value_unit(),
        "method": "value of the nearest voxel at evenly spaced points from `from` to `to`",
        "min": f64::from(min),
        "max": f64::from(max),
        "samples": samples,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_statistics() {
        let s = summarise((1..=101).map(|v| v as f32).collect());
        assert_eq!(
            (s["voxels"].as_u64(), s["mean"].as_f64(), s["min"].as_f64(), s["max"].as_f64()),
            (Some(101), Some(51.0), Some(1.0), Some(101.0))
        );
        assert_eq!((s["percentiles"]["p5"].as_f64(), s["percentiles"]["p50"].as_f64()), (Some(6.0), Some(51.0)));
        assert!((s["std"].as_f64().unwrap() - 29.15).abs() < 0.01);
    }

    #[test]
    fn polygon_inside_test() {
        let sq = [Vec2::ZERO, Vec2::new(2.0, 0.0), Vec2::new(2.0, 2.0), Vec2::new(0.0, 2.0)];
        assert!(inside(&sq, Vec2::ONE));
        assert!(!inside(&sq, Vec2::new(3.0, 1.0)));
    }
}
