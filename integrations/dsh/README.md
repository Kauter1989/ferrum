# FERRUM in the DeepSeek Harness — panel plugin

The panel shows a study inside the harness and **renders it itself**: FERRUM's
own wgpu renderer, compiled to WebAssembly and given the browser's WebGPU
device, draws MPR (axial, coronal, sagittal) and a 3D volume into a `<canvas>`
in the panel. There is no frame server and no streamed image: the voxels are
taken once from the view host and every frame after that is drawn by the GPU
of the machine — measured at 144 fps for a 512×512×252 CT on an RTX 3080
([`docs/webgpu-panel.md`](../../../docs/webgpu-panel.md)).

Why not an `<iframe>` of the host's page: the Desktop window is loaded from
the `dsh-app://` scheme, a secure context, and Chromium does not render
`http://` content in a frame there — the panel stayed black. A WebGPU canvas
in the panel has no such limit and is the real-time path the demo needs.

| Path | What |
|---|---|
| `plugin/package.json` | the package: `dsh.client.platform: "web"`, `exports["./client"] → lib/client.js` |
| `plugin/lib/index.js` | the host half — an empty `apply()`, as every client plugin needs a Loader row |
| `plugin/lib/client.js` | the browser bundle: a `main` page, a sidebar row, a header button, the canvas and its controls |
| `plugin/cordis.patch.snippet.yml` | the one entry to add to the profile |
| `panel/build.cmd` | builds `ferrum_wasm.js` and `ferrum_wasm_bg.wasm` from `crates/ferrum-wasm` |
| `panel/stand.html` | a standalone page that proves WebGPU works in a browser window |
| `smoke.js` | drives a live host over HTTP (state, frame, input, events); exits non-zero on any mismatch |
| `plugin/smoke-load.cjs` | loads `lib/client.js` against a stub `window.__ModuleLoader__` and checks the registrations |

## 1. Build the pieces

**From a release archive:** the built module (`ferrum_wasm.js`,
`ferrum_wasm_bg.wasm`) already sits next to `ferrum-cli`, and this folder
ships in the archive as `integrations/dsh/` — skip the build and start the
host (§2). The steps below are for running from a checkout.

```powershell
cargo build --release -p ferrum-cli
integrations\dsh\panel\build.cmd          # needs: rustup target add wasm32-unknown-unknown
                                          #        cargo install wasm-bindgen-cli --version 0.2.129
```

`build.cmd` writes `ferrum_wasm.js` and `ferrum_wasm_bg.wasm` into
`integrations\dsh\panel\`. The host serves them at `/panel/…` from the folder
next to its executable (or from `./panel`), so copy them there too:

```powershell
copy integrations\dsh\panel\ferrum_wasm* target\release\
```

They are build artifacts and are not committed.

## 2. Start the view host

Open a workspace first — the agent does this with `study open`, the desktop
app with *Open workspace*:

```powershell
.\target\release\ferrum-cli.exe study open --workspace ws\ct1 --source D:\data\ct1
.\target\release\ferrum-cli.exe view serve --workspace ws\ct1
{"url":"http://127.0.0.1:41234","token":"8f3c…","workspace":"ws\\ct1","backend":"gpu"}
```

The panel takes the study from `/api/volume` (the voxels, once) and does not
need the host to render anything; `backend` then only describes the host's
own rendering, which the panel does not use.

## 3. Enable the plugin

Append the entry from `plugin/cordis.patch.snippet.yml` to the profile's
`cordis.patch.yml`, fixing the absolute path, then restart the harness:

```yaml
- insert:
    - id: ferrum-panel
      name: 'file:///E:/projects/ferrum/sources/ferrum/integrations/dsh/plugin/lib/index.js'
```

A **FERRUM** row appears in the left sidebar and a **FERRUM** button in the
conversation header. The profile patch is composed at startup in this build:
editing it while the harness runs does not load the plugin.

## 4. Use the panel

1. paste the `url` and the `token` printed by `view serve` and press
   **Connect**;
2. the panel draws **MPR** by default: axial (top left), coronal (top right),
   sagittal (bottom left) and the 3D volume (bottom right);
3. drag over the canvas to orbit the 3D view, scroll to zoom;
   the panel opens in the conversation (FERRUM in the session header or the
   left sidebar) and, when the right sidebar is available, as a tab beside the
   chat: open it with **FERRUM** in the session header, or from the strip's
   **+** menu;
4. **3D** and **2D** switch to a single full-canvas view, and the select
   picks the 3D mode (`transfer_function` by default, with the desktop's CT
   soft tissue + bone preset; `tissue`, `isosurface` and `mip` are the others).

The address and the token are kept in `localStorage`. The token changes when
`view serve` restarts.

## 5. Checks

```powershell
node integrations\dsh\plugin\smoke-load.cjs                     # the bundle's registrations
node integrations\dsh\smoke.js http://127.0.0.1:41234 <token>    # the host over HTTP
```

## Limits

- The panel shows the study, not the segments yet: overlays in the browser
  are the next step (Stage 26).
- The whole volume crosses the wire once as f32 (264 MB for 512×512×252):
  fine on loopback, and compression or a 16-bit transfer is a later task.
- The panel does not start the host; the operator runs one command.
- No automated test of the WebGPU path: it needs a headed browser.