//! Points in the three forms commands accept (`docs/agent-skill.md` §7):
//! `{"voxel": [i, j, k]}` (0-based, canonical LPS grid),
//! `{"patient_mm": [x, y, z]}` (LPS millimetres) or
//! `{"render": "r-0007", "pixel": [x, y]}` (a pixel of an earlier render).

use std::path::Path;

use ferrum_domain::{SliceAxis, Volume};
use glam::{DVec3, Vec2, Vec3};
use serde_json::{json, Value};

use crate::envelope::{AgentError, ErrorCode};
use crate::params::vec3;

/// A point as given by the agent.
#[derive(Debug, Clone, PartialEq)]
pub enum PointInput {
    /// Continuous voxel coordinates; integers are voxel centres.
    Voxel(DVec3),
    /// LPS patient coordinates in millimetres.
    PatientMm(DVec3),
    /// Pixel `(x, y)` of an earlier render (pixel centres at `+0.5`).
    Render {
        /// Render id, e.g. `r-0007`.
        render: String,
        /// Pixel coordinates.
        pixel: [f64; 2],
    },
}

impl PointInput {
    /// Parses one of the three JSON forms.
    pub fn parse(v: &Value) -> Result<Self, AgentError> {
        let obj = v.as_object().ok_or_else(|| point_error("a point must be an object"))?;
        match (obj.get("voxel"), obj.get("patient_mm"), obj.get("render")) {
            (Some(p), None, None) => Ok(Self::Voxel(DVec3::from(vec3(p, "voxel").map_err(with_hint)?))),
            (None, Some(p), None) => Ok(Self::PatientMm(DVec3::from(vec3(p, "patient_mm").map_err(with_hint)?))),
            (None, None, Some(r)) => {
                let render = r.as_str().ok_or_else(|| point_error("render must be a render id"))?.to_owned();
                let px = obj.get("pixel").and_then(Value::as_array).filter(|a| a.len() == 2);
                let px = px.ok_or_else(|| point_error("pixel must be [x, y]"))?;
                let num = |i: usize| {
                    px[i].as_f64().filter(|x| x.is_finite()).ok_or_else(|| point_error("pixel must be [x, y]"))
                };
                Ok(Self::Render { render, pixel: [num(0)?, num(1)?] })
            }
            _ => Err(point_error("give exactly one of voxel, patient_mm or render + pixel")),
        }
    }
}

fn point_error(msg: &str) -> AgentError {
    with_hint(AgentError::bad_request(msg.to_owned()))
}

fn with_hint(e: AgentError) -> AgentError {
    e.hint(r#"points are {"voxel": [i, j, k]}, {"patient_mm": [x, y, z]} or {"render": "r-0001", "pixel": [x, y]}"#)
}

/// Continuous voxel coordinates of LPS point `p`.
pub fn patient_to_voxel(volume: &Volume, p: DVec3) -> DVec3 {
    let g = volume.geometry();
    let d = g.direction.as_dmat3();
    // the direction matrix is orthonormal: its inverse is its transpose
    (d.transpose() * (p - g.origin.as_dvec3())) / volume.spacing().as_dvec3()
}

/// LPS patient coordinates of continuous voxel `v`.
pub fn voxel_to_patient(volume: &Volume, v: DVec3) -> DVec3 {
    let g = volume.geometry();
    g.origin.as_dvec3() + g.direction.as_dmat3() * (v * volume.spacing().as_dvec3())
}

/// Resolves a point to continuous voxel coordinates. `renders` is the
/// folder of render sidecars. Fails with `out_of_volume` outside the grid.
pub fn resolve(volume: &Volume, p: &PointInput, renders: &Path) -> Result<DVec3, AgentError> {
    let v = match p {
        PointInput::Voxel(v) => *v,
        PointInput::PatientMm(mm) => patient_to_voxel(volume, *mm),
        PointInput::Render { render, pixel } => render_pixel_to_voxel(renders, render, *pixel)?,
    };
    let dims = volume.dims().as_uvec3().as_dvec3();
    if (0..3).any(|a| !(-0.5..=dims[a] - 0.5).contains(&v[a])) {
        let d = volume.dims();
        return Err(AgentError::new(
            ErrorCode::OutOfVolume,
            format!("point at voxel ({:.1}, {:.1}, {:.1}) is outside {}×{}×{}", v.x, v.y, v.z, d.x, d.y, d.z),
        )
        .hint("voxel indices are 0-based; patient_mm uses LPS millimetres"));
    }
    Ok(v)
}

/// Nearest voxel of a continuous position inside the grid.
pub fn nearest_voxel(volume: &Volume, v: DVec3) -> [u32; 3] {
    let d = volume.dims().as_uvec3();
    let c = |a: usize| v[a].round().clamp(0.0, f64::from(d[a] - 1)) as u32;
    [c(0), c(1), c(2)]
}

fn render_pixel_to_voxel(renders: &Path, id: &str, pixel: [f64; 2]) -> Result<DVec3, AgentError> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(AgentError::bad_request(format!("invalid render id {id:?}")));
    }
    let path = renders.join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path).map_err(|_| AgentError::not_found(format!("unknown render {id}")))?;
    let sidecar: Value = serde_json::from_str(&text).map_err(|e| AgentError::internal(format!("{id}: {e}")))?;
    let size = sidecar["size"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect::<Vec<_>>());
    if let Some([w, h]) = size.as_deref() {
        if !(0.0..=*w).contains(&pixel[0]) || !(0.0..=*h).contains(&pixel[1]) {
            return Err(AgentError::new(
                ErrorCode::OutOfVolume,
                format!("pixel {pixel:?} is outside the {w}×{h} render"),
            ));
        }
    }
    let m = &sidecar["pixel_to_voxel"];
    let row = |r: usize| -> Result<f64, AgentError> {
        let a = (0..3).map(|c| m[r][c].as_f64()).collect::<Option<Vec<_>>>();
        let a = a.ok_or_else(|| AgentError::internal(format!("{id}: bad pixel_to_voxel")))?;
        Ok(a[0] * pixel[0] + a[1] * pixel[1] + a[2])
    };
    Ok(DVec3::new(row(0)?, row(1)?, row(2)?))
}

/// JSON description of a point in every form: continuous `voxel`,
/// `voxel_index` (nearest voxel) and `patient_mm`.
pub fn describe(volume: &Volume, v: DVec3) -> Value {
    let r = |x: f64| (x * 1000.0).round() / 1000.0;
    let mm = voxel_to_patient(volume, v);
    json!({
        "voxel": [r(v.x), r(v.y), r(v.z)],
        "voxel_index": nearest_voxel(volume, v),
        "patient_mm": [r(mm.x), r(mm.y), r(mm.z)],
    })
}

/// In-plane millimetres (viewer convention, see `ferrum-annotations`) of
/// continuous voxel `v` on a slice of `plane`.
pub fn voxel_to_plane_mm(volume: &Volume, plane: SliceAxis, v: DVec3) -> Vec2 {
    let t = (v.as_vec3() + Vec3::splat(0.5)) / volume.dims().as_uvec3().as_vec3();
    plane.uv_of_tex(t) * plane.plane_size_mm(volume)
}

/// Parses a plane name (`axial`, `coronal`, `sagittal`).
pub fn parse_plane(name: &str) -> Result<SliceAxis, AgentError> {
    SliceAxis::ALL.into_iter().find(|a| a.label().eq_ignore_ascii_case(name)).ok_or_else(|| {
        AgentError::bad_request(format!("unknown plane {name:?}")).hint("planes are axial, coronal and sagittal")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrum_domain::{Dims3, Geometry};
    use glam::Mat3;

    fn volume() -> Volume {
        let dims = Dims3::new(10, 8, 6);
        let v = Volume::from_physical(dims, Vec3::new(0.5, 0.5, 2.0), &vec![0.0; dims.voxel_count()]).unwrap();
        // coronal acquisition: j runs inferior (−z), k runs posterior (+y)
        let direction = Mat3::from_cols(Vec3::X, Vec3::new(0.0, 0.0, -1.0), Vec3::Y);
        v.with_geometry(Geometry { origin: Vec3::new(-10.0, 5.0, 30.0), direction })
    }

    #[test]
    fn parse_forms_and_errors() {
        assert_eq!(
            PointInput::parse(&json!({ "voxel": [1, 2, 3] })).unwrap(),
            PointInput::Voxel(DVec3::new(1.0, 2.0, 3.0))
        );
        assert!(matches!(PointInput::parse(&json!({ "patient_mm": [1, 2, 3] })).unwrap(), PointInput::PatientMm(_)));
        let r = PointInput::parse(&json!({ "render": "r-0001", "pixel": [3.5, 4] })).unwrap();
        assert_eq!(r, PointInput::Render { render: "r-0001".into(), pixel: [3.5, 4.0] });
        for bad in [
            json!(1),
            json!({}),
            json!({ "voxel": [1, 2, 3], "patient_mm": [1, 2, 3] }),
            json!({ "voxel": [1, 2] }),
            json!({ "render": 1, "pixel": [1, 1] }),
            json!({ "render": "r", "pixel": [1] }),
            json!({ "render": "r", "pixel": ["a", 1] }),
        ] {
            assert!(PointInput::parse(&bad).unwrap_err().hint.is_some(), "{bad}");
        }
    }

    #[test]
    fn patient_and_voxel_round_trip() {
        let v = volume();
        let vox = DVec3::new(3.0, 2.0, 4.0);
        let mm = voxel_to_patient(&v, vox);
        assert!((mm - DVec3::new(-8.5, 13.0, 29.0)).length() < 1e-9, "{mm}");
        assert!((patient_to_voxel(&v, mm) - vox).length() < 1e-9);
        let d = describe(&v, vox);
        assert_eq!(d["voxel_index"], json!([3, 2, 4]));
        assert_eq!(d["patient_mm"], json!([-8.5, 13.0, 29.0]));
    }

    #[test]
    fn resolve_checks_bounds_and_renders() {
        let v = volume();
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve(&v, &PointInput::Voxel(DVec3::new(9.5, 0.0, 0.0)), dir.path()).is_ok());
        let e = resolve(&v, &PointInput::Voxel(DVec3::new(10.0, 0.0, 0.0)), dir.path()).unwrap_err();
        assert_eq!(e.code, ErrorCode::OutOfVolume);
        let mm = voxel_to_patient(&v, DVec3::new(1.0, 1.0, 1.0));
        assert!((resolve(&v, &PointInput::PatientMm(mm), dir.path()).unwrap() - DVec3::ONE).length() < 1e-9);
        let sidecar = json!({ "size": [20, 16], "pixel_to_voxel": [[0.5, 0, -0.5], [0, 0.5, -0.5], [0, 0, 2]] });
        std::fs::write(dir.path().join("r-0001.json"), sidecar.to_string()).unwrap();
        let p = |x, y| PointInput::Render { render: "r-0001".into(), pixel: [x, y] };
        assert_eq!(resolve(&v, &p(3.0, 5.0), dir.path()).unwrap(), DVec3::new(1.0, 2.0, 2.0));
        assert_eq!(resolve(&v, &p(30.0, 5.0), dir.path()).unwrap_err().code, ErrorCode::OutOfVolume);
        let unknown = PointInput::Render { render: "r-0002".into(), pixel: [1.0, 1.0] };
        assert_eq!(resolve(&v, &unknown, dir.path()).unwrap_err().code, ErrorCode::NotFound);
        let sneaky = PointInput::Render { render: "../x".into(), pixel: [1.0, 1.0] };
        assert_eq!(resolve(&v, &sneaky, dir.path()).unwrap_err().code, ErrorCode::BadRequest);
        std::fs::write(dir.path().join("r-0003.json"), "{}").unwrap();
        let broken = PointInput::Render { render: "r-0003".into(), pixel: [1.0, 1.0] };
        assert_eq!(resolve(&v, &broken, dir.path()).unwrap_err().code, ErrorCode::Internal);
    }

    #[test]
    fn planes_and_in_plane_mm() {
        assert_eq!(parse_plane("Axial").unwrap(), SliceAxis::Axial);
        assert!(parse_plane("oblique").unwrap_err().hint.is_some());
        let v = volume();
        // axial: x right from voxel i, y down from voxel j (top-left corner origin)
        let mm = voxel_to_plane_mm(&v, SliceAxis::Axial, DVec3::new(0.0, 0.0, 2.0));
        assert!((mm - Vec2::new(0.25, 0.25)).length() < 1e-6, "{mm}");
    }
}
