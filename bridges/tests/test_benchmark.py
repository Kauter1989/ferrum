"""Stage 23: the benchmark's external-holder options, as a dry run.

A fake ``ferrum-cli`` stands in for the real one: it answers every command
with an envelope and, for ``gpu hold``, runs the holder's command. No GPU
and no engine are needed; the measurements themselves are the field test."""

import importlib.util
import json
import os
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "benchmark_engines.py"

FAKE_CLI = """#!{python}
import json, os, sys
args = sys.argv[1:]
log = os.environ["FAKE_CLI_LOG"]
with open(log, "a") as f:
    f.write(json.dumps(args) + "\\n")
if args[:2] == ["gpu", "hold"]:
    cmd = args[args.index("--") + 1:]
    os.execvp(cmd[0], cmd)
if args[:2] == ["segment", "interactive"]:
    print(json.dumps({{"ok": True, "warnings": ["waited 2.5 s for GPU group bench"],
                      "data": {{"segment": {{"label": 1, "volume_ml": 12.0}}, "roi": {{"voxels": 1000}}}}}}))
else:
    print(json.dumps({{"ok": True, "warnings": [], "data": {{}}}}))
"""


def load():
    spec = importlib.util.spec_from_file_location("benchmark_engines", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def fake_cli(tmp_path, monkeypatch):
    cli = tmp_path / "ferrum-cli"
    cli.write_text(FAKE_CLI.format(python=sys.executable))
    cli.chmod(0o755)
    log = tmp_path / "calls.jsonl"
    monkeypatch.setenv("FAKE_CLI_LOG", str(log))
    return cli, log


def calls(log: Path) -> list:
    return [json.loads(line) for line in log.read_text().splitlines()]


# covers 23.5-a
@pytest.mark.skipif(not os.path.exists("/proc/self/task"), reason="the holder's pid is read from /proc (Linux)")
def test_the_holder_runs_under_gpu_hold_and_each_step_records_its_wait(tmp_path, fake_cli, capsys):
    bench = load()
    cli, log = fake_cli
    out = tmp_path / "bench.json"
    code = bench.main([
        "--series", str(tmp_path / "ct.nii.gz"), "--modality", "CT",
        "--interactive", "http://127.0.0.1:8765", "--point", "v:1,2,3",
        "--workspace", str(tmp_path / "ws"), "--ferrum-cli", str(cli),
        "--capacity-mib", "12288", "--external-holder", "sleep 30", "--lease-mib", "6144",
        "--out", str(out),
    ])  # fmt: skip
    assert code == 0, capsys.readouterr().err
    hold = calls(log)[0]
    assert hold == ["gpu", "hold", "--group", "bench", "--name", "external-holder", "--lease-mib", "6144",
                    "--", "sleep", "30"]  # fmt: skip
    config = (tmp_path / "ws" / "ferrum-agent.toml").read_text()
    assert "gpu_capacity_mib = { bench = 12288 }" in config
    report = json.loads(out.read_text())
    assert report["external"] == {"holder": "sleep", "lease_mib": 6144, "capacity_mib": 12288}
    steps = {r["step"]: r for r in report["steps"]}
    assert steps["S2 segment interactive (ROI)"]["waited_s"] == 2.5
    assert steps["study open"]["waited_s"] == 0.0
    assert "holder_mib" in steps["study open"], "measured per step (None without nvidia-smi)"
    assert "| Waited for GPU (s) |" in report["markdown"]
    assert "| 2.5 |" in report["markdown"]


# covers 23.5-a
def test_a_holder_needs_a_capacity(tmp_path, fake_cli):
    bench = load()
    cli, _ = fake_cli
    with pytest.raises(SystemExit) as e:
        bench.main(["--series", "x", "--ferrum-cli", str(cli), "--external-holder", "sleep 1"])
    assert e.value.code == 2


def test_waited_is_read_from_the_warning():
    bench = load()
    assert bench.waited_s({"warnings": ["admitted over 9000 MiB; waited 12.0 s for GPU group card"]}) == 12.0
    assert bench.waited_s({"warnings": ["research use only"]}) == 0.0
    assert bench.waited_s({}) == 0.0
