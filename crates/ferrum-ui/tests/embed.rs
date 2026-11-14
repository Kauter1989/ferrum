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
use ferrum_ui::{ViewerEvent, ViewerWidget};
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
    events: Vec<ViewerEvent>,
}

impl Host {
    fn new(dir: &Path, widget: ViewerWidget, panel: bool) -> Self {
        Self {
            viewer: viewer(dir),
            widget,
            panel: panel.then(PanelState::default),
            requests: Vec::new(),
            clicks: 0,
            events: Vec::new(),
        }
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
        egui::CentralPanel::default().show(ui, |ui| {
            self.events.extend(self.widget.show(ui, &mut self.viewer, None));
        });
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
        frame(&ctx, &mut |ui| {
            widget.show(ui, &mut viewer, None);
        });
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
        .map(|_| {
            delay(&frame(&ctx, &mut |ui| {
                widget.show(ui, &mut viewer, None);
            }))
        })
        .find(|d| *d > std::time::Duration::ZERO)
        .expect("an idle widget stops asking for frames");
    viewer.open_paths(vec![phantom_file(dir.path())]);
    let out = frame(&ctx, &mut |ui| {
        widget.show(ui, &mut viewer, None);
    });
    let wait = delay(&out);
    assert!(viewer.is_loading() || viewer.dataset().is_some());
    if viewer.is_loading() {
        assert_eq!(wait, std::time::Duration::ZERO, "a load in progress asks for the next frame (idle: {idle_wait:?})");
    }
    viewer.wait_idle();
}

// ------------------------------------------------------------------ events (24.2)

/// One click at `p`: move, press and release in a single frame.
fn click_at<S>(h: &mut Harness<'static, S>, p: egui::Pos2) {
    h.input_mut().events.extend([egui::Event::PointerMoved(p), button(p, true), button(p, false)]);
}

/// A pointer button event at `p`.
fn button(p: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        button: egui::PointerButton::Primary,
        pos: p,
        pressed,
        modifiers: egui::Modifiers::default(),
    }
}

// covers 24.2-a
#[test]
fn events_are_coalesced_per_frame() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(Host::new(dir.path(), ViewerWidget::new(), false));
    h.run();
    h.state_mut().events.clear();
    let before = h.state().viewer.slices.indices;
    // several slices in one frame: the pointer over the view, a 100-point wheel
    h.input_mut().events.extend([
        egui::Event::PointerMoved(egui::pos2(800.0, 500.0)),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -100.0),
            modifiers: egui::Modifiers::default(),
            phase: egui::TouchPhase::Move,
        },
    ]);
    h.step();
    let slices: Vec<&ViewerEvent> =
        h.state().events.iter().filter(|e| matches!(e, ViewerEvent::SliceChanged { .. })).collect();
    assert_eq!(slices.len(), 1, "one SliceChanged per frame: {:?}", h.state().events);
    let after = h.state().viewer.slices.indices;
    let axial = ferrum_domain::SliceAxis::Axial.normal_axis();
    assert_ne!(after[axial], before[axial], "the wheel scrolled the axial slice: {before:?} -> {after:?}");
    assert_eq!(
        slices[0],
        &ViewerEvent::SliceChanged { indices: after },
        "the event carries the frame's final indices, not one per step"
    );

    // a window drag: no event while the button is down, one when it ends
    h.get_by_label("Window / level").click();
    h.run();
    h.state_mut().events.clear();
    let win0 = h.state().viewer.slices.window;
    let p = egui::pos2(800.0, 500.0);
    h.input_mut().events.extend([egui::Event::PointerMoved(p), button(p, true)]);
    h.step();
    let p2 = p + egui::vec2(40.0, 30.0);
    h.input_mut().events.push(egui::Event::PointerMoved(p2));
    h.step();
    assert!(
        !h.state().events.iter().any(|e| matches!(e, ViewerEvent::WindowChanged { .. })),
        "no WindowChanged during the drag: {:?}",
        h.state().events
    );
    h.input_mut().events.push(button(p2, false));
    h.step();
    let wins: Vec<&ViewerEvent> =
        h.state().events.iter().filter(|e| matches!(e, ViewerEvent::WindowChanged { .. })).collect();
    assert_eq!(wins.len(), 1, "one WindowChanged at the end of the drag: {:?}", h.state().events);
    let win = h.state().viewer.slices.window;
    assert_ne!(win0, win, "the drag changed the window");
    assert_eq!(wins[0], &ViewerEvent::WindowChanged { center: win.center, width: win.width });
    h.step();
    assert_eq!(
        h.state().events.iter().filter(|e| matches!(e, ViewerEvent::WindowChanged { .. })).count(),
        1,
        "the window event is given once"
    );
}

// covers 24.2-b
#[test]
fn a_pick_gives_the_voxel_probe_gives() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(Host::new(dir.path(), ViewerWidget::new(), false));
    h.run();
    h.get_by_label("Voxel value").click();
    h.run();
    h.state_mut().events.clear();
    click_at(&mut h, egui::pos2(800.0, 500.0));
    h.step();
    let picks: Vec<ferrum_ui::Point> = h
        .state()
        .events
        .iter()
        .filter_map(|e| match e {
            ViewerEvent::PointPicked { point } => Some(*point),
            _ => None,
        })
        .collect();
    assert_eq!(picks.len(), 1, "one pick: {:?}", h.state().events);
    let probe = h.state().viewer.probe.expect("the probe read the voxel");
    assert_eq!(picks[0].voxel, probe.voxel.to_array(), "the event is the probe's voxel");
    let volume = &h.state().viewer.dataset().unwrap().volume;
    assert_eq!(
        picks[0].patient_mm,
        volume.voxel_to_patient_f64(probe.voxel.as_dvec3()).to_array(),
        "the patient position is the voxel centre's, as probe resolves it"
    );
}

// covers 24.2-c
#[test]
fn no_event_is_a_decision() {
    use ferrum_domain::{Annotation, Provenance, ReviewStatus, SliceAxis, SliceKey, Timestamp, VoxelBox};
    let dir = tempfile::tempdir().unwrap();
    let mut h = harness(Host::new(dir.path(), ViewerWidget::new(), true));
    h.run();
    h.state_mut().events.clear();
    // an engine's proposal is not a RoiDrawn
    let label = {
        let v = &mut h.state_mut().viewer;
        let label = v.add_segment("Tumour").unwrap();
        let bx = VoxelBox::new(glam::UVec3::new(8, 8, 6), glam::UVec3::new(16, 16, 12));
        v.apply_segment_mask(label, bx, &vec![1; bx.voxel_count()], false).unwrap();
        v.set_segment_provenance(label, Provenance::engine("nnInteractive", "2.6", true, Timestamp(1))).unwrap();
        label
    };
    h.run();
    assert!(
        !h.state().events.iter().any(|e| matches!(e, ViewerEvent::RoiDrawn { .. })),
        "an engine proposal is not drawn by the person: {:?}",
        h.state().events
    );
    // confirming it is a decision, not an event
    h.state_mut().events.clear();
    h.state_mut().viewer.review_segment(label, ReviewStatus::Confirmed, Some("dr.k")).unwrap();
    h.run();
    assert!(h.state().events.is_empty(), "a decision is not an event: {:?}", h.state().events);
    // an agent's annotation: the same
    let id = h.state_mut().viewer.add_annotation_with(
        SliceKey::new(SliceAxis::Axial, 2),
        Annotation::Distance { a: glam::Vec2::ZERO, b: glam::Vec2::new(4.0, 0.0) },
        Provenance::agent(Some("run-7".into()), Timestamp(5)),
    );
    h.run();
    assert!(!h.state().events.iter().any(|e| matches!(e, ViewerEvent::RoiDrawn { .. })));
    h.state_mut().events.clear();
    h.state_mut().viewer.review_annotation(id, ReviewStatus::Confirmed, Some("dr.k"));
    h.run();
    assert!(h.state().events.is_empty(), "confirming an annotation is not an event: {:?}", h.state().events);
    // control: a person's own segment IS a RoiDrawn, so the test can fail
    h.state_mut().events.clear();
    h.state_mut().viewer.add_segment("Mine").unwrap();
    h.run();
    assert!(
        h.state().events.iter().any(|e| matches!(e, ViewerEvent::RoiDrawn { item: ferrum_ui::RoiItem::Segment(_) })),
        "a person's segment is a RoiDrawn: {:?}",
        h.state().events
    );
}

// ------------------------------------------------------------------ the example host (24.4)

#[path = "../examples/embed_host.rs"]
#[allow(dead_code)] // the example's window and main are not driven here
mod embed_host;

// covers 24.4-a, 24.4-b
#[test]
fn the_example_host_receives_a_pick() {
    let dir = tempfile::tempdir().unwrap();
    let host = embed_host::HostState::new(viewer(dir.path()));
    let mut h = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .build_ui_state(|ui, host: &mut embed_host::HostState| embed_host::host_ui(ui, host, None), host);
    h.run();
    h.get_by_label("Voxel value").click();
    h.run();
    h.state_mut().events.clear();
    click_at(&mut h, egui::pos2(900.0, 500.0));
    h.step();
    assert!(h.state().events.iter().any(|e| e.contains("PointPicked")), "{:?}", h.state().events);
    // the host's own buttons call use cases; their effect comes back as events
    h.state_mut().events.clear();
    h.get_by_label("Next slice").click();
    h.run();
    assert!(h.state().events.iter().any(|e| e.contains("SliceChanged")), "{:?}", h.state().events);
    h.state_mut().events.clear();
    h.get_by_label("Lung window").click();
    h.run();
    assert!(h.state().events.iter().any(|e| e.contains("WindowChanged")), "{:?}", h.state().events);
}

// ------------------------------------------------------------------ licence and badge (24.5)

// covers 24.5-a, 24.5-b
#[test]
fn the_embedded_ai_widget_shows_licence_and_badge() {
    use ferrum_domain::{
        EngineError, EngineInfo, InteractiveSession, Provenance, SegmentationEngine, Timestamp, VoxelBox,
    };

    /// The mock engine declared research-only, as a real engine's manifest says.
    struct ResearchMock(ferrum_engines::MockEngine);
    impl SegmentationEngine for ResearchMock {
        fn info(&self) -> Result<EngineInfo, EngineError> {
            let mut info = self.0.info()?;
            info.research_only = true;
            Ok(info)
        }
        fn open_session(&self, volume: &Volume, modality: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
            self.0.open_session(volume, modality)
        }
        fn unload(&self) -> Result<(), EngineError> {
            self.0.unload()
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let server = ferrum_engines::EngineServer::start(
        Arc::new(ResearchMock(ferrum_engines::MockEngine::default())),
        "127.0.0.1:0",
        None,
    )
    .unwrap();
    let mut host = Host::new(dir.path(), ViewerWidget::new(), true);
    host.panel.as_mut().unwrap().ai.url = server.url();
    let mut h = harness(host);
    h.run();
    h.get_by_label_contains("Connect").click();
    h.step();
    h.state_mut().viewer.wait_ai_idle();
    h.run();
    assert!(h.state().viewer.ai().status().is_connected(), "{:?}", h.state().viewer.ai().status());
    h.get_by_label_contains("Research use only");
    h.get_by_label_contains("MIT — region growing");
    // an engine's proposal keeps its label in the segments widget
    {
        let v = &mut h.state_mut().viewer;
        let label = v.add_segment("Tumour").unwrap();
        let bx = VoxelBox::new(glam::UVec3::new(8, 8, 6), glam::UVec3::new(16, 16, 12));
        v.apply_segment_mask(label, bx, &vec![1; bx.voxel_count()], false).unwrap();
        v.set_segment_provenance(label, Provenance::engine("nnInteractive", "2.6", true, Timestamp(1))).unwrap();
    }
    h.run();
    // the label shows in the segments list and in the review queue
    assert!(h.get_all_by_label_contains("Proposed by engine").next().is_some());
}

// ------------------------------------------------------------------ privacy

// covers the events privacy row of the validation plan
#[test]
fn events_hold_no_identifiers() {
    use ferrum_domain::{SliceAxis, WindowPreset};

    /// Strings that must never appear in an event (the agent privacy test's study).
    const SECRETS: [&str; 7] =
        ["DOE^JANE", "JANE", "MRN-4711", "19700101", "ACC-0815", "St Example Hospital", "1.2.826.0.1.3680043.8.498"];

    let dir = tempfile::tempdir().unwrap();
    write_ct(dir.path());
    let mut viewer = Viewer::new(Arc::new(ferrum_io::CompositeRepository::default()));
    viewer.open_paths(vec![dir.path().to_path_buf()]);
    viewer.wait_idle();
    let mut widget = ViewerWidget::new();
    let ctx = egui::Context::default();
    let mut all: Vec<ViewerEvent> = Vec::new();
    let mut frame_with = |viewer: &mut Viewer, all: &mut Vec<ViewerEvent>| {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            all.extend(widget.show(ui, viewer, None));
        });
        out.textures_delta.clear();
    };
    frame_with(&mut viewer, &mut all);
    // drive every event kind through real frames on the study with patient data
    viewer.set_slice_index(SliceAxis::Axial, 3);
    frame_with(&mut viewer, &mut all);
    viewer.apply_window_preset(WindowPreset::Lung);
    frame_with(&mut viewer, &mut all);
    viewer.set_view_mode(ViewMode::Volume3d);
    viewer.volume.camera.distance += 0.5;
    frame_with(&mut viewer, &mut all);
    viewer.set_view_mode(ViewMode::Slice2d);
    viewer.tool = ToolKind::Probe;
    viewer.slice_input(
        SliceAxis::Axial,
        ferrum_app::InputKind::Press,
        glam::Vec2::splat(4.0),
        glam::Vec2::splat(100.0),
    );
    frame_with(&mut viewer, &mut all);
    let label = viewer.add_segment("Mine").unwrap();
    let bx = ferrum_domain::VoxelBox::new(glam::UVec3::new(2, 2, 2), glam::UVec3::new(6, 6, 4));
    viewer.apply_segment_mask(label, bx, &vec![1; bx.voxel_count()], false).unwrap();
    frame_with(&mut viewer, &mut all);
    // every event kind, constructed too: the scan covers the whole surface
    all.extend([
        ViewerEvent::PointPicked { point: ferrum_ui::Point { voxel: [1, 2, 3], patient_mm: [1.0, 2.0, 3.0] } },
        ViewerEvent::RoiDrawn { item: ferrum_ui::RoiItem::Annotation(7) },
        ViewerEvent::RoiDrawn { item: ferrum_ui::RoiItem::Segment(1) },
        ViewerEvent::SliceChanged { indices: [1, 2, 3] },
        ViewerEvent::WindowChanged { center: 40.0, width: 400.0 },
        ViewerEvent::CameraChanged,
    ]);
    assert!(all.iter().any(|e| matches!(e, ViewerEvent::PointPicked { .. })), "the frames produced events: {all:?}");
    for event in &all {
        let text = format!("{event:?}");
        for secret in SECRETS {
            assert!(!text.contains(secret), "an event carries an identifier: {text}");
        }
    }
}

/// A 16 × 16 × 8 CT with patient data in the header, as the agent's privacy
/// test writes it: the study the events are scanned on.
fn write_ct(dir: &Path) {
    use dicom_core::{DataElement, PrimitiveValue, VR};
    use dicom_dictionary_std::{tags, uids};
    use dicom_object::{FileMetaTableBuilder, InMemDicomObject};

    const STUDY_UID: &str = "1.2.826.0.1.3680043.8.498.12345";
    const SERIES_UID: &str = "1.2.826.0.1.3680043.8.498.67890";
    for k in 0..8u32 {
        let pixels: Vec<u16> = (0..256u32)
            .map(|p| {
                let (i, j) = (p % 16, p / 16);
                if (5..11).contains(&i) && (5..11).contains(&j) && (2..6).contains(&k) {
                    1424
                } else {
                    24
                }
            })
            .collect();
        let text = |t, vr, v: &str| DataElement::new(t, vr, PrimitiveValue::from(v));
        let elements = vec![
            text(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
            text(tags::SOP_INSTANCE_UID, VR::UI, &format!("{SERIES_UID}.{}", k + 1)),
            text(tags::MODALITY, VR::CS, "CT"),
            text(tags::PATIENT_NAME, VR::PN, "DOE^JANE"),
            text(tags::PATIENT_ID, VR::LO, "MRN-4711"),
            text(tags::PATIENT_BIRTH_DATE, VR::DA, "19700101"),
            text(tags::ACCESSION_NUMBER, VR::SH, "ACC-0815"),
            text(tags::INSTITUTION_NAME, VR::LO, "St Example Hospital"),
            text(tags::STUDY_INSTANCE_UID, VR::UI, STUDY_UID),
            text(tags::SERIES_INSTANCE_UID, VR::UI, SERIES_UID),
            text(tags::STUDY_DATE, VR::DA, "20240428"),
            text(tags::INSTANCE_NUMBER, VR::IS, &(k + 1).to_string()),
            text(tags::IMAGE_POSITION_PATIENT, VR::DS, &format!("-8\\-8\\{}", f64::from(k) * 2.0)),
            text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "1\\0\\0\\0\\1\\0"),
            text(tags::PIXEL_SPACING, VR::DS, "1\\1"),
            text(tags::SLICE_THICKNESS, VR::DS, "2"),
            text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
            text(tags::RESCALE_SLOPE, VR::DS, "1"),
            text(tags::RESCALE_INTERCEPT, VR::DS, "-1024"),
            DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1u16)),
            DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(16u16)),
            DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(15u16)),
            DataElement::new(tags::PIXEL_REPRESENTATION, VR::US, PrimitiveValue::from(0u16)),
            DataElement::new(tags::PIXEL_DATA, VR::OW, PrimitiveValue::U16(pixels.into())),
        ];
        let mut obj = InMemDicomObject::from_element_iter(elements);
        obj.put(text(tags::STUDY_TIME, VR::TM, "101500"));
        let meta = FileMetaTableBuilder::new()
            .media_storage_sop_class_uid(uids::CT_IMAGE_STORAGE)
            .media_storage_sop_instance_uid(format!("{SERIES_UID}.{}", k + 1))
            .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN);
        obj.with_meta(meta).unwrap().write_to_file(dir.join(format!("ct{k}.dcm"))).unwrap();
    }
}
