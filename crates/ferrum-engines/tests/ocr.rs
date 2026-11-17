//! The OCR bridge kind (Stage 31, criterion 31.3-b): the optional burned-in
//! text check behind `ferrum-engine/1`. Without a bridge the step is skipped
//! and reported; with one, detected regions come back; a wrong protocol, a
//! missing capability, an unreachable bridge or a bad answer is reported as
//! failed — never silently passed.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use ferrum_engines::ocr::{check, OcrBridge, OcrOutcome, OcrRegion};
use ferrum_engines::HttpConfig;
use serde_json::{json, Value};

/// A mock OCR bridge answering `/v1/info` and `/v1/ocr`, `n` requests in
/// total; returns its base URL.
fn mock(info: Value, answer: Value, n: usize) -> String {
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
                tiny_http::Response::from_string(info.to_string()).with_header(json_header.clone())
            } else if path.starts_with("/v1/ocr") {
                assert!(!body.is_empty(), "the check sends the image bytes");
                tiny_http::Response::from_string(answer.to_string()).with_header(json_header.clone())
            } else {
                tiny_http::Response::from_string("{}").with_status_code(404)
            };
            req.respond(reply).unwrap();
        }
    });
    url
}

fn ocr_info() -> Value {
    json!({ "protocol": "ferrum-engine/1", "name": "mock-ocr", "capabilities": { "ocr": true } })
}

#[test]
fn without_a_bridge_the_check_is_skipped_and_reported() {
    // covers 31.3-b
    let outcome = check(None, b"not-really-a-png");
    assert!(matches!(outcome, OcrOutcome::Skipped { .. }));
    let report = outcome.json();
    assert_eq!(report["status"], "skipped");
    assert!(report["reason"].as_str().unwrap().contains("ocr_bridge"));
    assert!(outcome.regions().is_empty());
}

#[test]
fn a_bridge_reports_detected_regions() {
    // covers 31.3-b
    let url = mock(ocr_info(), json!({ "regions": [{ "x": 5, "y": 3, "w": 12, "h": 6 }] }), 2);
    let bridge = OcrBridge::new(HttpConfig::new(url));
    let outcome = check(Some(&bridge), b"png-bytes");
    assert_eq!(outcome.regions(), &[OcrRegion { x: 5, y: 3, w: 12, h: 6 }]);
    let report = outcome.json();
    assert_eq!(report["status"], "detected");
    assert_eq!(report["regions"], json!([{ "x": 5, "y": 3, "w": 12, "h": 6 }]));
}

#[test]
fn no_detected_text_is_an_empty_region_list() {
    // covers 31.3-b: "detected nothing" is a real answer, not a skip.
    let url = mock(ocr_info(), json!({ "regions": [] }), 2);
    let bridge = OcrBridge::new(HttpConfig::new(url));
    let outcome = check(Some(&bridge), b"png-bytes");
    assert!(matches!(outcome, OcrOutcome::Detected { .. }));
    assert!(outcome.regions().is_empty());
    assert_eq!(outcome.json()["status"], "detected");
}

#[test]
fn a_wrong_protocol_is_refused_and_reported() {
    // covers 31.3-b
    let url = mock(json!({ "protocol": "other/1", "capabilities": { "ocr": true } }), json!({}), 1);
    let bridge = OcrBridge::new(HttpConfig::new(url));
    let outcome = check(Some(&bridge), b"png-bytes");
    assert!(matches!(outcome, OcrOutcome::Failed { .. }));
    assert_eq!(outcome.json()["status"], "failed");
    assert!(outcome.json()["error"].as_str().unwrap().contains("ferrum-engine/1"));
}

#[test]
fn a_bridge_without_the_ocr_capability_is_refused() {
    // covers 31.3-b
    let url = mock(json!({ "protocol": "ferrum-engine/1", "capabilities": { "interactive": true } }), json!({}), 1);
    let bridge = OcrBridge::new(HttpConfig::new(url));
    let outcome = check(Some(&bridge), b"png-bytes");
    assert!(matches!(outcome, OcrOutcome::Failed { .. }));
    assert!(outcome.json()["error"].as_str().unwrap().contains("ocr"));
}

#[test]
fn an_unreachable_bridge_is_reported_as_failed() {
    // covers 31.3-b: a configured bridge that cannot be reached is a
    // reported failure, never a silent pass.
    let bridge = OcrBridge::new(HttpConfig::new("http://127.0.0.1:1"));
    let outcome = check(Some(&bridge), b"png-bytes");
    assert!(matches!(outcome, OcrOutcome::Failed { .. }));
    assert_eq!(outcome.json()["status"], "failed");
}

#[test]
fn a_bad_answer_is_reported_as_failed() {
    // covers 31.3-b
    let url = mock(ocr_info(), json!({ "no_regions": true }), 2);
    let bridge = OcrBridge::new(HttpConfig::new(url));
    let outcome = check(Some(&bridge), b"png-bytes");
    assert!(matches!(outcome, OcrOutcome::Failed { .. }));
    assert!(outcome.json()["error"].as_str().unwrap().contains("regions"));
}
