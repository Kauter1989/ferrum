"""Workflow integration tests (Stage 30, 30.4): skill, hooks, publish, docs.

These live in tests/ (not .claude/) so ferrum-analyze sees the criterion
citations. The hook behaviour is also covered by .claude/hooks/test_hooks.py
for the CI python-lint job; here we pin the constants and the publish flow.
"""

import importlib.util
import json
import subprocess
from pathlib import Path

import publish
import pytest

ROOT = Path(__file__).resolve().parents[2]


def _hooks():
    spec = importlib.util.spec_from_file_location("ferrum_hooks", ROOT / ".claude" / "hooks" / "ferrum_hooks.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _run(cwd, *args):
    return subprocess.run(args, cwd=str(cwd), check=True, capture_output=True, text=True).stdout  # noqa: S603


# covers 30.4-a


def test_skill_exists_with_sections():
    text = (ROOT / ".claude" / "skills" / "ferrum-dashboard" / "SKILL.md").read_text(encoding="utf-8")
    assert text.startswith("---")
    assert "name: ferrum-dashboard" in text
    for section in ("## 2. Generate", "## 3. Verify", "## 4. Publish"):
        assert section in text


# covers 30.4-b


def test_publish_branch_exempt_from_name_rule():
    hooks = _hooks()
    assert {"gh-pages"} == hooks.PUBLISH_BRANCHES
    hooks.check_branch_names(["gh-pages"])  # must not raise
    with pytest.raises(hooks.Decision):
        hooks.check_branch_names(["claude/x"])


# covers 30.4-c


def test_matrix_has_dashboard_row_and_freshness_helper():
    hooks = _hooks()
    assert any("dev_plan" in pat and "ferrum-dashboard" in msg for pat, msg in hooks.MATRIX)
    assert callable(hooks.dashboard_freshness)


# covers 30.4-e


def test_integration_docs_updated():
    assert "ferrum-dashboard" in (ROOT / "CLAUDE.md").read_text(encoding="utf-8")
    change = (ROOT / ".claude" / "skills" / "ferrum-change" / "SKILL.md").read_text(encoding="utf-8")
    assert "dashboard" in change.lower()
    release = (ROOT / ".claude" / "skills" / "ferrum-release" / "SKILL.md").read_text(encoding="utf-8")
    assert "dashboard" in release.lower()


# covers 30.4-d


def test_publish_pushes_site_to_gh_pages(tmp_path):
    origin, work = tmp_path / "origin.git", tmp_path / "work"
    _run(tmp_path, "git", "init", "-q", "--bare", str(origin))
    _run(tmp_path, "git", "clone", "-q", str(origin), str(work))
    for key, value in (("user.email", "t@example.com"), ("user.name", "t"), ("commit.gpgsign", "false")):
        _run(work, "git", "config", key, value)
    (work / "dev_plan.md").write_text(
        "# plan\n\n## Stage 1 — One ✅\n\n| # | User story | Acceptance criteria |\n|---|---|---|\n"
        "| 1.1 | As a **user**, I want one. | [1.1-a] Done. ✅ |\n",
        encoding="utf-8",
        newline="\n",
    )
    (work / "Cargo.toml").write_text("[workspace]\nmembers = []\n", encoding="utf-8", newline="\n")
    _run(work, "git", "checkout", "-q", "-b", "develop")
    _run(work, "git", "add", "-A")
    _run(work, "git", "commit", "-q", "-m", "init")
    _run(work, "git", "push", "-q", "origin", "develop")
    _run(work, "git", "push", "-q", "origin", "develop:main")
    _run(work, "git", "tag", "v0.1.0")
    _run(work, "git", "push", "-q", "origin", "v0.1.0")

    assert publish.publish(work, ref="origin/develop", remote="origin") == 0

    tree = _run(origin, "git", "ls-tree", "--name-only", "gh-pages")
    assert "data.json" in tree and "index.html" in tree and ".nojekyll" in tree
    data = json.loads(_run(origin, "git", "show", "gh-pages:data.json"))
    dev = _run(work, "git", "rev-parse", "origin/develop").strip()
    assert data["meta"]["target_sha"] == dev
    assert data["meta"]["release_tag"] == "v0.1.0"
    # the caller's branch is untouched
    assert _run(work, "git", "rev-parse", "--abbrev-ref", "HEAD").strip() == "develop"
