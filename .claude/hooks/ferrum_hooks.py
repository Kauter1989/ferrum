#!/usr/bin/env python3
"""Claude Code hooks for FERRUM (.claude/settings.json).

One script, one sub-command per hook. Each reads the hook's JSON from stdin.
- guard-bash   PreToolUse Bash:  A protected branches, B no volume/patient data,
                                 D ask before pushing tags, E1/E2 pre-push checks
- guard-edit   PreToolUse Edit|Write|MultiEdit: C ADRs are append-only
- ask-release  PreToolUse merge/release MCP tools: D ask the user
- post-edit    PostToolUse Edit|Write|MultiEdit: F rustfmt, G matrix reminders
- session-start SessionStart:     H fresh base, lessons, environment
- pre-compact  PreCompact:        I compaction log for sizing calibration

Rules: a hook that fails internally lets the action through (fail open),
except A and B, which deny when they cannot decide safely. Lessons are in
.claude/lessons.md; the letters match the hook list in CLAUDE.md.
"""

import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
import time

PROTECTED = {"develop", "main"}
DATA_EXT = (".nii", ".nii.gz", ".dcm", ".dicom", ".nrrd", ".mha", ".mhd", ".mgz", ".ima")
BINARY_LIMIT = 1 << 20  # 1 MiB
# Files whose change must be covered by phantom_render.py (lesson L1).
VISUAL_PATHS = (
    "crates/ferrum-render/src/shaders/slice.wgsl",
    "crates/ferrum-agent/src/commands/view.rs",
    "crates/ferrum-agent/src/commands/tiles.rs",
    "crates/ferrum-domain/src/segmentation.rs",
)
SKIP_PREPUSH = "FERRUM_SKIP_PREPUSH=1"

# G: file pattern -> what else must change (ferrum-change §2).
MATRIX = [
    (r"crates/ferrum-render/src/shaders/slice\.wgsl$",
     "slice.wgsl changed: mirror it in the CPU path (ferrum-agent view.rs render_pixels/on_edge, "
     "SegmentStyle::blend), cover it in gpu_parity.rs, then run ferrum-visual-check (L1)."),
    (r"crates/ferrum-agent/src/commands/(view|tiles)\.rs$|crates/ferrum-domain/src/segmentation\.rs$",
     "Agent render/overlay code changed: keep slice.wgsl in step, keep the exact-geometry test "
     "(slice_renders_draw_closed_outlines_or_translucent_fills) passing, run "
     "phantom_render.py before pushing (the pre-push hook checks it) (L1)."),
    (r"crates/ferrum-render/src/shaders/volume\.wgsl$|crates/ferrum-render/src/cpu/raycast\.rs$",
     "Volume image formation changed: volume.wgsl and cpu/raycast.rs must stay mirrored, "
     "covered by gpu_parity.rs (CLAUDE.md rendering rule)."),
    (r"crates/ferrum-agent/src/schema\.rs$",
     "Agent schema changed: regenerate skills/ferrum/schemas/commands.json (ferrum-cli schema), "
     "update skills/ferrum/reference/commands.md and docs/agent-cli.md, add a contract test and a "
     "CLI parse test in crates/ferrum-cli/src/cli.rs (L8)."),
    (r"crates/ferrum-cli/src/cli\.rs$",
     "CLI changed: every new flag needs a parse test (call(&[\"ferrum-cli\", …])) and must land "
     "on the right subcommand; CLI and MCP give identical JSON (L8)."),
    (r"crates/ferrum-engines/src/(wire|client|protocol)[^/]*\.rs$|bridges/ferrum_bridges/protocol\.py$",
     "Engine protocol code changed: update docs/engine-protocol.md, the conformance suite "
     "(crates/ferrum-engines/tests/conformance.rs) and the bridges' tests."),
    (r"crates/ferrum-io/src/(workspace|engine_inputs|provenance)\.rs$",
     "Workspace format code changed: update docs/workspace-format.md (with the version example) "
     "and keep old files readable."),
    (r"crates/ferrum-agent/src/commands/(masks|interactive|segment)\.rs$",
     "Segment-mutating agent code changed: rights go through changeable_segment; keep a test that "
     "expects `forbidden` for a person's or confirmed segment (L7); seeds apart from prompts (L6)."),
    (r"crates/ferrum/src/ui/",
     "Desktop UI changed: logic belongs in ferrum-app (Viewer use cases); run "
     "`cargo test -p ferrum --test ui` — new rows can push buttons off the test window (L9)."),
    (r"crates/ferrum-io/src/(dicom|nifti)[^/]*\.rs$",
     "Input reader changed: reorient into LPS; check what metadata the format lacks (modality, units) "
     "and test NIfTI and DICOM (L10)."),
]


# ------------------------------------------------------------------ helpers
def read_input() -> dict:
    try:
        return json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError):
        return {}


def emit(event: str, **fields) -> None:
    out = {"hookSpecificOutput": {"hookEventName": event, **fields}}
    print(json.dumps(out))


class Decision(Exception):
    """A PreToolUse decision; the first one raised wins."""

    def __init__(self, decision: str, reason: str):
        super().__init__(reason)
        self.decision, self.reason = decision, reason


def deny(reason: str):
    raise Decision("deny", reason)


def ask(reason: str):
    raise Decision("ask", reason)


def git(cwd: str, *args: str, timeout: float = 10) -> str:
    out = subprocess.run(["git", "-C", cwd, *args], capture_output=True, text=True, timeout=timeout)
    if out.returncode != 0:
        raise RuntimeError(out.stderr.strip() or f"git {' '.join(args)} failed")
    return out.stdout.strip()


def repo_root(cwd: str) -> str:
    return git(cwd, "rev-parse", "--show-toplevel")


def state_dir(root: str) -> str:
    d = os.path.join(root, ".claude", "state")
    os.makedirs(d, exist_ok=True)
    return d


def visual_hash(root: str) -> str:
    h = hashlib.sha256()
    for rel in VISUAL_PATHS:
        p = os.path.join(root, rel)
        h.update(rel.encode())
        if os.path.exists(p):
            with open(p, "rb") as f:
                h.update(f.read())
    return h.hexdigest()


# ------------------------------------------------------------------ bash parsing
def segments(command: str):
    """Shell command split into simple commands (tokens), roughly."""
    for part in re.split(r"&&|\|\||;|\n|\|", command):
        try:
            toks = shlex.split(part, comments=True)
        except ValueError:
            toks = part.split()
        # drop leading env assignments (VAR=x cmd)
        while toks and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", toks[0]):
            toks = toks[1:]
        if toks:
            yield toks


def git_call(toks):
    """(cwd_override, subcommand, args) for a git invocation, else None."""
    if not toks or os.path.basename(toks[0]) != "git":
        return None
    i, cwd = 1, None
    while i < len(toks) and toks[i].startswith("-"):
        if toks[i] == "-C" and i + 1 < len(toks):
            cwd = toks[i + 1]
            i += 2
        elif toks[i] in ("-c", "--git-dir", "--work-tree") and i + 1 < len(toks):
            i += 2
        else:
            i += 1
    if i >= len(toks):
        return None
    return cwd, toks[i], toks[i + 1:]


def branch_of_ref(ref: str) -> str:
    ref = ref.lstrip("+")
    for p in ("refs/heads/", "heads/"):
        if ref.startswith(p):
            return ref[len(p):]
    return ref


def push_targets(args):
    """(flags, remote, refspecs) of `git push`."""
    flags, pos = [], []
    skip = False
    for a in args:
        if skip:
            skip = False
            continue
        if a in ("-o", "--push-option", "--receive-pack", "--exec", "--repo"):
            skip = True
            continue
        (flags if a.startswith("-") else pos).append(a)
    return flags, (pos[0] if pos else None), pos[1:]


# ------------------------------------------------------------------ A, B, D, E
def check_push(cwd, root, args, branch, command):
    flags, _remote, refspecs = push_targets(args)
    if "--all" in flags or "--mirror" in flags:
        return deny("Pushing all branches (--all/--mirror) would update develop/main. "
                    "Push the feature branch only (L2).")
    dests = []
    for spec in refspecs:
        dst = spec.split(":", 1)[1] if ":" in spec else spec
        if spec.startswith(":"):
            dst = spec[1:]
        dests.append(branch_of_ref(dst))
    if not refspecs:
        dests = [branch] if branch else []
    hit = sorted({d for d in dests if d in PROTECTED or d == "HEAD" and branch in PROTECTED})
    if hit:
        return deny(f"Push to {', '.join(hit)} is not allowed: develop and main change only through "
                    "merged PRs (CLAUDE.md, lesson L2). Push the feature branch and open a PR.")
    tags = "--tags" in flags or "--follow-tags" in flags or any(
        s.startswith("refs/tags/") or re.match(r"^v\d", s) for s in refspecs)
    if tags:
        return ask("Pushing tags publishes a version; releases need the user's word (ferrum-release, G3).")
    if SKIP_PREPUSH in command:
        return None
    return prepush_checks(root)


def prepush_checks(root):
    """E1 fmt + schema freshness, E2 visual-check marker. Deny on failure."""
    problems = []
    cargo = os.path.exists(os.path.join(root, "Cargo.toml"))
    if cargo:
        fmt = subprocess.run(["cargo", "fmt", "--all", "--", "--check"], cwd=root,
                             capture_output=True, text=True, timeout=120)
        if fmt.returncode != 0:
            problems.append("`cargo fmt --all -- --check` fails: run `cargo fmt --all` and commit.")
    try:
        base = git(root, "merge-base", "HEAD", "origin/develop")
        changed = set(git(root, "diff", "--name-only", base, "HEAD").splitlines())
    except (RuntimeError, subprocess.TimeoutExpired):
        changed = set()
    if cargo and any(c.startswith("crates/ferrum-agent/src/") or c.startswith("skills/ferrum/schemas/")
                     for c in changed):
        gen = subprocess.run(["cargo", "run", "-q", "-p", "ferrum-cli", "--", "schema"], cwd=root,
                             capture_output=True, text=True, timeout=600)
        path = os.path.join(root, "skills/ferrum/schemas/commands.json")
        if gen.returncode == 0 and os.path.exists(path):
            with open(path, encoding="utf-8") as f:
                if f.read().strip() != gen.stdout.strip():
                    problems.append("skills/ferrum/schemas/commands.json is stale: "
                                    "`cargo run -q -p ferrum-cli -- schema > skills/ferrum/schemas/commands.json`.")
    if changed & set(VISUAL_PATHS):
        marker = os.path.join(state_dir(root), "visual-check.json")
        ok = False
        if os.path.exists(marker):
            try:
                with open(marker, encoding="utf-8") as f:
                    ok = json.load(f).get("hash") == visual_hash(root)
            except (OSError, ValueError):
                ok = False
        if not ok:
            problems.append("Slice/overlay rendering changed since the last passing visual check: run "
                            "`python3 .claude/skills/ferrum-visual-check/phantom_render.py` and look at the "
                            "PNGs (ferrum-visual-check, lesson L1).")
    if problems:
        deny("Pre-push checks failed (prefix the push with FERRUM_SKIP_PREPUSH=1 only in an emergency):\n- "
             + "\n- ".join(problems))
    return None


def candidate_files(root, adds_or_all):
    files = set(git(root, "diff", "--cached", "--name-only").splitlines())
    if adds_or_all:
        files |= set(git(root, "diff", "--name-only").splitlines())
        files |= set(git(root, "ls-files", "--others", "--exclude-standard").splitlines())
    return sorted(f for f in files if f)


def data_problems(root, files):
    bad = []
    for rel in files:
        low = rel.lower()
        p = os.path.join(root, rel)
        if not os.path.isfile(p):
            continue  # deleted: removing data is fine
        if low.endswith(DATA_EXT):
            bad.append(f"{rel} (volume/DICOM file)")
            continue
        if rel.startswith("docs/images/"):
            continue
        size = os.path.getsize(p)
        if size > BINARY_LIMIT:
            with open(p, "rb") as f:
                if b"\0" in f.read(8192):
                    bad.append(f"{rel} (binary, {size // 1024} KiB)")
    return bad


def guard_bash(data):
    command = (data.get("tool_input") or {}).get("command", "")
    if "git" not in command:
        return
    base_cwd = data.get("cwd") or os.getcwd()
    switched = False
    for toks in segments(command):
        call = git_call(toks)
        if not call:
            continue
        cwd_override, sub, args = call
        cwd = os.path.join(base_cwd, cwd_override) if cwd_override else base_cwd
        if sub in ("checkout", "switch"):
            switched = True
            continue
        if sub not in ("commit", "push", "merge", "rebase", "add"):
            continue
        try:
            root = repo_root(cwd)
            branch = git(root, "rev-parse", "--abbrev-ref", "HEAD")
        except (RuntimeError, subprocess.TimeoutExpired, OSError):
            if sub in ("commit", "push"):
                return deny(f"Could not determine the git branch for `git {sub}`; check the repository.")
            continue
        on_protected = branch in PROTECTED and not switched
        if sub in ("commit", "merge", "rebase") and on_protected:
            return deny(f"`git {sub}` on {branch} is not allowed: work on a feat/ fix/ docs/ refactor/ chore/ "
                        "branch from origin/develop (L2, L3).")
        if sub == "commit":
            adds = "git add" in command or any(a in ("-a", "--all") or re.match(r"^-[a-zA-Z]*a", a) for a in args)
            bad = data_problems(root, candidate_files(root, adds))
            if bad:
                return deny("Volume, DICOM or large binary files would be committed: " + ", ".join(bad)
                            + ". FERRUM never commits patient or volume data, not even synthetic "
                            "(tests generate data at run time). Unstage or delete them.")
        if sub == "push":
            check_push(cwd, root, args, None if switched else branch, command)


# ------------------------------------------------------------------ C
def guard_edit(data):
    ti = data.get("tool_input") or {}
    path = ti.get("file_path") or ti.get("notebook_path") or ""
    if not path:
        return
    cwd = data.get("cwd") or os.getcwd()
    try:
        root = repo_root(cwd)
    except (RuntimeError, subprocess.TimeoutExpired, OSError):
        return
    rel = os.path.relpath(os.path.abspath(os.path.join(cwd, path)), root)
    if not re.match(r"^docs/decisions/\d{4}-.*\.md$", rel):
        return
    try:
        git(root, "cat-file", "-e", f"origin/develop:{rel}")
    except (RuntimeError, subprocess.TimeoutExpired):
        return  # a new ADR, not merged yet
    full = os.path.join(root, rel)
    current = open(full, encoding="utf-8").read() if os.path.exists(full) else ""
    tool = data.get("tool_name", "")
    if tool == "Write" and ti.get("content", "").startswith(current.rstrip()):
        return
    if tool == "Edit":
        old, new = ti.get("old_string", ""), ti.get("new_string", "")
        if old and new.startswith(old) and current.rstrip().endswith(old.rstrip()):
            return  # appending after the end of the ADR
    deny(f"{rel} is a merged ADR and ADRs are append-only (CLAUDE.md): add an amendment at the end, "
         "or write a new ADR that supersedes it.")


# ------------------------------------------------------------------ D
def ask_release(data):
    tool = data.get("tool_name", "")
    ti = data.get("tool_input") or {}
    if tool.endswith("merge_pull_request"):
        return ask(f"Merging PR #{ti.get('pullNumber', '?')} into its base: merges need the user's word "
                   "in every mode (ferrum-workflow G3).")
    if tool.endswith("actions_run_trigger") and "release" in str(ti.get("workflow_id", "")).lower():
        return ask(f"Running the release workflow (tag {(ti.get('inputs') or {}).get('tag', '?')}) publishes a "
                   "GitHub release: confirm (ferrum-release).")


# ------------------------------------------------------------------ F, G
def post_edit(data):
    ti = data.get("tool_input") or {}
    path = ti.get("file_path") or ""
    if not path:
        return
    cwd = data.get("cwd") or os.getcwd()
    try:
        root = repo_root(cwd)
    except (RuntimeError, subprocess.TimeoutExpired, OSError):
        return
    full = os.path.abspath(os.path.join(cwd, path))
    rel = os.path.relpath(full, root)
    if rel.endswith(".rs") and os.path.exists(full):
        subprocess.run(["rustfmt", "--edition", "2021", full], cwd=root, capture_output=True, timeout=30)
    notes = [msg for pat, msg in MATRIX if re.search(pat, rel)]
    if not notes:
        return
    seen_file = os.path.join(state_dir(root), f"reminders-{data.get('session_id', 'none')}.json")
    try:
        with open(seen_file, encoding="utf-8") as f:
            seen = set(json.load(f))
    except (OSError, ValueError):
        seen = set()
    fresh = [n for n in notes if hashlib.sha1(n.encode()).hexdigest() not in seen]
    if not fresh:
        return
    seen |= {hashlib.sha1(n.encode()).hexdigest() for n in fresh}
    with open(seen_file, "w", encoding="utf-8") as f:
        json.dump(sorted(seen), f)
    emit("PostToolUse", additionalContext="FERRUM matrix (ferrum-change §2): " + " ".join(fresh))


# ------------------------------------------------------------------ H
def session_start(data):
    cwd = data.get("cwd") or os.getcwd()
    try:
        root = repo_root(cwd)
    except (RuntimeError, subprocess.TimeoutExpired, OSError):
        return
    lines = ["FERRUM session start. Begin every task with the skill ferrum-workflow; read .claude/lessons.md."]
    try:
        git(root, "fetch", "-q", "origin", "develop", timeout=30)
        fetched = True
    except (RuntimeError, subprocess.TimeoutExpired):
        fetched = False
    try:
        branch = git(root, "rev-parse", "--abbrev-ref", "HEAD")
        behind, ahead = git(root, "rev-list", "--left-right", "--count", "origin/develop...HEAD").split()
        lines.append(f"Branch {branch}: {ahead} ahead, {behind} behind origin/develop"
                     + ("" if fetched else " (fetch failed; may be stale)") + ".")
        if branch in PROTECTED:
            lines.append(f"You are on {branch}: create a feature branch from origin/develop before any change (L2).")
        elif int(behind) > 0:
            lines.append("Merge or rebase origin/develop before designing or pushing (L3).")
    except (RuntimeError, subprocess.TimeoutExpired, ValueError):
        pass
    lessons = os.path.join(root, ".claude", "lessons.md")
    if os.path.exists(lessons):
        titles = [l[4:].strip() for l in open(lessons, encoding="utf-8") if l.startswith("### ")]
        lines.append("Lessons: " + "; ".join(t.split(" (")[0] for t in titles))
    target = os.path.join(root, "target")
    if os.path.isdir(target):
        try:
            out = subprocess.run(["du", "-sk", target], capture_output=True, text=True, timeout=5)
            gb = int(out.stdout.split()[0]) / 1024 / 1024
            if gb > 20:
                lines.append(f"target/ is {gb:.0f} GB: remove target/llvm-cov-target and target/release "
                             "if not needed (L15).")
        except (subprocess.TimeoutExpired, ValueError, IndexError):
            pass
    icd = "/usr/share/vulkan/icd.d"
    if not (os.path.isdir(icd) and any(n.startswith("lvp_icd") for n in os.listdir(icd))):
        lines.append("No lavapipe Vulkan driver: GPU tests skip locally (install mesa-vulkan-drivers).")
    emit("SessionStart", additionalContext="\n".join(lines))


# ------------------------------------------------------------------ I
def pre_compact(data):
    cwd = data.get("cwd") or os.getcwd()
    try:
        root = repo_root(cwd)
        branch = git(root, "rev-parse", "--abbrev-ref", "HEAD")
        head = git(root, "rev-parse", "--short", "HEAD")
    except (RuntimeError, subprocess.TimeoutExpired, OSError):
        return
    line = "\t".join([time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), branch, head,
                      str(data.get("trigger", "?")), str(data.get("session_id", "?"))])
    with open(os.path.join(state_dir(root), "compactions.log"), "a", encoding="utf-8") as f:
        f.write(line + "\n")


HANDLERS = {
    "guard-bash": guard_bash,
    "guard-edit": guard_edit,
    "ask-release": ask_release,
    "post-edit": post_edit,
    "session-start": session_start,
    "pre-compact": pre_compact,
}
FAIL_CLOSED = {"guard-bash"}


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] not in HANDLERS:
        print(f"usage: ferrum_hooks.py {{{'|'.join(HANDLERS)}}}", file=sys.stderr)
        return 0
    name = sys.argv[1]
    data = read_input()
    try:
        HANDLERS[name](data)
    except Decision as d:
        emit("PreToolUse", permissionDecision=d.decision, permissionDecisionReason=d.reason)
    except Exception as e:  # noqa: BLE001 — a hook must never crash the session
        cmd = str((data.get("tool_input") or {}).get("command", ""))
        if name in FAIL_CLOSED and re.search(r"\bgit\b.*\b(commit|push)\b", cmd):
            emit("PreToolUse", permissionDecision="deny",
                 permissionDecisionReason=f"FERRUM guard could not check this git command ({e}); "
                 "fix .claude/hooks/ferrum_hooks.py or ask the user.")
        print(f"ferrum_hooks {name}: {e}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
