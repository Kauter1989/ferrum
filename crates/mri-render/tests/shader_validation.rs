//! Parses and validates every WGSL shader with naga. Runs without a GPU,
//! so shader regressions are caught on any CI machine.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use mri_render::shaders::{BLIT_WGSL, SLICE_WGSL, VOLUME_WGSL};
use naga::valid::{Capabilities, ValidationFlags, Validator};

fn validate(name: &str, src: &str) -> naga::Module {
    let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
    Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    module
}

fn entry_points(m: &naga::Module) -> Vec<String> {
    m.entry_points.iter().map(|e| e.name.clone()).collect()
}

#[test]
fn volume_shader_is_valid_and_has_mode_override() {
    let m = validate("volume.wgsl", VOLUME_WGSL);
    assert_eq!(entry_points(&m), vec!["vs_main", "fs_main"]);
    assert!(m.overrides.iter().any(|(_, o)| o.name.as_deref() == Some("MODE")));
}

#[test]
fn slice_shader_is_valid() {
    let m = validate("slice.wgsl", SLICE_WGSL);
    assert_eq!(entry_points(&m), vec!["vs_main", "fs_main"]);
}

#[test]
fn blit_shader_is_valid() {
    let m = validate("blit.wgsl", BLIT_WGSL);
    assert_eq!(entry_points(&m), vec!["vs_main", "fs_main"]);
}

#[test]
fn uniform_block_size_matches_rust_layout() {
    let m = validate("volume.wgsl", VOLUME_WGSL);
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(m.to_ctx()).unwrap();
    let (_, ty) = m.types.iter().find(|(_, t)| t.name.as_deref() == Some("Uniforms")).unwrap();
    let handle = m.types.get(ty).unwrap();
    assert_eq!(layouter[handle].size as usize, std::mem::size_of::<mri_render::frame::VolumeUniforms>());

    let s = validate("slice.wgsl", SLICE_WGSL);
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(s.to_ctx()).unwrap();
    let (_, ty) = s.types.iter().find(|(_, t)| t.name.as_deref() == Some("SliceUniforms")).unwrap();
    let handle = s.types.get(ty).unwrap();
    assert_eq!(layouter[handle].size as usize, std::mem::size_of::<mri_render::frame::SliceUniforms>());
}
