//! `view volume`: a 3D render on the CPU (the reference ray caster of
//! `ferrum-render`, bit-compatible with the GPU renderer of the desktop
//! app), from one of the six standard viewpoints.
//!
//! A 3D image shows shape and context. Its pixels do not map to single
//! voxels, so measurements are taken on slices (`view slice`, `view mpr`).

use ferrum_domain::{ClipSettings, CtPreset, OrbitCamera, RenderMode, RenderSettings, TransferFunction, ViewPreset};
use ferrum_render::cpu::{CpuRaycaster, CpuScene, SegmentLayer};
use ferrum_render::FrameParams;
use glam::Vec2;
use serde_json::{json, Value};

use super::view::{overlays, save_render, size_param};
use super::Ctx;
use crate::envelope::{AgentError, Output};
use crate::params::Params;

/// Default image side of a 3D render (CPU time grows with the pixel count).
pub const DEFAULT_SIZE: u32 = 512;

fn view_preset(name: &str) -> Result<ViewPreset, AgentError> {
    let names = ["anterior", "posterior", "left", "right", "superior", "inferior"];
    names.iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| ViewPreset::ALL[i]).ok_or_else(|| {
        AgentError::bad_request(format!("unknown view {name:?}")).hint(format!("views: {}", names.join(", ")))
    })
}

fn ct_preset(name: &str) -> Option<CtPreset> {
    match name {
        "soft_tissue_bone" => Some(CtPreset::SoftTissueBone),
        "lung_vessels" => Some(CtPreset::LungVessels),
        "bone" => Some(CtPreset::Bone),
        _ => None,
    }
}

/// `view volume`: mode, preset or threshold, view, size, overlays.
pub fn volume(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let max = ctx.config.max_render_px;
    let study = ctx.study(p)?;
    let v = &study.volume;
    let range = v.range();
    let view = view_preset(p.str("view")?.unwrap_or("anterior"))?;
    let mode_name = p.str("mode")?.unwrap_or("mip");
    let mut settings = RenderSettings { empty_space_skipping: false, ..RenderSettings::default() };
    let mut tf = TransferFunction::linear_ramp();
    let mut described = json!({ "mode": mode_name });
    match mode_name {
        "mip" => settings.mode = RenderMode::Mip,
        "isosurface" => {
            settings.mode = RenderMode::Isosurface;
            let threshold = p
                .f64("threshold")?
                .ok_or_else(|| AgentError::bad_request("isosurface needs threshold (a value, e.g. 300 HU for bone)"))?
                as f32;
            settings.iso_threshold = range.normalize(threshold).clamp(0.0, 1.0);
            described["threshold"] = json!(threshold);
        }
        "transfer_function" => {
            let name = p.str("preset")?.unwrap_or("soft_tissue_bone");
            let preset = ct_preset(name).ok_or_else(|| {
                AgentError::bad_request(format!("unknown preset {name:?}"))
                    .hint("presets: soft_tissue_bone, lung_vessels, bone (CT values)")
            })?;
            settings.mode = RenderMode::TransferFunction;
            tf = TransferFunction::ct_preset(preset, range);
            described["preset"] = json!(name);
        }
        other => {
            return Err(AgentError::bad_request(format!("unknown mode {other:?}"))
                .hint("modes: mip, isosurface, transfer_function"))
        }
    }
    let size = size_param(p, max, DEFAULT_SIZE)?;
    let camera = OrbitCamera { orientation: view.orientation(), ..OrbitCamera::default() };
    let segments = overlays(p)?.contains(&"segments");
    let params = FrameParams::new(v, &camera, &settings, &ClipSettings::default(), Vec2::splat(size as f32), 0, None)
        .with_segments(segments);
    let lut = tf.bake(256);
    let seg_lut = study.segments.lut();
    let scene = CpuScene {
        volume: v,
        mask: None,
        ao: None,
        lut: &lut,
        occupancy: None,
        segments: segments.then_some(SegmentLayer { labels: study.segments.labels(), lut: &seg_lut }),
    };
    let pixels = CpuRaycaster::new(scene, &params).render(size, size);
    let rgb: Vec<u8> = pixels.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
    let warn = (mode_name == "transfer_function" && study.value_unit() != "HU")
        .then_some("the transfer-function presets assume CT values (HU)");
    let mut out = save_render(study, size, size, rgb, |id| {
        json!({
            "render": id,
            "kind": "volume",
            "view": view_name(view),
            "rendering": described,
            "size": [size, size],
            "renderer": "cpu ray caster (reference of the GPU renderer)",
            "overlays": if segments { json!(["segments"]) } else { json!([]) },
            "note": "A 3D render shows shape and context; its pixels do not map to single voxels. Measure on slices (view slice, view mpr) with probe, stats and measure.",
        })
    })?;
    if let Some(w) = warn {
        out = out.warn(w);
    }
    Ok(out)
}

fn view_name(v: ViewPreset) -> Value {
    json!(
        ["anterior", "posterior", "left", "right", "superior", "inferior"]
            [ViewPreset::ALL.iter().position(|x| *x == v).unwrap_or(0)]
    )
}
