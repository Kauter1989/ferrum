# FERRUM view in the DeepSeek Harness — test plugin

Stage 25, criterion 25.6. The plugin shows a FERRUM view host
([`ferrum-view/1`](../../../docs/view-host.md)) inside the harness: a page
with the study next to the conversation, frames drawn on the local GPU by
`ferrum-cli`, and the person's slice, window and picks working directly in
the panel.

This is a **test plugin** for the demo: a hand-written client bundle (no
build step) that embeds the host's own self-contained page in an iframe.
Its two halves:

| Path | What |
|---|---|
| `plugin/package.json` | the package: `dsh.client.platform: "web"`, `exports["./client"] → lib/client.js` |
| `plugin/lib/index.js` | the host half — an empty `apply()`, as every client plugin needs a Loader row |
| `plugin/lib/client.js` | the browser bundle: a `main` page, a sidebar row and a header button |
| `plugin/cordis.patch.snippet.yml` | the one entry to add to the profile |
| `smoke.js` | drives a live host over HTTP (state, frame, input, events); exits non-zero on any mismatch |
| `plugin/smoke-load.cjs` | loads `lib/client.js` against a stub `window.__ModuleLoader__` and checks the registrations |

## 1. Start the view host

Build the CLI once (release is what the demo uses):

```powershell
cargo build --release -p ferrum-cli
```

Open a workspace first — the agent does this with `study open`, the desktop
app with *Open workspace*:

```powershell
.\target\release\ferrum-cli.exe study open --workspace ws\ct1 --source D:\data\ct1 --modality CT
```

Then start the host for that workspace and keep the one JSON line it prints:

```powershell
.\target\release\ferrum-cli.exe view serve --workspace ws\ct1
{"url":"http://127.0.0.1:41234","token":"8f3c…","workspace":"ws\\ct1","backend":"gpu"}
```

`backend` says whether frames come from the GPU or the CPU reference (a
machine without an adapter falls back automatically; `--cpu` forces it).
Ctrl-C stops the host.

## 2. Enable the plugin

Append the entry from `plugin/cordis.patch.snippet.yml` to the profile's
`cordis.patch.yml` and fix the absolute path to this checkout:

```yaml
- insert:
    - id: ferrum-panel
      name: 'file:///E:/projects/ferrum/sources/ferrum/integrations/dsh/plugin/lib/index.js'
```

Then restart the harness (or let its HMR re-read the profile). A **FERRUM**
row appears in the left sidebar and a **FERRUM** button in the conversation
header; either opens the page.

Settings → Plugins shows the row as a plugin loaded from a path. It is not
listed as a bundle, because a patch-less client plugin is not a bundle (the
harness only manages bundles there) — that is expected for a test plugin.

## 3. Show the study

In the FERRUM page:

1. paste the `url` and the `token` from step 1;
2. press **Connect** — the page loads the host's own viewer in an iframe;
3. scroll the wheel over the image to change slices, drag to change the
   window, click to read the voxel; the plane buttons and the window presets
   are at the top.

The URL and the token are kept in `localStorage`, so a restart of the
harness needs no retyping — only a new host (a new token) after
`view serve` is restarted.

## 4. What the agent sees

The host records the person's actions (`slice_changed`, `window_changed`,
`plane_changed`, `segments_changed`, `point_picked`). They are readable
through the same token:

```powershell
Invoke-RestMethod "http://127.0.0.1:41234/api/events?token=<token>"
```

The agent's own command for them (`view events`) is Stage 26: in this stage
a harness reads the log over HTTP, as the smoke script does.

## 5. Checks

```powershell
node integrations/dsh/plugin/smoke-load.cjs
node integrations/dsh/smoke.js http://127.0.0.1:41234 <token>
```

`smoke` checks the state format, one frame's PNG signature and size, a slice
step, a probe, and the event log — exit code 0 means the host and this
plugin speak the same protocol.

## Limits of the test plugin

- It does not start the host: a browser bundle cannot spawn a process, and a
  host-side launcher is Stage 26 work. The operator runs `view serve` (one
  command) and pastes two values.
- It shows one view (2D slice); MPR and 3D are Stage 26 components.
- It needs the host and the harness on the same machine (loopback only).
- The harness's page and the host's page are different origins; the iframe
  is what makes that work without CORS. A plugin that draws the frames
  itself would need the host's `token` as a query parameter on every image
  (`/api/frame.png?token=…`), which is supported.

