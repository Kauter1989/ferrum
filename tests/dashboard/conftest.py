"""Fixtures for the dashboard generator tests (Stage 30).

The plan fixtures hold every shape the real dev_plan.md holds: ID-less
early stages, inline ✅ with PR references, struck criteria, a lagging
heading, an unknown marker, an empty criteria cell, letter stages and
Roadmap drafts. The git fixture builds a real repository: a release tag
with PR #5 merged before it, and a develop branch with PR #6 merged
after it — so lifecycle and PR-ancestry run against real git.
"""

import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts" / "dashboard"))

import generate

PLAN_TAG = """# Development plan

Legend: ✅ done · 🚧 in progress · 📋 planned.

## Stage 1 — Foundation ✅

| # | User story | Acceptance criteria |
|---|---|---|
| 1.1 | As a **developer**, I want layers. | Dependencies point inward. |

## Stage 2 — Mixed 🚧 (G1 ✅)

| # | User story | Acceptance criteria |
|---|---|---|
| 2.1 | As a **user**, I want mixing. | [2.1-a] First done. ✅ (PR 5) [2.1-b] Second pending. ~~[2.1-c] Struck.~~ why |
| 2.2 | As a **user**, I want ancestry. | [2.2-a] Merged but unmarked (PR 6). |

## Stage 3 — Planned 📋

| # | User story | Acceptance criteria |
|---|---|---|
| 3.1 | As a **user**, I want planning. | [3.1-a] Not started. |

## Stage 6 — Working 🚧

| # | User story | Acceptance criteria |
|---|---|---|
| 6.1 | As a **user**, I want progress. | [6.1-a] Being built. |

## Stage 8 — Bullet format ✅

**Criteria**

- [8.1-a] First bullet criterion
  with a continuation line. ✅
- [8.1-b] Second bullet criterion.
- ~~[8.1-c] Struck bullet.~~ reason

**Verification**

| Criterion | Level | Evidence | Outcome |
|---|---|---|---|
| 8.1-b | test | somewhere | ✅ |

## Roadmap

### Next

| # | What | Kind | Why now |
|---|---|---|---|
| 1 | Release things | release | soon |

### Backlog

- A backlog item.

### Stage 9 — Draft 📋 (draft)

| # | User story (draft) |
|---|---|
| 9.1 | As a **user**, I want drafts. |
"""

PLAN_TARGET = """# Development plan

Legend: ✅ done · 🚧 in progress · 📋 planned.

## Stage 1 — Foundation ✅

| # | User story | Acceptance criteria |
|---|---|---|
| 1.1 | As a **developer**, I want layers. | Dependencies point inward. |

## Stage 2 — Mixed 🚧 (G1 ✅)

| # | User story | Acceptance criteria |
|---|---|---|
| 2.1 | As a **user**, I want mixing. | [2.1-a] First done. ✅ (PR 5) [2.1-b] Second done. ✅ ~~[2.1-c] Struck.~~ why |
| 2.2 | As a **user**, I want ancestry. | [2.2-a] Merged but unmarked (PR 6). |

## Stage 3 — Planned 📋

| # | User story | Acceptance criteria |
|---|---|---|
| 3.1 | As a **user**, I want planning. | [3.1-a] Not started. |

## Stage 4 — Weird

| # | User story | Acceptance criteria |
|---|---|---|
| 4.1 | As a **user**, I want refusals. | [4.1-a] Something. |

## Stage 5 — Empty 🚧

| # | User story | Acceptance criteria |
|---|---|---|
| 5.1 | As a **user**, I want warnings. | |

## Stage 6 — Working 🚧

| # | User story | Acceptance criteria |
|---|---|---|
| 6.1 | As a **user**, I want progress. | [6.1-a] Being built. |

### Implementation

| Task | What | Criteria | PR | State |
|---|---|---|---|---|
| 6.1 | build it | 6.1-a | 1 | |

## Stage 7 — Hotfix 🚧

| # | User story | Acceptance criteria |
|---|---|---|
| 7.1 | As a **user**, I want hotfixes. | [7.1-a] Fixed on main only (PR 7). |

## Stage 8 — Bullet format ✅

**Criteria**

- [8.1-a] First bullet criterion
  with a continuation line. ✅
- [8.1-b] Second bullet criterion.
- ~~[8.1-c] Struck bullet.~~ reason

**Verification**

| Criterion | Level | Evidence | Outcome |
|---|---|---|---|
| 8.1-b | test | somewhere | ✅ |

## Stage F1 — A bug ✅

| # | User story | Acceptance criteria |
|---|---|---|
| F1.1 | As a **user**, I want fixes. | [F1.1-a] Fixed. ✅ |

## Roadmap

### Next

| # | What | Kind | Why now |
|---|---|---|---|
| 1 | Release things | release | soon |

### Backlog

- A backlog item.

### Stage 9 — Draft 📋 (draft)

| # | User story (draft) |
|---|---|
| 9.1 | As a **user**, I want drafts. |
"""

VALIDATION_MD = """# Validation

| Criterion | Verified by | Level |
|---|---|---|
| [2.1-b] | look at it | visual |
"""


def run_git(root, *args):
    argv = ["git", "-C", str(root), *args]
    return subprocess.run(argv, check=True, capture_output=True, text=True, encoding="utf-8").stdout  # noqa: S603


def commit_files(root, files, message):
    for name, content in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8", newline="\n")
    run_git(root, "add", "-A")
    run_git(root, "commit", "-m", message)


def merge_pr(root, branch, number):
    run_git(root, "checkout", "-b", branch)
    commit_files(root, {f"{branch}.txt": branch}, branch)
    run_git(root, "checkout", "-")
    run_git(root, "merge", "--no-ff", branch, "-m", f"Merge pull request #{number} from test/{branch}")


@pytest.fixture(scope="session")
def plan_repo(tmp_path_factory):
    """A repository with a release tag (PR #5 before it) and develop (PR #6 after).

    Session-scoped: the tests only read it (build_data reads committed
    state; cargo metadata --no-deps writes nothing).
    """
    tmp_path = tmp_path_factory.mktemp("plan-repo")
    run_git(tmp_path, "init", "-b", "main")
    run_git(tmp_path, "config", "user.email", "test@example.com")
    run_git(tmp_path, "config", "user.name", "Test")
    run_git(tmp_path, "config", "commit.gpgsign", "false")
    commit_files(
        tmp_path,
        {
            "dev_plan.md": PLAN_TAG,
        },
        "init",
    )
    # develop starts at init: the release-only hotfix (PR #7, merged into
    # main before the tag) never reaches it.
    run_git(tmp_path, "checkout", "-b", "develop")
    run_git(tmp_path, "checkout", "main")
    merge_pr(tmp_path, "feat/five", 5)
    merge_pr(tmp_path, "feat/hotfix", 7)
    run_git(tmp_path, "tag", "v0.1.0")
    run_git(tmp_path, "checkout", "develop")
    commit_files(
        tmp_path,
        {
            "dev_plan.md": PLAN_TARGET,
            "crates/ferrum-io/src/lib.rs": "// changed since the release\n",
            "crates/x/tests/t.rs": "// covers 2.1-a\n",
            "docs/validation.md": VALIDATION_MD,
        },
        "develop plan",
    )
    merge_pr(tmp_path, "feat/six", 6)
    # A body mention of an issue number is not a merge (review finding).
    (tmp_path / "note.txt").write_text("note\n", encoding="utf-8", newline="\n")
    run_git(tmp_path, "add", "-A")
    run_git(tmp_path, "commit", "-m", "docs: note", "-m", "mentions (#99) in the body")
    run_git(tmp_path, "remote", "add", "origin", "https://github.com/test/repo.git")
    return tmp_path


@pytest.fixture(scope="session")
def repo_data(plan_repo):
    return generate.build_data(plan_repo, ref="develop", release_ref="main")
