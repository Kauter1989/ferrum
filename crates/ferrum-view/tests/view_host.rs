//! The view host over HTTP: the page, state, frames, input, events and the
//! token (`docs/view-host.md`, criteria 25.1РІР‚-њ25.5).
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use ferrum_domain::{
    Annotation, AnnotationReport, AnnotationSet, LabelMap, NoProgress, ResultStore, Segment, SegmentStyle,
    SegmentationSet, SliceAxis, SliceImage, SliceKey, StudyInfo, Volume, VolumeRepository, WindowLevel,
};
use ferrum_io::workspace::Workspace;
use ferrum_io::{write_nifti, CompositeRepository, WorkspaceStore};
use ferrum_view::{BackendPreference, ServeOptions, Session, ViewServer};
use glam::{Vec2, Vec3};
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
    // The frame sidecars (2D mapping, 3D projection, MPR quarters) too.
    post(&token_url(&server, "/api/input"), r#"{"component":"volume"}"#);
    let (_, vol_state) = get_json(&token_url(&server, "/api/state"));
    post(&token_url(&server, "/api/input"), r#"{"component":"mpr"}"#);
    let (_, mpr_state) = get_json(&token_url(&server, "/api/state"));
    let workspace_path = f.workspace.to_string_lossy().into_owned();
    let texts = [
        state.to_string(),
        events.to_string(),
        String::from_utf8(page).unwrap(),
        vol_state["frame"]["mapping"].to_string(),
        mpr_state["frame"]["mapping"].to_string(),
    ];
    for text in texts {
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

// covers 29.4-a — a live token rotates; the previous one stays valid for the
// grace window; a wrong token refreshes nothing.
#[test]
fn the_token_rotates_and_the_previous_stays_valid_for_the_grace() {
    let f = fixture(false);
    let server = open(&f);
    let old = server.token().to_owned();
    let (status, reply) = post(&token_url(&server, "/api/token/refresh"), "{}");
    assert_eq!(status, 200, "{reply}");
    let new = reply["token"].as_str().unwrap().to_owned();
    assert_ne!(new, old);
    assert!(reply["expires_in"].as_u64().unwrap() > 0);
    // the new token works
    let (status, _) = get(&format!("{}/api/state?token={new}", server.url()));
    assert_eq!(status, 200);
    // the old token still works within the grace window
    let (status, _) = get(&token_url(&server, "/api/state"));
    assert_eq!(status, 200);
    // a wrong token is refused, and refreshes nothing
    let (status, _) = get(&format!("{}/api/state?token=0000", server.url()));
    assert_eq!(status, 401);
    let (status, _) = post(&format!("{}/api/token/refresh?token=0000", server.url()), "{}");
    assert_eq!(status, 401);
    // refresh is POST-only
    let (status, _) = get(&token_url(&server, "/api/token/refresh"));
    assert_eq!(status, 405);
}

// covers 29.4-a — an expired token authorizes nothing (short TTL for the test).
#[test]
fn an_expired_token_is_refused() {
    let f = fixture(false);
    let options = ServeOptions {
        token_ttl: std::time::Duration::from_millis(600),
        token_grace: std::time::Duration::from_millis(200),
        ..Default::default()
    };
    let server = open_with(&f, BackendPreference::Cpu, options);
    let (status, _) = get(&token_url(&server, "/api/state"));
    assert_eq!(status, 200);
    std::thread::sleep(std::time::Duration::from_millis(900));
    let (status, _) = get(&token_url(&server, "/api/state"));
    assert_eq!(status, 401, "the token expired");
    // with no live token, a refresh is refused too
    let (status, _) = post(&token_url(&server, "/api/token/refresh"), "{}");
    assert_eq!(status, 401);
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
    assert_eq!(header["storage"].as_str(), Some("u16"));
    assert_eq!(header["bytes"].as_u64(), Some(count * 2), "u16 per voxel");
    assert_eq!(header["value_unit"].as_str(), Some("HU"));

    let (status, body) = get(&token_url(&server, "/api/volume"));
    assert_eq!(status, 200);
    assert_eq!(body.len() as u64, count * 2, "the body is the voxels the header describes");
    // The u16 storage reconstructs the volume's physical value, i fastest.
    let range = f.volume.range();
    let stored = u16::from_le_bytes([body[0], body[1]]);
    let first = range.min + f32::from(stored) * (range.max - range.min) / 65535.0;
    assert!((first - f.volume.physical(0, 0, 0).unwrap()).abs() < 1e-3, "first voxel {first}");
    // A non-zero voxel pins the byte order and the reconstruction formula (the
    // background voxel (0,0,0) is 0 and would survive a byte-order mutation).
    let block_idx = (BLOCK.0 + BLOCK.1 * dims.x + BLOCK.2 * dims.x * dims.y) as usize;
    let stored_block = u16::from_le_bytes([body[block_idx * 2], body[block_idx * 2 + 1]]);
    assert_ne!(stored_block, 0, "the block voxel is non-zero");
    let recon_block = range.min + f32::from(stored_block) * (range.max - range.min) / 65535.0;
    let want = f.volume.physical(BLOCK.0, BLOCK.1, BLOCK.2).unwrap();
    assert!((recon_block - want).abs() < 1e-3, "block voxel {recon_block} != {want}");
    assert_eq!(header["sha256"].as_str().map(str::len), Some(64), "the header names a SHA-256");

    // The original f32 form stays available for a client that asks for it.
    let (status, header) = get_json(&token_url(&server, "/api/volume.json?storage=f32"));
    assert_eq!(status, 200);
    assert_eq!(header["storage"].as_str(), Some("f32"));
    assert_eq!(header["bytes"].as_u64(), Some(count * 4));
    let (status, body) = get(&token_url(&server, "/api/volume?storage=f32"));
    assert_eq!(status, 200);
    assert_eq!(body.len() as u64, count * 4);
    let first = f32::from_le_bytes([body[0], body[1], body[2], body[3]]);
    assert!((first - f.volume.physical(0, 0, 0).unwrap()).abs() < 1e-3, "f32 first voxel {first}");
    let block_idx = (BLOCK.0 + BLOCK.1 * dims.x + BLOCK.2 * dims.x * dims.y) as usize;
    let block = f32::from_le_bytes([
        body[block_idx * 4],
        body[block_idx * 4 + 1],
        body[block_idx * 4 + 2],
        body[block_idx * 4 + 3],
    ]);
    let want = f.volume.physical(BLOCK.0, BLOCK.1, BLOCK.2).unwrap();
    assert!((block - want).abs() < 1e-3, "f32 block voxel {block} != {want}");

    // The voxels need the token like every other data route.
    let (status, _) = get(&format!("{}/api/volume", server.url()));
    assert_eq!(status, 401);
}

// covers 29.3-a / 29.2-a — the label map crosses as u8 with the segment metadata.
#[test]
fn the_label_map_crosses_the_wire_for_the_overlay() {
    let f = fixture(true);
    let server = open(&f);
    let dims = f.volume.dims();
    let count = u64::from(dims.x) * u64::from(dims.y) * u64::from(dims.z);
    let (status, header) = get_json(&token_url(&server, "/api/labels.json"));
    assert_eq!(status, 200, "{header}");
    assert_eq!(header["format"].as_str(), Some("ferrum-labels/1"));
    assert_eq!(header["bytes"].as_u64(), Some(count), "u8 per voxel");
    assert!(!header["segments"].as_array().unwrap().is_empty(), "the fixture has a segment");
    let (status, body) = get(&token_url(&server, "/api/labels"));
    assert_eq!(status, 200);
    assert_eq!(body.len() as u64, count);
    let idx = (BLOCK.0 + BLOCK.1 * dims.x + BLOCK.2 * dims.x * dims.y) as usize;
    assert_eq!(body[idx], 1, "the block voxel is labelled");
    // a study without segmentation gives a zero grid
    let empty = fixture(false);
    let server = open(&empty);
    let (status, header) = get_json(&token_url(&server, "/api/labels.json"));
    assert_eq!(status, 200);
    assert!(header["segments"].as_array().unwrap().is_empty());
    let (status, body) = get(&token_url(&server, "/api/labels"));
    assert_eq!(status, 200);
    assert_eq!(body.len() as u64, count);
    assert!(body.iter().all(|&b| b == 0), "a study without segmentation gives a zero grid");
    let (status, _) = get(&format!("{}/api/labels", server.url()));
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

// covers 28.2-a/b, 29.1-a — the host applies a written view artifact in full
// within a refresh (no unhonoured fields remain); the events are persisted
// for the agent; the person takes over and is not reverted by a later refresh.
#[test]
fn the_host_applies_a_view_artifact_and_the_person_takes_over() {
    let f = fixture(false);
    let mut session = Session::open(&f.workspace, BackendPreference::Cpu).unwrap();
    let ws = Workspace::open(&f.workspace).unwrap();
    let camera = ferrum_domain::OrbitCamera { distance: 1.2, ..Default::default() };
    let state = ferrum_domain::ViewState {
        component: ferrum_domain::ViewComponent::Volume,
        plane: SliceAxis::Coronal,
        slice_index: 5,
        window: WindowLevel::new(-600.0, 1500.0),
        camera: Some(camera),
        tool: ferrum_domain::ViewTool::Probe,
        selected_segment: Some(2),
        ..Default::default()
    };
    ws.save_view_state(&state, 1, "hash").unwrap();

    assert!(session.refresh(), "the artifact change is applied");
    let doc = session.state_json().unwrap();
    assert_eq!(doc["component"], "volume");
    assert_eq!(doc["plane"], "coronal");
    assert_eq!(doc["slice_index"], 5);
    assert_eq!(doc["window"]["center"], -600.0);
    assert_eq!(doc["tool"], "probe");
    assert_eq!(doc["selected_segment"], 2);
    assert!((doc["camera"]["distance"].as_f64().unwrap() - 1.2).abs() < 1e-6, "{doc}");
    assert!(doc.get("unhonoured").is_none(), "the host honours everything: {doc}");

    // the events were persisted, so an agent reads the same sequence
    let (events, next, _) = ws.load_view_events().unwrap();
    assert!(next >= 1);
    assert!(events.iter().any(|(_, e)| matches!(e, ferrum_domain::ViewEvent::PlaneChanged { .. })));
    assert!(events.iter().any(|(_, e)| matches!(e, ferrum_domain::ViewEvent::CameraChanged { .. })));

    // the person takes over: a slice input wins, and a later refresh (no new
    // artifact revision) does not revert it
    session.input(br#"{"slice": 2}"#).unwrap();
    assert_eq!(session.state_json().unwrap()["slice_index"], 2);
    session.refresh();
    assert_eq!(session.state_json().unwrap()["slice_index"], 2, "the person's input is not reverted");
}

/// The kinds of the events after `since`, in order.
fn kinds_since(server: &ViewServer, since: u64) -> Vec<String> {
    let (_, doc) = get_json(&token_url(server, &format!("/api/events?since={since}")));
    doc["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap().to_owned()).collect()
}

// covers 29.1-a — the state document carries the full ViewState and every
// new input sets its field; the answer is the new state.
#[test]
fn the_state_carries_the_full_view_state() {
    let f = fixture(true);
    let server = open(&f);
    let (_, state) = get_json(&token_url(&server, "/api/state"));
    assert_eq!(state["component"].as_str(), Some("slice"));
    assert_eq!(state["tool"].as_str(), Some("pan"));
    assert_eq!(state["overlays"]["segments_shown"].as_bool(), Some(true));
    assert!(state["overlays"]["fill_opacity"].as_f64().is_some());
    assert_eq!(state["overlays"]["style"].as_str(), Some("fill_outline"));
    assert!(state["selected_segment"].is_null());
    assert!(state["camera"].is_null(), "a slice state has no camera");
    assert!(state.get("unhonoured").is_none());
    assert_state_inputs(&server);

    let (_, state) = post(&token_url(&server, "/api/input"), r#"{"component":"mpr"}"#);
    assert_eq!(state["component"].as_str(), Some("mpr"));
    let camera = &state["camera"];
    assert_eq!(camera["orientation"].as_array().unwrap().len(), 4);
    assert!(camera["distance"].as_f64().unwrap() > 0.0);
    assert_eq!(camera["target"].as_array().unwrap().len(), 3);
    assert!(camera["fov_y"].as_f64().unwrap() > 0.0);
    let (status, _) = post(&token_url(&server, "/api/input"), r#"{"component":"3d"}"#);
    assert_eq!(status, 400);
}

/// The overlay, selection, tool and per-segment inputs set their fields and
/// leave the rest alone; the answer is the new state.
fn assert_state_inputs(server: &ViewServer) {
    let (status, state) =
        post(&token_url(server, "/api/input"), r#"{"overlays":{"fill_opacity":0.3,"style":"outline"}}"#);
    assert_eq!(status, 200, "{state}");
    assert!((state["overlays"]["fill_opacity"].as_f64().unwrap() - 0.3).abs() < 1e-6);
    assert_eq!(state["overlays"]["style"].as_str(), Some("outline"));
    assert_eq!(state["overlays"]["segments_shown"].as_bool(), Some(true), "untouched fields stay");

    let (_, state) = post(&token_url(server, "/api/input"), r#"{"selected_segment":1}"#);
    assert_eq!(state["selected_segment"].as_u64(), Some(1));
    let (_, state) = post(&token_url(server, "/api/input"), r#"{"selected_segment":null}"#);
    assert!(state["selected_segment"].is_null());
    let (_, state) = post(&token_url(server, "/api/input"), r#"{"tool":"probe"}"#);
    assert_eq!(state["tool"].as_str(), Some("probe"));

    assert_eq!(state["segments"]["items"][0]["color"], serde_json::json!([230, 85, 75]));
    let (_, state) = post(&token_url(server, "/api/input"), r#"{"segment":{"label":1,"visible":false}}"#);
    assert_eq!(state["segments"]["items"][0]["visible"].as_bool(), Some(false));
    let (_, state) = post(&token_url(server, "/api/input"), r#"{"segment":{"label":1,"opacity":0.2}}"#);
    assert!((state["segments"]["items"][0]["opacity"].as_f64().unwrap() - 0.2).abs() < 1e-6);
    let (status, _) = post(&token_url(server, "/api/input"), r#"{"segment":{"label":9,"visible":true}}"#);
    assert_eq!(status, 400, "an unknown label is refused");
}

// covers 29.1-a — camera_changed and roi_drawn reach /api/events and the
// persisted log; the existing kinds stay.
#[test]
fn camera_and_roi_events_reach_the_event_log() {
    let f = fixture(false);
    let server = open(&f);
    post(&token_url(&server, "/api/input"), r#"{"step":1}"#);
    post(&token_url(&server, "/api/input"), r#"{"component":"volume"}"#);
    let (_, before) = get_json(&token_url(&server, "/api/state"));
    let (status, after) = post(&token_url(&server, "/api/input"), r#"{"camera":{"orbit":[0.4,0.2]}}"#);
    assert_eq!(status, 200, "{after}");
    assert_ne!(after["camera"]["orientation"], before["camera"]["orientation"], "the orbit moved the camera");
    let (_, zoomed) = post(&token_url(&server, "/api/input"), r#"{"camera":{"zoom":1.0}}"#);
    assert!(
        zoomed["camera"]["distance"].as_f64().unwrap() < after["camera"]["distance"].as_f64().unwrap(),
        "a positive zoom moves the eye closer"
    );
    let (_, targeted) = post(&token_url(&server, "/api/input"), r#"{"camera":{"target":[0.1,0.0,0.0]}}"#);
    assert!((targeted["camera"]["target"][0].as_f64().unwrap() - 0.1).abs() < 1e-6);

    post(&token_url(&server, "/api/input"), r#"{"component":"slice"}"#);
    let (status, _) = post(&token_url(&server, "/api/input"), r#"{"draw":{"tool":"distance","points":[[5,4],[6,5]]}}"#);
    assert_eq!(status, 200);

    let kinds = kinds_since(&server, 0);
    assert_eq!(kinds[0], "slice_changed", "the existing kinds stay: {kinds:?}");
    assert_eq!(kinds.iter().filter(|k| *k == "camera_changed").count(), 3, "{kinds:?}");
    assert_eq!(kinds.last().unwrap(), "roi_drawn", "{kinds:?}");
    let (_, doc) = get_json(&token_url(&server, "/api/events?since=0"));
    let roi = doc["events"].as_array().unwrap().last().unwrap();
    assert_eq!(roi["item"]["kind"].as_str(), Some("annotation"));
    assert!(roi["item"]["id"].as_u64().is_some(), "the event names the annotation's id");
    let cam = doc["events"].as_array().unwrap().iter().find(|e| e["kind"] == "camera_changed").unwrap();
    assert_eq!(cam["camera"]["orientation"].as_array().unwrap().len(), 4);

    // the agent reads the same sequence from the workspace
    let ws = Workspace::open(&f.workspace).unwrap();
    let (events, _, _) = ws.load_view_events().unwrap();
    assert!(events.iter().any(|(_, e)| matches!(e, ferrum_domain::ViewEvent::CameraChanged { .. })));
    assert!(events.iter().any(|(_, e)| matches!(e, ferrum_domain::ViewEvent::RoiDrawn { .. })));
}

// covers 29.1-a — a draw input drives the real tool machine: the annotation
// is committed, saved to the workspace and reported; a degenerate draw is not.
#[test]
fn a_draw_commits_an_annotation_through_the_tool_machine() {
    let f = fixture(false);
    let server = open(&f);
    let (_, state) = get_json(&token_url(&server, "/api/state"));
    let index = slice_index(&state);
    assert!(
        index >= u64::from(BLOCK.2) && index < u64::from(BLOCK.2 + BLOCK_SIZE.2),
        "the middle slice holds the block"
    );

    let (status, state) =
        post(&token_url(&server, "/api/input"), r#"{"draw":{"tool":"distance","points":[[5,4],[6,5]]}}"#);
    assert_eq!(status, 200, "{state}");
    assert_eq!(state["tool"].as_str(), Some("distance"));
    let kinds = kinds_since(&server, 0);
    assert_eq!(kinds, vec!["roi_drawn"], "{kinds:?}");

    // the annotation is in the workspace, on this slice, with the clicked mm
    let store = WorkspaceStore::open(&f.workspace, "FERRUM test").unwrap();
    let (annotations, _) = store.load(&f.volume).unwrap();
    let annotations = annotations.expect("the draw was saved");
    let drawn: Vec<_> = annotations.iter().collect();
    assert_eq!(drawn.len(), 1);
    let (id, key, annotation, _) = drawn[0];
    assert_eq!(key.index, index as u32);
    let ferrum_domain::Annotation::Distance { a, b } = annotation else { panic!("{annotation:?}") };
    assert!((a.x - 5.5).abs() < 1e-4 && (a.y - 4.5).abs() < 1e-4, "{a:?}");
    assert!((b.x - 6.5).abs() < 1e-4 && (b.y - 5.5).abs() < 1e-4, "{b:?}");
    // the event names exactly the annotation that was saved
    let (_, doc) = get_json(&token_url(&server, "/api/events?since=0"));
    let roi_id = doc["events"][0]["item"]["id"].as_u64().unwrap();
    assert_eq!(id, roi_id);

    // rect, angle and area commit through the same machine
    for body in [
        r#"{"draw":{"tool":"rect","points":[[4,3],[7,6]]}}"#,
        r#"{"draw":{"tool":"angle","points":[[4,4],[6,4],[6,6]]}}"#,
        r#"{"draw":{"tool":"area","points":[[4,4],[7,4],[7,6],[4,6]]}}"#,
    ] {
        let (status, _) = post(&token_url(&server, "/api/input"), body);
        assert_eq!(status, 200, "{body}");
    }
    assert_eq!(kinds_since(&server, 1), vec!["roi_drawn", "roi_drawn", "roi_drawn"]);

    // a degenerate distance (below the tool's minimum) commits nothing
    let (_, doc) = get_json(&token_url(&server, "/api/events?since=0"));
    let seq = doc["next"].as_u64().unwrap();
    let (status, _) = post(&token_url(&server, "/api/input"), r#"{"draw":{"tool":"distance","points":[[5,4],[5,4]]}}"#);
    assert_eq!(status, 200);
    assert!(kinds_since(&server, seq).is_empty(), "a zero-length distance is not an annotation");
    // bad draws are refused without a state change
    let (status, _) = post(&token_url(&server, "/api/input"), r#"{"draw":{"tool":"pan","points":[[0,0],[1,1]]}}"#);
    assert_eq!(status, 400);
    let (status, _) =
        post(&token_url(&server, "/api/input"), r#"{"draw":{"tool":"distance","points":[[99,99],[100,100]]}}"#);
    assert_eq!(status, 400, "a pixel outside the slice is refused");
}

// covers 29.2-c — the panel reads the measured values from /api/annotations:
// the host's tool machine computed them, the document names value/unit/label
// and the in-plane millimetre points, and holds no identifiers.
#[test]
fn the_annotations_endpoint_serves_the_measured_values() {
    let f = fixture(false);
    let server = open(&f);
    let (status, doc) = get_json(&token_url(&server, "/api/annotations"));
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["format"].as_str(), Some("ferrum-view/1"));
    assert_eq!(doc["annotations"].as_array().unwrap().len(), 0, "a fresh study has no measurements");

    let (_, state) = get_json(&token_url(&server, "/api/state"));
    let index = slice_index(&state);
    // distance (5,4)-(6,5) at 1 mm: the pixel centres are √2 mm apart
    let (status, _) = post(&token_url(&server, "/api/input"), r#"{"draw":{"tool":"distance","points":[[5,4],[6,5]]}}"#);
    assert_eq!(status, 200);
    let (_, doc) = get_json(&token_url(&server, "/api/annotations"));
    let items = doc["annotations"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let a = &items[0];
    assert_eq!(a["kind"].as_str(), Some("Distance"));
    assert_eq!(a["plane"].as_str(), Some("axial"));
    assert_eq!(a["slice_index"].as_u64(), Some(index));
    assert_eq!(a["unit"].as_str(), Some("mm"));
    let value = a["value"].as_f64().unwrap();
    assert!((value - 2.0f64.sqrt()).abs() < 1e-3, "the host measured √2 mm, got {value}");
    assert_eq!(a["label"].as_str(), Some("1.4 mm"));
    assert_eq!(a["points_mm"][0][0].as_f64(), Some(5.5));
    assert_eq!(a["points_mm"][1][1].as_f64(), Some(5.5));
    assert!(a["id"].as_u64().is_some(), "the event's id resolves to this item");
    assert_eq!(a["name"].as_str(), Some("Distance 1"));

    // angle (90°), rect and area carry their own units and values. The area
    // polygon is larger than the tool's 6 mm close tolerance: a vertex nearer
    // the first one would close the polygon early (the tool machine's rule).
    for (body, kind, unit, expected) in [
        (r#"{"draw":{"tool":"angle","points":[[4,4],[6,4],[6,6]]}}"#, "Angle", "deg", 90.0),
        (r#"{"draw":{"tool":"rect","points":[[4,3],[7,6]]}}"#, "Rectangle", "mm2", 9.0),
        (r#"{"draw":{"tool":"area","points":[[2,2],[12,2],[12,9],[2,9]]}}"#, "Area", "mm2", 70.0),
    ] {
        let (status, _) = post(&token_url(&server, "/api/input"), body);
        assert_eq!(status, 200, "{body}");
        let (_, doc) = get_json(&token_url(&server, "/api/annotations"));
        let last = doc["annotations"].as_array().unwrap().last().unwrap();
        assert_eq!(last["kind"].as_str(), Some(kind), "{body}");
        assert_eq!(last["unit"].as_str(), Some(unit), "{body}");
        assert!((last["value"].as_f64().unwrap() - expected).abs() < 1e-3, "{body}: {last}");
    }

    // numbers and labels only: the study's name never appears (25.5-a)
    let (_, doc) = get_json(&token_url(&server, "/api/annotations"));
    let text = doc.to_string();
    assert!(!text.contains("Jane_Doe") && !text.contains("patient_") && !text.contains("phantom"), "{text}");

    // the token guards the document; a wrong method is refused
    let (status, _) = get_json(&format!("{}/api/annotations", server.url()));
    assert_eq!(status, 401);
    let (status, _) = post(&token_url(&server, "/api/annotations"), "{}");
    assert_eq!(status, 405);
}

// covers 29.2-c — annotations an agent writes into the workspace reach the
// endpoint through the host's reload, the way segments do.
#[test]
fn agent_written_annotations_reach_the_annotations_endpoint() {
    let f = fixture(false);
    let server = open(&f);
    let workspace = Workspace::open(&f.workspace).unwrap();
    let mut set = AnnotationSet::default();
    set.add(SliceKey::new(SliceAxis::Axial, 4), Annotation::Distance { a: Vec2::ZERO, b: Vec2::new(3.0, 4.0) });
    let report = AnnotationReport::build(f.workspace.join("study"), StudyInfo::default(), &f.volume, &set);
    workspace.save_annotations(&report, "FERRUM test").unwrap();
    drop(workspace);
    let (status, doc) = get_json(&token_url(&server, "/api/annotations"));
    assert_eq!(status, 200);
    let items = doc["annotations"].as_array().unwrap();
    assert_eq!(items.len(), 1, "the agent's measurement is served: {doc}");
    assert!((items[0]["value"].as_f64().unwrap() - 5.0).abs() < 1e-3, "a 3-4-5 distance measures 5 mm");
    assert_eq!(items[0]["slice_index"].as_u64(), Some(4));
    assert!(!doc.to_string().contains("patient_"), "the source path never crosses the wire");
}

// covers 29.1-a — the volume frame is the 3D view at the fixed size, its
// sidecar a projection (camera + pick-ray), and a probe resolves through it.
#[test]
fn the_volume_frame_is_a_projection_of_the_camera() {
    let f = fixture(false);
    let server = open(&f);
    post(&token_url(&server, "/api/input"), r#"{"component":"volume"}"#);
    let (status, png) = get(&token_url(&server, "/api/frame.png"));
    assert_eq!(status, 200);
    let (w, h, pixels) = decode(&png);
    assert_eq!((w, h), (512, 512), "the 3D frame has the fixed size");
    let lit = pixels.chunks(4).filter(|p| p[0] > 10 || p[1] > 10 || p[2] > 10).count();
    assert!(lit > 1000, "the block is drawn ({lit} lit pixels)");
    assert!(lit < (w * h / 2) as usize, "the block is not the whole frame");
    assert!(pixels.chunks(4).all(|p| p[3] == 255), "the frame is opaque");

    let (_, state) = get_json(&token_url(&server, "/api/state"));
    let mapping = &state["frame"]["mapping"];
    assert_eq!(mapping["kind"].as_str(), Some("projection"));
    assert_eq!(mapping["size"], serde_json::json!([512, 512]));
    assert_eq!(mapping["camera"]["distance"], state["camera"]["distance"]);
    let ray = &mapping["pick_ray"];
    assert_eq!(ray["origin"].as_array().unwrap().len(), 3);
    assert!(ray["origin"][2].as_f64().unwrap() < -1.0, "the default eye sits at -z: {ray}");
    assert_eq!(ray["direction"].as_array().unwrap().len(), 3);
    assert!(mapping.get("pick").is_some(), "the sidecar names the centre pick (or null)");
    assert!(mapping.get("pixel_to_voxel").is_none(), "a 3D frame has no 2D affine");

    // a pixel on the block resolves through the camera to the block's value
    let (status, state) = post(&token_url(&server, "/api/input"), r#"{"probe":{"x":240,"y":240}}"#);
    assert_eq!(status, 200, "{state}");
    let probe = &state["probe"];
    assert!((probe["value"].as_f64().unwrap() - 400.0).abs() < 1e-3, "{probe}");
    let voxel: Vec<u64> = probe["voxel"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert!((4..8).contains(&voxel[0]) && (3..7).contains(&voxel[1]) && (2..6).contains(&voxel[2]), "{voxel:?}");
    // a ray that misses the volume is refused
    let (status, _) = post(&token_url(&server, "/api/input"), r#"{"probe":{"x":5,"y":5}}"#);
    assert_eq!(status, 400);

    // an orbit redraws the frame with the new camera
    let (_, before) = get(&token_url(&server, "/api/frame.png"));
    post(&token_url(&server, "/api/input"), r#"{"camera":{"orbit":[0.6,0.3]}}"#);
    let (_, after) = get(&token_url(&server, "/api/frame.png"));
    assert_ne!(before, after, "a new camera is a new frame");
}

// covers 29.1-a — the MPR frame tiles the four quarters like the desktop
// grid; the slice quarters keep the oracle's pixels at exact positions, the
// 3D quarter is drawn, and the sidecar names every quarter's mapping.
#[test]
fn the_mpr_frame_tiles_the_four_quarters() {
    let f = fixture(true);
    let server = open(&f);
    // A window that lifts the background off black, so the letterbox is
    // distinguishable from the image background (a check that can fail).
    post(&token_url(&server, "/api/input"), r#"{"window":{"center":-600,"width":1500}}"#);
    post(&token_url(&server, "/api/input"), r#"{"component":"mpr"}"#);
    let (_, state) = get_json(&token_url(&server, "/api/state"));
    let window = window_of(&state);
    let (status, png) = get(&token_url(&server, "/api/frame.png"));
    assert_eq!(status, 200);
    let (w, h, pixels) = decode(&png);
    assert_eq!((w, h), (512, 512));
    let at = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };
    let grey = |v: f32| ferrum_domain::color::unit_to_u8(window.apply(v));
    assert!(grey(-1000.0) > 0, "the background is not black under this window");
    assert_mpr_quarter_pixels(&at, &grey);

    // The sidecar names the quarters and their mappings.
    let (_, state) = get_json(&token_url(&server, "/api/state"));
    assert_mpr_mapping(&state["frame"]["mapping"]);

    // A probe resolves per quarter: axial block, sagittal background, 3D block.
    let (_, state) = post(&token_url(&server, "/api/input"), r#"{"probe":{"x":88,"y":104}}"#);
    assert!((state["probe"]["value"].as_f64().unwrap() - 400.0).abs() < 1e-3, "{state}");
    let (_, state) = post(&token_url(&server, "/api/input"), r#"{"probe":{"x":373,"y":394}}"#);
    assert!((state["probe"]["value"].as_f64().unwrap() + 1000.0).abs() < 1e-3, "{state}");
    let (_, state) = post(&token_url(&server, "/api/input"), r#"{"probe":{"x":370,"y":120}}"#);
    assert!((state["probe"]["value"].as_f64().unwrap() - 400.0).abs() < 1e-3, "{state}");
    let (status, _) = post(&token_url(&server, "/api/input"), r#"{"probe":{"x":8,"y":8}}"#);
    assert_eq!(status, 400, "the letterbox holds no voxel");
}

/// The exact geometry of the MPR quarters on the block phantom (lesson L1:
/// outlines on every side, at known pixels, not "some outline exists").
fn assert_mpr_quarter_pixels(at: &dyn Fn(u32, u32) -> [u8; 4], grey: &dyn Fn(f32) -> u8) {
    // Axial top-left: the 16x12 plane fits at scale 16 into [0,32]-[256,224].
    // The block (x 4..8, y 3..7) lands at x 64..128, y 80..144: outline in
    // full colour on every side, fill blended inside.
    let colour = [230u8, 85, 75];
    for (x, y) in [(64, 104), (127, 104), (88, 80), (88, 143)] {
        assert_eq!(&at(x, y)[..3], &colour, "axial outline at ({x}, {y})");
    }
    let g = grey(400.0);
    let expected = SegmentStyle::FillAndOutline.blend([g; 3], colour, 0.5, false);
    assert_eq!(&at(88, 104)[..3], &expected, "axial fill blends the segment colour");
    assert_eq!(at(8, 8), [0, 0, 0, 255], "the letterbox above the image is opaque black");

    // Coronal bottom-left: the 16x8 plane at scale 16 into [0,320]-[256,448];
    // the block (x 4..8, z 2..6) at y 6 is cut by this plane: outline at x 64.
    assert_eq!(&at(64, 392)[..3], &colour, "coronal outline");
    // Sagittal bottom-right: the 12x8 plane at scale 21 into [258,300]-[510,468];
    // the active sagittal index is x=8, outside the block: pure background.
    assert_eq!(at(373, 394)[0], grey(-1000.0), "sagittal at x=8 shows no block");
    // 3D top-right: the block is drawn somewhere in the quarter.
    let lit = (256..512)
        .flat_map(|x| (0..256).map(move |y| (x, y)))
        .filter(|&(x, y)| at(x, y)[0] > 10 || at(x, y)[1] > 10 || at(x, y)[2] > 10)
        .count();
    assert!(lit > 200, "the 3D quarter draws the block ({lit} lit pixels)");
}

/// The MPR sidecar: four quarters, each with its rect and its own mapping.
fn assert_mpr_mapping(mapping: &Value) {
    assert_eq!(mapping["kind"].as_str(), Some("mpr"));
    let quarters = mapping["quarters"].as_array().unwrap();
    assert_eq!(quarters.len(), 4);
    assert_eq!(quarters[0]["component"], "slice");
    assert_eq!(quarters[0]["plane"], "axial");
    assert_eq!(quarters[0]["rect"], serde_json::json!([0, 0, 256, 256]));
    assert_eq!(quarters[0]["image_rect"], serde_json::json!([0, 32, 256, 224]));
    assert_eq!(quarters[0]["mapping"]["pixel_to_voxel"][2][2], 4.0, "the axial mapping names the slice");
    assert_eq!(quarters[1]["component"], "volume");
    assert_eq!(quarters[1]["rect"], serde_json::json!([256, 0, 512, 256]));
    assert_eq!(quarters[1]["mapping"]["kind"], "projection");
    assert_eq!(quarters[2]["plane"], "coronal");
    assert_eq!(quarters[3]["plane"], "sagittal");
}

/// Mean deviation and outlier fraction of two same-size frames, the pattern
/// of the slice parity case above.
fn deviation(a: &[u8], b: &[u8]) -> (f64, usize) {
    let mut total = 0i64;
    let mut outliers = 0usize;
    for (x, y) in a.chunks(4).zip(b.chunks(4)) {
        let d = (0..3).map(|c| (i32::from(x[c]) - i32::from(y[c])).abs()).max().unwrap();
        total += i64::from(d);
        if d > 2 {
            outliers += 1;
        }
    }
    (total as f64 / (a.len() / 4) as f64, outliers)
}

// covers 29.1-b — the GPU and CPU draw the same 3D frame (the rendering rule:
// volume.wgsl <-> cpu/raycast.rs, one FrameParams, jitter off on both sides).
#[test]
fn the_gpu_backend_draws_the_same_volume_as_the_cpu() {
    let f = fixture(false);
    let mut cpu = Session::open(&f.workspace, BackendPreference::Cpu).unwrap();
    let mut gpu = match Session::open(&f.workspace, BackendPreference::Gpu) {
        Ok(session) => session,
        Err(_) => return, // no adapter on this machine: the CPU cases above cover the image
    };
    cpu.input(br#"{"component":"volume"}"#).unwrap();
    gpu.input(br#"{"component":"volume"}"#).unwrap();
    let (cw, ch, cpu_px) = decode(&cpu.frame().unwrap().png);
    let (gw, gh, gpu_px) = decode(&gpu.frame().unwrap().png);
    assert_eq!((cw, ch), (gw, gh));
    assert_eq!((cw, ch), (512, 512));
    let (mean, outliers) = deviation(&cpu_px, &gpu_px);
    assert!(mean < 2.5, "mean deviation {mean}");
    assert!(outliers * 100 <= (cw * ch) as usize * 2, "{outliers} pixels differ by more than 2");
    for px in [&cpu_px, &gpu_px] {
        let lit = px.chunks(4).filter(|p| p[0] > 10 || p[1] > 10 || p[2] > 10).count();
        assert!(lit > 1000, "both backends draw the block ({lit} lit pixels)");
    }
}

// covers 29.1-b — the GPU and CPU draw the same MPR composite: every quarter
// through the shared renderers, tiled by the one compositor.
#[test]
fn the_gpu_backend_draws_the_same_mpr_as_the_cpu() {
    let f = fixture(true);
    let mut cpu = Session::open(&f.workspace, BackendPreference::Cpu).unwrap();
    let mut gpu = match Session::open(&f.workspace, BackendPreference::Gpu) {
        Ok(session) => session,
        Err(_) => return, // no adapter on this machine: the CPU case above covers the image
    };
    cpu.input(br#"{"component":"mpr"}"#).unwrap();
    gpu.input(br#"{"component":"mpr"}"#).unwrap();
    let (cw, ch, cpu_px) = decode(&cpu.frame().unwrap().png);
    let (gw, gh, gpu_px) = decode(&gpu.frame().unwrap().png);
    assert_eq!((cw, ch), (gw, gh));
    assert_eq!((cw, ch), (512, 512));
    let (mean, outliers) = deviation(&cpu_px, &gpu_px);
    assert!(mean < 2.5, "mean deviation {mean}");
    assert!(outliers * 100 <= (cw * ch) as usize * 2, "{outliers} pixels differ by more than 2");

    // The axial quarter's outline is exact on both backends, on every side.
    let at = |px: &[u8], x: u32, y: u32| -> [u8; 3] {
        let i = ((y * cw + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2]]
    };
    for (x, y) in [(64, 104), (127, 104), (88, 80), (88, 143)] {
        assert_eq!(at(&gpu_px, x, y), [230, 85, 75], "GPU axial outline at ({x}, {y})");
        assert_eq!(at(&cpu_px, x, y), [230, 85, 75], "CPU axial outline at ({x}, {y})");
    }
}
