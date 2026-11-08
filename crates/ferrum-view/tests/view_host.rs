//! The view host over HTTP: the page, state, frames, input, events and the
//! token (`docs/view-host.md`, criteria 25.1РІР‚-њ25.5).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use ferrum_domain::{
    LabelMap, NoProgress, Segment, SegmentationSet, SliceAxis, SliceImage, Volume, VolumeRepository, WindowLevel,
};
use ferrum_io::workspace::Workspace;
use ferrum_io::{write_nifti, CompositeRepository};
use ferrum_view::{BackendPreference, ServeOptions, Session, ViewServer};
use glam::Vec3;
use serde_json::Value;

/// A workspace with a synthetic study: a bright block of 400 HU at voxels
/// `x 4..8, y 3..7, z 2..6` on a `-1000` background, 16Р“-”12Р“-”8 at 1 mm.
struct Fixture {
    _dir: tempfile::TempDir,
    workspace: PathBuf,
    volume: Volume,
}

const BLOCK: (u32, u32, u32) = (4, 3, 2);
const BLOCK_SIZE: (u32, u32, u32) = (4, 4, 4);

fn phantom() -> Volume {
    let dims = ferrum_domain::Dims3::new(16, 12, 8);
    let mut values = Vec::new();
    for k in 0..dims.z {
        for j in 0..dims.y {
            for i in 0..dims.x {
                let inside = i >= BLOCK.0
                    && i < BLOCK.0 + BLOCK_SIZE.0
                    && j >= BLOCK.1
                    && j < BLOCK.1 + BLOCK_SIZE.1
                    && k >= BLOCK.2
                    && k < BLOCK.2 + BLOCK_SIZE.2;
                values.push(if inside { 400.0 } else { -1000.0 });
            }
        }
    }
    Volume::from_physical(dims, Vec3::ONE, &values).unwrap()
}

/// A label map of the block as segment 1.
fn block_segments(volume: &Volume) -> SegmentationSet {
    let dims = volume.dims();
    let mut data = vec![0u8; dims.voxel_count()];
    for k in BLOCK.2..BLOCK.2 + BLOCK_SIZE.2 {
        for j in BLOCK.1..BLOCK.1 + BLOCK_SIZE.1 {
            for i in BLOCK.0..BLOCK.0 + BLOCK_SIZE.0 {
                data[(i + j * dims.x + k * dims.x * dims.y) as usize] = 1;
            }
        }
    }
    let labels = LabelMap::from_data(dims, data).unwrap();
    let mut set = SegmentationSet::from_labels(labels, vec![Segment::new(1, "block")]);
    set.set_color(1, [230, 85, 75]).unwrap();
    set.set_opacity(1, 0.5).unwrap();
    set
}

/// Writes the phantom as NIfTI (its folder carries a name that must never
/// reach the host's documents) and opens a workspace over it.
fn fixture(with_segments: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let study_dir = dir.path().join("patient_Jane_Doe_1234");
    std::fs::create_dir_all(&study_dir).unwrap();
    let nifti = study_dir.join("phantom.nii.gz");
    let volume = phantom();
    write_nifti(&volume, &nifti).unwrap();
    let repo = CompositeRepository::default();
    let series = repo.scan(std::slice::from_ref(&nifti), &NoProgress).unwrap();
    let descriptor = series.first().expect("the phantom is one series").clone();
    let root = dir.path().join("ws");
    let mut workspace = Workspace::create(&root, &study_dir, &descriptor, "FERRUM test").unwrap();
    workspace.set_modality(Some("CT")).unwrap();
    if with_segments {
        workspace.save_segments(&block_segments(&volume), &volume, "FERRUM test").unwrap();
    }
    drop(workspace);
    Fixture { _dir: dir, workspace: root, volume }
}

fn open(f: &Fixture) -> ViewServer {
    open_with(f, BackendPreference::Cpu, ServeOptions::default())
}

fn open_with(f: &Fixture, backend: BackendPreference, options: ServeOptions) -> ViewServer {
    let session = Session::open(&f.workspace, backend).expect("the fixture workspace opens");
    ViewServer::start(session, options).expect("the host binds loopback")
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .into()
}

fn get(url: &str) -> (u16, Vec<u8>) {
    let mut response = agent().get(url).call().unwrap();
    let status = response.status().as_u16();
    (status, response.body_mut().read_to_vec().unwrap())
}

fn get_json(url: &str) -> (u16, Value) {
    let (status, body) = get(url);
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The `Access-Control-Allow-Origin` the answer carries, if any.
fn cors(url: &str) -> Option<String> {
    let response = agent().get(url).call().unwrap();
    response.headers().get("access-control-allow-origin").and_then(|v| v.to_str().ok()).map(str::to_owned)
}

fn post(url: &str, body: &str) -> (u16, Value) {
    let mut response = agent().post(url).header("Content-Type", "application/json").send(body).unwrap();
    let status = response.status().as_u16();
    let bytes = response.body_mut().read_to_vec().unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn token_url(server: &ViewServer, path: &str) -> String {
    format!("{}{}", server.url(), path_with_token(server, path))
}

fn path_with_token(server: &ViewServer, path: &str) -> String {
    let separator = if path.contains('?') { '&' } else { '?' };
    format!("{path}{separator}token={}", server.token())
}

fn slice_index(state: &Value) -> u64 {
    state["slice_index"].as_u64().expect("a state has a slice")
}

fn window_of(state: &Value) -> WindowLevel {
    WindowLevel::new(
        state["window"]["center"].as_f64().unwrap() as f32,
        state["window"]["width"].as_f64().unwrap() as f32,
    )
}

/// Decodes a PNG frame into RGBA pixels.
fn decode(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let image = image::load_from_memory(bytes).expect("a PNG frame").to_rgba8();
    (image.width(), image.height(), image.into_raw())
}

#[test]
fn the_page_is_self_contained_and_needs_the_token() {
    // covers 25.1-b, 25.4-a
    let f = fixture(false);
    let server = open(&f);
    let (status, _) = get(&format!("{}/", server.url()));
    assert_eq!(status, 401, "the page needs the token");
    let (status, body) = get(&token_url(&server, "/"));
    assert_eq!(status, 200);
    let page = String::from_utf8(body).unwrap();
    assert!(page.contains("<script>"), "the page runs inline JavaScript");
    for external in ["<script src", "<link ", "http://", "https://", "//cdn"] {
        assert!(!page.contains(external), "the page must not reference {external:?}");
    }
    assert!(page.contains("/api/frame.png") && page.contains("/api/input"), "the page drives the host");
    assert!(page.contains("ferrum-view/1") || page.contains("FERRUM"), "the page names FERRUM");
}

#[test]
fn a_frame_is_the_slice_the_oracle_draws() {
    // covers 25.2-a, 25.3-a, 25.3-c
    let f = fixture(false);
    let server = open(&f);
    assert_eq!(server.backend(), "cpu");
    let (status, state) = get_json(&token_url(&server, "/api/state"));
    assert_eq!(status, 200);
    assert_eq!(state["format"].as_str(), Some("ferrum-view/1"));
    assert_eq!(state["plane"].as_str(), Some("axial"));
    assert_eq!(state["slice_count"].as_u64().unwrap(), u64::from(f.volume.dims().z));
    let index = slice_index(&state) as u32;
    let window = window_of(&state);
    let (status, png) = get(&token_url(&server, "/api/frame.png"));
    assert_eq!(status, 200);
    let (width, height, pixels) = decode(&png);
    let (plane_w, plane_h) = SliceAxis::Axial.plane_dims(&f.volume);
    assert_eq!((width, height), (plane_w, plane_h), "one pixel per voxel");
    let slice = SliceImage::extract(&f.volume, SliceAxis::Axial, index).unwrap();
    let mut worst = 0i32;
    for (pixel, &stored) in pixels.chunks(4).zip(&slice.pixels) {
        let expected = ferrum_domain::color::unit_to_u8(window.apply(f.volume.range().from_storage(stored)));
        worst = worst.max((i32::from(pixel[0]) - i32::from(expected)).abs());
        assert_eq!(pixel[0], pixel[1], "the slice is grey");
        assert_eq!(pixel[3], 255, "the slice is opaque");
    }
    assert_eq!(worst, 0, "the CPU backend is the oracle itself");
}

#[test]
fn the_wheel_moves_one_slice_and_clamps() {
    // covers 25.2-c
    let f = fixture(false);
    let server = open(&f);
    let (_, state) = get_json(&token_url(&server, "/api/state"));
    let start = slice_index(&state);
    let (status, state) = post(&token_url(&server, "/api/input"), r#"{"step":1}"#);
    assert_eq!(status, 200);
    assert_eq!(slice_index(&state), start + 1);
    let (_, state) = post(&token_url(&server, "/api/input"), r#"{"step":-1}"#);
    assert_eq!(slice_index(&state), start);
    let (_, state) = post(&token_url(&server, "/api/input"), r#"{"slice":9999}"#);
    let last = f.volume.dims().z - 1;
    assert_eq!(slice_index(&state), u64::from(last), "the index is clamped");
    let (_, state) = post(&token_url(&server, "/api/input"), r#"{"slice":2}"#);
    assert_eq!(slice_index(&state), 2);
    let (_, state) = post(&token_url(&server, "/api/input"), r#"{"plane":"coronal"}"#);
    assert_eq!(state["plane"].as_str(), Some("coronal"));
    assert_eq!(state["slice_count"].as_u64().unwrap(), u64::from(f.volume.dims().y));
    let (status, body) = post(&token_url(&server, "/api/input"), r#"{"plane":"oblique"}"#);
    assert_eq!(status, 400, "{body}");
    let (status, _) = post(&token_url(&server, "/api/input"), r#"{"step":1,"slice":3}"#);
    assert_eq!(status, 400, "one input per request");
    let (status, _) = post(&token_url(&server, "/api/input"), r#"{"window":{"center":1}}"#);
    assert_eq!(status, 400);
}

#[test]
fn a_window_change_redraws_the_frame() {
    // covers 25.2-a, 25.2-c
    let f = fixture(false);
    let server = open(&f);
    let (_, before) = get(&token_url(&server, "/api/frame.png"));
    let (status, state) = post(&token_url(&server, "/api/input"), r#"{"window":{"center":400,"width":100}}"#);
    assert_eq!(status, 200);
    assert!((window_of(&state).center - 400.0).abs() < 1e-3);
    let (_, after) = get(&token_url(&server, "/api/frame.png"));
    assert_ne!(before, after, "a new window is a new frame");
    let (_, state) = post(&token_url(&server, "/api/input"), r#"{"preset":"lung"}"#);
    assert!(window_of(&state).width > 400.0, "the lung preset is wider than 400 HU");
}

#[test]
fn a_pick_gives_the_voxel_probe_gives() {
    // covers 25.2-d
    let f = fixture(false);
    let server = open(&f);
    let (_, state) = get_json(&token_url(&server, "/api/state"));
    let index = slice_index(&state);
    let (status, state) = post(&token_url(&server, "/api/input"), r#"{"probe":{"x":6,"y":5}}"#);
    assert_eq!(status, 200);
    let probe = &state["probe"];
    assert_eq!(
        probe["voxel"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect::<Vec<_>>(),
        vec![6, 5, index]
    );
    assert_eq!(probe["unit"].as_str(), Some("HU"));
    let value = f.volume.physical(6, 5, index as u32).unwrap();
    assert!((probe["value"].as_f64().unwrap() - f64::from(value)).abs() < 1e-3);
    let mm = f.volume.voxel_to_patient_f64(glam::DVec3::new(6.5, 5.5, index as f64 + 0.5));
    let got: Vec<f64> = probe["patient_mm"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    for (a, b) in got.iter().zip([mm.x, mm.y, mm.z]) {
        assert!((a - b).abs() < 1e-3, "patient millimetres: {got:?} vs {mm:?}");
    }
    let (_, state) = post(&token_url(&server, "/api/input"), r#"{"probe":{"x":9999,"y":0}}"#);
    assert!(state["probe"].is_null(), "a pick outside the image is refused");
}

#[test]
fn events_are_orderly_and_hold_no_identifiers() {
    // covers 25.4-c, 25.5-a
    let f = fixture(true);
    let server = open(&f);
    post(&token_url(&server, "/api/input"), r#"{"step":1}"#);
    post(&token_url(&server, "/api/input"), r#"{"probe":{"x":6,"y":5}}"#);
    post(&token_url(&server, "/api/input"), r#"{"segments":false}"#);
    let (status, events) = get_json(&token_url(&server, "/api/events?since=0"));
    assert_eq!(status, 200);
    let list = events["events"].as_array().unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(list[0]["kind"].as_str(), Some("slice_changed"));
    assert_eq!(list[1]["kind"].as_str(), Some("point_picked"));
    assert_eq!(list[1]["unit"].as_str(), Some("HU"));
    assert!(list[1]["voxel"].is_array() && list[1]["patient_mm"].is_array());
    assert_eq!(list[2]["kind"].as_str(), Some("segments_changed"));
    let sequences: Vec<u64> = list.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert!(sequences.windows(2).all(|w| w[1] > w[0]), "sequence numbers increase: {sequences:?}");
    let (_, after) = get_json(&token_url(&server, &format!("/api/events?since={}", sequences[1])));
    assert_eq!(after["events"].as_array().unwrap().len(), 1, "since skips earlier events");
    // Nothing a person's file is named after may reach the harness.
    let (_, state) = get_json(&token_url(&server, "/api/state"));
    let (_, page) = get(&token_url(&server, "/"));
    for key in ["study", "series", "uid", "path", "source", "description", "patient", "name"] {
        assert!(state.get(key).is_none(), "the state has a {key:?} field");
    }
    let workspace_path = f.workspace.to_string_lossy().into_owned();
    for text in [state.to_string(), events.to_string(), String::from_utf8(page).unwrap()] {
        for needle in ["Jane", "Doe", "1234", ".nii", ".nii.gz", workspace_path.as_str()] {
            assert!(!text.contains(needle), "{needle:?} must not appear in {text}");
        }
    }
}

#[test]
fn a_segment_is_drawn_with_a_closed_outline_on_every_side() {
    // covers 25.2-b (the CPU rule mirrors slice.wgsl, lesson L1)
    let f = fixture(true);
    let server = open(&f);
    let (_, state) = get_json(&token_url(&server, "/api/state"));
    assert_eq!(state["segments"]["shown"].as_bool(), Some(true));
    let items = state["segments"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"].as_str(), Some("block"));
    let index = slice_index(&state) as u32;
    assert!(index >= BLOCK.2 && index < BLOCK.2 + BLOCK_SIZE.2, "the middle slice holds the block");
    let window = window_of(&state);
    let (_, png) = get(&token_url(&server, "/api/frame.png"));
    let (width, _, pixels) = decode(&png);
    let at = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };
    let grey = |x: u32, y: u32| -> u8 {
        let v = f.volume.physical(x, y, index).unwrap();
        ferrum_domain::color::unit_to_u8(window.apply(v))
    };
    let colour = [230u8, 85, 75];
    // The block on this slice: x 4..8, y 3..7. Interior: fill at half opacity.
    let interior = at(5, 4);
    let expected = ferrum_domain::SegmentStyle::FillAndOutline.blend([grey(5, 4); 3], colour, 0.5, false);
    assert_eq!(&interior[..3], &expected, "the fill blends the segment colour");
    // All four sides of the block are outlined in full colour.
    for (x, y) in [(BLOCK.0, 5), (BLOCK.0 + BLOCK_SIZE.0 - 1, 5), (5, BLOCK.1), (5, BLOCK.1 + BLOCK_SIZE.1 - 1)] {
        let pixel = at(x, y);
        assert_eq!(&pixel[..3], &colour, "outline at ({x}, {y})");
    }
    // Outside is untouched.
    assert_eq!(at(0, 0)[0], grey(0, 0));
}

#[test]
fn a_workspace_change_appears_on_the_next_state() {
    // covers the host's reload of a workspace the agent changes (16.8)
    let f = fixture(false);
    let server = open(&f);
    let (_, state) = get_json(&token_url(&server, "/api/state"));
    assert_eq!(state["segments"]["items"].as_array().unwrap().len(), 0);
    let workspace = Workspace::open(&f.workspace).unwrap();
    workspace.save_segments(&block_segments(&f.volume), &f.volume, "FERRUM test").unwrap();
    drop(workspace);
    let (_, state) = get_json(&token_url(&server, "/api/state"));
    let items = state["segments"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "the segment the agent saved is shown");
    assert_eq!(items[0]["name"].as_str(), Some("block"));
}

#[test]
fn a_wrong_token_an_unknown_route_and_a_wrong_method_are_refused() {
    // covers 25.1-b
    let f = fixture(false);
    let server = open(&f);
    let (status, _) = get(&format!("{}/api/state?token=0000", server.url()));
    assert_eq!(status, 401);
    let (status, _) = get(&format!("{}/api/state", server.url()));
    assert_eq!(status, 401);
    let (status, _) = get(&token_url(&server, "/api/nothing"));
    assert_eq!(status, 404);
    let (status, _) = post(&token_url(&server, "/api/state"), "{}");
    assert_eq!(status, 405);
    // The token in a header works as well as in the query.
    let mut response = agent()
        .get(format!("{}/api/state", server.url()))
        .header("Authorization", format!("Bearer {}", server.token()))
        .call()
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body = response.body_mut().read_to_vec().unwrap();
    assert!(!body.is_empty());
}

#[test]
fn the_volume_crosses_the_wire_once_for_a_client_that_renders_it() {
    // covers 26.3-a: the header names the grid, the range and a hash, the body
    // is the voxels themselves, and both need the token
    let f = fixture(false);
    let server = open(&f);
    let (status, header) = get_json(&token_url(&server, "/api/volume.json"));
    assert_eq!(status, 200, "{header}");
    let dims = f.volume.dims();
    assert_eq!(header["format"].as_str(), Some("ferrum-volume/1"));
    assert_eq!(
        header["dims"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect::<Vec<_>>(),
        vec![u64::from(dims.x), u64::from(dims.y), u64::from(dims.z)]
    );
    let count = u64::from(dims.x) * u64::from(dims.y) * u64::from(dims.z);
    assert_eq!(header["bytes"].as_u64(), Some(count * 4), "f32 per voxel");
    assert_eq!(header["value_unit"].as_str(), Some("HU"));

    let (status, body) = get(&token_url(&server, "/api/volume"));
    assert_eq!(status, 200);
    assert_eq!(body.len() as u64, count * 4, "the body is the voxels the header describes");
    // The values are the volume's own, i fastest.
    let first = f32::from_le_bytes([body[0], body[1], body[2], body[3]]);
    assert!((first - f.volume.physical(0, 0, 0).unwrap()).abs() < 1e-3, "first voxel {first}");
    assert_eq!(header["sha256"].as_str().map(str::len), Some(64), "the header names a SHA-256");

    // The voxels need the token like every other data route.
    let (status, _) = get(&format!("{}/api/volume", server.url()));
    assert_eq!(status, 401);
}

#[test]
fn the_panel_module_is_public_code_while_the_data_is_not() {
    // covers 26.3-b: a browser imports the module without being able to send
    // the token on that request, so the module is served without one; the
    // study itself stays behind the token.
    let f = fixture(false);
    let server = open(&f);
    let (status, _) = get(&token_url(&server, "/api/state"));
    assert_eq!(status, 200);

    // The module route answers without a token, and only for the two files a
    // browser needs: the route cannot be turned into a file server.
    for path in ["/panel/ferrum_wasm.js", "/panel/ferrum_wasm_bg.wasm"] {
        let (status, _) = get(&format!("{}{path}", server.url()));
        assert!(status == 200 || status == 404, "{path} answered {status}: a missing file is 404, never 401");
    }
    let (status, _) = get(&format!("{}/panel/crate::session", server.url()));
    assert_eq!(status, 404, "a path outside the two module files is refused");

    // Every data route still refuses a request without the token.
    for path in ["/api/state", "/api/volume", "/api/volume.json", "/api/frame.png"] {
        let (status, _) = get(&format!("{}{path}", server.url()));
        assert_eq!(status, 401, "{path} answered {status} without a token");
    }
}

#[test]
fn the_panel_may_read_an_authorised_answer_and_nothing_else() {
    // A harness panel lives on another origin, so it needs the CORS header to
    // read the study — but only once it has the token. On a refusal the header
    // would let any page on this machine tell that a view host is running,
    // which no page needs to know.
    let f = fixture(false);
    let server = open(&f);

    assert_eq!(
        cors(&token_url(&server, "/api/state")).as_deref(),
        Some("*"),
        "an authorised answer cannot be read cross-origin"
    );
    assert_eq!(cors(&token_url(&server, "/api/volume.json")).as_deref(), Some("*"));

    assert_eq!(cors(&format!("{}/api/state", server.url())), None, "a 401 named the host to any page");
    assert_eq!(cors(&format!("{}/api/nothing", server.url())), None, "a 404 named the host to any page");
    let wrong = format!("{}/api/state?token={}", server.url(), "0".repeat(32));
    assert_eq!(cors(&wrong), None, "a wrong token still answered with the header");
}

#[test]
fn the_gpu_backend_draws_the_same_slice_as_the_cpu() {
    // covers 25.3-b
    let f = fixture(true);
    let cpu = Session::open(&f.workspace, BackendPreference::Cpu).expect("the CPU backend opens");
    let gpu = match Session::open(&f.workspace, BackendPreference::Gpu) {
        Ok(session) => session,
        Err(_) => return, // no adapter on this machine: the CPU test above covers the image
    };
    let mut cpu = cpu;
    let mut gpu = gpu;
    let (cpu_png, gpu_png) = (cpu.frame().unwrap().png.clone(), gpu.frame().unwrap().png.clone());
    let (w, h, cpu_px) = decode(&cpu_png);
    let (gw, gh, gpu_px) = decode(&gpu_png);
    assert_eq!((w, h), (gw, gh), "the same native size");
    let mut total = 0i64;
    let mut outliers = 0usize;
    for (a, b) in cpu_px.chunks(4).zip(gpu_px.chunks(4)) {
        let d = (i32::from(a[0]) - i32::from(b[0])).abs().max((i32::from(a[1]) - i32::from(b[1])).abs());
        total += i64::from(d);
        if d > 2 {
            outliers += 1;
        }
    }
    let mean = total as f64 / f64::from(w * h);
    assert!(mean < 2.5, "mean deviation {mean}");
    assert!(outliers * 100 <= (w * h) as usize * 2, "{outliers} pixels differ by more than 2");

    // The block's outline is drawn by both, on all four sides (the default
    // slice is the middle one, inside the block).
    for (x, y) in [(BLOCK.0, 5), (BLOCK.0 + BLOCK_SIZE.0 - 1, 5), (5, BLOCK.1), (5, BLOCK.1 + BLOCK_SIZE.1 - 1)] {
        let i = ((y * w + x) * 4) as usize;
        assert_eq!(&gpu_px[i..i + 3], &[230, 85, 75], "the GPU draws the outline at ({x}, {y})");
        assert_eq!(&cpu_px[i..i + 3], &[230, 85, 75], "the CPU draws the outline at ({x}, {y})");
    }
}
