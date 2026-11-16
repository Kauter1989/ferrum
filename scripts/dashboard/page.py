"""Render the dashboard/1 aggregate into the self-contained page (Stage 30, PR 2).

The page is fully static: native <details> for expand/collapse, data-*
attributes for PR 3's interactivity (filters, search, SVG graph read the
DOM). No duplicated JSON payload; data.json is written alongside by
generate.py for machine consumers. Design: docs/dashboard.md §3.5.
"""

from __future__ import annotations

import html

from crate_graph import TIERS
from generate import BADGE_TEXT, BUCKETS, HEADING_BADGE


def _esc(text: str) -> str:
    return html.escape(str(text))


def _badge(bucket: str) -> str:
    return f'<span class="badge lc-{bucket}">{BADGE_TEXT[bucket]}</span>'


def _bar(stage: dict) -> str:
    total = sum(stage["counts"].values())
    if not total:
        return ""
    segments = "".join(
        f'<span class="seg seg-{b}" style="width:{100 * stage["counts"][b] / total:.1f}%"></span>'
        for b in BUCKETS
        if stage["counts"][b]
    )
    return f'<span class="bar" role="img" aria-label="{_esc(stage["counts"])}">{segments}</span>'


def _criterion(unit: dict, meta: dict) -> str:
    repo, sha = meta["repo_url"], meta["target_sha"]
    tests = " · ".join(
        f'<a href="{_esc(f"{repo}/blob/{sha}/{path}" if repo else path)}">{_esc(path)}</a>' for path in unit["tests"]
    )
    prs = " · ".join(f'<a href="{_esc(f"{repo}/pull/{n}" if repo else f"#{n}")}">#{n}</a>' for n in unit["prs"])
    levels = _esc(" · ".join(unit["levels"]))
    coverage = " ".join(part for part in (tests, levels, prs) if part)
    cov_html = f' <span class="cov">{coverage}</span>' if coverage else ""
    return (
        f'<div class="criterion" data-bucket="{unit["bucket"]}"><span class="cid">[{_esc(unit["id"])}]</span> '
        f'{_badge(unit["bucket"])} <span class="ctext">{_esc(unit["text"])}</span>{cov_html}</div>'
    )


def _stage_card(stage: dict, meta: dict) -> str:
    stories = []
    for story in stage["stories"]:
        criteria = "".join(_criterion(u, meta) for u in story["criteria"])
        stories.append(
            f'<div class="story"><div class="storytext">{_esc(story["id"])} — {_esc(story["text"])}</div>'
            f"{criteria}</div>"
        )
    head = HEADING_BADGE[stage["heading_status"]]
    return (
        f'<details class="stage" id="stage-{_esc(stage["id"])}" data-stage="{_esc(stage["id"])}" '
        f'data-bucket="{stage["bucket"]}" data-status="{stage["heading_status"]}"><summary>'
        f'<span class="sid">{_esc(stage["id"])}</span> <span class="stitle">{_esc(stage["title"])}</span> '
        f'<span class="badge hs">{head}</span> {_badge(stage["bucket"])} {_bar(stage)} '
        f'<span class="counts">{sum(stage["counts"].values())} items</span></summary>'
        f'<div class="stories">{"".join(stories)}</div></details>'
    )


def _crate_graph(data: dict) -> str:
    """A layered dependency graph: tiers as rows, crates as nodes, edges as
    lines. Rendered server-side so it works without JS; the inline script
    adds click-to-highlight and pan/zoom."""
    crates = data["crates"]
    if not crates:
        return "<p>No crate graph.</p>"
    by_tier: dict[int, list[dict]] = {}
    for crate in crates:
        by_tier.setdefault(TIERS.get(crate["name"], 99), []).append(crate)
    pos: dict[str, tuple[float, float]] = {}
    nodes = []
    for row, tier in enumerate(sorted(by_tier)):
        members = sorted(by_tier[tier], key=lambda c: c["name"])
        y = 50 + row * 90
        for i, crate in enumerate(members):
            x = 40 + i * 190
            pos[crate["name"]] = (x + 80, y + 22)
            classes = "crate-node"
            if crate["changed_since_release"]:
                classes += " changed"
            if crate["violations"]:
                classes += " viol"
            nodes.append(
                f'<g class="{classes}" data-crate="{_esc(crate["name"])}" transform="translate({x},{y})">'
                f'<rect width="160" height="44" rx="8"></rect>'
                f'<text x="80" y="27">{_esc(crate["name"])}</text></g>'
            )
    edges = []
    for crate in crates:
        x1, y1 = pos[crate["name"]]
        for dep in crate["deps"]:
            if dep in pos:
                x2, y2 = pos[dep]
                edges.append(
                    f'<line class="crate-edge" data-from="{_esc(crate["name"])}" data-to="{_esc(dep)}" '
                    f'x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}"></line>'
                )
    width = max(40 + len(members) * 190 for members in by_tier.values()) + 40
    height = 50 + len(by_tier) * 90
    return (
        f'<svg id="crate-svg" viewBox="0 0 {width} {height}" role="img" '
        f'aria-label="Crate dependency graph, {len(crates)} crates">'
        f'<g id="crate-edges">{"".join(edges)}</g><g id="crate-nodes">{"".join(nodes)}</g></svg>'
    )


def _crate_table(data: dict) -> str:
    tag = data["meta"]["release_tag"]
    rows = []
    for crate in data["crates"]:
        mark = f"changed since {tag}" if crate["changed_since_release"] and tag else ""
        rows.append(
            f'<tr data-crate="{_esc(crate["name"])}" data-changed="{str(crate["changed_since_release"]).lower()}">'
            f"<td>{_esc(crate['name'])}</td><td>{_esc(crate['description'])}</td>"
            f'<td>{_esc(", ".join(crate["deps"]))}</td><td class="mark">{_esc(mark)}</td>'
            f'<td class="viol">{_esc("; ".join(crate["violations"]))}</td></tr>'
        )
    return (
        '<table class="crates"><thead><tr><th>Crate</th><th>Description</th><th>Depends on</th>'
        f"<th>Since {_esc(tag or 'the release')}</th><th>Layering</th></tr></thead>"
        f"<tbody>{''.join(rows)}</tbody></table>"
    )


def _roadmap(roadmap: dict) -> str:
    rows = "".join(
        f"<tr><td>{_esc(item['n'])}</td><td>{_esc(item['what'])}</td>"
        f"<td>{_esc(item['kind'])}</td><td>{_esc(item['why'])}</td></tr>"
        for item in roadmap["next"]
    )
    next_table = (
        "<table><thead><tr><th>#</th><th>What</th><th>Kind</th><th>Why now</th></tr></thead>"
        f"<tbody>{rows}</tbody></table>"
    )
    backlog = "".join(f"<li>{_esc(item)}</li>" for item in roadmap["backlog"])
    drafts = []
    for draft in roadmap["drafts"]:
        stories = "".join(f'<div class="storytext">{_esc(s["id"])} — {_esc(s["text"])}</div>' for s in draft["stories"])
        drafts.append(
            f'<details class="stage"><summary><span class="sid">{_esc(draft["id"])}</span> '
            f'<span class="stitle">{_esc(draft["title"])}</span> {_badge("planned")}</summary>{stories}</details>'
        )
    return f"<h3>Next</h3>{next_table}<h3>Backlog</h3><ul>{backlog}</ul><h3>Draft stages</h3>{''.join(drafts)}"


def _reports(data: dict) -> str:
    if not data["reports"]:
        return "<p>No committed reports yet.</p>"
    sha = data["meta"]["target_sha"]
    repo = data["meta"]["repo_url"]
    items = []
    for report in data["reports"]:
        href = f"{repo}/blob/{sha}/{report['path']}" if repo else report["path"]
        items.append(f'<li><a href="{_esc(href)}">{_esc(report["label"])}</a> — {_esc(report["path"])}</li>')
    return f"<ul>{''.join(items)}</ul>"


def render_page(data: dict, template: str) -> str:
    """Fill the skeleton; the page is fully static (PR 2).

    The plan data is rendered into the HTML with data-* attributes (PR 3's
    interactivity reads the DOM); data.json is written alongside for
    machine consumers — no duplicated payload inside the page.
    """
    meta = data["meta"]
    tag = meta["release_tag"] or "no release"
    pills = (
        f'<span class="pill">release: {_esc(tag)}</span>'
        f'<span class="pill">{_esc(meta["target_ref"])} @ {_esc(meta["target_sha"][:7])}</span>'
        f'<span class="pill">{_esc(meta["generated_at"])}</span>'
        f'<span class="pill">{len(meta["warnings"])} warnings</span>'
    )
    warnings = "".join(f"<li>{_esc(w)}</li>" for w in meta["warnings"])
    warnings_html = f'<ul class="warnings">{warnings}</ul>' if warnings else "<p>No warnings.</p>"
    footer = (
        f"Sources: release {_esc(tag)} ({_esc(meta['release_sha'])}), target {_esc(meta['target_ref'])} "
        f"({_esc(meta['target_sha'])}), generated {_esc(meta['generated_at'])}. Schema {_esc(data['schema'])}. "
        "Derived from git describe, git show, git grep, git log, cargo metadata — no hand entry."
    )
    fills = {
        "<!--META_PILLS-->": pills,
        "<!--STAGES-->": "".join(_stage_card(s, meta) for s in data["stages"]),
        "<!--CRATES-->": _crate_table(data),
        "<!--CRATE_GRAPH-->": _crate_graph(data),
        "<!--ROADMAP-->": _roadmap(data["roadmap"]),
        "<!--REPORTS-->": _reports(data),
        "<!--WARNINGS-->": warnings_html,
        "<!--FOOTER-->": footer,
        "<!--INTERACTIVE-->": "",
    }
    page = template
    for placeholder, value in fills.items():
        page = page.replace(placeholder, value)
    return page
