"""The WebGPU path in a headed browser (Stage 29, criterion 29.5-b).

`webgpu.mjs` launches the machine's Edge headed (headless Chromium has no WebGPU
adapter, lesson L30), loads the stand page and checks that the renderer draws
the phantom in every view mode and render technique. It skips gracefully when
there is no browser, no adapter or no Playwright, so a CI run without a headed
GPU browser stays green.

This wrapper runs that script from the repository's own test command. It skips
when no Node >= 18 is available (Playwright needs it; set `FERRUM_NODE` to a
newer node) or when `playwright-core` is not installed, and names what was
skipped. The render itself is verified on a machine with a WebGPU adapter — a
field test (see integrations/dsh/README.md).
"""

import os
import re
import shutil
import subprocess
from pathlib import Path

import pytest

TESTS = Path(__file__).resolve().parent
SCRIPT = TESTS / "webgpu.mjs"


def node() -> str:
    candidate = os.environ.get("FERRUM_NODE") or shutil.which("node")
    if candidate is None:
        pytest.skip("node is not on PATH: the WebGPU path was not exercised")
    try:
        version = subprocess.run([candidate, "--version"], capture_output=True, text=True, timeout=30).stdout.strip()  # noqa: S603
    except (OSError, subprocess.SubprocessError):
        pytest.skip("node could not be run: the WebGPU path was not exercised")
    match = re.match(r"v(\d+)", version)
    if not match or int(match.group(1)) < 18:
        pytest.skip(f"node >= 18 is required for Playwright (found {version}); set FERRUM_NODE to a newer node")
    return candidate


def test_the_webgpu_path_renders_in_a_headed_browser():
    # covers 29.5-b
    executable = node()
    if not (TESTS / "node_modules" / "playwright-core").exists():
        pytest.skip("playwright-core is not installed (npm install in integrations/dsh/tests)")
    proc = subprocess.run(  # noqa: S603
        [executable, str(SCRIPT)],
        capture_output=True,
        text=True,
        timeout=240,
        cwd=TESTS,
    )
    out = proc.stdout + proc.stderr
    print(out)
    assert proc.returncode == 0, f"the WebGPU test failed:\n{out}"
    assert "ok:" in out or "SKIPPED:" in out, f"the WebGPU test neither passed nor skipped:\n{out}"
