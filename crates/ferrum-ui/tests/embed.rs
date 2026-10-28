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
    for mode in [ViewMode::Mpr, ViewMode::Volume3d, ViewMode::Slice2d] {
        h.state_mut().viewer.set_view_mode(mode);
        h.run();
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
