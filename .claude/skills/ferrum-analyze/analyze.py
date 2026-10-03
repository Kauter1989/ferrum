#!/usr/bin/env python3
"""Coverage of a stage's acceptance criteria (read-only).

Reads the stage section of dev_plan.md, collects the criterion IDs
([N.k-x]), and reports for each how it is verified:
- tests: Rust/Python/shell files outside docs that mention the ID
  (convention: a `// covers N.k-x` or `# covers N.k-x` comment on the test);
- other levels: rows of a validation table in docs/*.md that mention the
  ID together with `visual`, `field` or `manual`.
It also reports criteria of task rows that do not exist, duplicate IDs,
and IDs cited in code that the spec does not define.

Usage (repo root):
    python3 .claude/skills/ferrum-analyze/analyze.py --stage 18 [--root .]
Exit code 1 when a criterion has no verification at all.
"""

import argparse
import os
import re
import sys

CODE_EXT = (".rs", ".py", ".sh", ".wgsl")
SKIP_DIRS = {"target", ".git", ".claude", "node_modules", "__pycache__", ".venv"}
LEVELS = ("visual", "field", "manual")


def stage_section(text: str, stage: str) -> str:
    """The `## Stage N ...` section up to the next level-2 heading."""
    m = re.search(rf"^## Stage {re.escape(stage)}\b.*?$", text, re.M)
    if not m:
        sys.exit(f"no '## Stage {stage}' heading in dev_plan.md")
    rest = text[m.end():]
    n = re.search(r"^## ", rest, re.M)
    return rest[: n.start()] if n else rest


def walk(root: str, exts):
    for d, dirs, files in os.walk(root):
        dirs[:] = [x for x in dirs if x not in SKIP_DIRS]
        for f in files:
            if f.endswith(exts):
                yield os.path.join(d, f)


def main() -> int:
    p = argparse.ArgumentParser(description="acceptance-criteria coverage of a stage")
    p.add_argument("--stage", required=True, help="stage number, e.g. 18")
    p.add_argument("--root", default=".", help="repository root")
    a = p.parse_args()
    stage = a.stage
    sec = stage_section(open(os.path.join(a.root, "dev_plan.md"), encoding="utf-8").read(), stage)

    id_re = re.compile(rf"\[({re.escape(stage)}\.\d+-[a-z])\]")
    struck = set(re.findall(rf"~~\s*\[({re.escape(stage)}\.\d+-[a-z])\]", sec))
    ids = id_re.findall(sec)
    dupes = sorted({i for i in ids if ids.count(i) > 1})
    criteria = [i for i in dict.fromkeys(ids) if i not in struck]
    tasks = set(re.findall(rf"^\|\s*({re.escape(stage)}\.\d+)\s*\|", sec, re.M))
    if not criteria:
        print(f"Stage {stage}: no criterion IDs [{stage}.k-x] found — add IDs (ferrum-specify)")
        return 1

    cite_re = re.compile(rf"\b({re.escape(stage)}\.\d+-[a-z])\b")
    tests, cited = {}, set()
    for path in walk(a.root, CODE_EXT):
        try:
            text = open(path, encoding="utf-8").read()
        except (UnicodeDecodeError, OSError):
            continue
        for cid in set(cite_re.findall(text)):
            cited.add(cid)
            tests.setdefault(cid, []).append(os.path.relpath(path, a.root))

    levels = {}
    docs = os.path.join(a.root, "docs")
    for path in walk(docs, (".md",)) if os.path.isdir(docs) else []:
        for line in open(path, encoding="utf-8"):
            if not line.lstrip().startswith("|"):
                continue
            low = line.lower()
            for cid in set(cite_re.findall(line)):
                for lv in LEVELS:
                    if re.search(rf"\b{lv}\b", low):
                        levels.setdefault(cid, set()).add(lv)

    missing = []
    print(f"Stage {stage}: {len(criteria)} criteria, {len(struck)} struck through\n")
    print(f"{'criterion':<12} {'tests':<45} other")
    for cid in criteria:
        t = ", ".join(sorted(set(tests.get(cid, []))))[:45] or "—"
        o = ", ".join(sorted(levels.get(cid, []))) or "—"
        print(f"{cid:<12} {t:<45} {o}")
        if cid not in tests and cid not in levels:
            missing.append(cid)

    problems = []
    orphans = sorted({c.rsplit("-", 1)[0] for c in criteria} - tasks)
    if orphans:
        problems.append(f"criteria of tasks without a row in the stage table: {', '.join(orphans)}")
    if dupes:
        problems.append(f"duplicate IDs: {', '.join(dupes)}")
    unknown = sorted(cited - set(criteria) - struck)
    if unknown:
        problems.append(f"IDs cited in code but not defined in the spec: {', '.join(unknown)}")
    if missing:
        problems.append(f"criteria with no verification: {', '.join(missing)}")

    print()
    for pr in problems:
        print(f"! {pr}")
    if not problems:
        print("every criterion has a test or a validation row")
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
