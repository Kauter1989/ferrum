#!/usr/bin/env python3
"""Benchmark the segmentation engines on one GPU (Stage 17.6).

Runs the agent scenarios of docs/agent-segmentation.md through ``ferrum-cli``
against running bridges and records, per step, the wall time and the peak
GPU memory in use (sampled with ``nvidia-smi``). The output is JSON plus a
Markdown table for §2 of the design ("GPU budget on an RTX 3080 Ti").

Requirements: the bridges (``docker compose --profile sequential up`` in
bridges/), ``ferrum-cli`` on PATH or given with --ferrum-cli, ``nvidia-smi``,
and a CT series you may use. No data is written outside the workspace.

Example::

    scripts/benchmark_engines.py --series /data/ct/lung_053.nii.gz --modality CT \\
        --interactive http://127.0.0.1:8765 --automatic http://127.0.0.1:8766 \\
        --point v:251,198,156 --labels liver spleen kidney_left kidney_right \\
        --workspace /tmp/ferrum-bench --out bench.json

Only standard-library Python; nothing here talks to the network except the
local engine URLs (through ferrum-cli).
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from typing import List, Optional


class GpuSampler(threading.Thread):
    """Samples ``memory.used`` (MiB) of one GPU until stopped."""

    def __init__(self, gpu: int, interval_s: float = 0.2):
        super().__init__(daemon=True)
        self.gpu, self.interval_s = gpu, interval_s
        self.samples: List[int] = []
        self.stop_flag = threading.Event()

    def read(self) -> Optional[int]:
        try:
            out = subprocess.run(
                ["nvidia-smi", f"--id={self.gpu}", "--query-gpu=memory.used", "--format=csv,noheader,nounits"],
                capture_output=True, text=True, timeout=5, check=True,
            ).stdout.strip()
            return int(out.splitlines()[0])
        except (OSError, subprocess.SubprocessError, ValueError, IndexError):
            return None

    def run(self) -> None:
        while not self.stop_flag.is_set():
            v = self.read()
            if v is not None:
                self.samples.append(v)
            self.stop_flag.wait(self.interval_s)

    def stop(self) -> Optional[int]:
        self.stop_flag.set()
        self.join()
        return max(self.samples) if self.samples else None


def ferrum(cli: str, config: str, *args: str) -> dict:
    """Runs one ferrum-cli command and returns its envelope."""
    env = dict(os.environ, FERRUM_AGENT_CONFIG=config)
    out = subprocess.run([cli, *args], capture_output=True, text=True, env=env)
    try:
        return json.loads(out.stdout)
    except json.JSONDecodeError:
        return {"ok": False, "error": {"code": "cli", "message": out.stderr.strip() or out.stdout.strip()}}


def step(results: list, name: str, gpu: int, cli: str, config: str, *args: str) -> dict:
    """One measured call."""
    sampler = GpuSampler(gpu)
    baseline = sampler.read()
    sampler.start()
    start = time.monotonic()
    env = ferrum(cli, config, *args)
    seconds = time.monotonic() - start
    peak = sampler.stop()
    data = env.get("data") or {}
    seg = data.get("segment") or {}
    row = {
        "step": name,
        "ok": env.get("ok", False),
        "seconds": round(seconds, 2),
        "gpu_mib_before": baseline,
        "gpu_mib_peak": peak,
        "roi_voxels": (data.get("roi") or {}).get("voxels"),
        "volume_ml": seg.get("volume_ml"),
        "segments": len(data.get("segments") or []) or None,
        "failed_checks": (data.get("checks") or {}).get("failed"),
        "error": (env.get("error") or {}).get("message"),
    }
    results.append(row)
    status = "ok" if row["ok"] else f"FAILED: {row['error']}"
    print(f"{name:<38} {row['seconds']:>7.2f} s  peak {peak} MiB  {status}", file=sys.stderr)
    return env


def gpu_model(gpu: int) -> str:
    """Name of the GPU, or a note when ``nvidia-smi`` is missing (memory is then not measured)."""
    try:
        out = subprocess.run(["nvidia-smi", f"--id={gpu}", "--query-gpu=name", "--format=csv,noheader"],
                             capture_output=True, text=True, timeout=5)
        return out.stdout.strip() or "unknown GPU"
    except (OSError, subprocess.SubprocessError):
        return "no NVIDIA GPU found (memory not measured)"


def markdown(rows: list, gpu_name: str) -> str:
    lines = [
        f"Measured on {gpu_name} with `scripts/benchmark_engines.py`.",
        "",
        "| Step | Time (s) | GPU memory before → peak (MiB) | Region (voxels) | Result |",
        "|---|---|---|---|---|",
    ]
    for r in rows:
        result = (
            f"{r['volume_ml']} ml" if r["volume_ml"] is not None
            else f"{r['segments']} segments" if r["segments"] else ("ok" if r["ok"] else "failed")
        )
        lines.append(
            f"| {r['step']} | {r['seconds']} | {r['gpu_mib_before']} → {r['gpu_mib_peak']} | {r['roi_voxels'] or '—'} | {result} |"
        )
    return "\n".join(lines)


def write_config(path: str, engines: List[str], workspace_root: str) -> None:
    quoted = ", ".join(f'"{e}"' for e in engines)
    with open(path, "w", encoding="utf-8") as f:
        f.write(
            "[data]\n"
            f'workspace_root = "{workspace_root}"\n'
            "[network]\n"
            f"engines = [{quoted}]\n"
            f"gpu_groups = {{ bench = [{quoted}] }}\n"
        )


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--series", required=True, help="DICOM folder or NIfTI file")
    p.add_argument("--modality", help="modality for NIfTI input (e.g. CT)")
    p.add_argument("--interactive", help="interactive engine URL (e.g. nnInteractive bridge)")
    p.add_argument("--automatic", help="automatic engine URL (e.g. TotalSegmentator bridge)")
    p.add_argument("--point", help="include point for the interactive steps, e.g. v:251,198,156")
    p.add_argument("--exclude", help="exclude point for the refinement step")
    p.add_argument("--labels", nargs="*", default=[], help="structures for the automatic step (default: all)")
    p.add_argument("--workspace", default=None, help="workspace root (default: a temporary folder)")
    p.add_argument("--ferrum-cli", default=shutil.which("ferrum-cli") or "ferrum-cli")
    p.add_argument("--gpu", type=int, default=0)
    p.add_argument("--out", help="write the JSON report here")
    args = p.parse_args(argv)

    root = args.workspace or tempfile.mkdtemp(prefix="ferrum-bench-")
    os.makedirs(root, exist_ok=True)
    engines = [e for e in (args.interactive, args.automatic) if e]
    config = os.path.join(root, "ferrum-agent.toml")
    write_config(config, engines, root)
    gpu_name = gpu_model(args.gpu)
    cli, ws, rows = args.ferrum_cli, "bench", []
    modality = ["--modality", args.modality] if args.modality else []

    opened = step(rows, "study open", args.gpu, cli, config, "study", "open", "-w", ws, args.series)
    if not opened.get("ok"):
        print(json.dumps(opened, indent=2), file=sys.stderr)
        return 1
    if args.automatic:
        # a workspace of its own: a voxel holds one segment, and S2 must find its object free
        ferrum(cli, config, "study", "open", "-w", "bench-auto", args.series)
        labels = [x for label in args.labels for x in ("--label", label)]
        step(rows, "S1 segment auto", args.gpu, cli, config, "segment", "auto", "-w", "bench-auto",
             "--engine", args.automatic, "--name-prefix", "auto/", *labels, *modality)
    if args.interactive and args.point:
        first = step(rows, "S2 segment interactive (ROI)", args.gpu, cli, config, "segment", "interactive", "-w", ws,
                     "--engine", args.interactive, "--name", "bench", *modality, f"+{args.point}")
        label = ((first.get("data") or {}).get("segment") or {}).get("label")
        if label is not None:
            refine = [f"-{args.exclude}"] if args.exclude else [f"+{args.point}"]
            step(rows, "S2 refine (replay + 1 prompt)", args.gpu, cli, config, "segment", "interactive", "-w", ws,
                 "--engine", args.interactive, "--segment", str(label), "--append", *modality, *refine)
            step(rows, "S2 segment shape", args.gpu, cli, config, "segment", "shape", "-w", ws, str(label))
        step(rows, "S2 segment interactive (whole volume)", args.gpu, cli, config, "segment", "interactive", "-w", ws,
             "--engine", args.interactive, "--whole-volume", *modality, f"+{args.point}")

    report = {"gpu": gpu_name, "series_note": "not recorded (no identifiers)", "steps": rows,
              "markdown": markdown(rows, gpu_name)}
    if args.out:
        with open(args.out, "w", encoding="utf-8") as f:
            json.dump(report, f, indent=2)
    print(report["markdown"])
    return 0 if all(r["ok"] for r in rows) else 1


if __name__ == "__main__":
    sys.exit(main())
