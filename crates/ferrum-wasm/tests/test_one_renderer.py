"""The browser renderer is the desktop renderer, not a second one.

`ferrum-wasm` binds `ferrum-render` to a `<canvas>` through WebGPU. Its whole
value is that there is one image-formation model: a change to `volume.wgsl`
or to the frame model reaches the desktop and the browser together. That is a
property of the source, so it is checked by reading the source: the crate must
carry no shader of its own and must not march rays by hand.

The test needs no GPU and no browser.
"""

import re
from pathlib import Path

CRATE = Path(__file__).resolve().parents[1]
SOURCE = CRATE / "src" / "lib.rs"
SHADERS = CRATE.parent / "ferrum-render" / "src" / "shaders"


def test_the_browser_renderer_carries_no_shader_of_its_own():
    # covers 26.1-b
    shaders = sorted(path.name for path in SHADERS.glob("*.wgsl"))

    sources = list(CRATE.rglob("*.wgsl"))
    assert not sources, f"ferrum-wasm ships its own shaders: {[p.name for p in sources]}"
    text = SOURCE.read_text(encoding="utf-8")
    assert "include_str!" not in text, "ferrum-wasm embeds a shader"
    assert shaders, "ferrum-render has no shaders to reuse"


def test_the_browser_renderer_hands_the_frame_model_to_ferrum_render():
    # covers 26.1-b
    text = SOURCE.read_text(encoding="utf-8")

    # The renderer and its per-frame parameters come from the crates the
    # desktop uses; the MPR layout only decides the rects.
    for symbol in ["VolumeRenderer", "FrameParams", "SliceParams"]:
        assert symbol in text, f"ferrum-wasm does not use {symbol}"
    for source in ["ferrum_render", "ferrum_render::gpu"]:
        assert source in text, f"ferrum-wasm does not depend on {source}"
    # Camera, render modes and the window come from the domain, as on the
    # desktop, so a panel and the app rotate and window the same way.
    for symbol in ["OrbitCamera", "RenderMode", "WindowLevel"]:
        assert symbol in text, f"ferrum-wasm does not take {symbol} from ferrum-domain"


def test_the_mpr_layout_gives_every_view_its_own_quarter():
    # covers 26.2-a
    text = SOURCE.read_text(encoding="utf-8")

    # Four rects, one per view, computed from the canvas: axial, coronal,
    # sagittal and 3D. A layout with fewer is a layout that hides a view.
    assert "fn mpr_rects" in text, "the MPR layout has no rect function"
    body = text[text.index("fn mpr_rects") :]
    body = body[: body.index("\n    }\n")]
    # Four rects as `(min, max)` pairs; the first uses Vec2::ZERO for its min.
    assert body.count("Vec2::new(") >= 7, f"the four quarters are not four rects: {body}"
    assert re.search(r"-> \[\(Vec2, Vec2\); 4\]", body), "the layout is not four rects"
    assert "Vec2::ZERO" in body, "the first quarter does not start at the origin"
    # Each slice is scissored to its own quarter: without that the last slice's
    # full-canvas triangle erases the others (found on a real CT).
    assert re.search(r"pass\.set_scissor_rect\(", text), "slices are not scissored to their quarters"
    # The 3D image is placed through a viewport, not stretched over the canvas.
    assert re.search(r"pass\.set_viewport\(", text), "the 3D view is not placed in its quarter"
    # A slice is never stretched: its rect is fitted to the plane's shape.
    assert "fn fit_rect" in text, "slices are not fitted to their aspect"
    assert "plane_dims" in text, "the fit does not use the plane's own dims"


def test_the_desktop_render_modes_and_ct_presets_are_offered():
    # covers 26.2-b
    text = SOURCE.read_text(encoding="utf-8")

    # The four modes come from the domain, not from a list copied here.
    assert "RenderMode::ALL" in text, "the modes are not taken from ferrum-domain"
    assert "set_render_mode" in text, "the panel cannot choose a mode"
    # The CT presets are the domain's, so the browser shows the desktop's
    # anatomy instead of the legacy ramp.
    assert "CtPreset" in text, "the CT presets are not taken from ferrum-domain"
    assert "TransferFunction::ct_preset" in text, "no preset is built for the volume's range"
    assert "set_ct_preset" in text, "the panel cannot set a preset"
    # The presets are built for the volume's own value range, so a CT in
    # Hounsfield units and a volume in raw units both render sensibly.
    assert "volume.range()" in text, "a preset is not built for the volume's range"


def test_the_crate_is_inert_off_wasm_so_the_workspace_builds():
    # covers 26.1-a
    text = SOURCE.read_text(encoding="utf-8")
    first = next(line for line in text.splitlines() if line.strip() and not line.startswith("//!"))
    assert first.strip() == '#![cfg(target_arch = "wasm32")]', (
        f"the module body is not gated on wasm; the first item is {first!r}"
    )
    manifest = (CRATE / "Cargo.toml").read_text(encoding="utf-8")
    assert 'target_arch = "wasm32"' in manifest, "the dependencies are not gated on wasm"
    assert "[lib]" in manifest, "no [lib] section"
