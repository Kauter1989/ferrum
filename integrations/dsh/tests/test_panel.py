"""The DeepSeek Harness panel bundle, checked without the harness.

The bundle is a classic script that registers itself with the harness's module
loader and returns a Cordis plugin; `smoke-load.cjs` loads it against a stub
loader and asserts the registrations. This file runs that script from the
repository's own test command and checks the pieces the panel needs to be
reachable and to fit beside the conversation.

Nothing here needs Node in CI's Rust jobs: the test skips when `node` is not on
PATH, and the description says what was skipped.
"""

import json
import shutil
import subprocess
from pathlib import Path

import pytest

PLUGIN = Path(__file__).resolve().parents[1] / "plugin"
MANIFEST = PLUGIN / "package.json"
BUNDLE = PLUGIN / "lib" / "client.js"
SMOKE = PLUGIN / "smoke-load.cjs"


def node() -> str:
    executable = shutil.which("node")
    if executable is None:
        pytest.skip("node is not on PATH: the panel bundle was not exercised")
    return executable


def test_the_bundle_is_a_classic_script_the_harness_can_serve():
    # covers 26.4-a
    text = BUNDLE.read_text(encoding="utf-8")

    assert "window.__ModuleLoader__.load(" in text, "the bundle does not register itself"
    for line in text.splitlines():
        stripped = line.lstrip()
        assert not stripped.startswith(("import ", "export ")), (
            f"the bundle is not a classic script: {line.strip()[:60]}"
        )
    assert "await " not in text.split("factory:")[0], "the bundle awaits at load time"

    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    assert manifest["dsh"]["client"]["platform"] == "web"
    assert manifest["exports"]["./client"], "the package exports no client bundle"


def test_the_registrations_make_the_panel_reachable():
    # covers 26.4-a
    # The executable and the script are this repository's own files.
    result = subprocess.run(  # noqa: S603
        [node(), str(SMOKE)],
        cwd=PLUGIN,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == 0, f"smoke-load failed:\n{result.stdout}\n{result.stderr}"
    assert "registers 5 slots" in result.stdout, result.stdout


def test_the_panel_takes_the_token_from_the_person_not_the_environment():
    # covers 26.4-b
    text = BUNDLE.read_text(encoding="utf-8")

    # A stale token must name its cause: a bare 401 leaves the person guessing.
    assert "refused the token" in text, "a refused token is not explained"
    assert "ferrum-cli view serve" in text, "the message does not say where a token comes from"
    # The address is loopback only, and the token shape is checked before a request.
    assert "isLoopback" in text, "a remote address is not refused"
    assert "tokenLooksValid" in text, "a malformed token is not refused"


def test_the_mode_dropdown_states_its_own_colours():
    """A native popup does not inherit the page's palette.

    With `color: inherit` the options came out white on a white popup inside a
    dark harness: the text was there and could not be read. The list and its
    options therefore carry explicit colours, and the option list is the four
    desktop render modes.
    """
    text = BUNDLE.read_text(encoding="utf-8")

    assert "select: {" in text, "the mode dropdown has no style of its own"
    assert "option: {" in text, "the dropdown options have no style of their own"
    # Both the popup and the options set a foreground colour.
    for style in ("select: {", "option: {"):
        block = text[text.index(style) :]
        block = block[: block.index("}")]
        assert "color:" in block, f"{style} sets no text colour"
        assert "background:" in block, f"{style} sets no background"

    for mode in ("tissue", "isosurface", "mip", "transfer_function"):
        assert f'"{mode}"' in text, f"the dropdown does not offer {mode}"
