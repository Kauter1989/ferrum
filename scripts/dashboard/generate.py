#!/usr/bin/env python3
"""Generate the FERRUM development dashboard (Stage 30, dashboard/1).

A derived view of the plan and the architecture: every number comes from
git and the repository, never from hand entry. Reads committed state only
(two revisions of dev_plan.md, cargo metadata, git grep, git log), so two
runs on the same inputs are byte-identical. The wall clock is not an
input: the generation stamp is the target commit's date.

Usage (repo root):
    python scripts/dashboard/generate.py [--root .] [--ref HEAD]
        [--release-ref origin/main] [--out target/dashboard-site]

Writes <out>/data.json (dashboard/1) and <out>/index.html (the
self-contained page). Python 3.11+ (tomllib); git and cargo on PATH
(cargo optional: Cargo.toml fallback). Design: docs/dashboard.md.
Tests: tests/dashboard (cite the criteria).
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import time
from pathlib import Path

from crate_graph import TIERS, changed_crates, check_layering, load_crates

SCHEMA = "dashboard/1"
BUCKETS = ("released", "develop", "progress", "planned", "unknown")
STAGE_PRIORITY = ("unknown", "progress", "develop", "released", "planned")
BADGE_TEXT = {
    "released": "in release",
    "develop": "in develop",
    "progress": "in progress",
    "planned": "planned",
    "unknown": "unknown",
}
HEADING_BADGE = {"done": "✅ done", "progress": "🚧 in progress", "planned": "📋 planned", "unknown": "unknown"}
CODE_EXT = (".rs", ".py", ".sh", ".wgsl")
SKIP_DIRS = {"target", ".git", ".claude", "node_modules", "__pycache__", ".venv"}
LEVELS = ("visual", "field", "manual")
CID_PATTERN = r"\b[A-Za-z0-9]+\.[0-9]+-[a-z]\b"


class Git:
    """Git commands against one repository, UTF-8 explicit (L41/L29)."""

    def __init__(self, root: Path | str):
        self.root = str(root)

    def _run(self, *args: str) -> subprocess.CompletedProcess:
        argv = ["git", "-C", self.root, *args]
        return subprocess.run(  # noqa: S603 - git on PATH, no shell
            argv,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )

    def run(self, *args: str) -> str:
        proc = self._run(*args)
        if proc.returncode != 0:
            raise RuntimeError(f"git {' '.join(args)} failed: {proc.stderr.strip()}")
        return proc.stdout

    def try_run(self, *args: str) -> str:
        """Stdout when git answers 0 or 1 (no match); raise otherwise."""
        proc = self._run(*args)
        if proc.returncode in (0, 1):
            return proc.stdout
        raise RuntimeError(f"git {' '.join(args)} failed: {proc.stderr.strip()}")

    def show(self, rev: str, path: str) -> str:
        return self.run("show", f"{rev}:{path}")

    def is_ancestor(self, ancestor: str, descendant: str) -> bool:
        proc = self._run("merge-base", "--is-ancestor", ancestor, descendant)
        if proc.returncode in (0, 1):
            return proc.returncode == 0
        raise RuntimeError(f"git merge-base failed: {proc.stderr.strip()}")


# ---------------------------------------------------------------- plan parsing


def _heading_status(line: str) -> str:
    """The first marker in the heading wins: '🚧 (G1 ✅)' is progress."""
    found = [
        (line.find(mark), status)
        for mark, status in (("✅", "done"), ("🚧", "progress"), ("📋", "planned"))
        if mark in line
    ]
    return min(found)[1] if found else "unknown"


def _cells(line: str) -> list[str]:
    """The trimmed cells of a Markdown table row (trailing pipe optional)."""
    parts = re.split(r"(?<!\\)\|", line)
    end = -1 if parts[-1].strip() == "" else None
    return [p.strip().replace("\\|", "|") for p in parts[1:end]]


def _is_separator(cells: list[str]) -> bool:
    return bool(cells) and all(re.fullmatch(r"[-: ]+", c) for c in cells)


def _prs(text: str) -> list[int]:
    """PR numbers cited as #N or (PR N), in order, deduplicated."""
    numbers = re.findall(r"#(\d+)", text) + re.findall(r"\(PR (\d+)\)", text)
    return list(dict.fromkeys(int(n) for n in numbers))


def _parse_criteria(sid: str, cell: str, heading: str, warnings: list[str], story_id: str) -> list[dict]:
    """Criterion units of one story row; a row without IDs is one story unit."""
    if not cell or cell == "—":
        if heading in ("done", "progress"):
            warnings.append(f"Stage {sid}: story {story_id} has an empty criteria cell")
        return [{"kind": "story", "id": story_id, "text": "", "done_marker": False, "prs": [], "empty_cell": True}]
    struck_spans = re.findall(r"~~.*?~~", cell)
    struck_ids = {cid for span in struck_spans for cid in re.findall(rf"\[({re.escape(sid)}\.\d+-[a-z])\]", span)}
    cleaned = re.sub(r"~~.*?~~", "", cell)
    id_re = re.compile(rf"\[({re.escape(sid)}\.\d+-[a-z])\]")
    matches = [m for m in id_re.finditer(cleaned) if m.group(1) not in struck_ids]
    if not matches:
        text = re.sub(r"\s+", " ", cleaned).strip()
        return [{"kind": "story", "id": story_id, "text": text, "done_marker": "✅" in text, "prs": _prs(text)}]
    units = []
    for i, match in enumerate(matches):
        end = matches[i + 1].start() if i + 1 < len(matches) else len(cleaned)
        segment = re.sub(r"\s+", " ", cleaned[match.end() : end]).strip()
        units.append(
            {
                "kind": "criterion",
                "id": match.group(1),
                "text": segment,
                "done_marker": "✅" in segment,
                "prs": _prs(segment),
            }
        )
    return units


def _parse_stories(sid: str, body: str, heading: str, warnings: list[str], draft: bool = False) -> list[dict]:
    """Story rows of the stage's user-story table only.

    Tables are recognised by their header: '| # | User story | …' is the
    story table; '| Task | …' (the Implementation table) is not — its rows
    may reuse N.k numbers (Stage 26's task 26.4 is not a story).
    """
    stories = []
    in_story_table = False
    for line in body.splitlines():
        if not line.lstrip().startswith("|"):
            in_story_table = False
            continue
        cells = _cells(line)
        if len(cells) < 2 or _is_separator(cells):
            continue
        if cells[0] == "#":
            in_story_table = "user story" in cells[1].lower()
            continue
        if cells[0].lower() in ("task", "criterion"):
            in_story_table = False
            continue
        if not in_story_table or not re.fullmatch(rf"{re.escape(sid)}\.\d+", cells[0]):
            continue
        if draft:
            stories.append({"id": cells[0], "text": cells[1], "criteria": []})
            continue
        cell = cells[2] if len(cells) >= 3 else ""
        stories.append(
            {"id": cells[0], "text": cells[1], "criteria": _parse_criteria(sid, cell, heading, warnings, cells[0])}
        )
    return stories


def _parse_bullet_criteria(sid: str, body: str) -> list[dict]:
    """Criteria written as a bullet list (Stages 26, F1, F2): '- [N.k-x] …'
    with indented continuation lines, grouped into stories by the N.k
    prefix. A struck bullet (~~[N.k-x]~~) is excluded."""
    bullet_re = re.compile(rf"^-\s+(~~)?\[({re.escape(sid)}\.\d+-[a-z])\]")
    units: dict[str, dict] = {}
    order: list[str] = []
    current: dict | None = None
    for line in body.splitlines():
        match = bullet_re.match(line)
        if match:
            struck, cid = match.group(1), match.group(2)
            current = None
            if not struck:
                current = {"kind": "criterion", "id": cid, "text": line[match.end() :], "done_marker": False, "prs": []}
                units[cid] = current
                order.append(cid)
            continue
        if current is None:
            continue
        if line.startswith((" ", "\t")) and line.strip() and not line.lstrip().startswith(("- ", "#", "|", "**")):
            current["text"] += " " + line.strip()
        else:
            current = None
    stories: dict[str, dict] = {}
    for cid in order:
        unit = units[cid]
        unit["text"] = re.sub(r"\s+", " ", re.sub(r"~~.*?~~", "", unit["text"])).strip()
        unit["done_marker"] = "✅" in unit["text"]
        unit["prs"] = _prs(unit["text"])
        prefix = cid.rsplit("-", 1)[0]
        stories.setdefault(prefix, {"id": prefix, "text": "", "criteria": []})["criteria"].append(unit)
    return list(stories.values())


def _mark_verified(stories: list[dict], body: str, sid: str) -> None:
    """A Verification-table row '| N.k-x | … | ✅ |' marks the criterion done."""
    by_id = {u["id"]: u for s in stories for u in s["criteria"]}
    id_re = re.compile(rf"^({re.escape(sid)}\.\d+-[a-z])$")
    for line in body.splitlines():
        if not line.lstrip().startswith("|"):
            continue
        cells = _cells(line)
        if len(cells) >= 2 and id_re.fullmatch(cells[0]) and "✅" in cells[-1]:
            unit = by_id.get(cells[0])
            if unit is not None:
                unit["done_marker"] = True


def parse_plan(text: str) -> dict:
    """Stages, roadmap and warnings of one dev_plan.md revision (30.1-a/f)."""
    warnings: list[str] = []
    stages: list[dict] = []
    roadmap: dict = {"next": [], "backlog": [], "drafts": []}
    headings = list(re.finditer(r"^## (.+)$", text, re.M))
    for i, heading in enumerate(headings):
        body = text[heading.end() : headings[i + 1].start() if i + 1 < len(headings) else len(text)]
        title = heading.group(1).strip()
        stage_match = re.match(r"Stage (\S+) — (.+)", title)
        if stage_match:
            sid = stage_match.group(1)
            status = _heading_status(title)
            if status == "unknown":
                warnings.append(f"Stage {sid}: unknown heading marker")
            stories = _parse_stories(sid, body, status, warnings)
            if not stories:
                stories = _parse_bullet_criteria(sid, body)
            _mark_verified(stories, body, sid)
            stages.append(
                {
                    "id": sid,
                    "title": stage_match.group(2).strip(),
                    "heading_status": status,
                    "stories": stories,
                }
            )
        elif title == "Roadmap":
            roadmap = _parse_roadmap(body)
    return {"stages": stages, "roadmap": roadmap, "warnings": warnings}


def _parse_roadmap(body: str) -> dict:
    roadmap: dict = {"next": [], "backlog": [], "drafts": []}
    sections = list(re.finditer(r"^### (.+)$", body, re.M))
    for i, section in enumerate(sections):
        content = body[section.end() : sections[i + 1].start() if i + 1 < len(sections) else len(body)]
        title = section.group(1).strip()
        if title == "Next":
            for line in content.splitlines():
                cells = _cells(line) if line.lstrip().startswith("|") else []
                if len(cells) >= 4 and cells[0].isdigit():
                    roadmap["next"].append({"n": cells[0], "what": cells[1], "kind": cells[2], "why": cells[3]})
        elif title.startswith("Backlog"):
            roadmap["backlog"] = [m.group(1).strip() for m in re.finditer(r"^- (.+)$", content, re.M)]
        else:
            draft = re.match(r"Stage (\S+) — (.+)", title)
            if draft:
                sid = draft.group(1)
                roadmap["drafts"].append(
                    {
                        "id": sid,
                        "title": draft.group(2).strip(),
                        "heading_status": _heading_status(title),
                        "stories": _parse_stories(sid, content, "planned", [], draft=True),
                    }
                )
    return roadmap


def stage_units(stage: dict) -> list[dict]:
    return [unit for story in stage["stories"] for unit in story["criteria"]]


# ---------------------------------------------------------------- lifecycle


def done_at(unit: dict, heading_status: str, ancestry: dict[int, bool]) -> bool:
    """Done at a revision: inline ✅, a ✅ heading, or a cited merged PR."""
    if unit.get("empty_cell"):
        return False
    return unit["done_marker"] or heading_status == "done" or any(ancestry.get(pr, False) for pr in unit["prs"])


def item_bucket(done_tag: bool, done_target: bool, target_heading: str) -> str:
    """One criterion's lifecycle bucket from done@tag, done@target and the heading."""
    if done_tag:
        return "released"
    if done_target:
        return "develop"
    if target_heading == "progress":
        return "progress"
    if target_heading == "planned":
        return "planned"
    return "unknown"


def stage_aggregate(buckets: list[str]) -> tuple[str, dict]:
    """A stage's bucket (highest-priority present) and exact per-bucket counts."""
    counts = {b: buckets.count(b) for b in BUCKETS}
    for bucket in STAGE_PRIORITY:
        if counts[bucket]:
            return bucket, counts
    return "unknown", counts


def pr_ancestry(git: Git, ref: str, release_sha: str | None, prs: list[int]) -> dict:
    """For each cited PR: is a merge with that number an ancestor of target / tag.

    Merges are identified by SUBJECT only, across all refs: a body mention
    of an issue number is not a merge (review finding), and a release-only
    merge invisible from the target must still count for the tag.
    """
    log = git.try_run("log", "--all", "--format=%H%x09%s")
    candidates: dict[int, list[str]] = {}
    for line in log.splitlines():
        sha, _, subject = line.partition("\t")
        match = re.match(r"^Merge pull request #(\d+) ", subject) or re.search(r"\(#(\d+)\)$", subject)
        if match:
            candidates.setdefault(int(match.group(1)), []).append(sha)
    ancestry = {}
    for number in prs:
        shas = candidates.get(number, [])
        ancestry[number] = {
            "target": any(git.is_ancestor(sha, ref) for sha in shas),
            "tag": bool(release_sha) and any(git.is_ancestor(sha, release_sha) for sha in shas),
        }
    return ancestry


# ---------------------------------------------------------------- coverage


def scan_coverage(git: Git, ref: str) -> dict:
    """The ferrum-analyze convention at a committed revision (30.1-c)."""
    tests: dict[str, set] = {}
    levels: dict[str, set] = {}
    code = git.try_run("grep", "-n", "-E", CID_PATTERN, ref, "--", "*.rs", "*.py", "*.sh", "*.wgsl")
    for line in code.splitlines():
        parts = line.split(":", 3)
        if len(parts) < 4:
            continue
        path, content = parts[1], parts[3]
        if Path(path).suffix not in CODE_EXT or any(p in SKIP_DIRS for p in Path(path).parts):
            continue
        for cid in set(re.findall(CID_PATTERN, content)):
            tests.setdefault(cid, set()).add(path)
    docs = git.try_run("grep", "-n", "-E", CID_PATTERN, ref, "--", "docs")
    for line in docs.splitlines():
        parts = line.split(":", 3)
        if len(parts) < 4 or Path(parts[1]).suffix != ".md" or not parts[3].lstrip().startswith("|"):
            continue
        content = parts[3]
        for cid in set(re.findall(CID_PATTERN, content)):
            for level in LEVELS:
                if re.search(rf"\b{level}\b", content.lower()):
                    levels.setdefault(cid, set()).add(level)
    return {
        cid: {"tests": sorted(tests.get(cid, set())), "levels": sorted(levels.get(cid, set()))}
        for cid in set(tests) | set(levels)
    }


# ---------------------------------------------------------------- aggregate


def _repo_url(git: Git, warnings: list[str]) -> str | None:
    """The public GitHub HTTPS URL of origin, or None (links disabled)."""
    try:
        url = git.try_run("remote", "get-url", "origin").strip()
    except RuntimeError:
        warnings.append("no origin remote; repo_url omitted (links disabled)")
        return None
    if not url:
        return None
    ssh = re.match(r"git@github\.com:([^/@]+/[^/@]+?)(?:\.git)?$", url) or re.match(
        r"ssh://git@github\.com/([^/@]+/[^/@]+?)(?:\.git)?$", url
    )
    if ssh:
        return f"https://github.com/{ssh.group(1)}"
    https = re.match(r"https://github\.com/([^/@]+/[^/@]+?)(?:\.git)?$", url)
    if https:
        return f"https://github.com/{https.group(1)}"
    # Never publish a credential-bearing or local-path remote (30.1-e).
    warnings.append("origin remote is not a public GitHub HTTPS URL; repo_url omitted")
    return None


def build_data(root: Path, ref: str = "HEAD", release_ref: str = "origin/main") -> dict:
    """The dashboard/1 aggregate of one target revision against the release."""
    git = Git(root)
    warnings: list[str] = []
    target_sha = git.run("rev-parse", "--verify", ref).strip()
    git.run("rev-parse", "--verify", release_ref)
    try:
        tag = git.try_run("describe", "--tags", "--abbrev=0", release_ref).strip() or None
    except RuntimeError:
        tag = None  # no tag reachable from the release ref
    if tag:
        release_sha = git.run("rev-parse", "--verify", f"{tag}^{{commit}}").strip()
    else:
        release_sha = git.run("rev-parse", "--verify", release_ref).strip()
        warnings.append(f"no release tag reachable from {release_ref}; done items are develop")
    plan = parse_plan(git.show(target_sha, "dev_plan.md"))
    warnings += plan["warnings"]
    tag_stages = {}
    if tag:
        tag_plan = parse_plan(git.show(release_sha, "dev_plan.md"))
        tag_stages = {s["id"]: s for s in tag_plan["stages"]}
    all_stages = list(plan["stages"]) + list(tag_stages.values())
    prs = sorted({pr for stage in all_stages for unit in stage_units(stage) for pr in unit["prs"]})
    ancestry = pr_ancestry(git, target_sha, release_sha if tag else None, prs)
    coverage = scan_coverage(git, target_sha)
    repo_url = _repo_url(git, warnings)
    stages = []
    for stage in plan["stages"]:
        tag_stage = tag_stages.get(stage["id"])
        tag_heading = tag_stage["heading_status"] if tag_stage else "absent"
        tag_units = {u["id"]: u for u in stage_units(tag_stage)} if tag_stage else {}
        tag_ancestry = {n: a["tag"] for n, a in ancestry.items()}
        target_ancestry = {n: a["target"] for n, a in ancestry.items()}
        units = stage_units(stage)
        buckets = []
        for unit in units:
            tag_unit = tag_units.get(unit["id"])
            if tag_unit is not None:
                merged_prs = sorted(set(tag_unit.get("prs", [])) | set(unit["prs"]))
                done_tag = done_at({**tag_unit, "prs": merged_prs}, tag_heading, tag_ancestry)
            else:
                done_tag = any(tag_ancestry.get(pr, False) for pr in unit["prs"])
            done_target = done_at(unit, stage["heading_status"], target_ancestry)
            bucket = (
                "unknown" if unit.get("empty_cell") else item_bucket(done_tag, done_target, stage["heading_status"])
            )
            cov = coverage.get(unit["id"], {"tests": [], "levels": []})
            unit.update(done=done_target, bucket=bucket, tests=cov["tests"], levels=cov["levels"])
            unit.pop("done_marker", None)
            unit.pop("empty_cell", None)
            buckets.append(bucket)
        bucket, counts = stage_aggregate(buckets)
        if not units:
            warnings.append(f"Stage {stage['id']}: no parseable criteria (prose-only?)")
        if stage["heading_status"] == "progress" and units and all(u["done"] for u in units):
            warnings.append(f"Stage {stage['id']}: heading 🚧 lags — all criteria done")
        stages.append({**stage, "bucket": bucket, "counts": counts})
    crates = load_crates(Path(root), warnings)
    for crate in crates:
        if crate["name"] not in TIERS:
            warnings.append(f"crate {crate['name']} is not in the layering tier table")
    violations = check_layering(crates)
    changed = changed_crates(git, release_sha if tag else None, target_sha)
    for crate in crates:
        crate["changed_since_release"] = crate["name"] in changed
    reports = [
        {"path": path, "label": Path(path).stem.removesuffix("-report")}
        for path in git.run("ls-tree", "-r", "--name-only", target_sha, "--", "docs").splitlines()
        if path.endswith("-report.html")
    ]
    meta = {
        "generated_at": git.run("show", "-s", "--format=%cI", target_sha).strip(),
        "release_tag": tag,
        "release_sha": release_sha,
        "target_ref": ref,
        "target_sha": target_sha,
        "repo_url": repo_url,
        "warnings": warnings,
    }
    return {
        "schema": SCHEMA,
        "meta": meta,
        "stages": stages,
        "roadmap": plan["roadmap"],
        "crates": crates,
        "layering_violations": violations,
        "reports": reports,
    }


# ---------------------------------------------------------------- main


def main() -> int:
    """CLI: write data.json and the self-contained index.html for a revision."""
    parser = argparse.ArgumentParser(description="generate the FERRUM development dashboard data (dashboard/1)")
    parser.add_argument("--root", default=".", help="repository root")
    parser.add_argument("--ref", default="HEAD", help="target revision (branch, SHA, origin/develop)")
    parser.add_argument("--release-ref", default="origin/main", help="revision the release tag is read from")
    parser.add_argument("--out", default="target/dashboard-site", help="output directory")
    args = parser.parse_args()
    root = Path(args.root).resolve()
    started = time.monotonic()
    try:
        data = build_data(root, ref=args.ref, release_ref=args.release_ref)
    except RuntimeError as error:
        print(f"dashboard: {error}", file=sys.stderr)
        return 1
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "data.json").write_text(
        json.dumps(data, sort_keys=True, ensure_ascii=False, indent=1) + "\n", encoding="utf-8", newline="\n"
    )
    from page import render_page

    template = (Path(__file__).resolve().parent / "template.html").read_text(encoding="utf-8")
    (out / "index.html").write_text(render_page(data, template), encoding="utf-8", newline="\n")
    elapsed = time.monotonic() - started
    criteria = sum(len(stage_units(s)) for s in data["stages"])
    buckets = {b: sum(s["counts"][b] for s in data["stages"]) for b in BUCKETS}
    print(f"dashboard: {len(data['stages'])} stages, {criteria} items, buckets {buckets}")
    print(
        f"dashboard: {len(data['crates'])} crates, {len(data['layering_violations'])} violations, "
        f"{len(data['meta']['warnings'])} warnings"
    )
    print(f"dashboard: wrote {out / 'data.json'} and {out / 'index.html'} in {elapsed:.2f}s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
