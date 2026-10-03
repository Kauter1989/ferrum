# `ferrum-cli`: the agent skill on the command line and over MCP

`ferrum-cli` lets an AI agent, or a script, work with a medical image the
way a careful person works with FERRUM:
- open a series;
- look at slices;
- take numbers from probes, statistics and measurements;
- leave annotations and segments for a clinician to review.

The design is in [agent-skill.md](agent-skill.md); segmentation with
engines in [agent-segmentation.md](agent-segmentation.md). This page is
the reference for what is implemented (Stages 15 and 17).

```bash
cargo build --release -p ferrum-cli        # target/release/ferrum-cli
ferrum-cli study open -w ws/ct1 /data/incoming/lung_053.nii.gz
ferrum-cli view slice -w ws/ct1 --plane axial --slice-number 120 --window lung
ferrum-cli probe -w ws/ct1 r-0001:412,318
ferrum-cli measure distance -w ws/ct1 mm:-42.1,10.5,-130 mm:-30.4,12.0,-130
```

## Output

Every command prints one JSON envelope on stdout:

```json
{ "api": "ferrum-agent/1", "ok": true, "data": { … }, "warnings": [ … ],
  "provenance": { "ferrum": "FERRUM 0.2.3", "command": "probe", "params": { … },
                  "source_sha256": "…", "renderer": "cpu", "time": "2026-10-01T12:00:00Z" } }
```

| Exit code | Meaning |
|---|---|
| 0 | success (`ok: true`) |
| 1 | command error: `ok: false` and `error: {code, message, hint}` |
| 2 | usage error (unknown command, malformed argument); message on stderr |

Error codes: `bad_request`, `no_study`, `not_found`, `out_of_volume`,
`source_changed`, `forbidden`, `limit`, `internal`. The `hint` says what
to do next.

## Points

| Written | JSON | Meaning |
|---|---|---|
| `v:251,198,156` | `{"voxel": [251, 198, 156]}` | 0-based voxel `(i, j, k)` of the canonical LPS grid; integers are voxel centres |
| `mm:-42.1,10.5,-130` | `{"patient_mm": [-42.1, 10.5, -130]}` | LPS patient millimetres |
| `r-0007:412,318` | `{"render": "r-0007", "pixel": [412, 318]}` | pixel of an earlier render; pixel centres lie at `+0.5` |

Every output point carries all forms:
- `voxel`: continuous;
- `voxel_index`: the nearest voxel;
- `patient_mm`.

Slice numbers are 1-based (`slice_number`), as in the viewer.

## Commands

All commands except `study scan` take `-w/--workspace`. Relative
workspace paths are placed under the operator's `data.workspace_root`.

| Command | What it does |
|---|---|
| `study scan <paths…>` | Lists series: pseudonymised id, format, modality, dims, description |
| `study open -w W <path> [--series S] [--modality M]` | Creates or reuses the workspace W for a series (SHA-256 of every source file) and describes it; `--modality` declares the modality of a source that carries none (NIfTI), e.g. `CT` for values in HU, and is kept in the workspace |
| `study info -w W` | Dims, spacing, LPS origin and direction, value unit, slice counts, window presets, annotation and segment counts |
| `view slice -w W --plane P (--slice-number N \| --at POINT) [--window lung\|C,W] [--size PX] [--overlay segments [--segment-style outline\|fill\|fill_outline] [--segment-opacity A]]` | Renders a slice to `renders/r-NNNN.png` with a sidecar JSON |
| `view montage -w W --plane P [--from N] [--to N] [--step N] [--columns N] [--window …] [--size PX]` | Several slices of one plane as a grid; every tile labelled with its slice number (default: at most 16 tiles) |
| `view mpr -w W POINT [--window …] [--size PX]` | Axial, coronal and sagittal slices through a point, with a crosshair |
| `view volume -w W [--mode mip\|isosurface\|transfer_function] [--threshold V] [--preset soft_tissue_bone\|lung_vessels\|bone] [--view anterior\|…\|inferior] [--size PX]` | 3D render on the CPU from a standard viewpoint |
| `probe -w W POINT` | Value of the nearest voxel, with unit (HU for CT) |
| `stats -w W (--box A B \| --sphere C --radius-mm R \| --segment L [--box A B] \| --annotation ID)` | Voxels, volume in ml, mean, std, min, max, percentiles; a segment with a box gives the part of the segment inside the box (e.g. a slab of slices) |
| `profile -w W FROM TO [--samples N]` | Values along a line with distances in mm (default: one sample per smallest voxel spacing) |
| `measure distance\|angle\|area -w W POINT…` | Value, unit, method and points; distances also give an uncertainty of one voxel spacing |
| `annotate add -w W --kind K --plane P [--name N] [--text T] [--agent ID] POINT…` | Distance, angle, area, rectangle or text on one slice, proposed by the agent |
| `annotate list\|rename\|delete -w W …` | Annotations with provenance; agents rename or delete only their own |
| `segment list -w W` | Segments with voxel count, ml and provenance |
| `segment threshold -w W --seed POINT --min V --max V [--max-ml ML] [--name N] [--agent ID]` | 6-connected region growing from a seed within a value range, as a proposed segment; existing segments are kept |
| `segment rename\|delete -w W …` | Only segments the agent created or asked an engine for |
| `segment shape -w W L` | Shape from the voxels: ml, extent in mm, slice ranges and largest slices, axial long and short axis with end points, border contact, components, laterality (for names with *left*/*right*) |
| `segment components -w W L [--min-ml ML] [--split]` | 26-connected components, largest first, with centroid, box and ml; `--split` makes a segment of each further component of at least `--min-ml` |
| `segment compare -w W A B [--b-workspace W2]` | Dice, Jaccard, volumes and difference, HD95 and Hausdorff distance in mm, centroid distance; B may be in another workspace of the same series |
| `segment edit -w W L --op keep_largest\|fill_holes\|restrict_to_box\|remove_small [--box A B] [--min-ml ML]` | Deterministic clean-up of a segment the agent created or asked an engine for |
| `engine list` | Every allowed engine: reachable, name, version, modes, `deterministic`, `research_only`, GPU group |
| `engine info [--engine URL]` | Capabilities, labels, `deterministic`, `research_only` and licence of a `ferrum-engine/1` engine |
| `segment interactive -w W [--engine URL] [--name N] [--agent ID] [--segment L [--append \| --undo]] [--from-segment L] [--roi A B \| --whole-volume] [--min-ml ML] [--max-ml ML] [--modality M] PROMPT…` | Prompts an interactive engine ([below](#segmentation-engines)): `+POINT` / `-POINT`, `±box:POINT:POINT`, `±lasso:P1;P2;P3`, `±scribble:P1;P2`. A new object becomes a segment proposed by the engine; `--segment` refines one made earlier |
| `segment auto -w W [--engine URL] [--label NAME]… [--name-prefix P] [--modality M] [--agent ID]` | Runs an automatic engine (e.g. TotalSegmentator); every structure found becomes a segment proposed by the engine, with checks; existing segments keep their voxels |
| `review list -w W` | Proposed annotations and segments |
| `export bundle -w W [--format ferrum\|dicom]…` | `export/` in the workspace, each file with SHA-256: `report.json` and, by format, `annotations.json` + `segments.nii.gz` + `segments.json` (`ferrum`, default) or `segmentation.dcm` (DICOM SEG) + `measurements.dcm` (SR, TID 1500) (`dicom`); unconfirmed items are marked in every file, rejected ones left out of DICOM ([format](workspace-format.md#5-dicom-export)) |
| `review confirm\|reject -w W (--annotation ID \| --segment L) --by NAME` | Only if the operator allows harness review |
| `run "<command>" --params '<json>'` | Any command with JSON parameters: the same call the MCP server makes |
| `commands` | The command names |
| `schema ["<command>"]` | JSON Schema of a command's parameters (or of all commands) |
| `eval tasks\|phantoms\|grade\|engine` | Skill evaluations ([below](#skill-package-and-evaluations)); `eval engine [ADDR]` serves FERRUM's mock engine |
| `mcp [--workspace-root DIR]` | Serves every command as an MCP tool over stdio ([below](#mcp-server)) |

**Parameter checks:** every call is checked against its JSON Schema
before it runs. A missing, misspelt or mistyped parameter is a
`bad_request` that names the parameter, e.g. `params.points[1]: not a
point`.

## Renders

`view slice` writes a PNG and a sidecar JSON with the mapping:

```json
{ "render": "r-0001", "kind": "slice", "plane": "axial", "slice_number": 16, "size": [128, 128],
  "pixel_mm": 0.3125, "window": { "center": 400, "width": 1800 },
  "pixel_to_voxel": [[0.3125, 0, -0.5], [0, 0.3125, -0.5], [0, 0, 15]],
  "pixel_to_patient_mm": [[0.3125, 0, -20.5], [0, 0.3125, -20.5], [0, 0, 130]],
  "orientation": { "left": "R", "right": "L", "top": "A", "bottom": "P" } }
```

**Pixels:**
- Pixels are square, and the image keeps the slice's physical aspect
  ratio.
- Each pixel shows the nearest voxel; no values are interpolated.
- The largest side is `--size` (default 768), capped by the operator's
  `max_render_px`.

**Mapping:**
- The maps are affine in `(x, y, 1)`.
- An agent can point at what it sees with `r-0001:x,y`.
- Take every number from `probe`, `stats` or `measure`, never from grey
  values.

**Overlays:** `segments` draws the segments in their colours:
- `segment_style`: `outline` (default; a closed outline 1–3 pixels wide,
  about a third of a voxel on screen; the
  image stays visible inside), `fill` (translucent fill) or
  `fill_outline` (both);
- `segment_opacity` (0–1): the fill opacity for every segment; default
  each segment's own opacity.

`view montage` and `view mpr` take the same options; the sidecar records
them.

**Tiled renders.**
- `view montage` and `view mpr` compose several slices into one image.
  Their sidecars list `tiles`, each with:
  - `plane`, `slice_number` and `origin` (its top-left pixel);
  - its own `pixel_to_voxel` and `pixel_to_patient_mm` maps, in pixel
    coordinates relative to the origin.
- `r-0003:x,y` on a tiled render resolves through the tile under the
  pixel. A pixel between tiles is `out_of_volume`.
- Tiles are labelled with their slice number, and MPR tiles also with
  `A`, `C` or `S`. MPR draws a crosshair with a small gap at the point.
- Labels are numbers and plane letters only, never identifiers.

**3D renders.**
- `view volume` uses the CPU reference ray caster, the same image
  formation as the desktop app's GPU renderer.
- Modes:
  - `mip`;
  - `isosurface` at a threshold in data values;
  - a CT transfer-function preset.
- The camera looks from one of six standard viewpoints.
- A 3D pixel does not correspond to one voxel. The sidecar has no
  mapping, and pointing into a 3D render is a `bad_request`.

## Provenance and review

Everything created through `ferrum-cli` is **proposed**:
- `author: agent`, with the `--agent` id if one is given;
- shown in the desktop app with **Confirm / Reject**.

Agents may change or delete only items that agents created, and engine
segments made at an agent's request (`provenance.requested_by`), until
a person confirms them. Items drawn by people, engine results nobody
asked for through the agent, and confirmed segments are protected
(`forbidden`). Changing a segment sets it back to
*proposed*.

A harness may confirm or reject items only when the operator sets
`review.allow_harness_confirmation = true`. It must name the person
(`--by`), and the decision goes to the audit log.

## Workspace and audit

**Workspace:**
- The workspace (`ferrum-workspace` v1, see
  [workspace-format.md](workspace-format.md)) holds the results:
  annotations, segments, renders, `audit.jsonl` and, for engine
  segments, the prompts behind them (`engine_inputs.json`).
- Every call reloads the series and first verifies the source hashes. A
  changed source fails with `source_changed`.

**Audit log:**
- Every call on a workspace appends one line: time, command, parameters,
  success or error code, and the source hash.
- Reviews are logged the same way.

## Operator configuration

The operator sets the configuration in `ferrum-agent.toml`, passed with
`--config` or `FERRUM_AGENT_CONFIG`. Commands cannot change it.

```toml
[data]
read_roots = ["/data/incoming"]       # sources outside are refused (forbidden)
workspace_root = "/data/workspaces"   # relative workspaces go here; others are refused

[privacy]
expose_identifiers = false            # accession numbers
expose_dates = false                  # study date and time
pseudonymise_uids = true              # UIDs and series ids become salted hashes (anon-…)
salt = "site-secret"

[network]
engines = ["http://127.0.0.1:8765", "http://127.0.0.1:8766"]   # segmentation engines the agent may use
gpu_groups = { gpu0 = ["http://127.0.0.1:8765", "http://127.0.0.1:8766"] }  # engines sharing a GPU: one call at a time

[limits]
max_render_px = 1024
max_voxels = 600_000_000
engine_job_timeout_s = 900            # also the longest wait for a GPU group
max_prompts_per_object = 8            # interactive prompts per object
roi_margin_mm = 48                    # region around the prompts sent to interactive engines
allow_research_only = true            # false: research-only engines are forbidden

[review]
allow_harness_confirmation = false
```

- Unknown or mistyped settings are an error, never a silent default.
- Without a configuration, every path is readable. Each envelope then
  warns about it.
- Outputs never contain patient names, IDs or birth dates. The agent does
  not output DICOM attributes.

## MCP server

`ferrum-cli mcp` serves the same commands to MCP-capable harnesses. It
uses the Model Context Protocol over stdio: JSON-RPC 2.0, one message per
line, protocol revisions 2025-06-18, 2025-03-26 and 2024-11-05.

```json
{ "mcpServers": {
    "ferrum": { "command": "ferrum-cli", "args": ["mcp", "--workspace-root", "/data/workspaces"],
                "env": { "FERRUM_AGENT_CONFIG": "/etc/ferrum-agent.toml" } } } }
```

**Tools:**
- Every command is a tool named `ferrum_<command>`, e.g.
  `ferrum_view_slice` or `ferrum_measure_distance`.
- Each tool has its JSON Schema as `inputSchema` and hints for clients:
  read-only, destructive.

**Results:**
- A tool result holds the same envelope as the command line, as text and
  as `structuredContent`.
- Renders add the PNG as image content.
- Command errors are tool results with `isError: true`, not protocol
  errors, so the agent sees the error code and hint.

**Guidance:** the server's `instructions` summarise the clinical safety
rules: not a medical device, numbers from tools, no diagnoses, review at
the end.

**Loaded series stay in memory between calls:**
- A series is reused while its source files keep their size and
  modification time; otherwise it is hashed and loaded again.
- Annotations and segments are read from the workspace on every call, so
  decisions made meanwhile in the desktop app are never overwritten.

**Workspace root:** `--workspace-root` applies only when the operator
configuration sets none.

**Equivalence:** a test runs the same session through the command line
and through MCP and checks that both give identical JSON.

## Segmentation engines

`engine list`, `engine info`, `segment interactive` and `segment auto`
reach engines over the [FERRUM Engine Protocol](engine-protocol.md):
FERRUM's mock engine, or the nnInteractive, TotalSegmentator and MONAI
Label bridges. The scenarios they serve are in
[agent-segmentation.md](agent-segmentation.md).

**Which engines:**
- The operator lists the allowed URLs in `[network] engines`. The first
  one is the default.
- Without a configuration file, only loopback URLs (`127.0.0.1`,
  `localhost`, `[::1]`) are allowed. Any other URL is `forbidden`.
- `FERRUM_ENGINE_URL` supplies a default when none is listed.
- With `limits.allow_research_only = false`, engines that report
  `research_only` are `forbidden` for segmentation (`engine info` still
  works).

**One GPU, several engines:**
- `network.gpu_groups` names engines that share a GPU. FERRUM runs one
  engine call of a group at a time, across processes (a lock file per
  group in the temporary folder), waiting up to `engine_job_timeout_s`.
- On a 12 GB card, nnInteractive and the full TotalSegmentator model fit
  only in turn; the bridges free their GPU memory between calls
  ([ai-demo.md](ai-demo.md#one-gpu-for-all-engines)).

**Interactive objects:**
- The prompts of every object are stored in the workspace
  (`engine_inputs.json`, [format](workspace-format.md)). `--segment L`
  refines that object: `--append` adds prompts to the stored ones,
  `--undo` drops the last one, otherwise the new prompts replace them.
  Each call replays the prompts on a fresh engine session and writes the
  result into the same segment, as a new `revision`.
- `--from-segment L` redoes a segment the agent made (e.g. by threshold)
  with the engine, seeded with one lasso per plane from its mask.
- Only a **region of interest** is uploaded: the prompts' bounding box
  plus `roi_margin_mm`, or `--roi`, or `--whole-volume`. An object that
  reaches an inner face of the region fails the `roi` check.
- Lassos and scribbles are points on one slice (`lasso:P1;P2;P3`): the
  lasso is filled, the scribble is a one-voxel stroke.
- At most `max_prompts_per_object` prompts per object (lasso seeds of
  `--from-segment` not counted); then the call fails with `limit`.

**Checks:** every engine result carries `checks` (`failed` lists the
failed ones) and a warning per failure: `empty`, `size` (`--min-ml`,
`--max-ml`), `components` (largest part < 90 %), `border` (touches the
volume edge), `roi`, `laterality`, `overlap` (voxels kept by other
segments), `stability` (Dice to the previous revision < 0.95).

**Privacy and limits:**
- The token comes from `FERRUM_ENGINE_TOKEN`, never from a file or a
  parameter.
- Engines receive voxels, geometry and the modality, never identifiers.
  NIfTI has no modality: give `--modality`.
- An automatic job may run up to `limits.engine_job_timeout_s` seconds
  (default 900). After that it is cancelled and reported as `limit`.

**Results:**
- Results are segments proposed by the engine: author `engine` (name,
  version, `research_only`), `requested_by` the agent, status `proposed`.
- Envelopes warn when the engine is for research use only, and when it
  does not guarantee identical results on replay (`deterministic: false`).

**MCP:** the server keeps the engine session of an interactive object
open (up to four), so a refinement sends only the new prompts; results
equal those of the command line. Sessions of engines in a GPU group are
closed after each call. With a `progressToken`, automatic jobs send
`notifications/progress`.

## Skill package and evaluations

[`skills/ferrum/`](../skills/ferrum) is the skill an agent loads:
- `SKILL.md`: when and how to use FERRUM safely;
- `reference/`: pages the agent reads when needed;
- `schemas/commands.json`: generated by `ferrum-cli schema`; a test fails
  when it is stale.

The release archives contain `ferrum-cli`, the skill, and
`ferrum-plugin/`: a plugin folder with the skill, a manifest, an MCP
configuration and the bundled binary.

**Evaluations** (`skills/ferrum/evals/`) are tasks with known answers on
a synthetic phantom, e.g. "measure the diameter of the round object" or
"on which axial slice is it largest". Tasks marked `engine` need a
segmentation engine: `ferrum-cli eval engine` serves FERRUM's mock
engine.

| Command | What it does |
|---|---|
| `ferrum-cli eval tasks` | Lists the tasks |
| `ferrum-cli eval phantoms DIR` | Writes the phantoms |
| `ferrum-cli eval grade --task ID transcript.json` | Grades a transcript; exit 0 when every check passes |

A transcript is `{"calls": [{command, arguments, result}], "answer"}`.
The grader checks that the answer:
- has the right numbers within the tolerance;
- states the unit;
- takes its numbers from tool results;
- makes no diagnostic claims;
- asks for review when proposals were created;
- used the commands the task requires (e.g. `segment interactive`).

The test suite holds a reference solution for every task.

**Privacy:** an end-to-end test runs every command on a CT series whose
header carries a patient name, ID, birth date, accession number,
institution, dates and UIDs. It scans every envelope, render and export
file, and none of these strings may appear. Annotation reports written by
the agent pass the same filter as `study info`: UIDs pseudonymised, dates
and accession numbers only with consent.

## Limits

Note:
- NIfTI files carry no modality, so their values have no unit. If you
  know the file is CT, open it with `--modality CT`; values are then
  reported in HU (`modality_source: declared`).
- The command line reloads the series for each call (a CT series in about
  0.3 s); the MCP server keeps it in memory.
