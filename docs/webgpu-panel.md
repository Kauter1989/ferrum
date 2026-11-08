# WebGPU in the DeepSeek Harness panel — feasibility note

Stage 26 investigation (2026-11-04), before any design. It answers one
question: can FERRUM's own wgpu renderer draw into a panel of the DeepSeek
Harness in real time, so a person rotates a volume and scrolls MPR views
without a frame server in between?

**Answer: yes, measured.** The same `volume.wgsl`, compiled to
`wasm32-unknown-unknown` and given the browser's WebGPU adapter, renders at
**144 fps** in a Chromium window on this machine (RTX 3080), with the frame
presented straight into the canvas.

## What was built to find out

`crates/ferrum-wasm` is a thin binding of `ferrum-render`: it takes a
`<canvas>`, asks the browser for a WebGPU adapter, configures the surface and
exposes `set_volume`, `set_mode` (a name: `slice`, `volume` or `mpr`, refused
with the value named when it is anything else, F1.1-a),
`set_render_mode`, `set_window`,
`set_slice` (a plane name and an index), `set_ct_preset`, `orbit`, `zoom`,
`reset_camera`, `resize`, `render` and `adapter_info`. The rendering
code is not a second implementation: it is `VolumeRenderer` with the shaders
from `ferrum-render`. The MPR geometry lives in `src/layout.rs`, whose tests
run on the host (`crates/ferrum-wasm/tests/layout.rs`), and the mode names in
`src/mode.rs`, tested by `tests/mode_names.rs` — both compile on the host even
though the crate is wasm-only.

`integrations/dsh/panel/stand.html` is the throwaway page that proved it: a
synthetic chest-like volume, drag to orbit, wheel to zoom, a select for the
four render modes and a live fps counter in the status line.

## Measurements

| What | Result |
|---|---|
| `cargo build -p ferrum-wasm --target wasm32-unknown-unknown --release` | 44 s, `ferrum_wasm.wasm` 1.6 MB before bindgen processing |
| After `wasm-bindgen --target web` | 522 KB wasm + 73 KB JS glue |
| Crate compilation of the dependencies on wasm | `ferrum-domain`, `ferrum-processing`, `ferrum-app` and `ferrum-render --features gpu` build for `wasm32` **with no source changes** |
| Frame rate in Chromium, 1317×856 canvas, `tissue` mode | **144 fps** |
| Backend reported by wgpu | `BrowserWebGpu` |

## What blocked it first, and why it matters

Two failures happened before the one above, and both are worth remembering:

1. **No adapter, native backends named.** `wgpu::Backends::all()` does not
   include `BROWSER_WEBGPU` on wasm, so the instance asked for Vulkan, DX12,
   Metal and GL, found none in the browser and reported
   `webgpu found no adapters`. The binding must name the backend:
   `InstanceDescriptor::new_without_display_handle()` with
   `backends = Backends::BROWSER_WEBGPU`.
2. **Headless Chromium has no WebGPU adapter** on this machine even with
   `--enable-unsafe-webgpu`; a headed window does. A CI check therefore cannot
   rely on headless screenshots for this path.

The same class of surprise explains why the earlier `<iframe>` panel was
black: the Desktop window is loaded from the `dsh-app://` scheme, which is a
secure context, and Chromium does not let such a page frame `http://`
content the way a plain page would. Serving `http://127.0.0.1` is not a
substitute for rendering in the panel.

## What this means for the design

- The panel does not need a frame server: the renderer runs inside it.
- The cross-origin `<iframe>` route is dead for the Desktop harness; the panel
  must load FERRUM's own module, not a remote page.
- `ferrum-domain`, `ferrum-processing` and `ferrum-app` are already wasm-clean,
  so the application layer (use cases, segments, metrics) can follow the
  renderer into the browser without being forked.
- The open design questions are the ones ADR 0014 already names: how the
  volume reaches the client (policy and size), how the agent's `ViewState`
  reaches the panel, and how a person's decisions travel back. None of them
  change because the renderer turned out to be portable.

## Not done here

The stand page is a probe, not the panel: no MPR layout, no segments, no
transfer-function editor, no `ferrum-view/1` state, no events, no tests. The
binding has no automated test, because a WebGPU run needs a headed browser;
the measurement above is a field observation, recorded here so the design
quotes a measured number rather than an estimate (lesson L13).
