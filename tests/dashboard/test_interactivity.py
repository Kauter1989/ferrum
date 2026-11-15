"""Interactivity structure tests (Stage 30, 30.3): filters, graph, L35.

The behaviour runs in the browser; these pin the structure the inline
script reads (data-* attributes, controls, SVG nodes/edges) and check the
script's syntax with node. The manual rows in docs/dashboard.md cover a
person clicking through filters and the graph in both themes.
"""

import re
import shutil
import subprocess

import page
import pytest

# covers 30.3-a


def test_filter_controls_present(repo_data, template_text):
    html = page.render_page(repo_data, template_text)
    for bucket in ("released", "develop", "progress", "planned", "unknown"):
        assert f'<input type="checkbox" data-bucket="{bucket}"' in html
    assert 'id="status-filter"' in html
    assert 'id="search"' in html
    assert 'id="filter-count"' in html
    assert 'data-status="progress"' in html


# covers 30.3-b


def test_crate_graph_nodes_and_edges(repo_data, template_text):
    html = page.render_page(repo_data, template_text)
    assert 'id="crate-svg"' in html
    names = {c["name"] for c in repo_data["crates"]}
    for name in names:
        assert f'data-crate="{name}"' in html
    for crate in repo_data["crates"]:
        for dep in crate["deps"]:
            if dep in names:
                assert f'data-from="{crate["name"]}" data-to="{dep}"' in html


# covers 30.3-c


def test_selects_declare_color_scheme(template_text):
    assert "color-scheme:light dark" in template_text
    assert ".filters option" in template_text


# covers 30.3-a


def test_inline_script_is_valid_js(repo_data, template_text, tmp_path):
    if not shutil.which("node"):
        pytest.skip("node not installed")
    html = page.render_page(repo_data, template_text)
    scripts = re.findall(r"<script>(.*?)</script>", html, re.S)
    assert scripts, "the page must carry the interactivity script"
    for i, code in enumerate(scripts):
        path = tmp_path / f"script{i}.js"
        path.write_text(code, encoding="utf-8")
        subprocess.run(["node", "--check", str(path)], check=True, capture_output=True)  # noqa: S603, S607
