"""Generator tests: plan parsing, lifecycle, coverage, crates, output.

Every test cites the Stage 30 criterion it verifies (ferrum-analyze reads
these comments). Fixtures live in conftest.py; the git fixture is a real
repository, so ancestry and grep run against real git.
"""

import json
import re

import generate

# covers 30.1-a


def test_plan_parses_to_the_exact_fixture():
    parsed = generate.parse_plan(generate_target_plan())
    stages = parsed["stages"]
    assert [s["id"] for s in stages] == ["1", "2", "3", "4", "5", "6", "F1"]
    assert [s["heading_status"] for s in stages] == [
        "done",
        "progress",
        "planned",
        "unknown",
        "progress",
        "progress",
        "done",
    ]
    assert stages[1]["title"].startswith("Mixed")
    story = stages[1]["stories"][0]
    assert story["id"] == "2.1"
    assert story["text"] == "As a **user**, I want mixing."
    criteria = story["criteria"]
    assert [c["id"] for c in criteria] == ["2.1-a", "2.1-b"]
    assert criteria[0]["done_marker"] is True
    assert criteria[0]["prs"] == [5]
    assert criteria[0]["text"] == "First done. ✅ (PR 5)"
    assert criteria[1]["done_marker"] is True
    assert criteria[1]["prs"] == []
    assert stages[1]["stories"][1]["criteria"][0]["prs"] == [6]


def generate_target_plan():
    from conftest import PLAN_TARGET

    return PLAN_TARGET


# covers 30.1-a


def test_implementation_tables_are_not_stories():
    """Stage 26's task 26.4 is an Implementation row, not a story (real-plan bug)."""
    parsed = generate.parse_plan(generate_target_plan())
    stage6 = next(s for s in parsed["stages"] if s["id"] == "6")
    assert [st["id"] for st in stage6["stories"]] == ["6.1"]
    assert len(stage6["stories"][0]["criteria"]) == 1
    assert not any("Stage 6" in w for w in parsed["warnings"])


# covers 30.1-a


def test_struck_criteria_are_excluded():
    parsed = generate.parse_plan(generate_target_plan())
    ids = [c["id"] for s in parsed["stages"] for st in s["stories"] for c in st["criteria"]]
    assert "2.1-c" not in ids


# covers 30.1-a


def test_letter_and_draft_stages_parse():
    parsed = generate.parse_plan(generate_target_plan())
    f1 = next(s for s in parsed["stages"] if s["id"] == "F1")
    assert f1["stories"][0]["criteria"][0]["id"] == "F1.1-a"
    drafts = parsed["roadmap"]["drafts"]
    assert [d["id"] for d in drafts] == ["9"]
    assert drafts[0]["heading_status"] == "planned"
    assert drafts[0]["stories"][0]["id"] == "9.1"
    assert parsed["roadmap"]["next"] == [{"n": "1", "what": "Release things", "kind": "release", "why": "soon"}]
    assert parsed["roadmap"]["backlog"] == ["A backlog item."]


# covers 30.1-a


def test_idless_stages_parse_as_story_units():
    parsed = generate.parse_plan(generate_target_plan())
    story = parsed["stages"][0]["stories"][0]
    assert len(story["criteria"]) == 1
    unit = story["criteria"][0]
    assert unit["kind"] == "story"
    assert unit["id"] == "1.1"
    assert unit["text"] == "Dependencies point inward."
    assert unit["done_marker"] is False


# covers 30.1-b


def test_lifecycle_buckets_on_a_fixture_repository(repo_data):
    assert repo_data["meta"]["release_tag"] == "v0.1.0"
    expected = {
        "1": ("released", {"released": 1}),
        "2": ("develop", {"released": 1, "develop": 2}),
        "3": ("planned", {"planned": 1}),
        "4": ("unknown", {"unknown": 1}),
        "5": ("unknown", {"unknown": 1}),
        "6": ("progress", {"progress": 1}),
        "F1": ("develop", {"develop": 1}),
    }
    for stage in repo_data["stages"]:
        bucket, counts = expected[stage["id"]]
        assert stage["bucket"] == bucket, stage["id"]
        for key, value in counts.items():
            assert stage["counts"][key] == value, (stage["id"], key)
        assert sum(stage["counts"].values()) == len(generate.stage_units(stage))
    criteria = {c["id"]: c for s in repo_data["stages"] for c in generate.stage_units(s)}
    assert criteria["2.1-a"]["bucket"] == "released"
    assert criteria["2.1-b"]["bucket"] == "develop"
    assert criteria["2.2-a"]["bucket"] == "develop"
    assert criteria["3.1-a"]["bucket"] == "planned"
    assert criteria["6.1-a"]["bucket"] == "progress"


# covers 30.1-b


def test_lagging_heading_criterion_is_develop(repo_data):
    stage2 = next(s for s in repo_data["stages"] if s["id"] == "2")
    assert stage2["heading_status"] == "progress"
    assert all(u["done"] for u in generate.stage_units(stage2))
    assert stage2["bucket"] == "develop"
    assert any("Stage 2" in w and "lags" in w for w in repo_data["meta"]["warnings"])


# covers 30.1-b


def test_stage_bucket_priority_and_counts():
    assert generate.stage_aggregate(["released", "develop"])[0] == "develop"
    assert generate.stage_aggregate(["released", "progress"])[0] == "progress"
    assert generate.stage_aggregate(["planned", "unknown"])[0] == "unknown"
    assert generate.stage_aggregate(["released", "released"])[0] == "released"
    assert generate.stage_aggregate(["planned"])[0] == "planned"
    bucket, counts = generate.stage_aggregate(["released", "develop", "develop"])
    assert bucket == "develop"
    assert counts == {"released": 1, "develop": 2, "progress": 0, "planned": 0, "unknown": 0}


# covers 30.1-b


def test_no_release_tag_makes_done_items_develop(plan_repo):
    root_sha = generate.Git(plan_repo).run("rev-list", "--max-parents=0", "develop").strip()
    data = generate.build_data(plan_repo, ref="develop", release_ref=root_sha)
    assert data["meta"]["release_tag"] is None
    stage1 = next(s for s in data["stages"] if s["id"] == "1")
    assert stage1["bucket"] == "develop"
    assert any("no release tag" in w for w in data["meta"]["warnings"])


# covers 30.1-c


def test_coverage_follows_the_analyze_convention(repo_data):
    criteria = {c["id"]: c for s in repo_data["stages"] for c in generate.stage_units(s)}
    assert criteria["2.1-a"]["tests"] == ["crates/x/tests/t.rs"]
    assert criteria["2.1-b"]["levels"] == ["visual"]
    assert criteria["3.1-a"]["tests"] == []
    assert criteria["3.1-a"]["levels"] == []


# covers 30.1-d


def test_crate_graph_and_layering_violations(repo_data):
    crates = {c["name"]: c for c in repo_data["crates"]}
    assert set(crates) == {"ferrum-domain", "ferrum-io", "ferrum-app"}
    assert crates["ferrum-domain"]["description"] == "Domain."
    assert crates["ferrum-io"]["deps"] == ["ferrum-domain"]
    assert crates["ferrum-domain"]["violations"] == ["ferrum-domain → ferrum-io (layering: dependencies point inward)"]
    assert crates["ferrum-app"]["violations"] == ["ferrum-app → wgpu (ferrum-app must compile without wgpu/egui)"]
    assert crates["ferrum-io"]["violations"] == []
    rules = {(v["from"], v["to"]) for v in repo_data["layering_violations"]}
    assert rules == {("ferrum-domain", "ferrum-io"), ("ferrum-app", "wgpu")}


# covers 30.1-d


def test_fallback_parses_the_workspace_toml(plan_repo):
    crates = {c["name"]: c for c in generate.crates_from_toml(plan_repo)}
    assert set(crates) == {"ferrum-domain", "ferrum-io", "ferrum-app"}
    assert crates["ferrum-app"]["deps"] == ["ferrum-domain", "wgpu"]
    assert crates["ferrum-domain"]["description"] == "Domain."


# covers 30.1-d


def test_changed_crates_are_marked(repo_data):
    crates = {c["name"]: c for c in repo_data["crates"]}
    assert crates["ferrum-io"]["changed_since_release"] is True
    assert crates["ferrum-domain"]["changed_since_release"] is False
    assert crates["ferrum-app"]["changed_since_release"] is False


# covers 30.1-e


def test_output_is_byte_identical_across_runs(plan_repo):
    first = generate.build_data(plan_repo, ref="develop", release_ref="main")
    second = generate.build_data(plan_repo, ref="develop", release_ref="main")
    dump = lambda d: json.dumps(d, sort_keys=True, ensure_ascii=False)  # noqa: E731
    assert dump(first) == dump(second)
    assert first["meta"]["generated_at"] == second["meta"]["generated_at"]


# covers 30.1-e


def test_output_holds_no_machine_paths(repo_data):
    text = json.dumps(repo_data, ensure_ascii=False)
    assert not re.search(r"[A-Za-z]:\\|/home/|/Users/|/mnt/", text)
    assert repo_data["meta"]["repo_url"] == "https://github.com/test/repo"


# covers 30.1-f


def test_unknown_marker_warns_and_marks_unknown():
    parsed = generate.parse_plan(generate_target_plan())
    stage4 = next(s for s in parsed["stages"] if s["id"] == "4")
    assert stage4["heading_status"] == "unknown"
    assert any("Stage 4" in w for w in parsed["warnings"])


# covers 30.1-f


def test_empty_criteria_cell_warns(repo_data):
    stage5 = next(s for s in repo_data["stages"] if s["id"] == "5")
    assert stage5["bucket"] == "unknown"
    assert any("Stage 5" in w and "empty" in w for w in repo_data["meta"]["warnings"])


# covers 30.1-g


def test_ci_runs_the_dashboard_tests():
    """The python-lint job must install pytest and run tests/dashboard."""
    from pathlib import Path

    ci = (Path(__file__).resolve().parents[2] / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    assert "pipx install pytest" in ci
    assert "pytest tests/dashboard" in ci


# covers 30.1-b


def test_done_at_rules():
    unit = {"kind": "criterion", "id": "1.1-a", "text": "", "done_marker": False, "prs": [7]}
    assert generate.done_at(unit, "done", {}) is True
    assert generate.done_at(unit, "progress", {}) is False
    assert generate.done_at(unit, "progress", {7: True}) is True
    assert generate.done_at(unit, "progress", {7: False}) is False
    marked = {**unit, "done_marker": True}
    assert generate.done_at(marked, "planned", {}) is True


# covers 30.1-b


def test_item_bucket_rules():
    assert generate.item_bucket(True, True, "done") == "released"
    assert generate.item_bucket(False, True, "progress") == "develop"
    assert generate.item_bucket(False, False, "progress") == "progress"
    assert generate.item_bucket(False, False, "planned") == "planned"
    assert generate.item_bucket(False, False, "unknown") == "unknown"
