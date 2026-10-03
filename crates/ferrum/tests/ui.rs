//! End-to-end UI tests driving the real egui application with
//! egui_kittest: navigation through the accessibility tree without a GPU,
//! and full-window renders through wgpu (lavapipe in CI) to verify that
//! 2D, 3D and MPR views actually draw the volume.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;
use ferrum::ViewerApp;
use ferrum_app::{ToolKind, ViewMode};
use ferrum_domain::{Annotation, Dims3, RenderMode, SliceAxis, SliceKey, Volume};
use glam::Vec3;

/// Writes a sphere phantom as NIfTI and returns its path.
fn phantom_file(dir: &Path) -> PathBuf {
    let dims = Dims3::new(48, 48, 40);
    let c = (dims.as_vec3() - Vec3::ONE) * 0.5;
    let mut vals = Vec::with_capacity(dims.voxel_count());
    for k in 0..dims.z {
        for j in 0..dims.y {
            for i in 0..dims.x {
                let r = ((Vec3::new(i as f32, j as f32, k as f32) - c) / c).length();
                vals.push(if r < 0.5 {
                    1000.0
                } else if r < 0.8 {
                    300.0
                } else {
                    0.0
                });
            }
        }
    }
    let v = Volume::from_physical(dims, Vec3::ONE, &vals).unwrap();
    let path = dir.join("phantom.nii");
    ferrum_io::write_nifti(&v, &path).unwrap();
    path
}

fn loaded_app(dir: &Path, render_state: Option<&egui_wgpu::RenderState>) -> ViewerApp {
    let repo = Arc::new(ferrum_io::CompositeRepository::default());
    let mut app = ViewerApp::new(render_state, repo, vec![phantom_file(dir)]);
    app.viewer.wait_idle();
    assert!(app.viewer.dataset().is_some(), "{:?}", app.viewer.status);
    app
}

#[test]
fn welcome_screen_without_data() {
    let repo = Arc::new(ferrum_io::CompositeRepository::default());
    let app = ViewerApp::new(None, repo, vec![]);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1000.0, 700.0))
        .build_ui_state(|ui, app: &mut ViewerApp| app.show(ui, None), app);
    h.run();
    h.get_by_label("Open folder");
    h.get_by_label("Open files");
    assert!(h.query_by_label("Settings panel").is_none(), "docks hidden without data");
}

#[test]
fn switching_modes_and_tools_through_the_ui() {
    let dir = tempfile::tempdir().unwrap();
    let app = loaded_app(dir.path(), None);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .build_ui_state(|ui, app: &mut ViewerApp| app.show(ui, None), app);
    h.run();
    assert_eq!(h.state().viewer.view_mode(), ViewMode::Slice2d);

    h.get_by_label("Distance").click();
    h.run();
    assert_eq!(h.state().viewer.tool, ToolKind::Distance);

    h.get_by_label("3D").click();
    h.run();
    assert_eq!(h.state().viewer.view_mode(), ViewMode::Volume3d);
    h.get_by_label("Isosurface").click();
    h.run();
    assert_eq!(h.state().viewer.volume.settings.mode, RenderMode::Isosurface);

    h.get_by_label("MPR").click();
    h.run();
    assert_eq!(h.state().viewer.view_mode(), ViewMode::Mpr);

    h.get_by_label("Settings panel").click();
    h.run();
    assert!(h.query_by_label("Isosurface").is_none(), "panel hidden");
    h.get_by_label("Settings panel").click();
    h.run();

    h.get_by_label("Info").click();
    h.run();
    h.get_by_label("Series information");
}

#[test]
fn each_view_mode_offers_only_its_tools_and_settings() {
    let dir = tempfile::tempdir().unwrap();
    let app = loaded_app(dir.path(), None);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1400.0, 1000.0))
        .build_ui_state(|ui, app: &mut ViewerApp| app.show(ui, None), app);
    h.run();

    // 2D: slice tools, segmentation tools (AI ones disabled without an engine), image settings
    for label in ["Distance", "Region", "Image", "Details"] {
        h.get_by_label(label);
    }
    assert!(h.get_by_label("AI point").accesskit_node().is_disabled(), "AI tools wait for an engine");
    assert!(!h.get_by_label("Region").accesskit_node().is_disabled(), "the region tool works right away");
    assert!(h.query_by_label("Volume").is_none() && h.query_by_label("Rotate").is_none());
    h.get_by_label_contains("Choose a segmentation tool");

    // 3D: navigation and eraser; no slice or segmentation tools, no engine controls
    h.get_by_label("3D").click();
    h.run();
    for label in ["Rotate", "Volume eraser", "Isosurface", "Details"] {
        h.get_all_by_label(label).next().unwrap();
    }
    for label in ["Distance", "Region", "AI point", "Image", "Engine URL", "Tolerance ±"] {
        assert!(h.query_by_label(label).is_none(), "{label} in 3D");
    }
    h.get_by_label_contains("Segment in 2D");
    h.get_by_label("Volume eraser").click();
    h.run();
    assert!(h.state().viewer.volume.eraser_enabled);

    // MPR: slice and segmentation tools plus both settings tabs; the eraser stays in 3D
    h.get_by_label("MPR").click();
    h.run();
    assert!(!h.state().viewer.volume.eraser_enabled, "MPR's 3D cell must not erase unnoticed");
    for label in ["Distance", "Region", "Image", "Volume", "Details"] {
        h.get_by_label(label);
    }
    h.get_by_label("Volume").click();
    h.run();
    assert!(h.query_by_label("Volume eraser").is_none(), "no eraser outside 3D");
    h.get_by_label("Opacity");
}

#[test]
fn annotation_list_navigates_to_the_slice_and_is_2d_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = loaded_app(dir.path(), None);
    let id = app.viewer.add_annotation(
        SliceKey::new(SliceAxis::Coronal, 7),
        Annotation::Distance { a: glam::Vec2::ZERO, b: glam::Vec2::new(5.0, 0.0) },
    );
    app.viewer.rename_annotation(id, "Lesion");
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .build_ui_state(|ui, app: &mut ViewerApp| app.show(ui, None), app);
    h.run();
    assert_eq!(h.state().viewer.view_mode(), ViewMode::Slice2d);
    assert!(h.query_by_label("Smooth").is_none(), "filters are hidden");
    h.get_by_label_contains("Export JSON");

    h.get_by_label("Go to Lesion").click();
    h.run();
    assert_eq!(h.state().viewer.slices.axis, SliceAxis::Coronal);
    assert_eq!(h.state().viewer.slices.index(SliceAxis::Coronal), 7);

    h.get_by_label("MPR").click();
    h.run();
    h.get_by_label("Image").click();
    h.run();
    assert!(h.query_by_label("Go to Lesion").is_none(), "annotation list is shown in the 2D view only");

    h.get_by_label("2D").click();
    h.run();
    h.get_by_label("Delete Lesion").click();
    h.run();
    assert!(h.state().viewer.annotations().is_empty());
}

/// Labels a box around the centre of the phantom as segment "Tumour".
fn add_tumour(app: &mut ViewerApp) -> u8 {
    let label = app.viewer.add_segment("Tumour").unwrap();
    app.viewer.set_segment_color(label, [255, 0, 0]).unwrap();
    app.viewer.set_segment_opacity(label, 1.0).unwrap();
    let bx = ferrum_domain::VoxelBox::new(glam::UVec3::new(16, 16, 12), glam::UVec3::new(32, 32, 28));
    app.viewer.apply_segment_mask(label, bx, &vec![1; bx.voxel_count()], false).unwrap();
    label
}

#[test]
fn segment_list_edits_segments_in_both_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = loaded_app(dir.path(), None);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 1400.0))
        .build_ui_state(|ui, app: &mut ViewerApp| app.show(ui, None), app);
    h.run();
    // the built-in region tool in the toolbar creates a segment without an engine
    h.get_by_label("Region").click();
    h.run();
    assert_eq!(h.state().viewer.tool, ToolKind::Region);
    assert!(h.get_all_by_label("Tolerance ±").count() > 0, "the settings of the active tool are shown");
    let viewer = &mut h.state_mut().viewer;
    viewer.set_slice_index(SliceAxis::Axial, 20);
    viewer.slice_input(
        SliceAxis::Axial,
        ferrum_app::InputKind::Press,
        glam::Vec2::splat(100.0),
        glam::Vec2::splat(200.0),
    );
    h.run();
    assert_eq!(h.state().viewer.segment_summaries().len(), 1);
    h.get_by_label("Delete Region 1").click();
    h.run();
    assert!(h.state().viewer.segment_summaries().is_empty());

    let label = add_tumour(h.state_mut());
    h.run();
    h.get_by_label("Hide Tumour").click();
    h.run();
    assert!(!h.state().viewer.segmentation().set().unwrap().segment(label).unwrap().visible);
    h.get_by_label("Show Tumour").click();
    h.run();
    // slices show the segments as an outline, a fill or both; the fill opacity applies to fills only
    use ferrum_domain::SegmentStyle;
    assert_eq!(h.state().viewer.segmentation().style, SegmentStyle::FillAndOutline);
    h.get_by_label("Outline").click();
    h.run();
    assert_eq!(h.state().viewer.segmentation().style, SegmentStyle::Outline);
    assert!(h.get_all_by_label_contains("Fill opacity").any(|n| n.accesskit_node().is_disabled()));
    h.get_by_label("Fill").click();
    h.run();
    assert_eq!(h.state().viewer.segmentation().style, SegmentStyle::Fill);
    assert!(h.get_all_by_label_contains("Fill opacity").all(|n| !n.accesskit_node().is_disabled()));
    h.state_mut().viewer.set_segment_fill_opacity(0.25);
    h.run();
    let params =
        h.state().viewer.slice_params(SliceAxis::Axial, (glam::Vec2::ZERO, glam::Vec2::splat(400.0)), 1.0).unwrap();
    assert_eq!((params.segment_style, params.fill_opacity), (SegmentStyle::Fill, 0.25));
    h.get_by_label("Both").click();
    h.run();
    h.get_by_label_contains("Undo edit").click();
    h.run();
    assert_eq!(h.state().viewer.segment_summaries()[0].voxels, 0);
    // the 3D view shows the segments but does not create them
    h.get_by_label("3D").click();
    h.run();
    h.get_by_label("Opacity of Tumour");
    h.get_by_label("Colour of Tumour");
    assert!(h.query_by_label("Region").is_none(), "no segmentation tools in 3D");
    assert!(h.query_by_label("Engine URL").is_none());
    h.get_by_label_contains("Segment in 2D").click();
    h.run();
    assert_eq!(h.state().viewer.view_mode(), ViewMode::Slice2d);
    app = loaded_app(dir.path(), None);
    assert!(app.viewer.segment_summaries().is_empty(), "a new study starts without segments");
}

#[test]
fn ai_tools_are_visible_but_disabled_until_an_engine_connects() {
    let dir = tempfile::tempdir().unwrap();
    let app = loaded_app(dir.path(), None);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 1000.0))
        .build_ui_state(|ui, app: &mut ViewerApp| app.show(ui, None), app);
    h.run();
    h.get_by_label("Engine URL");
    h.get_by_label("AI point").click();
    h.run();
    assert_eq!(h.state().viewer.tool, ToolKind::Pan, "disabled without an engine");

    // connect to the mock engine through the reference server
    let server =
        ferrum_engines::EngineServer::start(Arc::new(ferrum_engines::MockEngine::default()), "127.0.0.1:0", None)
            .unwrap();
    h.state_mut().panel.ai.url = server.url();
    h.run();
    h.get_by_label_contains("Connect").click();
    h.step(); // the app keeps repainting while the engine is busy
    h.state_mut().viewer.wait_ai_idle();
    h.run();
    assert!(h.state().viewer.ai().status().is_connected(), "{:?}", h.state().viewer.ai().status());
    h.get_by_label("AI point").click();
    h.run();
    assert_eq!(h.state().viewer.tool, ToolKind::AiPoint);

    // a prompt in the axial view segments the bright core of the phantom
    let viewer = &mut h.state_mut().viewer;
    viewer.set_slice_index(SliceAxis::Axial, 20);
    viewer.slice_input(
        SliceAxis::Axial,
        ferrum_app::InputKind::Press,
        glam::Vec2::splat(100.0),
        glam::Vec2::splat(200.0),
    );
    viewer.wait_ai_idle();
    h.run();
    let target = h.state().viewer.ai().target().expect("target segment");
    assert!(h.state().viewer.segmentation().set().unwrap().voxel_count(target) > 1000);
    h.get_by_label(&format!("{} Accept", egui_phosphor::light::CHECK)).click();
    h.step();
    h.state_mut().viewer.wait_ai_idle();
    h.run();
    assert_eq!(h.state().viewer.ai().target(), None);
    assert_eq!(h.state().viewer.segment_summaries().len(), 1, "the accepted segment stays");
    h.get_by_label_contains("Disconnect").click();
    h.run();
    assert!(!h.state().viewer.ai().status().is_connected());
    assert_eq!(h.state().viewer.tool, ToolKind::Pan);
}

#[test]
fn automatic_segmentation_runs_from_the_panel() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = loaded_app(dir.path(), None);
    let server =
        ferrum_engines::EngineServer::start(Arc::new(ferrum_engines::MockEngine::default()), "127.0.0.1:0", None)
            .unwrap();
    app.panel.ai.url = server.url();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 1800.0))
        .build_ui_state(|ui, app: &mut ViewerApp| app.show(ui, None), app);
    h.run();
    h.get_by_label_contains("Connect").click();
    h.step();
    h.state_mut().viewer.wait_ai_idle();
    h.run();
    h.get_by_label("Filter structures");
    h.get_by_label("bright").click();
    h.run();
    h.get_by_label_contains("Segment 1 structure(s)").click();
    h.step();
    h.state_mut().viewer.wait_ai_idle();
    h.run();
    let rows = h.state().viewer.segment_summaries();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].segment.name, "bright");
    assert!(rows[0].voxels > 1000, "{}", rows[0].voxels);
    assert!(rows[0].segment.provenance.is_pending());

    // the segment list and the review queue show the proposal; the user confirms it
    assert_eq!(h.get_all_by_label_contains("Proposed by engine").count(), 2);
    h.get_all_by_label("Confirm").next().unwrap().click();
    h.run();
    let p = &h.state().viewer.segment_summaries()[0].segment.provenance;
    assert_eq!(p.status, ferrum_domain::ReviewStatus::Confirmed);
    h.get_by_label_contains("Confirmed · engine");
    h.get_by_label("Reopen").click();
    h.run();
    assert!(h.state().viewer.segment_summaries()[0].segment.provenance.is_pending());
}

fn gpu_render_state() -> Option<egui_wgpu::RenderState> {
    if let Err(e) = ferrum_render::gpu::GpuContext::headless() {
        assert!(std::env::var("FERRUM_REQUIRE_GPU").is_err(), "GPU required: {e}");
        eprintln!("skipping GPU UI test: {e}");
        return None;
    }
    let mut setup = egui_kittest::wgpu::default_wgpu_setup();
    if let egui_wgpu::WgpuSetup::CreateNew(c) = &mut setup {
        c.device_descriptor = Arc::new(ferrum_render::gpu::device_descriptor);
    }
    Some(egui_kittest::wgpu::create_render_state(setup, egui_wgpu::RendererOptions::PREDICTABLE))
}

/// Fraction of pixels inside `rect` that are clearly brighter than the
/// dark view background.
fn lit_fraction(img: &image::RgbaImage, rect: egui::Rect) -> f64 {
    let mut lit = 0usize;
    let mut total = 0usize;
    for y in rect.min.y as u32..rect.max.y as u32 {
        for x in rect.min.x as u32..rect.max.x as u32 {
            if let Some(p) = img.get_pixel_checked(x, y) {
                total += 1;
                if p.0[0].max(p.0[1]).max(p.0[2]) > 60 {
                    lit += 1;
                }
            }
        }
    }
    lit as f64 / total.max(1) as f64
}

#[test]
fn every_view_mode_renders_the_volume() {
    let Some(rs) = gpu_render_state() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let app = loaded_app(dir.path(), Some(&rs));
    let renderer = egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs.clone());
    let rs2 = rs.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .renderer(renderer)
        .build_ui_state(move |ui, app: &mut ViewerApp| app.show(ui, Some(&rs2)), app);
    let out_dir = std::env::var_os("FERRUM_SNAPSHOT_DIR").map(PathBuf::from);
    // central area: between the studies sidebar (232 pt) and the settings
    // panel (324 pt), below the header and toolbar, above the status bar
    let central = egui::Rect::from_min_max(egui::pos2(242.0, 114.0), egui::pos2(866.0, 762.0));
    for (mode, render) in [
        (ViewMode::Slice2d, RenderMode::Tissue),
        (ViewMode::Volume3d, RenderMode::Isosurface),
        (ViewMode::Volume3d, RenderMode::Tissue),
        (ViewMode::Volume3d, RenderMode::Mip),
        (ViewMode::Volume3d, RenderMode::TransferFunction),
        (ViewMode::Mpr, RenderMode::Isosurface),
    ] {
        h.state_mut().viewer.set_view_mode(mode);
        h.state_mut().viewer.volume.settings.mode = render;
        h.run();
        let img = h.render().expect("render");
        let name = format!("{mode:?}_{render:?}").to_lowercase();
        if let Some(d) = &out_dir {
            std::fs::create_dir_all(d).unwrap();
            img.save(d.join(format!("{name}.png"))).unwrap();
        }
        let lit = lit_fraction(&img, central);
        eprintln!("{name}: {:.1}% lit", lit * 100.0);
        assert!(lit > 0.03, "{name}: view looks empty ({:.2}% lit)", lit * 100.0);
    }
}

#[test]
fn segments_are_drawn_in_2d_and_3d() {
    let Some(rs) = gpu_render_state() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut app = loaded_app(dir.path(), Some(&rs));
    add_tumour(&mut app);
    let renderer = egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs.clone());
    let rs2 = rs.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .renderer(renderer)
        .build_ui_state(move |ui, app: &mut ViewerApp| app.show(ui, Some(&rs2)), app);
    let central = egui::Rect::from_min_max(egui::pos2(242.0, 114.0), egui::pos2(866.0, 762.0));
    let red = |img: &image::RgbaImage| {
        let mut n = 0usize;
        for y in central.min.y as u32..central.max.y as u32 {
            for x in central.min.x as u32..central.max.x as u32 {
                let p = img.get_pixel(x, y).0;
                if p[0] > 150 && p[1] < 80 && p[2] < 80 {
                    n += 1;
                }
            }
        }
        n
    };
    for mode in [ViewMode::Slice2d, ViewMode::Volume3d] {
        h.state_mut().viewer.set_view_mode(mode);
        h.state_mut().viewer.volume.settings.mode = RenderMode::Isosurface;
        h.state_mut().viewer.set_segments_shown(true);
        h.run();
        let img = h.render().expect("render");
        if let Some(d) = std::env::var_os("FERRUM_SNAPSHOT_DIR").map(PathBuf::from) {
            std::fs::create_dir_all(&d).unwrap();
            img.save(d.join(format!("segments_{mode:?}.png").to_lowercase())).unwrap();
        }
        let shown = red(&img);
        h.state_mut().viewer.set_segments_shown(false);
        h.run();
        let hidden = red(&h.render().expect("render"));
        eprintln!("{mode:?}: {shown} red pixels with segments, {hidden} without");
        assert!(shown > 2000 && hidden < 50, "{mode:?}: {shown} / {hidden}");
    }
}

#[test]
fn start_screen_renders_with_recent_files() {
    let Some(rs) = gpu_render_state() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mut recent = ferrum::ui::recent::RecentFiles::load(&dir.path().join("recent.txt"));
    recent.record(&[PathBuf::from("/data/chest_ct/lung_053.nii.gz")]);
    recent.record(&[PathBuf::from("/data/knee_mri/series_3")]);
    let repo = Arc::new(ferrum_io::CompositeRepository::default());
    let app = ViewerApp::new(Some(&rs), repo, vec![]).with_recent(recent);
    let renderer = egui_kittest::wgpu::WgpuTestRenderer::from_render_state(rs.clone());
    let rs2 = rs.clone();
    let mut h = Harness::builder()
        .with_size(egui::vec2(1600.0, 960.0))
        .renderer(renderer)
        .build_ui_state(move |ui, app: &mut ViewerApp| app.show(ui, Some(&rs2)), app);
    h.run();
    h.get_by_label("Open folder");
    let img = h.render().expect("render");
    if let Some(d) = std::env::var_os("FERRUM_SNAPSHOT_DIR").map(PathBuf::from) {
        std::fs::create_dir_all(&d).unwrap();
        img.save(d.join("start_screen.png")).unwrap();
    }
    let whole = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1600.0, 960.0));
    assert!(lit_fraction(&img, whole) > 0.005, "start screen is empty");
}

#[test]
fn agent_proposals_are_reviewed_in_the_desktop_app() {
    use ferrum_domain::{AnnotationReport, AnnotationSet, NoProgress, Provenance, Timestamp, VolumeRepository};
    let dir = tempfile::tempdir().unwrap();
    // an agent's workspace: the series, one proposed measurement
    let src = phantom_file(dir.path());
    let repo = ferrum_io::CompositeRepository::default();
    let series = repo.scan(std::slice::from_ref(&src), &NoProgress).unwrap().remove(0);
    let ws_dir = dir.path().join("ws");
    let ws = ferrum_io::Workspace::create(&ws_dir, &src, &series, "agent").unwrap();
    let volume = repo.load(&series, &NoProgress).unwrap().volume;
    let mut set = AnnotationSet::default();
    let key = SliceKey::new(SliceAxis::Axial, 2);
    let id = set.add_with(
        key,
        Annotation::Distance { a: glam::Vec2::ZERO, b: glam::Vec2::new(4.0, 0.0) },
        Provenance::agent(Some("run-7".into()), Timestamp(5)),
    );
    set.rename(id, "Agent distance");
    ws.save_annotations(&AnnotationReport::build(src.clone(), Default::default(), &volume, &set), "agent").unwrap();

    let mut app = ViewerApp::new(None, Arc::new(ferrum_io::CompositeRepository::default()), vec![]);
    app.open_workspace(&ws_dir);
    app.viewer.wait_idle();
    assert_eq!(app.viewer.review_queue().len(), 1, "{:?}", app.viewer.status);
    let mut h = Harness::builder()
        .with_size(egui::vec2(1200.0, 1400.0))
        .build_ui_state(|ui, app: &mut ViewerApp| app.show(ui, None), app);
    h.run();
    h.get_by_label("Open workspace");
    h.get_by_label_contains("proposal(s) are not findings");
    h.get_by_label("Reviewer name").click();
    h.run();
    h.get_by_label("Reviewer name").type_text("dr.k");
    h.run();
    // every Confirm (review queue or annotation list) records the decision in the workspace
    h.get_all_by_label("Confirm").next().unwrap().click();
    h.run();
    assert!(h.state().viewer.review_queue().is_empty());
    let log = ws.read_audit().unwrap();
    let last = log.last().unwrap();
    assert_eq!((last["command"].as_str(), last["params"]["by"].as_str()), (Some("review confirm"), Some("dr.k")));
    let saved = ferrum_io::read_annotations(&ws_dir.join("annotations.json"), None).unwrap();
    let p = saved.provenance(id).unwrap();
    assert_eq!(
        (p.status, p.reviewed_by.as_deref(), p.author.kind()),
        (ferrum_domain::ReviewStatus::Confirmed, Some("dr.k"), "agent")
    );
    h.get_by_label_contains("Nothing waits for review");
}
