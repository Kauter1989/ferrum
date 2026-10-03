//! Agent segmentation scenarios (Stage 17, `docs/agent-segmentation.md`):
//! refinement through stored prompt histories, regions of interest,
//! scribble/lasso prompts, mask measurements and edits, quality checks,
//! GPU groups and the MCP session cache — against FERRUM's reference
//! server with the mock engine.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrum_agent::mcp::McpServer;
use ferrum_agent::{Agent, AgentConfig};
use ferrum_domain::{EngineError, EngineInfo, InteractiveSession, Provenance, SegmentationEngine, Timestamp, Volume};
use ferrum_engines::{EngineServer, MockEngine};
use serde_json::{json, Value};

/// The mock engine, declared research-only (as nnInteractive's weights are).
struct ResearchOnly(MockEngine);

impl SegmentationEngine for ResearchOnly {
    fn info(&self) -> Result<EngineInfo, EngineError> {
        Ok(EngineInfo { research_only: true, license: "CC BY-NC-SA 4.0 (test)".into(), ..self.0.info()? })
    }
    fn open_session(&self, volume: &Volume, modality: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
        self.0.open_session(volume, modality)
    }
}

/// Records the size of every uploaded volume.
struct Recording(MockEngine, Arc<std::sync::Mutex<Vec<usize>>>);

impl SegmentationEngine for Recording {
    fn info(&self) -> Result<EngineInfo, EngineError> {
        self.0.info()
    }
    fn open_session(&self, volume: &Volume, modality: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
        self.1.lock().unwrap().push(volume.dims().voxel_count());
        self.0.open_session(volume, modality)
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    _server: EngineServer,
    url: String,
    ws: String,
    src: std::path::PathBuf,
    uploads: Arc<std::sync::Mutex<Vec<usize>>>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let src = ferrum_agent::evals::write_phantoms(&dir.path().join("in")).unwrap().remove(0);
        let uploads = Arc::new(std::sync::Mutex::new(Vec::new()));
        let engine = Recording(MockEngine::default(), uploads.clone());
        let server = EngineServer::start(Arc::new(engine), "127.0.0.1:0", None).unwrap();
        let url = server.url();
        let ws = dir.path().join("ws").to_string_lossy().into_owned();
        Self { dir, _server: server, url, ws, src, uploads }
    }

    fn agent(&self) -> Agent {
        self.configured(|_| {})
    }

    fn configured(&self, f: impl FnOnce(&mut AgentConfig)) -> Agent {
        let mut c = AgentConfig { engines: vec![self.url.clone()], is_default: false, ..AgentConfig::default() };
        f(&mut c);
        Agent::new(c)
    }

    fn params(&self, extra: Value) -> Value {
        let mut p = json!({ "workspace": self.ws });
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        p
    }
}

fn ok(agent: &Agent, command: &str, params: Value) -> Value {
    let e = agent.run(command, &params);
    assert_eq!(e["ok"], true, "{command}: {e:#}");
    e
}

fn code(agent: &Agent, command: &str, params: Value) -> String {
    let e = agent.run(command, &params);
    assert_eq!(e["ok"], false, "{command} should fail: {e:#}");
    e["error"]["code"].as_str().unwrap().to_owned()
}

fn point(v: [u32; 3], positive: bool) -> Value {
    json!({ "type": "point", "point": { "voxel": v }, "positive": positive })
}

fn voxels(e: &Value) -> u64 {
    e["data"]["segment"]["voxels"].as_u64().unwrap()
}

fn failed(e: &Value) -> Vec<String> {
    e["data"]["checks"]["failed"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect()
}

#[test]
fn regions_of_interest_are_cut_around_the_prompts() {
    let f = Fixture::new();
    let agent = f.configured(|c| c.roi_margin_mm = 4.0);
    ok(&agent, "study open", json!({ "workspace": f.ws, "path": f.src }));
    let e = ok(&agent, "segment interactive", f.params(json!({ "prompts": [point([14, 20, 15], true)] })));
    let d = &e["data"];
    // the point ± 4 mm, clamped to the grid; the sphere (± 8 mm) is cut by it, which the checks report
    assert_eq!(d["roi"], json!({ "min": [10, 16, 13], "max": [19, 25, 18], "voxels": 9 * 9 * 5 }));
    assert_eq!(f.uploads.lock().unwrap().last().copied(), Some(9 * 9 * 5));
    assert_eq!(failed(&e), ["roi"]);
    // refining grows the region around the new prompts; whole_volume takes everything
    let label = d["segment"]["label"].as_u64().unwrap();
    let wide = ok(
        &agent,
        "segment interactive",
        f.params(json!({ "segment": label, "whole_volume": true, "prompts": [point([14, 20, 15], true)] })),
    );
    assert!(failed(&wide).iter().all(|c| c != "roi"));
    assert!(voxels(&wide) > voxels(&e));
}

#[test]
fn interactive_objects_are_refined_from_their_stored_prompts() {
    let f = Fixture::new();
    let agent = f.agent();
    ok(&agent, "study open", json!({ "workspace": f.ws, "path": f.src }));
    let first = ok(
        &agent,
        "segment interactive",
        f.params(json!({ "name": "Sphere", "agent": "run-1", "prompts": [point([14, 20, 15], true)] })),
    );
    let d = &first["data"];
    let label = d["segment"]["label"].as_u64().unwrap();
    assert_eq!(d["revision"], 1);
    assert_eq!(d["segment"]["provenance"]["author"]["kind"], "engine");
    assert_eq!(d["segment"]["provenance"]["requested_by"], json!({ "kind": "agent", "id": "run-1" }));
    assert_eq!(f.uploads.lock().unwrap().last().copied(), d["roi"]["voxels"].as_u64().map(|v| v as usize));
    assert!(failed(&first).is_empty(), "{:#}", d["checks"]);
    let sphere = voxels(&first);
    assert!(std::path::Path::new(&f.ws).join("engine_inputs.json").exists());

    // refine: a negative box clears the upper slices; the stored point is replayed first
    let refined = ok(
        &agent,
        "segment interactive",
        f.params(json!({
            "segment": label, "append": true,
            "prompts": [{ "type": "box", "positive": false, "min": { "voxel": [0, 0, 16] }, "max": { "voxel": [39, 39, 29] } }],
        })),
    );
    assert_eq!((refined["data"]["revision"].as_u64(), refined["data"]["prompts"].as_u64()), (Some(2), Some(2)));
    assert_eq!(refined["data"]["segment"]["label"].as_u64(), Some(label), "the same segment");
    assert!(voxels(&refined) < sphere);
    assert!(failed(&refined).contains(&"stability".to_owned()), "a large change has not converged");
    // undo the box: the sphere is back, and stable against the previous revision of the object
    let undone = ok(&agent, "segment interactive", f.params(json!({ "segment": label, "undo": true })));
    assert_eq!((voxels(&undone), undone["data"]["revision"].as_u64()), (sphere, Some(3)));
    // replace the prompts: the same point again gives the same object (deterministic replay)
    let again = ok(
        &agent,
        "segment interactive",
        f.params(json!({ "segment": label, "prompts": [point([14, 20, 15], true)] })),
    );
    assert_eq!(voxels(&again), sphere);
    assert_eq!(again["data"]["checks"]["stability"], 1.0);

    assert_eq!(code(&agent, "segment interactive", f.params(json!({ "segment": label, "undo": true }))), "bad_request");
    assert_eq!(
        code(
            &agent,
            "segment interactive",
            f.params(json!({ "segment": label, "undo": true, "prompts": [point([1, 1, 1], true)] }))
        ),
        "bad_request"
    );
    assert_eq!(code(&agent, "segment interactive", f.params(json!({}))), "bad_request", "no prompts");
    // the agent may clean up what it asked the engine for
    ok(&agent, "segment rename", f.params(json!({ "label": label, "name": "Round object" })));
    ok(&agent, "segment delete", f.params(json!({ "label": label })));
}

#[test]
fn lassos_scribbles_and_seeds_from_a_segment() {
    let f = Fixture::new();
    let agent = f.agent();
    ok(&agent, "study open", json!({ "workspace": f.ws, "path": f.src }));
    let corners = [[29, 29, 15], [32, 29, 15], [32, 32, 15], [29, 32, 15]];
    let lasso = ok(
        &agent,
        "segment interactive",
        f.params(json!({ "name": "Lasso", "prompts": [{ "type": "lasso", "points": corners.iter().map(|v| json!({ "voxel": v })).collect::<Vec<_>>() }] })),
    );
    assert_eq!(voxels(&lasso), 16, "a filled 4 × 4 square on one slice");
    let scribble = ok(
        &agent,
        "segment interactive",
        f.params(json!({ "name": "Stroke", "prompts": [{ "type": "scribble", "points": [{ "voxel": [2, 2, 3] }, { "voxel": [8, 2, 3] }] }] })),
    );
    assert_eq!(voxels(&scribble), 7, "a one-voxel line");
    assert_eq!(
        code(&agent, "segment interactive", f.params(json!({ "prompts": [{ "type": "lasso", "points": [{ "voxel": [1, 1, 1] }, { "voxel": [5, 1, 2] }, { "voxel": [5, 5, 3] }] }] }))),
        "bad_request",
        "a lasso on no single slice"
    );

    // a threshold region of the agent, redone by the engine from lassos of its own mask
    let t = ok(
        &agent,
        "segment threshold",
        f.params(json!({ "seed": { "voxel": [30, 30, 14] }, "min": 500, "max": 2000, "name": "Cube", "agent": "a" })),
    );
    let label = t["data"]["segment"]["label"].as_u64().unwrap();
    let redone = ok(&agent, "segment interactive", f.params(json!({ "from_segment": label, "agent": "a" })));
    let seg = &redone["data"]["segment"];
    assert_eq!((seg["label"].as_u64(), seg["provenance"]["author"]["kind"].as_str()), (Some(label), Some("engine")));
    assert_eq!(redone["data"]["prompts"], 3, "one lasso per plane");
    assert!(seg["voxels"].as_u64().unwrap() > 0);
    // it is now an engine object with stored prompts: refinable
    ok(
        &agent,
        "segment interactive",
        f.params(json!({ "segment": label, "append": true, "prompts": [point([30, 30, 14], true)] })),
    );
}

#[test]
fn engine_calls_follow_the_operator_limits() {
    let f = Fixture::new();
    let agent = f.configured(|c| c.max_prompts_per_object = 2);
    ok(&agent, "study open", json!({ "workspace": f.ws, "path": f.src }));
    let three = json!({ "prompts": [point([14, 20, 15], true), point([14, 20, 14], true), point([14, 20, 13], true)] });
    assert_eq!(code(&agent, "segment interactive", f.params(three)), "limit");
    let small_roi = json!({
        "prompts": [point([14, 20, 15], true)],
        "roi": { "min": { "voxel": [0, 0, 0] }, "max": { "voxel": [3, 3, 3] } },
    });
    assert_eq!(code(&agent, "segment interactive", f.params(small_roi)), "out_of_volume");
    let whole = ok(
        &agent,
        "segment interactive",
        f.params(json!({ "prompts": [point([14, 20, 15], true)], "whole_volume": true })),
    );
    assert_eq!(whole["data"]["roi"]["voxels"], 40 * 40 * 30);

    // a person's segment and an engine result nobody asked for are protected
    let label = whole["data"]["segment"]["label"].as_u64().unwrap();
    let ws = ferrum_io::Workspace::open(std::path::Path::new(&f.ws)).unwrap();
    let vol = ferrum_io::read_nifti(&f.src).unwrap().0;
    let mut set = ws.load_segments(&vol).unwrap().unwrap();
    set.set_provenance(label as u8, Provenance::engine("E", "1", false, Timestamp(1))).unwrap();
    ws.save_segments(&set, &vol, "test").unwrap();
    assert_eq!(code(&agent, "segment delete", f.params(json!({ "label": label }))), "forbidden");
    assert_eq!(code(&agent, "segment edit", f.params(json!({ "segment": label, "op": "keep_largest" }))), "forbidden");
    assert_eq!(
        code(
            &agent,
            "segment interactive",
            f.params(json!({ "segment": label, "prompts": [point([14, 20, 15], true)] }))
        ),
        "forbidden"
    );
    // a person's confirmation protects what the agent asked for, too
    let mut confirmed =
        Provenance::engine("E", "1", false, Timestamp(1)).requested_by(ferrum_domain::Author::Agent { id: None });
    confirmed.review(ferrum_domain::ReviewStatus::Confirmed, Some("dr.k"), Timestamp(2));
    set.set_provenance(label as u8, confirmed).unwrap();
    ws.save_segments(&set, &vol, "test").unwrap();
    let e = agent.run("segment delete", &f.params(json!({ "label": label })));
    assert_eq!(e["error"]["code"], "forbidden");
    assert!(e["error"]["message"].as_str().unwrap().contains("confirmed"));

    // research-only engines can be switched off by the operator
    let research = EngineServer::start(Arc::new(ResearchOnly(MockEngine::default())), "127.0.0.1:0", None).unwrap();
    let strict = Agent::new(AgentConfig {
        engines: vec![research.url()],
        allow_research_only: false,
        is_default: false,
        ..AgentConfig::default()
    });
    assert_eq!(
        code(&strict, "segment interactive", f.params(json!({ "prompts": [point([14, 20, 15], true)] }))),
        "forbidden"
    );
    assert_eq!(code(&strict, "segment auto", f.params(json!({}))), "forbidden");
    ok(&strict, "engine info", json!({}));
}

#[test]
fn segments_are_measured_and_edited_from_voxels() {
    let f = Fixture::new();
    let agent = f.agent();
    ok(&agent, "study open", json!({ "workspace": f.ws, "path": f.src }));
    // the sphere: radius 8 mm at 1 × 1 × 2 mm, centre voxel (14, 20, 15)
    let t = ok(
        &agent,
        "segment threshold",
        f.params(
            json!({ "seed": { "voxel": [14, 20, 15] }, "min": 0, "max": 200, "name": "Sphere left", "agent": "a" }),
        ),
    );
    let sphere = t["data"]["segment"]["label"].as_u64().unwrap();
    let s = ok(&agent, "segment shape", f.params(json!({ "segment": sphere })));
    let shape = &s["data"]["shape"];
    assert_eq!(shape["extent_mm"], json!([17.0, 17.0, 18.0]));
    assert_eq!(shape["slices"]["axial"]["largest"], 16);
    assert_eq!((shape["components"].as_u64(), shape["touches_border"].as_bool()), (Some(1), Some(false)));
    let long = shape["long_axis"]["mm"].as_f64().unwrap();
    assert!((15.5..=16.5).contains(&long), "{long}");
    let short = shape["short_axis"]["mm"].as_f64().unwrap();
    assert!((15.0..=16.5).contains(&short), "{short}");
    assert_eq!(shape["centroid"]["voxel"], json!([14.0, 20.0, 15.0]));
    // LPS: +x is the patient's left; the sphere lies right of the volume centre
    assert_eq!(shape["laterality"]["consistent"], false);
    assert_eq!(shape["laterality"]["observed"], "right");

    // the part of the segment on one slice, and a segment cut to that slice
    let slab =
        json!({ "segment": sphere, "box": { "min": { "voxel": [0, 0, 15] }, "max": { "voxel": [39, 39, 15] } } });
    let st = ok(&agent, "stats", f.params(slab));
    let on_slice = shape["slices"]["axial"]["largest_voxels"].as_u64().unwrap();
    assert_eq!(st["data"]["voxels"].as_u64(), Some(on_slice));
    assert_eq!(st["data"]["region"], "segment within box");

    // two objects in one segment: components, split, compare, keep_largest
    let both = ok(
        &agent,
        "segment interactive",
        f.params(json!({ "name": "Both", "agent": "a", "whole_volume": true,
            "prompts": [point([30, 30, 14], true), { "type": "lasso", "points": [{ "voxel": [2, 2, 3] }, { "voxel": [4, 2, 3] }, { "voxel": [4, 4, 3] }] }] })),
    );
    let both_label = both["data"]["segment"]["label"].as_u64().unwrap();
    assert_eq!(both["data"]["checks"]["components"], 2);
    assert!(!failed(&both).contains(&"components".to_owned()), "the cube holds over 90 % of the voxels");
    let c = ok(&agent, "segment components", f.params(json!({ "segment": both_label })));
    assert_eq!(c["data"]["components"].as_array().unwrap().len(), 2);
    let cube_voxels = c["data"]["components"][0]["voxels"].as_u64().unwrap();
    assert_eq!(cube_voxels, 6 * 6 * 6);
    let kept = ok(&agent, "segment edit", f.params(json!({ "segment": both_label, "op": "keep_largest" })));
    assert_eq!(kept["data"]["segment"]["voxels"].as_u64(), Some(cube_voxels));
    let cut = ok(
        &agent,
        "segment edit",
        f.params(json!({ "segment": both_label, "op": "restrict_to_box", "box": { "min": { "voxel": [0, 0, 12] }, "max": { "voxel": [39, 39, 13] } } })),
    );
    assert_eq!(cut["data"]["segment"]["voxels"].as_u64(), Some(72));
    ok(&agent, "segment edit", f.params(json!({ "segment": both_label, "op": "fill_holes" })));
    ok(&agent, "segment edit", f.params(json!({ "segment": both_label, "op": "remove_small", "min_ml": 0.001 })));

    // split a two-part threshold segment of the agent
    let lassos = ok(
        &agent,
        "segment interactive",
        f.params(json!({ "name": "Marks", "agent": "a", "whole_volume": true, "prompts": [
            { "type": "lasso", "points": [{ "voxel": [2, 30, 3] }, { "voxel": [5, 30, 3] }, { "voxel": [5, 33, 3] }] },
            { "type": "lasso", "points": [{ "voxel": [30, 2, 25] }, { "voxel": [31, 2, 25] }, { "voxel": [31, 3, 25] }] }] })),
    );
    let marks = lassos["data"]["segment"]["label"].as_u64().unwrap();
    let split = ok(&agent, "segment components", f.params(json!({ "segment": marks, "split": true })));
    let new = &split["data"]["new_segments"];
    assert_eq!(new.as_array().unwrap().len(), 1);
    assert_eq!(new[0]["name"], "Marks #2");
    assert_eq!(new[0]["provenance"]["author"]["kind"], "engine");

    // agreement: a segment with itself in another workspace of the same series, and two disjoint ones
    let other = f.dir.path().join("ws2").to_string_lossy().into_owned();
    ok(&agent, "study open", json!({ "workspace": other, "path": f.src }));
    let t2 = ok(
        &agent,
        "segment threshold",
        json!({ "workspace": other, "seed": { "voxel": [14, 20, 15] }, "min": 0, "max": 200 }),
    );
    let same = ok(
        &agent,
        "segment compare",
        f.params(json!({ "a": sphere, "b": t2["data"]["segment"]["label"], "b_workspace": other })),
    );
    let g = &same["data"]["agreement"];
    assert_eq!(
        (g["dice"].as_f64(), g["hd95_mm"].as_f64(), g["volume_difference_ml"].as_f64()),
        (Some(1.0), Some(0.0), Some(0.0))
    );
    let apart = ok(&agent, "segment compare", f.params(json!({ "a": sphere, "b": both_label })));
    assert_eq!(apart["data"]["agreement"]["dice"], 0.0);
    assert!(apart["data"]["agreement"]["centroid_distance_mm"].as_f64().unwrap() > 10.0);
    assert_eq!(code(&agent, "segment compare", f.params(json!({ "a": sphere, "b": 99 }))), "not_found");
    assert_eq!(code(&agent, "segment shape", f.params(json!({ "segment": 99 }))), "not_found");
}

#[test]
fn automatic_results_carry_checks_prefixes_and_requester() {
    let f = Fixture::new();
    let agent = f.agent();
    ok(&agent, "study open", json!({ "workspace": f.ws, "path": f.src }));
    let auto = ok(&agent, "segment auto", f.params(json!({ "name_prefix": "mock/", "agent": "a1" })));
    let segs = auto["data"]["segments"].as_array().unwrap();
    let names: Vec<&str> = segs.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["mock/intermediate", "mock/bright"]);
    for s in segs {
        assert_eq!(s["provenance"]["requested_by"]["id"], "a1");
        assert_eq!(s["checks"]["empty"], false);
        assert_eq!(s["checks"]["overlap_voxels"], 0);
    }
    // a second run keeps the first one's voxels and reports the overlap
    let second = ok(&agent, "segment auto", f.params(json!({ "name_prefix": "again/", "labels": ["bright"] })));
    let s = &second["data"]["segments"][0];
    assert_eq!(s["checks"]["empty"], true);
    assert_eq!(s["checks"]["overlap_voxels"], 216);
    assert!(second["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("found nothing")));
    let label = segs[1]["label"].as_u64().unwrap();
    ok(&agent, "segment delete", f.params(json!({ "label": label })));
}

#[test]
fn engines_are_listed_and_grouped() {
    let f = Fixture::new();
    let agent = f.configured(|c| {
        c.engines.push("http://127.0.0.1:9".into());
        c.gpu_groups = vec![(format!("seg-{}", std::process::id()), vec![f.url.clone()])];
    });
    let list = ok(&agent, "engine list", json!({}));
    let engines = list["data"]["engines"].as_array().unwrap();
    assert_eq!((engines[0]["reachable"].as_bool(), engines[1]["reachable"].as_bool()), (Some(true), Some(false)));
    assert!(engines[0]["gpu_group"].as_str().unwrap().starts_with("seg-"));
    assert_eq!(engines[0]["deterministic"], true);
    // engines of a GPU group still work; their sessions are never kept open
    ok(&agent, "study open", json!({ "workspace": f.ws, "path": f.src }));
    let mut server = McpServer::new(agent);
    let r =
        mcp(&mut server, 1, "segment interactive", &f.params(json!({ "prompts": [point([14, 20, 15], true)] })), None);
    assert_eq!(r["result"]["isError"], false);
    assert_eq!(server.cached_engine_sessions(), 0);
    let none = ok(&Agent::new(AgentConfig { is_default: false, ..AgentConfig::default() }), "engine list", json!({}));
    if std::env::var("FERRUM_ENGINE_URL").is_err() {
        assert!(none["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("no engine")));
    }
}

fn mcp(server: &mut McpServer, id: u64, command: &str, args: &Value, token: Option<&str>) -> Value {
    let mut params = json!({ "name": format!("ferrum_{}", command.replace(' ', "_")), "arguments": args });
    if let Some(t) = token {
        params["_meta"] = json!({ "progressToken": t });
    }
    let mut notes = Vec::new();
    let reply = server
        .handle_with(&json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": params }), &mut |n| {
            notes.push(n)
        })
        .unwrap();
    json!({ "result": reply["result"], "notes": notes })
}

#[test]
fn mcp_keeps_sessions_and_reports_progress_with_the_same_results() {
    let f = Fixture::new();
    let agent = f.agent();
    ok(&agent, "study open", json!({ "workspace": f.ws, "path": f.src }));
    let mut server = McpServer::new(f.agent());
    let first =
        mcp(&mut server, 1, "segment interactive", &f.params(json!({ "prompts": [point([14, 20, 15], true)] })), None);
    let label = first["result"]["structuredContent"]["data"]["segment"]["label"].as_u64().unwrap();
    assert_eq!(server.cached_engine_sessions(), 1);
    let uploads = f.uploads.lock().unwrap().len();
    let upper =
        json!({ "type": "box", "positive": false, "min": { "voxel": [0, 0, 16] }, "max": { "voxel": [39, 39, 29] } });
    let refine = json!({ "segment": label, "append": true, "prompts": [upper] });
    let via_mcp = mcp(&mut server, 2, "segment interactive", &f.params(refine.clone()), None);
    assert_eq!(f.uploads.lock().unwrap().len(), uploads, "the open session was reused: no new upload");
    // the command line replays the stored prompts on a fresh session and gets the same object
    let undo = ok(&agent, "segment interactive", f.params(json!({ "segment": label, "undo": true })));
    assert_eq!(undo["data"]["revision"], 3);
    let via_cli = ok(&agent, "segment interactive", f.params(refine));
    let m = &via_mcp["result"]["structuredContent"]["data"];
    let c = &via_cli["data"];
    assert!(m["segment"]["voxels"].as_u64().unwrap() > 0);
    assert_eq!(m["segment"]["voxels"], c["segment"]["voxels"]);
    assert_eq!(m["checks"], c["checks"]);
    assert_eq!(m["roi"], c["roi"]);

    // automatic jobs report progress when the client asks for it
    let auto = mcp(&mut server, 3, "segment auto", &f.params(json!({ "name_prefix": "p/" })), Some("tok"));
    assert_eq!(auto["result"]["isError"], false);
    let notes = auto["notes"].as_array().unwrap();
    assert!(!notes.is_empty());
    assert_eq!(notes[0]["method"], "notifications/progress");
    assert_eq!(notes[0]["params"]["progressToken"], "tok");
}

#[test]
fn lasso_seeds_do_not_count_against_the_prompt_limit() {
    let f = Fixture::new();
    let agent = f.configured(|c| c.max_prompts_per_object = 1);
    ok(&agent, "study open", json!({ "workspace": f.ws, "path": f.src }));
    let t = ok(
        &agent,
        "segment threshold",
        f.params(json!({ "seed": { "voxel": [30, 30, 14] }, "min": 500, "max": 2000, "agent": "a" })),
    );
    let label = t["data"]["segment"]["label"].as_u64().unwrap();
    let seeded = ok(
        &agent,
        "segment interactive",
        f.params(json!({ "from_segment": label, "prompts": [point([30, 30, 14], true)] })),
    );
    assert_eq!(seeded["data"]["prompts"], 4, "three seeds and one prompt");
    let more = json!({ "segment": label, "append": true, "prompts": [point([31, 30, 14], true)] });
    assert_eq!(code(&agent, "segment interactive", f.params(more)), "limit");
    ok(&agent, "segment interactive", f.params(json!({ "segment": label, "undo": true })));
    assert_eq!(
        code(&agent, "segment interactive", f.params(json!({ "segment": label, "undo": true }))),
        "bad_request",
        "seeds cannot be undone"
    );
}
