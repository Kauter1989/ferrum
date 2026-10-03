//! Reference solutions of the skill evaluations: every task is solvable
//! with the tools, and a careful answer built from tool results passes the
//! grader.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use ferrum_agent::evals::{grade, serve_mock_engine, tasks, write_phantoms};
use ferrum_agent::Agent;
use serde_json::{json, Value};

struct Run {
    agent: Agent,
    ws: String,
    engine: String,
    calls: Vec<Value>,
}

impl Run {
    fn call(&mut self, command: &str, mut args: Value) -> Value {
        args["workspace"] = json!(self.ws);
        let result = self.agent.run(command, &args);
        assert_eq!(result["ok"], true, "{command}: {result:#}");
        self.calls.push(json!({ "command": command, "arguments": args, "result": result }));
        result["data"].clone()
    }

    fn transcript(self, answer: String) -> Value {
        json!({ "calls": self.calls, "answer": answer })
    }
}

fn solve(id: &str, run: &mut Run) -> String {
    let v = |i: f64, j: f64, k: f64| json!({ "voxel": [i, j, k] });
    match id {
        "sphere_diameter" => {
            // find the edges along a line through the object, then measure between them
            let p = run.call("profile", json!({ "from": v(0.0, 20.0, 15.0), "to": v(39.0, 20.0, 15.0) }));
            let inside: Vec<f64> = p["samples"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|s| s["value"].as_f64().unwrap() > 0.0)
                .map(|s| s["voxel"][0].as_f64().unwrap())
                .collect();
            let (a, b) = (inside[0], inside[inside.len() - 1]);
            let d = run.call("measure distance", json!({ "points": [v(a, 20.0, 15.0), v(b, 20.0, 15.0)] }));
            format!(
                "The diameter is {} mm (distance between the edge voxels along a line through the centre, ± 2 mm).",
                d["value"]
            )
        }
        "sphere_volume" => {
            let s = run.call(
                "segment threshold",
                json!({ "seed": v(14.0, 20.0, 15.0), "min": 50, "max": 150, "name": "Sphere" }),
            );
            format!(
                "Volume {} ml by region growing within 50..150 from the centre. The segment is a proposal: please review it in FERRUM.",
                s["segment"]["volume_ml"]
            )
        }
        "cube_mean" => {
            let s = run.call("stats", json!({ "box": { "min": v(28.0, 28.0, 12.0), "max": v(33.0, 33.0, 17.0) } }));
            format!("The mean value is {} over {} voxels inside the object.", s["mean"], s["voxels"])
        }
        "largest_slice" => {
            let r = run.call("view slice", json!({ "plane": "axial", "at": v(14.0, 20.0, 15.0) }));
            format!("The object is largest on axial slice {} (the slice through its centre).", r["slice_number"])
        }
        "cube_centre" => {
            let p = run.call("probe", json!({ "point": v(30.5, 30.5, 14.5) }));
            let mm = &p["patient_mm"];
            format!("The centre lies at ({}, {}, {}) mm (LPS).", mm[0], mm[1], mm[2])
        }
        "engine_sphere_volume" => {
            let s = run.call(
                "segment interactive",
                json!({ "engine": run.engine, "name": "Sphere", "prompts": [{ "type": "point", "point": v(14.0, 20.0, 15.0) }] }),
            );
            format!(
                "Volume {} ml, segmented by the engine from one point in the object. The segment is an engine proposal: please review it in FERRUM.",
                s["segment"]["volume_ml"]
            )
        }
        "engine_long_axis" => {
            let s = run.call(
                "segment interactive",
                json!({ "engine": run.engine, "prompts": [{ "type": "point", "point": v(14.0, 20.0, 15.0) }] }),
            );
            let shape = run.call("segment shape", json!({ "segment": s["segment"]["label"] }));
            format!(
                "The longest axial diameter is {} mm on axial slice {} (between voxel centres, ± {} mm). The segment is a proposal for review.",
                shape["shape"]["long_axis"]["mm"], shape["shape"]["axes_slice_number"], shape["shape"]["uncertainty_mm"]
            )
        }
        "engine_auto_bright" => {
            let a = run.call("segment auto", json!({ "engine": run.engine }));
            let bright = a["segments"].as_array().unwrap().iter().find(|s| s["name"] == "bright").unwrap().clone();
            format!(
                "The brightest structure has {} ml (segment of the automatic engine). The segments are proposals: please review them.",
                bright["volume_ml"]
            )
        }
        other => panic!("no reference solution for {other}"),
    }
}

#[test]
fn every_task_has_a_passing_reference_solution() {
    let dir = tempfile::tempdir().unwrap();
    let phantoms = write_phantoms(&dir.path().join("phantoms")).unwrap();
    let engine = serve_mock_engine("127.0.0.1:0").unwrap();
    for task in tasks().unwrap() {
        let src = phantoms.iter().find(|p| p.to_string_lossy().contains(&task.phantom)).unwrap();
        let mut run = Run {
            agent: Agent::default(),
            ws: dir.path().join("ws").join(&task.id).to_string_lossy().into_owned(),
            engine: engine.url(),
            calls: Vec::new(),
        };
        run.call("study open", json!({ "path": src }));
        let answer = solve(&task.id, &mut run);
        let g = grade(&task, &run.transcript(answer.clone()));
        assert!(g.passed(), "{}: {answer}\n{:?}", task.id, g.notes);
    }
}
