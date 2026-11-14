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

With a model server on the same card (Stage 23, docs/gpu-sharing.md)::

    scripts/benchmark_engines.py ... --capacity-mib 12288 \
        --external-holder "llama-server -m model.gguf --port 8080" --lease-mib 6144 \
        --wait-ready http://127.0.0.1:8080/health

The holder runs under ``ferrum-cli gpu hold`` in the benchmark's GPU group,
so engine calls wait for room next to it; each step records the holder's
own GPU memory and the time the call waited for the GPU.

Only standard-library Python; nothing here talks to the network except the
local engine URLs (through ferrum-cli).
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request


def nvidia_smi(*args: str, check: bool = True) -> str:
    """Output of ``nvidia-smi`` with ``args``; ``OSError`` when it is not installed."""
    smi = shutil.which("nvidia-smi")
    if smi is None:
        raise OSError("nvidia-smi not found")
    # a fixed program and argument list, no shell
    return subprocess.run([smi, *args], capture_output=True, text=True, timeout=5, check=check).stdout  # noqa: S603


class GpuSampler(threading.Thread):
    """Samples ``memory.used`` (MiB) of one GPU until stopped."""

    def __init__(self, gpu: int, interval_s: float = 0.2):
        super().__init__(daemon=True)
        self.gpu, self.interval_s = gpu, interval_s
        self.samples: list[int] = []
        self.stop_flag = threading.Event()

    def read(self) -> int | None:
        try:
            out = nvidia_smi(f"--id={self.gpu}", "--query-gpu=memory.used", "--format=csv,noheader,nounits").strip()
            return int(out.splitlines()[0])
        except (OSError, subprocess.SubprocessError, ValueError, IndexError):
            return None

    def run(self) -> None:
        while not self.stop_flag.is_set():
            v = self.read()
            if v is not None:
                self.samples.append(v)
            self.stop_flag.wait(self.interval_s)

    def stop(self) -> int | None:
        self.stop_flag.set()
        self.join()
        return max(self.samples) if self.samples else None


def process_mib(pid: int | None) -> int | None:
    """GPU memory of one process (and nothing else) per ``nvidia-smi``; ``None`` when unknown."""
    if pid is None:
        return None
    try:
        out = nvidia_smi("--query-compute-apps=pid,used_memory", "--format=csv,noheader,nounits")
    except (OSError, subprocess.SubprocessError):
        return None
    rows = [[c.strip() for c in line.split(",")] for line in out.splitlines()]
    mine = [r[1] for r in rows if len(r) == 2 and r[0] == str(pid) and r[1].isdigit()]
    return sum(int(m) for m in mine) if mine else None


def waited_s(env: dict) -> float:
    """Seconds the call waited for the GPU, from its warning (0 when it did not wait)."""
    for w in env.get("warnings") or []:
        m = re.search(r"waited ([0-9.]+) s for GPU group", w)
        if m:
            return float(m.group(1))
    return 0.0


class Holder:
    """The external holder under ``ferrum-cli gpu hold``, for the benchmark's group."""

    def __init__(self, cli: str, config: str, command: str, lease_mib: int | None):
        self.argv = [cli, "gpu", "hold", "--group", "bench", "--name", "external-holder"]
        if lease_mib is not None:
            self.argv += ["--lease-mib", str(lease_mib)]
        self.argv += ["--", *shlex.split(command)]
        self.env = dict(os.environ, FERRUM_AGENT_CONFIG=config)
        self.process: subprocess.Popen | None = None

    def start(self) -> None:
        self.process = subprocess.Popen(self.argv, env=self.env)  # noqa: S603 - the operator's own command

    def running(self) -> bool:
        return self.process is not None and self.process.poll() is None

    def wait_started(self, timeout_s: float = 600) -> int | None:
        """Waits until ``gpu hold`` holds its lease and runs the command; the
        command's pid, or ``None`` if ``gpu hold`` exited first (no lease, a
        bad command)."""
        end = time.monotonic() + timeout_s
        while time.monotonic() < end and self.running():
            pid = self.command_pid()
            if pid is not None:
                return pid
            time.sleep(0.1)
        return None

    def command_pid(self) -> int | None:
        """Pid of the holder's command (the child of ``gpu hold``), for its GPU memory."""
        if self.process is None:
            return None
        try:
            with open(f"/proc/{self.process.pid}/task/{self.process.pid}/children", encoding="ascii") as f:
                kids = f.read().split()
            return int(kids[0]) if kids else None
        except (OSError, ValueError):
            return None

    def stop(self) -> None:
        if not self.running():
            return
        pid = self.command_pid()
        self.process.send_signal(signal.SIGINT)  # gpu hold passes it on and releases after the command
        try:
            self.process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            # the command runs in its own process group: end it too, or it keeps the GPU (and the lease)
            if pid is not None:
                with contextlib.suppress(OSError):
                    os.killpg(pid, signal.SIGKILL)
            self.process.kill()


def wait_ready(url: str, holder: Holder, timeout_s: float = 600) -> bool:
    """Polls ``url`` until it answers 200 (a model server loading its weights);
    ``False`` if the holder exits first. No proxy: the URL is local."""
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    end = time.monotonic() + timeout_s
    while time.monotonic() < end and holder.running():
        try:
            with opener.open(url, timeout=5) as r:
                if r.status == 200:
                    return True
        except (OSError, ValueError):
            pass
        time.sleep(1)
    return False


def ferrum(cli: str, config: str, *args: str) -> dict:
    """Runs one ferrum-cli command and returns its envelope."""
    env = dict(os.environ, FERRUM_AGENT_CONFIG=config)
    out = subprocess.run([cli, *args], capture_output=True, text=True, env=env)  # noqa: S603 - ferrum-cli, no shell
    try:
        return json.loads(out.stdout)
    except json.JSONDecodeError:
        return {"ok": False, "error": {"code": "cli", "message": out.stderr.strip() or out.stdout.strip()}}


def step(results: list, name: str, gpu: int, cli: str, config: str, *args: str, holder_pid=None) -> dict:
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
        "waited_s": waited_s(env),
        "holder_mib": process_mib(holder_pid),
    }
    results.append(row)
    status = "ok" if row["ok"] else f"FAILED: {row['error']}"
    print(f"{name:<38} {row['seconds']:>7.2f} s  peak {peak} MiB  {status}", file=sys.stderr)
    return env


def gpu_model(gpu: int) -> str:
    """Name of the GPU, or a note when ``nvidia-smi`` is missing (memory is then not measured)."""
    try:
        return (
            nvidia_smi(f"--id={gpu}", "--query-gpu=name", "--format=csv,noheader", check=False).strip() or "unknown GPU"
        )
    except (OSError, subprocess.SubprocessError):
        return "no NVIDIA GPU found (memory not measured)"


def markdown(rows: list, gpu_name: str) -> str:
    lines = [
        f"Measured on {gpu_name} with `scripts/benchmark_engines.py`.",
        "",
        "| Step | Time (s) | Waited for GPU (s) | GPU memory before → peak (MiB) | Holder (MiB) | Region (voxels)"
        " | Result |",
        "|---|---|---|---|---|---|---|",
    ]
    for r in rows:
        result = (
            f"{r['volume_ml']} ml"
            if r["volume_ml"] is not None
            else f"{r['segments']} segments"
            if r["segments"]
            else ("ok" if r["ok"] else "failed")
        )
        holder = r.get("holder_mib") if r.get("holder_mib") is not None else "—"
        lines.append(
            f"| {r['step']} | {r['seconds']} | {r.get('waited_s', 0.0)} | {r['gpu_mib_before']} → {r['gpu_mib_peak']}"
            f" | {holder} | {r['roi_voxels'] or '—'} | {result} |"
        )
    return "\n".join(lines)


def write_config(path: str, engines: list[str], workspace_root: str, capacity_mib: int | None = None) -> None:
    quoted = ", ".join(f'"{e}"' for e in engines)
    capacity = f"gpu_capacity_mib = {{ bench = {capacity_mib} }}\n" if capacity_mib else ""
    with open(path, "w", encoding="utf-8") as f:
        f.write(
            "[data]\n"
            f'workspace_root = "{workspace_root}"\n'
            "[network]\n"
            f"engines = [{quoted}]\n"
            f"gpu_groups = {{ bench = [{quoted}] }}\n"
            f"{capacity}"
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
    p.add_argument("--capacity-mib", type=int, help="GPU memory of the card for the benchmark's group (with a holder)")
    p.add_argument("--external-holder", help="command of a model server held next to the engines (gpu hold)")
    p.add_argument("--lease-mib", type=int, help="the holder's lease in MiB")
    p.add_argument("--wait-ready", help="health URL of the holder, polled before the steps start")
    args = p.parse_args(argv)
    if args.external_holder and not args.capacity_mib:
        p.error("--external-holder needs --capacity-mib: without a capacity the group admits one holder at a time")

    root = args.workspace or tempfile.mkdtemp(prefix="ferrum-bench-")
    os.makedirs(root, exist_ok=True)
    engines = [e for e in (args.interactive, args.automatic) if e]
    config = os.path.join(root, "ferrum-agent.toml")
    write_config(config, engines, root, args.capacity_mib)
    holder = Holder(args.ferrum_cli, config, args.external_holder, args.lease_mib) if args.external_holder else None
    try:
        if holder is not None:
            holder.start()
            if holder.wait_started() is None:
                print("the external holder exited before it held its lease (see its output)", file=sys.stderr)
                return 1
            if args.wait_ready and not wait_ready(args.wait_ready, holder):
                print(f"the holder did not answer {args.wait_ready}", file=sys.stderr)
                return 1
        return run_steps(args, root, config, holder)
    finally:
        if holder is not None:
            holder.stop()


def run_steps(args, root: str, config: str, holder: Holder | None) -> int:
    """The measured steps; the report goes to stdout and ``--out``."""
    gpu_name = gpu_model(args.gpu)
    cli, ws, rows = args.ferrum_cli, "bench", []
    modality = ["--modality", args.modality] if args.modality else []
    holder_pid = holder.command_pid() if holder is not None else None
    record = {
        # the program's name only: a full path may name the operator's home directory
        "holder": os.path.basename(shlex.split(args.external_holder)[0]) if args.external_holder else None,
        "lease_mib": args.lease_mib,
        "capacity_mib": args.capacity_mib,
    }

    def run(name: str, *a: str) -> dict:
        if holder is not None and not holder.running():
            # the steps would measure a card without the holder: say so instead
            rows.append({"step": name, "ok": False, "error": "the external holder exited", "seconds": 0.0,
                         "waited_s": 0.0, "gpu_mib_before": None, "gpu_mib_peak": None, "holder_mib": None,
                         "roi_voxels": None, "volume_ml": None, "segments": None, "failed_checks": None})  # fmt: skip
            return {"ok": False}
        return step(rows, name, args.gpu, cli, config, *a, holder_pid=holder_pid)

    opened = run("study open", "study", "open", "-w", ws, args.series)
    if not opened.get("ok"):
        print(json.dumps(opened, indent=2), file=sys.stderr)
        return 1
    if args.automatic:
        # a workspace of its own: a voxel holds one segment, and S2 must find its object free
        ferrum(cli, config, "study", "open", "-w", "bench-auto", args.series)
        labels = [x for label in args.labels for x in ("--label", label)]
        run(
            "S1 segment auto",
            "segment",
            "auto",
            "-w",
            "bench-auto",
            "--engine",
            args.automatic,
            "--name-prefix",
            "auto/",
            *labels,
            *modality,
        )
    if args.interactive and args.point:
        first = run(
            "S2 segment interactive (ROI)",
            "segment",
            "interactive",
            "-w",
            ws,
            "--engine",
            args.interactive,
            "--name",
            "bench",
            *modality,
            f"+{args.point}",
        )
        label = ((first.get("data") or {}).get("segment") or {}).get("label")
        if label is not None:
            refine = [f"-{args.exclude}"] if args.exclude else [f"+{args.point}"]
            run(
                "S2 refine (replay + 1 prompt)",
                "segment",
                "interactive",
                "-w",
                ws,
                "--engine",
                args.interactive,
                "--segment",
                str(label),
                "--append",
                *modality,
                *refine,
            )
            run("S2 segment shape", "segment", "shape", "-w", ws, str(label))
        run(
            "S2 segment interactive (whole volume)",
            "segment",
            "interactive",
            "-w",
            ws,
            "--engine",
            args.interactive,
            "--whole-volume",
            *modality,
            f"+{args.point}",
        )

    report = {
        "gpu": gpu_name,
        "series_note": "not recorded (no identifiers)",
        "external": record,
        "steps": rows,
        "markdown": markdown(rows, gpu_name),
    }
    if args.out:
        with open(args.out, "w", encoding="utf-8") as f:
            json.dump(report, f, indent=2)
    print(report["markdown"])
    return 0 if all(r["ok"] for r in rows) else 1


if __name__ == "__main__":
    sys.exit(main())
