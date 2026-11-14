//! The viewer embedded in a host's own layout (Stage 24): no shell of its
//! own, laid out and driven without a GPU, the tool bar and the panels
//! optional.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use ferrum_app::{ToolKind, ViewMode, Viewer};
use ferrum_domain::{Dims3, Volume};
use ferrum_ui::panels::{self, PanelRequest, PanelState};
use ferrum_ui::ViewerWidget;
use glam::Vec3;

/// A sphere phantom as NIfTI.
fn phantom_file(dir: &Path) -> PathBuf {
    let dims = Dims3::new(32, 32, 24);
    let c = (dims.as_vec3() - Vec3::ONE) * 0.5;
    let mut vals = Vec::with_capacity(dims.voxel_count());
    for k in 0..dims.z {
        for j in 0..dims.y {
            for i in 0..dims.x {
                let r = ((Vec3::new(i as f32, j as f32, k as f32) - c) / c).length();
                vals.push(if r < 0.6 { 400.0 } else { -1000.0 });
            }
        }
    }
    let path = dir.join("phantom.nii");
    ferrum_io::write_nifti(&Volume::from_physical(dims, Vec3::ONE, &vals).unwrap(), &path).unwrap();
    path
}

fn viewer(dir: &Path) -> Viewer {
    let mut v = Viewer::new(Arc::new(ferrum_io::CompositeRepository::default()));
    v.open_paths(vec![phantom_file(dir)]);
    v.wait_idle();
    v.declare_modality("CT").unwrap();
    v
}

/// A host: its own panel on the left, the widget in the centre, and
/// optionally FERRUM's side panel on the right.
struct Host {
    viewer: Viewer,
    widget: ViewerWidget,
    panel: Option<PanelState>,
    requests: Vec<PanelRequest>,
    clicks: u32,
}

impl Host {
    fn new(dir: &Path, widget: ViewerWidget, panel: bool) -> Self {
        Self { viewer: viewer(dir), widget, panel: panel.then(PanelState::default), requests: Vec::new(), clicks: 0 }
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("host").exact_size(200.0).show(ui, |ui| {
            ui.label("Conversation");
            if ui.button("Host button").clicked() {
                self.clicks += 1;
            }
        });
        if let Some(panel) = &mut self.panel {
            egui::Panel::right("ferrum-panel").exact_size(324.0).show(ui, |ui| {
                self.requests.extend(panels::side_panel(ui, &mut self.viewer, panel));
            });
        }
        egui::CentralPanel::default().show(ui, |ui| self.widget.show(ui, &mut self.viewer, None));
    }
}

fn harness(host: Host) -> Harness<'static, Host> {
    Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(|ui, h: &mut Host| h.ui(ui), host)
}

// covers 24.1-a
#[test]
fn ferrum_ui_has_no_shell_dependencies() {
    let out = std::process::Command::new(env!("CARGO"))
        .args(["tree", "-p", "ferrum-ui", "-e", "normal", "--prefix", "none", "--offline"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let tree = String::from_utf8(out.stdout).unwrap();
    let crates: Vec<&str> = tree.lines().filter_map(|l| l.split_whitespace().next()).collect();
    assert!(crates.contains(&"egui-wgpu"), "the tree was read: {crates:?}");
    for shell in ["eframe", "rfd", "winit"] {
        assert!(!crates.contains(&shell), "{shell} is a window or dialog crate");
    }
}

// covers 24.1-b, 24.1-c
#[test]
fn the_widget_lays_out_without_a_gpu_next_to_the_hosts_panel() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(Host::new(dir.path(), ViewerWidget::new(), false));
    h.run();
    h.get_by_label("Host button").click();
    h.run();
    assert_eq!(h.state().clicks, 1, "the host's own panel works next to the widget");
    // the tool bar is drawn by the widget, and its tools drive the viewer
    h.get_by_label("Distance").click();
    h.run();
    assert_eq!(h.state().viewer.tool, ToolKind::Distance);
    // the widget's tool bar switches modes, and each mode offers its tools
    for (mode, label) in
        [("MPR", ViewMode::Mpr), ("3D", ViewMode::Volume3d), ("2D", ViewMode::Slice2d)].map(|(l, m)| (m, l))
    {
        h.get_by_label(label).click();
        h.run();
        assert_eq!(h.state().viewer.view_mode(), mode);
        let rotate = h.query_by_label("Rotate").is_some();
        let distance = h.query_by_label("Distance").is_some();
        assert_eq!((rotate, distance), (mode == ViewMode::Volume3d, mode != ViewMode::Volume3d), "{mode:?}");
    }
    assert!(h.query_by_label("Engine URL").is_none(), "no panel unless the host places one");
}

// covers 24.1-c
#[test]
fn toolbar_and_panels_are_optional() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(Host::new(dir.path(), ViewerWidget::new().with_toolbar(false), true));
    h.run();
    assert!(h.query_by_label("Distance").is_none(), "no tool bar");
    h.get_by_label("Engine URL");
    // the panel's file actions come back as requests; the host opens its own dialogs
    h.get_by_label_contains("Import").click();
    h.run();
    assert_eq!(h.state().requests, vec![PanelRequest::ImportLabels]);
    h.run();
    assert_eq!(h.state().requests.len(), 1, "a request is given once");
}

// covers 24.3-c
#[test]
fn the_image_formation_model_stays_in_ferrum_render() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        assert_ne!(
            path.extension().and_then(|e| e.to_str()),
            Some("wgsl"),
            "{}: shaders live in ferrum-render",
            path.display()
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("fn raycast") && !text.contains("@fragment"),
            "{}: no image formation here",
            path.display()
        );
    }
}

/// One frame of `ctx` without a renderer (its textures are dropped).
fn frame(ctx: &egui::Context, ui: &mut dyn FnMut(&mut egui::Ui)) -> egui::FullOutput {
    let mut out = ctx.run_ui(egui::RawInput::default(), ui);
    out.textures_delta.clear();
    out
}

// covers 24.1-b
#[test]
fn the_host_keeps_its_fonts_and_style_and_still_gets_the_icons() {
    let dir = tempfile::tempdir().unwrap();
    let mut viewer = viewer(dir.path());
    let mut widget = ViewerWidget::new().with_theme(false);
    let ctx = egui::Context::default();
    // the host's own font and style, set before the widget's first frame
    let data = egui::FontDefinitions::default().font_data.values().next().unwrap().as_ref().clone();
    let family = egui::epaint::text::InsertFontFamily {
        family: egui::FontFamily::Proportional,
        priority: egui::epaint::text::FontPriority::Highest,
    };
    ctx.add_font(egui::epaint::text::FontInsert::new("host-font", data, vec![family]));
    ctx.set_theme(egui::Theme::Light);
    for _ in 0..2 {
        frame(&ctx, &mut |ui| widget.show(ui, &mut viewer, None));
    }
    let fonts: Vec<String> = ctx.fonts(|f| f.definitions().font_data.keys().cloned().collect());
    assert!(fonts.iter().any(|k| k == "host-font"), "the host's font is kept: {fonts:?}");
    assert!(fonts.iter().any(|k| k == ferrum_ui::theme::ICON_FONT), "the icons are there: {fonts:?}");
    assert_eq!(ctx.theme(), egui::Theme::Light, "with_theme(false) leaves the host's style");
}

// covers 24.1-b
#[test]
fn a_study_being_loaded_keeps_frames_coming() {
    let dir = tempfile::tempdir().unwrap();
    let mut viewer = Viewer::new(Arc::new(ferrum_io::CompositeRepository::default()));
    let mut widget = ViewerWidget::new();
    let ctx = egui::Context::default();
    let delay = |out: &egui::FullOutput| out.viewport_output.values().next().unwrap().repaint_delay;
    // egui asks for a few frames of its own at first; then an idle widget asks for none
    let idle_wait = (0..20)
        .map(|_| delay(&frame(&ctx, &mut |ui| widget.show(ui, &mut viewer, None))))
        .find(|d| *d > std::time::Duration::ZERO)
        .expect("an idle widget stops asking for frames");
    viewer.open_paths(vec![phantom_file(dir.path())]);
    let out = frame(&ctx, &mut |ui| widget.show(ui, &mut viewer, None));
    let wait = delay(&out);
    assert!(viewer.is_loading() || viewer.dataset().is_some());
    if viewer.is_loading() {
        assert_eq!(wait, std::time::Duration::ZERO, "a load in progress asks for the next frame (idle: {idle_wait:?})");
    }
    viewer.wait_idle();
}
