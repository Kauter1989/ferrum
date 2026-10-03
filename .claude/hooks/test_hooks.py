#!/usr/bin/env python3
"""Tests for ferrum_hooks.py: every blocking hook stops what it must and lets the rest through.

    python3 .claude/hooks/test_hooks.py
Builds throw-away git repositories (with a bare origin) in a temp dir.
"""

import json
import os
import subprocess
import sys
import tempfile
import unittest

HOOKS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "ferrum_hooks.py")


def sh(cwd, *args):
    subprocess.run(args, cwd=cwd, check=True, capture_output=True)


def run_hook(name, payload):
    out = subprocess.run([sys.executable, HOOKS, name], input=json.dumps(payload),
                         capture_output=True, text=True, timeout=120)
    assert out.returncode == 0, out.stderr
    return json.loads(out.stdout)["hookSpecificOutput"] if out.stdout.strip() else None


class Repo:
    """A clone with origin/develop and origin/main, checked out on a feature branch."""

    def __init__(self):
        self.tmp = tempfile.TemporaryDirectory()
        base = self.tmp.name
        origin, self.path = os.path.join(base, "origin.git"), os.path.join(base, "work")
        sh(base, "git", "init", "-q", "--bare", origin)
        sh(base, "git", "clone", "-q", origin, self.path)
        for k, v in (("user.email", "t@example.com"), ("user.name", "t"), ("commit.gpgsign", "false")):
            sh(self.path, "git", "config", k, v)
        self.write("docs/decisions/0001-first.md", "# ADR 1\n\nDecision.\n")
        self.write("crates/ferrum-agent/src/commands/view.rs", "fn a() {}\n")
        sh(self.path, "git", "checkout", "-q", "-b", "develop")
        sh(self.path, "git", "add", "-A")
        sh(self.path, "git", "commit", "-q", "-m", "init")
        sh(self.path, "git", "push", "-q", "origin", "develop")
        sh(self.path, "git", "push", "-q", "origin", "develop:main")
        sh(self.path, "git", "checkout", "-q", "-b", "feat/x")

    def write(self, rel, text, mode="w"):
        p = os.path.join(self.path, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, mode) as f:
            f.write(text)
        return p

    def bash(self, command):
        return run_hook("guard-bash", {"tool_name": "Bash", "tool_input": {"command": command}, "cwd": self.path})

    def close(self):
        self.tmp.cleanup()


def decision(out):
    return out and out.get("permissionDecision")


class ProtectedBranches(unittest.TestCase):  # A
    def setUp(self):
        self.r = Repo()

    def tearDown(self):
        self.r.close()

    def test_commit_on_develop_is_denied(self):
        sh(self.r.path, "git", "checkout", "-q", "develop")
        self.assertEqual(decision(self.r.bash("git commit -m x")), "deny")

    def test_commit_on_feature_branch_passes(self):
        self.assertIsNone(self.r.bash("git commit -m x"))

    def test_branching_off_in_the_same_command_passes(self):
        sh(self.r.path, "git", "checkout", "-q", "develop")
        self.assertIsNone(self.r.bash("git checkout -b feat/y origin/develop && git commit -m x"))

    def test_push_to_protected_refspecs_is_denied(self):
        for cmd in ("git push origin develop", "git push origin HEAD:main", "git push origin +feat/x:refs/heads/develop",
                    "git push --all origin", "cd . && git -C . push -u origin develop"):
            self.assertEqual(decision(self.r.bash(cmd)), "deny", cmd)

    def test_bare_push_from_develop_is_denied(self):
        sh(self.r.path, "git", "checkout", "-q", "develop")
        self.assertEqual(decision(self.r.bash("git push")), "deny")

    def test_feature_push_passes(self):
        self.assertIsNone(self.r.bash("FERRUM_SKIP_PREPUSH=1 git push -u origin feat/x"))

    def test_non_git_commands_pass(self):
        self.assertIsNone(self.r.bash("echo develop && ls"))


class NoData(unittest.TestCase):  # B
    def setUp(self):
        self.r = Repo()

    def tearDown(self):
        self.r.close()

    def test_staged_nifti_is_denied(self):
        self.r.write("data/ct.nii.gz", "x")
        sh(self.r.path, "git", "add", "data/ct.nii.gz")
        out = self.r.bash("git commit -m x")
        self.assertEqual(decision(out), "deny")
        self.assertIn("ct.nii.gz", out["permissionDecisionReason"])

    def test_add_all_and_commit_with_untracked_dicom_is_denied(self):
        self.r.write("series/IM0001.dcm", "x")
        self.assertEqual(decision(self.r.bash("git add -A && git commit -m x")), "deny")

    def test_large_binary_is_denied_but_not_in_docs_images(self):
        self.r.write("blob.bin", "\0" * (2 << 20))
        self.assertEqual(decision(self.r.bash("git add -A && git commit -m x")), "deny")
        os.remove(os.path.join(self.r.path, "blob.bin"))
        self.r.write("docs/images/shot.png", "\0" * (2 << 20))
        self.assertIsNone(self.r.bash("git add -A && git commit -m x"))

    def test_deleting_a_data_file_passes(self):
        self.r.write("old.nii", "x")
        sh(self.r.path, "git", "add", "old.nii")
        sh(self.r.path, "git", "-c", "core.hooksPath=/dev/null", "commit", "-q", "-m", "x")
        sh(self.r.path, "git", "rm", "-q", "old.nii")
        self.assertIsNone(self.r.bash("git commit -m remove"))


class AdrsAppendOnly(unittest.TestCase):  # C
    def setUp(self):
        self.r = Repo()
        self.adr = os.path.join(self.r.path, "docs/decisions/0001-first.md")

    def tearDown(self):
        self.r.close()

    def edit(self, tool, **ti):
        return run_hook("guard-edit", {"tool_name": tool, "tool_input": {"file_path": self.adr, **ti},
                                       "cwd": self.r.path})

    def test_rewriting_a_merged_adr_is_denied(self):
        self.assertEqual(decision(self.edit("Edit", old_string="Decision.", new_string="Other.")), "deny")
        self.assertEqual(decision(self.edit("Write", content="# ADR 1\n\nOther.\n")), "deny")

    def test_appending_an_amendment_passes(self):
        self.assertIsNone(self.edit("Edit", old_string="Decision.", new_string="Decision.\n\n## Amendment\n"))
        self.assertIsNone(self.edit("Write", content="# ADR 1\n\nDecision.\n\n## Amendment\n"))

    def test_new_adr_passes(self):
        out = run_hook("guard-edit", {"tool_name": "Write", "cwd": self.r.path, "tool_input": {
            "file_path": os.path.join(self.r.path, "docs/decisions/0002-new.md"), "content": "x"}})
        self.assertIsNone(out)


class AskUser(unittest.TestCase):  # D
    def test_merge_and_release_ask(self):
        self.assertEqual(decision(run_hook("ask-release", {"tool_name": "mcp__github__merge_pull_request",
                                                           "tool_input": {"pullNumber": 41}})), "ask")
        self.assertEqual(decision(run_hook("ask-release", {"tool_name": "mcp__github__actions_run_trigger",
                                                           "tool_input": {"workflow_id": "release.yml",
                                                                          "inputs": {"tag": "v0.4.0"}}})), "ask")
        self.assertIsNone(run_hook("ask-release", {"tool_name": "mcp__github__actions_run_trigger",
                                                   "tool_input": {"workflow_id": "ci.yml"}}))

    def test_pushing_tags_asks(self):
        r = Repo()
        try:
            self.assertEqual(decision(r.bash("git push origin --tags")), "ask")
            self.assertEqual(decision(r.bash("git push origin v0.4.0")), "ask")
        finally:
            r.close()


class VisualMarker(unittest.TestCase):  # E2
    def test_render_change_needs_a_fresh_visual_check(self):
        r = Repo()
        try:
            r.write("crates/ferrum-agent/src/commands/view.rs", "fn a() { /* changed */ }\n")
            sh(r.path, "git", "commit", "-q", "-am", "render")
            out = r.bash("git push -u origin feat/x")
            self.assertEqual(decision(out), "deny")
            self.assertIn("phantom_render.py", out["permissionDecisionReason"])
            sys.path.insert(0, os.path.dirname(HOOKS))
            import ferrum_hooks
            with open(os.path.join(ferrum_hooks.state_dir(r.path), "visual-check.json"), "w") as f:
                json.dump({"hash": ferrum_hooks.visual_hash(r.path)}, f)
            self.assertIsNone(r.bash("git push -u origin feat/x"))
            r.write("crates/ferrum-agent/src/commands/view.rs", "fn a() { /* again */ }\n")
            self.assertEqual(decision(r.bash("git push -u origin feat/x")), "deny")
        finally:
            r.close()


class Reminders(unittest.TestCase):  # F, G, H, I
    def test_matrix_reminder_once_per_session_and_rustfmt(self):
        r = Repo()
        try:
            p = r.write("crates/ferrum-render/src/shaders/slice.wgsl", "x\n")
            payload = {"tool_name": "Edit", "tool_input": {"file_path": p}, "cwd": r.path, "session_id": "s1"}
            out = run_hook("post-edit", payload)
            self.assertIn("view.rs", out["additionalContext"])
            self.assertIsNone(run_hook("post-edit", payload))
            rs = r.write("crates/x/src/lib.rs", "fn   f( ) {  }\n")
            run_hook("post-edit", {"tool_name": "Write", "tool_input": {"file_path": rs}, "cwd": r.path})
            with open(rs) as f:
                self.assertEqual(f.read(), "fn f() {}\n")
        finally:
            r.close()

    def test_session_start_and_pre_compact(self):
        r = Repo()
        try:
            r.write(".claude/lessons.md", "### L1 — Outlines on two sides (2026-10)\n")
            out = run_hook("session-start", {"cwd": r.path})
            self.assertIn("Branch feat/x", out["additionalContext"])
            self.assertIn("Outlines on two sides", out["additionalContext"])
            run_hook("pre-compact", {"cwd": r.path, "trigger": "auto", "session_id": "s1"})
            with open(os.path.join(r.path, ".claude/state/compactions.log")) as f:
                log = f.read()
            self.assertIn("feat/x", log)
            self.assertIn("auto", log)
        finally:
            r.close()


if __name__ == "__main__":
    unittest.main(verbosity=1)
