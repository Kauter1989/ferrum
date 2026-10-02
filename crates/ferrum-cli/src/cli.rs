//! Command-line grammar: one sub-command per agent command; each builds
//! the JSON parameters the agent takes.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};

use crate::object;

/// FERRUM agent skill: medical image commands with JSON output.
#[derive(Debug, Parser)]
#[command(name = "ferrum-cli", version, about, long_about = None)]
pub struct Cli {
    /// Operator configuration (default: $FERRUM_AGENT_CONFIG).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// The command.
    #[command(subcommand)]
    pub command: Command,
}

/// Workspace argument shared by most commands.
#[derive(Debug, Args)]
pub struct Ws {
    /// Workspace directory (relative paths go below data.workspace_root).
    #[arg(long, short)]
    pub workspace: String,
}

/// Agent id recorded in the provenance of created items.
#[derive(Debug, Args)]
pub struct AgentId {
    /// Id of the agent or harness session, recorded in the provenance.
    #[arg(long)]
    pub agent: Option<String>,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Studies: scan, open, info.
    #[command(subcommand)]
    Study(Study),
    /// Renders.
    #[command(subcommand)]
    View(View),
    /// Value of the voxel nearest to a point.
    Probe {
        #[command(flatten)]
        ws: Ws,
        /// The point.
        point: String,
    },
    /// Statistics of a region (give exactly one region).
    Stats {
        #[command(flatten)]
        ws: Ws,
        /// Box between two corner points (inclusive).
        #[arg(long = "box", num_args = 2, value_names = ["MIN", "MAX"])]
        bx: Option<Vec<String>>,
        /// Sphere centre (with --radius-mm).
        #[arg(long, requires = "radius_mm")]
        sphere: Option<String>,
        /// Sphere radius in mm.
        #[arg(long)]
        radius_mm: Option<f64>,
        /// Segment label.
        #[arg(long)]
        segment: Option<u64>,
        /// Area or rectangle annotation id.
        #[arg(long)]
        annotation: Option<u64>,
    },
    /// Segmentation engines.
    #[command(subcommand)]
    Engine(EngineCmd),
    /// Values along a line.
    Profile {
        #[command(flatten)]
        ws: Ws,
        /// Start point.
        from: String,
        /// End point.
        to: String,
        /// Number of samples.
        #[arg(long)]
        samples: Option<u64>,
    },
    /// Measurements between points.
    #[command(subcommand)]
    Measure(Measure),
    /// Report, annotations and segments for hand-off.
    #[command(subcommand)]
    Export(Export),
    /// Named annotations.
    #[command(subcommand)]
    Annotate(Annotate),
    /// Segments.
    #[command(subcommand)]
    Segment(Segment),
    /// Proposed items and review decisions.
    #[command(subcommand)]
    Review(Review),
    /// Runs any command with JSON parameters (as the MCP server will).
    Run {
        /// Command name, e.g. "view slice".
        command: String,
        /// Parameters as a JSON object.
        #[arg(long, default_value = "{}")]
        params: String,
    },
    /// Lists the command names.
    Commands,
    /// Prints the JSON Schema of one command's parameters (or of all).
    Schema {
        /// Command name, e.g. "view slice".
        command: Option<String>,
    },
    /// Skill evaluations: tasks, phantoms and grading.
    #[command(subcommand)]
    Eval(Eval),
    /// Serves every command as an MCP tool over stdio.
    Mcp {
        /// Workspace root, if the operator configuration sets none.
        #[arg(long)]
        workspace_root: Option<PathBuf>,
    },
}

/// `study …`.
#[derive(Debug, Subcommand)]
pub enum Study {
    /// Lists the series in files and folders.
    Scan {
        /// Files or folders.
        #[arg(required = true)]
        paths: Vec<String>,
    },
    /// Opens a series into a workspace.
    Open {
        #[command(flatten)]
        ws: Ws,
        /// DICOM folder or NIfTI file.
        path: String,
        /// Series (from study scan) when the source holds several.
        #[arg(long)]
        series: Option<String>,
    },
    /// Describes the open study.
    Info {
        #[command(flatten)]
        ws: Ws,
    },
}

/// `view …`.
#[derive(Debug, Subcommand)]
pub enum View {
    /// Several slices of one plane as a labelled grid.
    Montage {
        #[command(flatten)]
        ws: Ws,
        /// axial, coronal or sagittal.
        #[arg(long)]
        plane: String,
        /// First slice number.
        #[arg(long)]
        from: Option<u64>,
        /// Last slice number.
        #[arg(long)]
        to: Option<u64>,
        /// Every n-th slice.
        #[arg(long)]
        step: Option<u64>,
        /// Tiles per row.
        #[arg(long)]
        columns: Option<u64>,
        /// Window preset (lung, bone, …) or CENTER,WIDTH.
        #[arg(long, allow_hyphen_values = true)]
        window: Option<String>,
        /// Largest side in pixels.
        #[arg(long)]
        size: Option<u64>,
        /// Overlays (segments).
        #[arg(long = "overlay")]
        overlays: Vec<String>,
    },
    /// 3D render on the CPU from a standard viewpoint.
    Volume {
        #[command(flatten)]
        ws: Ws,
        /// mip (default), isosurface or transfer_function.
        #[arg(long)]
        mode: Option<String>,
        /// Isosurface value (e.g. 300 HU for bone).
        #[arg(long, allow_hyphen_values = true)]
        threshold: Option<f64>,
        /// soft_tissue_bone, lung_vessels or bone.
        #[arg(long)]
        preset: Option<String>,
        /// anterior, posterior, left, right, superior or inferior.
        #[arg(long)]
        view: Option<String>,
        /// Image side in pixels.
        #[arg(long)]
        size: Option<u64>,
        /// Overlays (segments).
        #[arg(long = "overlay")]
        overlays: Vec<String>,
    },
    /// Axial, coronal and sagittal slices through a point.
    Mpr {
        #[command(flatten)]
        ws: Ws,
        /// The point.
        at: String,
        /// Window preset (lung, bone, …) or CENTER,WIDTH.
        #[arg(long, allow_hyphen_values = true)]
        window: Option<String>,
        /// Largest side in pixels.
        #[arg(long)]
        size: Option<u64>,
        /// Overlays (segments).
        #[arg(long = "overlay")]
        overlays: Vec<String>,
    },
    /// Renders one slice to PNG + sidecar JSON.
    Slice {
        #[command(flatten)]
        ws: Ws,
        /// axial, coronal or sagittal.
        #[arg(long)]
        plane: String,
        /// 1-based slice number.
        #[arg(long, conflicts_with = "at")]
        slice_number: Option<u64>,
        /// The slice through this point.
        #[arg(long)]
        at: Option<String>,
        /// Window preset (lung, bone, …) or CENTER,WIDTH.
        #[arg(long, allow_hyphen_values = true)]
        window: Option<String>,
        /// Largest side in pixels.
        #[arg(long)]
        size: Option<u64>,
        /// Overlays (segments).
        #[arg(long = "overlay")]
        overlays: Vec<String>,
    },
}

/// `engine …`.
#[derive(Debug, Subcommand)]
pub enum EngineCmd {
    /// Capabilities, labels and licence of an engine.
    Info {
        /// Engine URL (default: the first allowed one).
        #[arg(long)]
        engine: Option<String>,
    },
}

/// `eval …`.
#[derive(Debug, Subcommand)]
pub enum Eval {
    /// Lists the evaluation tasks.
    Tasks,
    /// Writes the phantoms the tasks run on.
    Phantoms {
        /// Output folder.
        dir: PathBuf,
    },
    /// Grades a transcript ({"calls": [...], "answer": "..."}) of one task.
    Grade {
        /// Task id.
        #[arg(long)]
        task: String,
        /// Transcript JSON file.
        transcript: PathBuf,
    },
}

/// `export …`.
#[derive(Debug, Subcommand)]
pub enum Export {
    /// Writes export/ in the workspace with hashes.
    Bundle {
        #[command(flatten)]
        ws: Ws,
    },
}

/// `measure …`.
#[derive(Debug, Subcommand)]
pub enum Measure {
    /// Distance between two points (mm).
    Distance(MeasureArgs),
    /// Angle at the second of three points (degrees).
    Angle(MeasureArgs),
    /// Area of a planar polygon (mm²).
    Area(MeasureArgs),
}

/// Points of a measurement.
#[derive(Debug, Args)]
pub struct MeasureArgs {
    #[command(flatten)]
    ws: Ws,
    /// The points.
    #[arg(required = true)]
    points: Vec<String>,
}

/// `annotate …`.
#[derive(Debug, Subcommand)]
pub enum Annotate {
    /// Adds an annotation on one slice.
    Add {
        #[command(flatten)]
        ws: Ws,
        #[command(flatten)]
        agent: AgentId,
        /// distance, angle, area, rectangle or text.
        #[arg(long)]
        kind: String,
        /// axial, coronal or sagittal.
        #[arg(long)]
        plane: String,
        /// Name shown in the viewer.
        #[arg(long)]
        name: Option<String>,
        /// Text of a text annotation.
        #[arg(long)]
        text: Option<String>,
        /// The points (on one slice).
        #[arg(required = true)]
        points: Vec<String>,
    },
    /// Lists annotations with provenance.
    List {
        #[command(flatten)]
        ws: Ws,
    },
    /// Renames an annotation the agent created.
    Rename {
        #[command(flatten)]
        ws: Ws,
        /// Annotation id.
        id: u64,
        /// New name.
        name: String,
    },
    /// Deletes an annotation the agent created.
    Delete {
        #[command(flatten)]
        ws: Ws,
        /// Annotation id.
        id: u64,
    },
}

/// `segment …`.
#[derive(Debug, Subcommand)]
pub enum Segment {
    /// Lists segments with volumes and provenance.
    List {
        #[command(flatten)]
        ws: Ws,
    },
    /// Region growing from a seed within a value range.
    Threshold {
        #[command(flatten)]
        ws: Ws,
        #[command(flatten)]
        agent: AgentId,
        /// Seed point.
        #[arg(long)]
        seed: String,
        /// Lowest value (e.g. HU).
        #[arg(long, allow_hyphen_values = true)]
        min: f64,
        /// Highest value.
        #[arg(long, allow_hyphen_values = true)]
        max: f64,
        /// Refuse regions larger than this (ml).
        #[arg(long)]
        max_ml: Option<f64>,
        /// Segment name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Prompts an interactive engine (points: `+v:i,j,k` or `-v:i,j,k`; boxes: `box:A:B`).
    Interactive {
        #[command(flatten)]
        ws: Ws,
        /// Engine URL (default: the first allowed one).
        #[arg(long)]
        engine: Option<String>,
        /// Segment name.
        #[arg(long)]
        name: Option<String>,
        /// Prompts in order: +POINT, -POINT, +box:POINT:POINT, -box:POINT:POINT.
        #[arg(required = true, allow_hyphen_values = true)]
        prompts: Vec<String>,
    },
    /// Runs an automatic engine.
    Auto {
        #[command(flatten)]
        ws: Ws,
        /// Engine URL (default: the first allowed one).
        #[arg(long)]
        engine: Option<String>,
        /// Structures (default: all).
        #[arg(long = "label")]
        labels: Vec<String>,
    },
    /// Renames a segment the agent created.
    Rename {
        #[command(flatten)]
        ws: Ws,
        /// Segment label.
        label: u64,
        /// New name.
        name: String,
    },
    /// Deletes a segment the agent created.
    Delete {
        #[command(flatten)]
        ws: Ws,
        /// Segment label.
        label: u64,
    },
}

/// `review …`.
#[derive(Debug, Subcommand)]
pub enum Review {
    /// Lists proposed annotations and segments.
    List {
        #[command(flatten)]
        ws: Ws,
    },
    /// Confirms an item (only if the operator allows it).
    Confirm(Decision),
    /// Rejects an item (only if the operator allows it).
    Reject(Decision),
}

/// A review decision.
#[derive(Debug, Args)]
pub struct Decision {
    #[command(flatten)]
    ws: Ws,
    /// Annotation id.
    #[arg(long, conflicts_with = "segment")]
    annotation: Option<u64>,
    /// Segment label.
    #[arg(long)]
    segment: Option<u64>,
    /// The person who decided.
    #[arg(long)]
    by: String,
}

/// `v:i,j,k`, `mm:x,y,z` or `r-0001:x,y` → JSON point.
pub fn parse_point(s: &str) -> Result<Value, String> {
    let (kind, rest) =
        s.split_once(':').ok_or_else(|| format!("point {s:?}: expected v:i,j,k, mm:x,y,z or r-0001:x,y"))?;
    let nums: Vec<f64> = rest
        .split(',')
        .map(|x| x.trim().parse::<f64>().map_err(|_| format!("point {s:?}: {x:?} is not a number")))
        .collect::<Result<_, _>>()?;
    match (kind, nums.len()) {
        ("v" | "voxel", 3) => Ok(json!({ "voxel": nums })),
        ("mm" | "patient_mm", 3) => Ok(json!({ "patient_mm": nums })),
        (r, 2) if r.starts_with("r-") => Ok(json!({ "render": r, "pixel": nums })),
        _ => Err(format!("point {s:?}: expected v:i,j,k, mm:x,y,z or r-0001:x,y")),
    }
}

/// `+POINT` / `-POINT` (include / exclude) or `±box:POINT:POINT`.
pub fn parse_prompt(s: &str) -> Result<Value, String> {
    let (positive, rest) = match s.split_at_checked(1) {
        Some(("+", r)) => (true, r),
        Some(("-", r)) => (false, r),
        _ => (true, s),
    };
    if let Some(b) = rest.strip_prefix("box:") {
        let (a, z) = split_box(b).ok_or_else(|| format!("prompt {s:?}: expected box:POINT:POINT"))?;
        return Ok(json!({ "type": "box", "positive": positive, "min": parse_point(a)?, "max": parse_point(z)? }));
    }
    Ok(json!({ "type": "point", "positive": positive, "point": parse_point(rest)? }))
}

/// Splits `v:1,2,3:v:4,5,6` into its two points.
fn split_box(b: &str) -> Option<(&str, &str)> {
    let rest = b.get(1..)?;
    let second =
        [":v:", ":mm:", ":voxel:", ":patient_mm:", ":r-"].iter().filter_map(|k| rest.find(k).map(|i| i + 2)).min()?;
    Some((b[..second].trim_end_matches(':'), &b[second..]))
}

/// Preset name or `CENTER,WIDTH`.
pub fn parse_window(s: &str) -> Result<Value, String> {
    match s.split_once(',') {
        Some((c, w)) => {
            let n = |x: &str| x.trim().parse::<f64>().map_err(|_| format!("window {s:?}: expected CENTER,WIDTH"));
            Ok(json!({ "center": n(c)?, "width": n(w)? }))
        }
        None => Ok(json!(s)),
    }
}

fn points(list: &[String]) -> Result<Value, String> {
    list.iter().map(|p| parse_point(p)).collect::<Result<Vec<_>, _>>().map(Value::Array)
}

fn ws(w: &Ws) -> (&'static str, Option<Value>) {
    ("workspace", Some(json!(w.workspace)))
}

fn some<T: Into<Value>>(v: Option<T>) -> Option<Value> {
    v.map(Into::into)
}

impl Command {
    /// Command name and JSON parameters.
    pub fn to_call(&self) -> Result<(String, Value), String> {
        let call = |name: &str, fields: Vec<(&str, Option<Value>)>| (name.to_owned(), object(fields));
        Ok(match self {
            Command::Study(s) => match s {
                Study::Scan { paths } => call("study scan", vec![("paths", Some(json!(paths)))]),
                Study::Open { ws: w, path, series } => {
                    call("study open", vec![ws(w), ("path", Some(json!(path))), ("series", some(series.clone()))])
                }
                Study::Info { ws: w } => call("study info", vec![ws(w)]),
            },
            Command::View(v) => view_call(v)?,
            Command::Profile { ws: w, from, to, samples } => call(
                "profile",
                vec![
                    ws(w),
                    ("from", Some(parse_point(from)?)),
                    ("to", Some(parse_point(to)?)),
                    ("samples", some(*samples)),
                ],
            ),
            Command::Export(Export::Bundle { ws: w }) => call("export bundle", vec![ws(w)]),
            Command::Engine(EngineCmd::Info { engine }) => call("engine info", vec![("engine", some(engine.clone()))]),
            Command::Probe { ws: w, point } => call("probe", vec![ws(w), ("point", Some(parse_point(point)?))]),
            Command::Stats { ws: w, bx, sphere, radius_mm, segment, annotation } => {
                let bx = bx.as_ref().map(|b| -> Result<Value, String> {
                    Ok(json!({ "min": parse_point(&b[0])?, "max": parse_point(&b[1])? }))
                });
                let sphere = sphere.as_deref().map(|c| -> Result<Value, String> {
                    Ok(json!({ "center": parse_point(c)?, "radius_mm": radius_mm }))
                });
                call(
                    "stats",
                    vec![
                        ws(w),
                        ("box", bx.transpose()?),
                        ("sphere", sphere.transpose()?),
                        ("segment", some(*segment)),
                        ("annotation", some(*annotation)),
                    ],
                )
            }
            Command::Measure(m) => {
                let (name, a) = match m {
                    Measure::Distance(a) => ("measure distance", a),
                    Measure::Angle(a) => ("measure angle", a),
                    Measure::Area(a) => ("measure area", a),
                };
                call(name, vec![ws(&a.ws), ("points", Some(points(&a.points)?))])
            }
            Command::Annotate(a) => annotate_call(a)?,
            Command::Segment(s) => segment_call(s)?,
            Command::Review(r) => match r {
                Review::List { ws: w } => call("review list", vec![ws(w)]),
                Review::Confirm(d) | Review::Reject(d) => {
                    let name = if matches!(r, Review::Confirm(_)) { "review confirm" } else { "review reject" };
                    call(
                        name,
                        vec![
                            ws(&d.ws),
                            ("annotation", some(d.annotation)),
                            ("segment", some(d.segment)),
                            ("by", Some(json!(d.by))),
                        ],
                    )
                }
            },
            Command::Run { command, params } => {
                let p: Value = serde_json::from_str(params).map_err(|e| format!("--params: {e}"))?;
                (command.clone(), p)
            }
            Command::Commands => ("commands".to_owned(), json!({})),
            Command::Schema { command } => ("schema".to_owned(), json!({ "command": command })),
            Command::Eval(e) => match e {
                Eval::Tasks => ("eval tasks".to_owned(), json!({})),
                Eval::Phantoms { dir } => ("eval phantoms".to_owned(), json!({ "dir": dir })),
                Eval::Grade { task, transcript } => {
                    ("eval grade".to_owned(), json!({ "task": task, "transcript": transcript }))
                }
            },
            Command::Mcp { workspace_root } => ("mcp".to_owned(), json!({ "workspace_root": workspace_root })),
        })
    }
}

fn view_call(v: &View) -> Result<(String, Value), String> {
    let call = |name: &str, fields: Vec<(&str, Option<Value>)>| (name.to_owned(), object(fields));
    Ok(match v {
        View::Slice { ws: w, plane, slice_number, at, window, size, overlays } => call(
            "view slice",
            vec![
                ws(w),
                ("plane", Some(json!(plane))),
                ("slice_number", some(*slice_number)),
                ("at", at.as_deref().map(parse_point).transpose()?),
                ("window", window.as_deref().map(parse_window).transpose()?),
                ("size", some(*size)),
                ("overlays", (!overlays.is_empty()).then(|| json!(overlays))),
            ],
        ),
        View::Montage { ws: w, plane, from, to, step, columns, window, size, overlays } => call(
            "view montage",
            vec![
                ws(w),
                ("plane", Some(json!(plane))),
                ("from", some(*from)),
                ("to", some(*to)),
                ("step", some(*step)),
                ("columns", some(*columns)),
                ("window", window.as_deref().map(parse_window).transpose()?),
                ("size", some(*size)),
                ("overlays", (!overlays.is_empty()).then(|| json!(overlays))),
            ],
        ),
        View::Mpr { ws: w, at, window, size, overlays } => call(
            "view mpr",
            vec![
                ws(w),
                ("at", Some(parse_point(at)?)),
                ("window", window.as_deref().map(parse_window).transpose()?),
                ("size", some(*size)),
                ("overlays", (!overlays.is_empty()).then(|| json!(overlays))),
            ],
        ),
        View::Volume { ws: w, mode, threshold, preset, view, size, overlays } => call(
            "view volume",
            vec![
                ws(w),
                ("mode", some(mode.clone())),
                ("threshold", some(*threshold)),
                ("preset", some(preset.clone())),
                ("view", some(view.clone())),
                ("size", some(*size)),
                ("overlays", (!overlays.is_empty()).then(|| json!(overlays))),
            ],
        ),
    })
}

fn annotate_call(a: &Annotate) -> Result<(String, Value), String> {
    Ok(match a {
        Annotate::Add { ws: w, agent, kind, plane, name, text, points: pts } => (
            "annotate add".into(),
            object(vec![
                ws(w),
                ("agent", some(agent.agent.clone())),
                ("kind", Some(json!(kind))),
                ("plane", Some(json!(plane))),
                ("name", some(name.clone())),
                ("text", some(text.clone())),
                ("points", Some(points(pts)?)),
            ]),
        ),
        Annotate::List { ws: w } => ("annotate list".into(), object(vec![ws(w)])),
        Annotate::Rename { ws: w, id, name } => {
            ("annotate rename".into(), object(vec![ws(w), ("id", Some(json!(id))), ("name", Some(json!(name)))]))
        }
        Annotate::Delete { ws: w, id } => ("annotate delete".into(), object(vec![ws(w), ("id", Some(json!(id)))])),
    })
}

fn segment_call(s: &Segment) -> Result<(String, Value), String> {
    Ok(match s {
        Segment::List { ws: w } => ("segment list".into(), object(vec![ws(w)])),
        Segment::Threshold { ws: w, agent, seed, min, max, max_ml, name } => (
            "segment threshold".into(),
            object(vec![
                ws(w),
                ("agent", some(agent.agent.clone())),
                ("seed", Some(parse_point(seed)?)),
                ("min", Some(json!(min))),
                ("max", Some(json!(max))),
                ("max_ml", some(*max_ml)),
                ("name", some(name.clone())),
            ]),
        ),
        Segment::Interactive { ws: w, engine, name, prompts } => (
            "segment interactive".into(),
            object(vec![
                ws(w),
                ("engine", some(engine.clone())),
                ("name", some(name.clone())),
                (
                    "prompts",
                    Some(Value::Array(prompts.iter().map(String::as_str).map(parse_prompt).collect::<Result<_, _>>()?)),
                ),
            ]),
        ),
        Segment::Auto { ws: w, engine, labels } => (
            "segment auto".into(),
            object(vec![
                ws(w),
                ("engine", some(engine.clone())),
                ("labels", (!labels.is_empty()).then(|| json!(labels))),
            ]),
        ),
        Segment::Rename { ws: w, label, name } => {
            ("segment rename".into(), object(vec![ws(w), ("label", Some(json!(label))), ("name", Some(json!(name)))]))
        }
        Segment::Delete { ws: w, label } => {
            ("segment delete".into(), object(vec![ws(w), ("label", Some(json!(label)))]))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_and_windows() {
        assert_eq!(parse_point("v:1,2,3").unwrap(), json!({ "voxel": [1.0, 2.0, 3.0] }));
        assert_eq!(parse_point("mm:-42.1,10.5,-130").unwrap(), json!({ "patient_mm": [-42.1, 10.5, -130.0] }));
        assert_eq!(parse_point("r-0007:412,318").unwrap(), json!({ "render": "r-0007", "pixel": [412.0, 318.0] }));
        for bad in ["1,2,3", "v:1,2", "mm:a,b,c", "x:1,2,3"] {
            assert!(parse_point(bad).is_err(), "{bad}");
        }
        assert_eq!(
            parse_prompt("+v:1,2,3").unwrap(),
            json!({ "type": "point", "positive": true, "point": { "voxel": [1.0, 2.0, 3.0] } })
        );
        assert_eq!(parse_prompt("-mm:1,2,3").unwrap()["positive"], false);
        let b = parse_prompt("box:v:1,2,3:mm:-4,5,6").unwrap();
        assert_eq!((b["min"]["voxel"][2].as_f64(), b["max"]["patient_mm"][0].as_f64()), (Some(3.0), Some(-4.0)));
        assert_eq!(parse_prompt("-box:r-0001:1,2:r-0001:5,6").unwrap()["max"]["render"], "r-0001");
        assert!(parse_prompt("box:v:1,2,3").is_err() && parse_prompt("+x").is_err());
        assert_eq!(parse_window("lung").unwrap(), json!("lung"));
        assert_eq!(parse_window("-600,1500").unwrap(), json!({ "center": -600.0, "width": 1500.0 }));
        assert!(parse_window("a,b").is_err());
    }

    #[test]
    fn command_lines_become_calls() {
        let call = |args: &[&str]| Cli::try_parse_from(args).unwrap().command.to_call().unwrap();
        let (name, p) = call(&["ferrum-cli", "measure", "distance", "-w", "ws", "v:0,0,0", "mm:1,2,3"]);
        assert_eq!(name, "measure distance");
        assert_eq!(p["points"][1], json!({ "patient_mm": [1.0, 2.0, 3.0] }));
        let (name, p) = call(&[
            "ferrum-cli",
            "view",
            "slice",
            "-w",
            "ws",
            "--plane",
            "axial",
            "--slice-number",
            "3",
            "--window",
            "-600,1500",
            "--overlay",
            "segments",
        ]);
        assert_eq!(
            (name.as_str(), &p["window"]["center"], &p["overlays"][0]),
            ("view slice", &json!(-600.0), &json!("segments"))
        );
        assert!(p.get("at").is_none(), "absent options are left out");
        let (_, p) = call(&["ferrum-cli", "stats", "-w", "ws", "--box", "v:0,0,0", "v:2,2,2"]);
        assert_eq!(p["box"]["max"], json!({ "voxel": [2.0, 2.0, 2.0] }));
        let (_, p) = call(&["ferrum-cli", "stats", "-w", "ws", "--sphere", "v:1,1,1", "--radius-mm", "2"]);
        assert_eq!(p["sphere"]["radius_mm"], json!(2.0));
        let (_, p) = call(&[
            "ferrum-cli",
            "segment",
            "threshold",
            "-w",
            "ws",
            "--seed",
            "v:1,1,1",
            "--min",
            "-100",
            "--max",
            "200",
        ]);
        assert_eq!((p["min"].as_f64(), p["max"].as_f64()), (Some(-100.0), Some(200.0)));
        let (name, p) = call(&["ferrum-cli", "run", "probe", "--params", r#"{"workspace":"w"}"#]);
        assert_eq!((name.as_str(), p["workspace"].as_str()), ("probe", Some("w")));
        let (name, p) = call(&["ferrum-cli", "review", "reject", "-w", "ws", "--segment", "2", "--by", "dr.k"]);
        assert_eq!((name.as_str(), p["segment"].as_u64()), ("review reject", Some(2)));
        let bad = Cli::try_parse_from(["ferrum-cli", "probe", "-w", "ws", "1,2,3"]).unwrap().command.to_call();
        assert!(bad.is_err());
        assert!(
            Cli::try_parse_from(["ferrum-cli", "stats", "-w", "ws", "--sphere", "v:1,1,1"]).is_err(),
            "radius is required"
        );
    }
}
