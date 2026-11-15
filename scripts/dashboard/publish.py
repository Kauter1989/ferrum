#!/usr/bin/env python3
"""Publish the dashboard to the gh-pages branch (Stage 30, 30.4-d).

Generates the site from a committed revision (default origin/develop) and
commits it to gh-pages — an orphan branch holding only the generated site
plus .nojekyll — then pushes. GitHub Pages serves gh-pages from the branch;
no Actions minutes are spent (the 10 builds/hour soft limit is ample). Run
after a merge or a release (skill ferrum-dashboard).

The publication uses a throw-away detached worktree, so the caller's
working tree and branch are never touched. gh-pages holds only generated
text, so a force-push is safe and keeps the branch a pure publication
target. The local cargo gate (hook E) exempts gh-pages: CI never runs on it.

Usage (repo root):
    python scripts/dashboard/publish.py [--root .] [--ref origin/develop]
        [--remote origin] [--dry-run]
"""

from __future__ import annotations

import argparse
import contextlib
import json
import sys
import tempfile
from pathlib import Path

from generate import Git, build_data
from page import render_page


def publish(root: Path, ref: str = "origin/develop", remote: str = "origin", dry_run: bool = False) -> int:
    """Generate the site at ref and push it to <remote>/gh-pages."""
    git = Git(root)
    git.run("fetch", remote)
    data = build_data(root, ref=ref, release_ref=f"{remote}/main")
    template = (Path(__file__).resolve().parent / "template.html").read_text(encoding="utf-8")
    html = render_page(data, template)
    payload = json.dumps(data, sort_keys=True, ensure_ascii=False, indent=1) + "\n"
    sha = data["meta"]["target_sha"]
    tag = data["meta"]["release_tag"] or "no-release"
    if dry_run:
        print(f"dashboard: would publish {sha[:7]} ({tag}) to {remote}/gh-pages")
        return 0
    tmp = tempfile.mkdtemp(prefix="ferrum-dashboard-publish-")
    try:
        git.run("worktree", "add", "--detach", tmp, ref)
        pub = Git(tmp)
        pub.run("checkout", "--orphan", "gh-pages-publish")
        pub.run("rm", "-rf", "--quiet", ".")
        (Path(tmp) / "data.json").write_text(payload, encoding="utf-8", newline="\n")
        (Path(tmp) / "index.html").write_text(html, encoding="utf-8", newline="\n")
        (Path(tmp) / ".nojekyll").write_text("", encoding="utf-8", newline="\n")
        pub.run("add", "-A")
        pub.run("commit", "-m", f"dashboard: publish {sha[:7]} ({tag})")
        pub.run("push", "--force", remote, "HEAD:gh-pages")
    finally:
        git.run("worktree", "remove", "--force", tmp)
        with contextlib.suppress(RuntimeError):
            git.run("branch", "-D", "gh-pages-publish")
    print(f"dashboard: published {sha[:7]} ({tag}) to {remote}/gh-pages")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="publish the FERRUM dashboard to gh-pages")
    parser.add_argument("--root", default=".", help="repository root")
    parser.add_argument("--ref", default="origin/develop", help="committed revision to publish")
    parser.add_argument("--remote", default="origin", help="remote whose gh-pages branch is updated")
    parser.add_argument("--dry-run", action="store_true", help="generate and report, do not push")
    args = parser.parse_args()
    try:
        return publish(Path(args.root).resolve(), ref=args.ref, remote=args.remote, dry_run=args.dry_run)
    except RuntimeError as error:
        print(f"dashboard: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
