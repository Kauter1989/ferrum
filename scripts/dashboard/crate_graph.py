#!/usr/bin/env python3
"""Crate graph and layering check for the dashboard (Stage 30, PR 2, 30.1-d).

Nodes are the workspace crates (cargo metadata --no-deps, Cargo.toml
fallback); edges the intra-workspace dependencies. The declared layering
(CLAUDE.md) is checked: dependencies point inward through the tier table,
with the documented same-tier pairs and the per-crate prohibitions. Every
violating edge is named; a crate changed since the release tag is marked.
The git helper is passed in (duck-typed: .run), so this module does not
import generate. Design: docs/dashboard.md §3.4.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import tomllib

# The declared layering (CLAUDE.md): dependencies point inward. Tiers run
# inner → outer; an edge is allowed when it points strictly inward or is a
# documented same-tier pair. The mermaid in docs/architecture.md draws the
# main edges, not a closed whitelist — a host or adapter may reach any inner
# crate (ferrum-app → ferrum-radiomics is named in CLAUDE.md itself).
TIERS = {
    "ferrum-domain": 0,
    "ferrum-gpu-lock": 1,
    "ferrum-processing": 2,
    "ferrum-render": 2,
    "ferrum-io": 2,
    "ferrum-engines": 2,
    "ferrum-radiomics": 2,
    "ferrum-app": 3,
    "ferrum-ui": 4,
    "ferrum-view": 4,
    "ferrum-agent": 4,
    "ferrum": 5,
    "ferrum-cli": 5,
    "ferrum-wasm": 5,
}
SAME_TIER_ALLOWED = {("ferrum-engines", "ferrum-gpu-lock"), ("ferrum-render", "ferrum-processing")}
APP_FORBIDDEN = {"wgpu", "egui", "egui-wgpu", "eframe"}
APP_RULE = "ferrum-app must compile without wgpu/egui"
GPU_LOCK_RULE = "ferrum-gpu-lock depends on no other FERRUM crate"
INWARD_RULE = "layering: dependencies point inward"
# Per-crate prohibitions beyond the tier direction (CLAUDE.md,
# docs/architecture.md): the agent's closed dependency list, the UI's
# window/dialog ban, the view host's egui ban.
FORBIDDEN_EXTERNALS = {
    "ferrum-app": APP_FORBIDDEN,
    "ferrum-agent": {"wgpu", "egui", "egui-wgpu", "eframe"},
    "ferrum-view": {"egui", "egui-wgpu", "eframe"},
    "ferrum-ui": {"eframe", "rfd"},
}
AGENT_ALLOWED = {"ferrum-domain", "ferrum-io", "ferrum-engines", "ferrum-radiomics", "ferrum-render"}
AGENT_RULE = "ferrum-agent depends only on domain/io/engines/radiomics/render, without wgpu/egui"
VIEW_RULE = "ferrum-view must not know about egui"
UI_RULE = "ferrum-ui opens no windows or file dialogs (no eframe, no rfd)"
EXTERNAL_RULES = {"ferrum-app": APP_RULE, "ferrum-agent": AGENT_RULE, "ferrum-view": VIEW_RULE, "ferrum-ui": UI_RULE}


def _crate_deps(name: str, dep_names: list[str], workspace: set[str]) -> list[str]:
    """Intra-workspace edges plus the crate's forbidden externals (both paths)."""
    deps = {d for d in dep_names if d in workspace}
    deps |= {d for d in dep_names if d in FORBIDDEN_EXTERNALS.get(name, set())}
    return sorted(deps)


def crates_from_metadata(meta: dict) -> list[dict]:
    """Workspace crates from `cargo metadata --no-deps --format-version 1`."""
    packages = meta["packages"]
    workspace = {p["name"] for p in packages}
    crates = []
    for package in packages:
        names = [d["name"] for d in package.get("dependencies", []) if d.get("kind") is None]
        crates.append(
            {
                "name": package["name"],
                "description": package.get("description") or "",
                "deps": _crate_deps(package["name"], names, workspace),
            }
        )
    return sorted(crates, key=lambda c: c["name"])


def crates_from_toml(root: Path) -> list[dict]:
    """Fallback when cargo is unavailable: parse the workspace manifests."""
    workspace_toml = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    members = workspace_toml["workspace"]["members"]
    parsed = [tomllib.loads((root / m / "Cargo.toml").read_text(encoding="utf-8")) for m in members]
    workspace = {p["package"]["name"] for p in parsed}
    crates = []
    for data in parsed:
        package = data["package"]
        crates.append(
            {
                "name": package["name"],
                "description": package.get("description", ""),
                "deps": _crate_deps(package["name"], list(data.get("dependencies", {})), workspace),
            }
        )
    return sorted(crates, key=lambda c: c["name"])


def load_crates(root: Path, warnings: list[str]) -> list[dict]:
    """cargo metadata, with the Cargo.toml fallback; warn when neither works."""
    manifest = root / "Cargo.toml"
    if not manifest.exists():
        warnings.append("no cargo workspace found; the crate graph is empty")
        return []
    try:
        argv = ["cargo", "metadata", "--no-deps", "--format-version", "1", "--manifest-path", str(manifest)]
        proc = subprocess.run(  # noqa: S603 - cargo on PATH, no shell
            argv,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=True,
            cwd=str(root),
        )
        return crates_from_metadata(json.loads(proc.stdout))
    except (OSError, subprocess.SubprocessError, ValueError, KeyError):
        try:
            return crates_from_toml(root)
        except (OSError, ValueError, KeyError):
            warnings.append("cargo metadata and the Cargo.toml fallback both failed; the crate graph is empty")
            return []


def check_layering(crates: list[dict]) -> list[dict]:
    """Name every edge against the declared layering (30.1-d)."""
    workspace = {c["name"] for c in crates}
    violations = []
    for crate in crates:
        name = crate["name"]
        tier = TIERS.get(name)
        found = []
        for dep in crate["deps"]:
            rule = None
            if dep in FORBIDDEN_EXTERNALS.get(name, set()):
                rule = EXTERNAL_RULES[name]
            elif name == "ferrum-gpu-lock" and dep in workspace:
                rule = GPU_LOCK_RULE
            elif name == "ferrum-agent" and dep in workspace and dep not in AGENT_ALLOWED:
                rule = AGENT_RULE
            elif dep in workspace and tier is not None:
                dep_tier = TIERS.get(dep)
                inward = dep_tier is not None and dep_tier < tier
                if not inward and (name, dep) not in SAME_TIER_ALLOWED:
                    rule = INWARD_RULE
            if rule:
                found.append(f"{name} → {dep} ({rule})")
                violations.append({"from": name, "to": dep, "rule": rule})
        crate["violations"] = found
    return violations


def changed_crates(git, release_sha: str | None, target_sha: str) -> set[str]:
    """Crates with any file changed between the release and the target."""
    if not release_sha:
        return set()
    diff = git.run("diff", "--name-only", release_sha, target_sha, "--", "crates")
    names = set()
    for line in diff.splitlines():
        parts = Path(line).parts
        if len(parts) >= 3 and parts[0] == "crates":
            names.add(parts[1])
    return names
