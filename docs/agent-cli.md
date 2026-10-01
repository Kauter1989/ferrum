# `ferrum-cli`: the agent skill on the command line

`ferrum-cli` lets an AI agent, or a script, work with a medical image the
way a careful person works with FERRUM:
- open a series;
- look at slices;
- take numbers from probes, statistics and measurements;
- leave annotations and segments for a clinician to review.

The design is in [agent-skill.md](agent-skill.md). This page is the
reference for what is implemented (Stage 15.3, first part).

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
  "provenance": { "ferrum": "FERRUM 0.1.0", "command": "probe", "params": { … },
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
| `study open -w W <path> [--series S]` | Creates or reuses the workspace W for a series (SHA-256 of every source file) and describes it |
| `study info -w W` | Dims, spacing, LPS origin and direction, value unit, slice counts, window presets, annotation and segment counts |
| `view slice -w W --plane P (--slice-number N \| --at POINT) [--window lung\|C,W] [--size PX] [--overlay segments]` | Renders a slice to `renders/r-NNNN.png` with a sidecar JSON |
| `probe -w W POINT` | Value of the nearest voxel, with unit (HU for CT) |
| `stats -w W (--box A B \| --sphere C --radius-mm R \| --segment L \| --annotation ID)` | Voxels, volume in ml, mean, std, min, max, percentiles |
| `measure distance\|angle\|area -w W POINT…` | Value, unit, method and points; distances also give an uncertainty of one voxel spacing |
| `annotate add -w W --kind K --plane P [--name N] [--text T] [--agent ID] POINT…` | Distance, angle, area, rectangle or text on one slice, proposed by the agent |
| `annotate list\|rename\|delete -w W …` | Annotations with provenance; agents rename or delete only their own |
| `segment list -w W` | Segments with voxel count, ml and provenance |
| `segment threshold -w W --seed POINT --min V --max V [--max-ml ML] [--name N] [--agent ID]` | 6-connected region growing from a seed within a value range, as a proposed segment; existing segments are kept |
| `segment rename\|delete -w W …` | Only segments the agent created |
| `review list -w W` | Proposed annotations and segments |
| `review confirm\|reject -w W (--annotation ID \| --segment L) --by NAME` | Only if the operator allows harness review |
| `run "<command>" --params '<json>'` | Any command with JSON parameters: the same call the MCP server will make |
| `commands` | The command names |

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

**Overlays:** `segments` draws segment outlines in their colours.

## Provenance and review

Everything created through `ferrum-cli` is **proposed**:
- `author: agent`, with the `--agent` id if one is given;
- shown in the desktop app with **Confirm / Reject**.

Agents may change or delete only items that agents created. Items drawn
by people or proposed by engines are protected (`forbidden`).

A harness may confirm or reject items only when the operator sets
`review.allow_harness_confirmation = true`. It must name the person
(`--by`), and the decision goes to the audit log.

## Workspace and audit

**Workspace:**
- The workspace (`ferrum-workspace` v1, see
  [workspace-format.md](workspace-format.md)) holds the results:
  annotations, segments, renders and `audit.jsonl`.
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

[limits]
max_render_px = 1024
max_voxels = 600_000_000

[review]
allow_harness_confirmation = false
```

- Unknown or mistyped settings are an error, never a silent default.
- Without a configuration, every path is readable. Each envelope then
  warns about it.
- Outputs never contain patient names, IDs or birth dates. The agent does
  not output DICOM attributes.

## Limits of this first part

These are planned:
- `view mpr`, `view montage` and `view volume` (3D);
- `profile`;
- `export bundle`;
- JSON Schemas;
- the MCP server (15.4);
- engine commands (15.7).

Also note:
- NIfTI files carry no modality, so their values have no unit. Treat
  them as HU only if you know the file is CT.
- The CLI reloads the series for each call (a CT series in about 0.3 s).
  The MCP server will keep it in memory.
