//! The OCR check in `view slice` (Stage 31, criterion 31.3-b): without a
//! bridge the step is skipped and reported in the sidecar; with a bridge,
//! detected regions extend the mask when masking is requested; a failure is
//! reported — never silently passed.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use ferrum_agent::{Agent, AgentConfig};
use ferrum_domain::{Dims3, Volume};
use serde_json::{json, Value};

const COLS: u32 = 32;
const ROWS: u32 = 24;

/// A gradient phantom with a bright block at x 5..17, y 3..9 (the "text").
fn phantom() -> Volume {
    let dims = Dims3::new(COLS, ROWS, 3);
    let values: Vec<f32> = (0..dims.voxel_count())
        .map(|p| {
            let (x, y) = ((p % (COLS * ROWS) as usize) as u32 % COLS, (p % (COLS * ROWS) as usize) as u32 / COLS);
            if (5..17).contains(&x) && (3..9).contains(&y) {
                1000.0
            } else {
                ((x + 2 * y) % 256) as f32
            }
        })
        .collect();
    Volume::from_physical(dims, glam::Vec3::ONE, &values).unwrap()
}

/// A mock OCR bridge: `/v1/info` + `/v1/ocr` reporting one region, `n`
/// requests in total; returns its base URL.
fn mock_ocr(region: Value, n: usize) -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr());
    std::thread::spawn(move || {
        let json_header: tiny_http::Header = "Content-Type: application/json".parse().unwrap();
        for _ in 0..n {
            let Ok(mut req) = server.recv() else { return };
            let path = req.url().to_owned();
            let mut body = Vec::new();
            std::io::Read::read_to_end(req.as_reader(), &mut body).unwrap();
            let reply = if path.starts_with("/v1/info") {
                let info =
                    json!({ "protocol": "ferrum-engine/1", "name": "mock-ocr", "capabilities": { "ocr": true } });
                tiny_http::Response::from_string(info.to_string()).with_header(json_header.clone())
            } else {
                assert!(!body.is_empty(), "the render's PNG bytes reach the bridge");
                tiny_http::Response::from_string(json!({ "regions": [region] }).to_string())
                    .with_header(json_header.clone())
            };
            req.respond(reply).unwrap();
        }
    });
    url
}

struct Fixture {
    _dir: tempfile::TempDir,
    ws: String,
    agent: Agent,
}

fn fixture(ocr_bridge: Option<String>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    ferrum_io::write_nifti(&phantom(), &data.join("phantom.nii.gz")).unwrap();
    let ws = dir.path().join("ws").to_string_lossy().into_owned();
    let config = AgentConfig { ocr_bridge, ..AgentConfig::default() };
    let agent = Agent::new(config);
    let env = agent.run("study open", &json!({ "workspace": ws, "path": data }));
    assert_eq!(env["ok"], true, "study open: {env:#}");
    Fixture { _dir: dir, ws, agent }
}

impl Fixture {
    fn slice(&self, extra: Value) -> Value {
        let mut p = json!({
            "workspace": self.ws, "plane": "axial", "slice_number": 2,
            "window": "full_range", "size": COLS,
        });
        if let (Some(o), Value::Object(m)) = (p.as_object_mut(), extra) {
            o.extend(m);
        }
        let env = self.agent.run("view slice", &p);
        assert_eq!(env["ok"], true, "view slice: {env:#}");
        env["data"].clone()
    }
}

fn pixels(path: &str) -> Vec<u8> {
    image::open(path).unwrap().to_rgb8().into_raw()
}

#[test]
fn ocr_without_a_bridge_is_skipped_and_reported() {
    // covers 31.3-b
    let f = fixture(None);
    let base = f.slice(json!({}));
    let checked = f.slice(json!({ "ocr": true }));
    let ocr = &checked["ocr"];
    assert_eq!(ocr["status"], "skipped");
    assert!(ocr["reason"].as_str().unwrap().contains("ocr_bridge"));
    assert!(checked.get("mask").is_none(), "no masking was requested");
    assert_eq!(pixels(checked["image"].as_str().unwrap()), pixels(base["image"].as_str().unwrap()));
}

#[test]
fn detected_regions_extend_the_mask() {
    // covers 31.3-b: the bridge reports text regions; the mask covers them.
    // The render is 1:1, so the region (5,3,12,6) maps to the conservative
    // voxel box (4,2)-(17,9) on slice 1 (corners rounded outwards).
    let url = mock_ocr(json!({ "x": 5, "y": 3, "w": 12, "h": 6 }), 2);
    let f = fixture(Some(url));
    let base = f.slice(json!({}));
    let masked = f.slice(json!({ "ocr": true, "masked": true }));
    assert_eq!(masked["ocr"]["status"], "detected");
    assert_eq!(masked["ocr"]["regions"], json!([{ "x": 5, "y": 3, "w": 12, "h": 6 }]));
    let mask = &masked["mask"];
    assert_eq!(mask["applied"], true);
    assert_eq!(mask["source"], "ocr");
    assert_eq!(mask["boxes"], json!([{ "min": [4, 2, 1], "max": [17, 9, 2] }]));
    let (bpx, mpx) = (pixels(base["image"].as_str().unwrap()), pixels(masked["image"].as_str().unwrap()));
    for y in 0..ROWS {
        for x in 0..COLS {
            let i = ((y * COLS + x) * 3) as usize;
            if (4..17).contains(&x) && (2..9).contains(&y) {
                assert_eq!(mpx[i], 0, "the conservative OCR box is filled at ({x}, {y})");
            } else {
                assert_eq!(mpx[i..i + 3], bpx[i..i + 3], "pixel ({x}, {y}) outside the box is unchanged");
            }
        }
    }
}

#[test]
fn ocr_without_masked_reports_but_does_not_mask() {
    // covers 31.3-b
    let url = mock_ocr(json!({ "x": 5, "y": 3, "w": 12, "h": 6 }), 2);
    let f = fixture(Some(url));
    let base = f.slice(json!({}));
    let checked = f.slice(json!({ "ocr": true }));
    assert_eq!(checked["ocr"]["status"], "detected");
    assert!(checked.get("mask").is_none(), "masking was not requested");
    assert_eq!(pixels(checked["image"].as_str().unwrap()), pixels(base["image"].as_str().unwrap()));
}

#[test]
fn a_failed_bridge_is_reported_not_passed() {
    // covers 31.3-b
    let f = fixture(Some("http://127.0.0.1:1".into()));
    let checked = f.slice(json!({ "ocr": true, "masked": true }));
    assert_eq!(checked["ocr"]["status"], "failed");
    assert!(checked["ocr"]["error"].as_str().is_some());
    // nothing was detected, so nothing was masked — and that is reported
    assert_eq!(checked["mask"]["applied"], false);
    assert_eq!(checked["mask"]["source"], "none");
}
