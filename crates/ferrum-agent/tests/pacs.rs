//! PACS agent commands against a mock DICOMweb PACS (Stage 32, PR 3):
//! query/retrieve are read-only and pseudonymised, the CLI and MCP give
//! identical JSON, and the SSRF allowlist refuses non-allowed hosts.
//! Send-off (STOW/C-MOVE/C-STORE) is absent from the command set.
//!
//! // covers 32.4-a, 32.4-b

#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
use ferrum_agent::mcp::McpServer;
use ferrum_agent::{Agent, AgentConfig};
use serde_json::{json, Value};

const STUDY_UID: &str = "1.2.826.0.1.3680043.8.498.12345";
const SERIES_UID: &str = "1.2.826.0.1.3680043.8.498.67890";
/// Identifiers that must never appear in a PACS command's output.
const SECRETS: [&str; 6] = ["DOE^JANE", "MRN-4711", "19700101", "ACC-0815", STUDY_UID, SERIES_UID];

/// One 8×8 CT slice with patient identifiers, as Part 10 bytes.
fn slice(k: u32) -> Vec<u8> {
    let pixels: Vec<u16> =
        (0..64u32).map(|p| if (2..6).contains(&(p % 8)) && (2..6).contains(&(p / 8)) { 1424 } else { 24 }).collect();
    let text = |t, vr, v: &str| DataElement::new(t, vr, PrimitiveValue::from(v));
    let elements = vec![
        text(tags::SOP_CLASS_UID, VR::UI, uids::CT_IMAGE_STORAGE),
        text(tags::SOP_INSTANCE_UID, VR::UI, &format!("{SERIES_UID}.{}", k + 1)),
        text(tags::MODALITY, VR::CS, "CT"),
        text(tags::PATIENT_NAME, VR::PN, "DOE^JANE"),
        text(tags::PATIENT_ID, VR::LO, "MRN-4711"),
        text(tags::PATIENT_BIRTH_DATE, VR::DA, "19700101"),
        text(tags::ACCESSION_NUMBER, VR::SH, "ACC-0815"),
        text(tags::STUDY_INSTANCE_UID, VR::UI, STUDY_UID),
        text(tags::SERIES_INSTANCE_UID, VR::UI, SERIES_UID),
        text(tags::STUDY_DATE, VR::DA, "20240428"),
        text(tags::SERIES_DESCRIPTION, VR::LO, "Axial 2.0"),
        text(tags::INSTANCE_NUMBER, VR::IS, &(k + 1).to_string()),
        text(tags::IMAGE_POSITION_PATIENT, VR::DS, &format!("-4\\-4\\{}", f64::from(k) * 2.0)),
        text(tags::IMAGE_ORIENTATION_PATIENT, VR::DS, "1\\0\\0\\0\\1\\0"),
        text(tags::PIXEL_SPACING, VR::DS, "1\\1"),
        text(tags::PHOTOMETRIC_INTERPRETATION, VR::CS, "MONOCHROME2"),
        text(tags::RESCALE_SLOPE, VR::DS, "1"),
        text(tags::RESCALE_INTERCEPT, VR::DS, "-1024"),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1u16)),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(8u16)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(8u16)),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(16u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(16u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(15u16)),
        DataElement::new(tags::PIXEL_REPRESENTATION, VR::US, PrimitiveValue::from(0u16)),
        DataElement::new(tags::PIXEL_DATA, VR::OW, PrimitiveValue::U16(pixels.into())),
    ];
    let obj = InMemDicomObject::from_element_iter(elements);
    let file = obj.with_meta(FileMetaTableBuilder::new().transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)).unwrap();
    let mut bytes = Vec::new();
    file.write_all(&mut bytes).unwrap();
    bytes
}

fn qido_json() -> String {
    json!([{
        "00080060": { "vr": "CS", "Value": ["CT"] },
        "0008103E": { "vr": "LO", "Value": ["Axial 2.0"] },
        "0020000D": { "vr": "UI", "Value": [STUDY_UID] },
        "0020000E": { "vr": "UI", "Value": [SERIES_UID] },
        "00201209": { "vr": "IS", "Value": ["4"] },
        "00280010": { "vr": "US", "Value": [8] },
        "00280011": { "vr": "US", "Value": [8] },
    }])
    .to_string()
}

fn multipart(parts: &[Vec<u8>]) -> (Vec<u8>, String) {
    let boundary = "ferrum-agent-mock";
    let mut body = Vec::new();
    for part in parts {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Type: application/dicom\r\n\r\n").as_bytes());
        body.extend_from_slice(part);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (body, format!("multipart/related; type=\"application/dicom\"; boundary={boundary}"))
}

struct MockPacs {
    root: String,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MockPacs {
    fn start() -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            _ => panic!("expected an IP listen address"),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = stop.clone();
        let parts: Vec<Vec<u8>> = (0..4).map(slice).collect();
        let handle = std::thread::spawn(move || {
            while !stop_t.load(Ordering::Relaxed) {
                let Ok(Some(req)) = server.recv_timeout(Duration::from_millis(50)) else { continue };
                let url = req.url().to_string();
                let path = url.split('?').next().unwrap_or(&url);
                // QIDO series query ends with /series (global or study-scoped
                // /studies/{uid}/series); WADO retrieval ends with /series/{uid}.
                let response = if path.ends_with("/series") {
                    tiny_http::Response::from_string(qido_json())
                        .with_header("Content-Type: application/dicom+json".parse::<tiny_http::Header>().unwrap())
                } else {
                    let (body, ct) = multipart(&parts);
                    tiny_http::Response::from_data(body)
                        .with_header(format!("Content-Type: {ct}").parse::<tiny_http::Header>().unwrap())
                };
                let _ = req.respond(response);
            }
        });
        Self { root: format!("dicomweb://127.0.0.1:{port}/dicom-web"), stop, handle: Some(handle) }
    }

    fn query_url(&self) -> String {
        format!("{}?StudyInstanceUID={STUDY_UID}", self.root)
    }
}

impl Drop for MockPacs {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn scan_text(text: &str, found: &mut Vec<String>) {
    for s in SECRETS {
        if text.contains(s) {
            found.push(s.to_string());
        }
    }
}

/// Data without what legitimately differs between two runs (times, paths).
fn comparable(envelope: &Value, ws_root: &str) -> Value {
    let mut v = envelope.clone();
    if let Some(p) = v.get_mut("provenance") {
        p["time"] = Value::Null;
        p["params"] = Value::Null;
    }
    let text = if ws_root.is_empty() {
        v.to_string()
    } else {
        let escaped = ws_root.replace('\\', "\\\\");
        v.to_string().replace(&escaped, "<ws>").replace(ws_root, "<ws>")
    };
    serde_json::from_str(&text).unwrap()
}

fn mcp_call(server: &mut McpServer, id: u64, command: &str, args: &Value) -> Value {
    let name = format!("ferrum_{}", command.replace(' ', "_"));
    let msg =
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": args } });
    server.handle(&msg).unwrap()["result"].clone()
}

#[test]
fn pacs_query_and_retrieve_are_pseudonymised_and_cli_equals_mcp() {
    // covers 32.4-a, 32.4-b: read-only query/retrieve, pseudonymised output,
    // identical JSON over the command line and MCP.
    let pacs = MockPacs::start();
    let dir = tempfile::tempdir().unwrap();
    let cli_ws = dir.path().join("cli").to_string_lossy().into_owned();
    let mcp_ws = dir.path().join("mcp").to_string_lossy().into_owned();
    let agent = Agent::default();
    let mut server = McpServer::new(Agent::default());
    let url = pacs.query_url();

    // query: pseudonymised series, no identifiers
    let query = json!({ "url": url });
    let direct = agent.run("pacs query", &query);
    assert_eq!(direct["ok"], true, "{direct:#}");
    let entry = &direct["data"]["series"][0];
    assert!(entry["series"].as_str().unwrap().starts_with("anon-"), "series id is pseudonymised");
    assert_eq!(entry["modality"], "CT");
    assert_eq!(entry["dims"], json!([8, 8, 4]));
    let via_mcp = mcp_call(&mut server, 1, "pacs query", &query);
    assert_eq!(comparable(&direct, &cli_ws), comparable(&via_mcp["structuredContent"], &mcp_ws), "pacs query");
    assert_eq!(via_mcp["isError"], false);

    // retrieve into a workspace, then describe / probe
    let retrieve_cli = json!({ "workspace": cli_ws, "url": url });
    let retrieve_mcp = json!({ "workspace": mcp_ws, "url": url });
    let direct = agent.run("pacs retrieve", &retrieve_cli);
    assert_eq!(direct["ok"], true, "{direct:#}");
    assert_eq!(direct["data"]["dims"], json!([8, 8, 4]));
    assert_eq!(direct["data"]["value_unit"], "HU");
    let via_mcp = mcp_call(&mut server, 2, "pacs retrieve", &retrieve_mcp);
    assert_eq!(comparable(&direct, &cli_ws), comparable(&via_mcp["structuredContent"], &mcp_ws), "pacs retrieve");

    // a follow-up command on the retrieved workspace works (re-retrieve)
    let probe = json!({ "workspace": cli_ws, "point": { "voxel": [3, 3, 1] } });
    let p = agent.run("probe", &probe);
    assert_eq!(p["ok"], true, "{p:#}");
    assert!((p["data"]["value"].as_f64().unwrap() - 400.0).abs() < 0.1, "the bright block is 400 HU");

    // no identifier leaks in any envelope (params echo excluded: harness input)
    let mut found = Vec::new();
    for env in [&direct, &p] {
        let mut clean = env.clone();
        clean["provenance"]["params"] = Value::Null;
        scan_text(&clean.to_string(), &mut found);
    }
    scan_text(&direct.to_string().replace(&url, "<url>"), &mut found);
    assert!(found.is_empty(), "identifiers leaked: {found:#?}");
}

#[test]
fn pacs_commands_refuse_disallowed_hosts() {
    // covers 32.4-a (SSRF): the allowlist and the metadata block refuse before
    // any network call; send-off is not a command.
    let agent = Agent::default();
    // non-loopback under the default config is forbidden
    let env = agent.run("pacs query", &json!({ "url": "dicomweb://pacs.example/dicom-web?PatientID=1" }));
    assert_eq!(env["error"]["code"], "forbidden", "{env:#}");
    assert!(!env.to_string().contains("PatientID=1"), "the query value never leaks");
    // the cloud metadata endpoint is always forbidden, even allow-listed
    let config = AgentConfig {
        pacs: vec!["dicomweb://169.254.169.254/latest".into()],
        is_default: false,
        ..AgentConfig::default()
    };
    let env = Agent::new(config).run("pacs query", &json!({ "url": "dicomweb://169.254.169.254/latest/meta-data" }));
    assert_eq!(env["error"]["code"], "forbidden", "{env:#}");
    // retrieve over DIMSE (C-MOVE) is host-only
    let env = agent.run("pacs retrieve", &json!({ "workspace": "ws", "url": "dicom://127.0.0.1:104/PACS" }));
    assert_eq!(env["error"]["code"], "bad_request", "{env:#}");
    // echo needs a DIMSE URL
    let env = agent.run("pacs echo", &json!({ "url": "dicomweb://127.0.0.1:8042/dicom-web" }));
    assert_eq!(env["error"]["code"], "bad_request", "{env:#}");
    // send-off commands do not exist
    for cmd in ["pacs stow", "pacs store", "pacs move", "stow", "pacs send"] {
        let env = agent.run(cmd, &json!({}));
        assert_eq!(env["error"]["code"], "bad_request", "{cmd} should be unknown: {env:#}");
    }
    assert!(Agent::commands().all(|c| !c.contains("stow") && !c.contains("move") && !c.contains("store")));
}

#[test]
fn pacs_echo_cli_equals_mcp_on_a_refusal() {
    // covers 32.4-a: the echo command gives identical JSON over both
    // transports (here a refusal, which needs no live DIMSE node).
    let agent = Agent::default();
    let mut server = McpServer::new(Agent::default());
    let args = json!({ "url": "dicom://pacs.example:104/PACS" });
    let direct = agent.run("pacs echo", &args);
    assert_eq!(direct["ok"], false);
    let via_mcp = mcp_call(&mut server, 1, "pacs echo", &args);
    assert_eq!(comparable(&direct, ""), comparable(&via_mcp["structuredContent"], ""), "pacs echo");
    assert_eq!(via_mcp["isError"], true);
}

#[test]
fn a_crafted_pacs_manifest_cannot_steer_a_reload() {
    // covers 32.4-a (SSRF, L49 family): every reload re-checks the allowlist,
    // so a planted manifest pointing at a disallowed host is refused.
    let pacs = MockPacs::start();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws").to_string_lossy().into_owned();
    let agent = Agent::default();
    let env = agent.run("pacs retrieve", &json!({ "workspace": ws, "url": pacs.query_url() }));
    assert_eq!(env["ok"], true, "{env:#}");
    // tamper with the recorded source URL
    let manifest_path = Path::new(&ws).join("workspace.json");
    let mut manifest: Value = serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest["source"]["path"] = json!("dicomweb://evil.example/dicom-web?StudyInstanceUID=1");
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
    let env = agent.run("study info", &json!({ "workspace": ws }));
    assert_eq!(env["error"]["code"], "forbidden", "a disallowed reload host is refused: {env:#}");
    assert!(!env.to_string().contains("StudyInstanceUID=1"), "the query value never leaks");
}

#[test]
fn retrieved_workspace_records_the_pacs_url() {
    // covers 32.4-a: the workspace source is the URL; no local patient files
    let pacs = MockPacs::start();
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws").to_string_lossy().into_owned();
    let agent = Agent::default();
    let env = agent.run("pacs retrieve", &json!({ "workspace": ws, "url": pacs.query_url() }));
    assert_eq!(env["ok"], true, "{env:#}");
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(Path::new(&ws).join("workspace.json")).unwrap()).unwrap();
    assert_eq!(manifest["source"]["format"], "DICOMweb");
    assert!(manifest["source"]["path"].as_str().unwrap().starts_with("dicomweb://127.0.0.1:"));
    assert_eq!(manifest["source"]["files"].as_array().unwrap().len(), 0, "no local patient files");
    // reopening re-retrieves and still works
    let info = agent.run("study info", &json!({ "workspace": ws }));
    assert_eq!(info["ok"], true, "{info:#}");
    assert_eq!(info["data"]["dims"], json!([8, 8, 4]));
}
