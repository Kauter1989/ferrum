"""Command line: ``ferrum-bridge <engine> [options]``.

Engines: ``nninteractive`` (interactive), ``totalsegmentator`` (automatic),
``monailabel`` (a running MONAI Label server: interactive and automatic)
and ``fake`` (model-free, for checking a set-up and for tests).
"""

from __future__ import annotations

import argparse
import logging
import os

import uvicorn

from .protocol import create_app


def _parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="ferrum-bridge", description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--host", default=os.environ.get("FERRUM_BRIDGE_HOST", "127.0.0.1"),
                   help="interface to listen on (default 127.0.0.1; use an SSH tunnel for remote access)")
    p.add_argument("--port", type=int, default=int(os.environ.get("FERRUM_BRIDGE_PORT", "8765")))
    p.add_argument("--token", default=os.environ.get("FERRUM_ENGINE_TOKEN"),
                   help="require 'Authorization: Bearer <token>' (env FERRUM_ENGINE_TOKEN)")
    p.add_argument("--session-ttl", type=int, default=3600)
    sub = p.add_subparsers(dest="engine", required=True)

    nni = sub.add_parser("nninteractive", help="nnInteractive: interactive 3D segmentation (GPU)")
    nni.add_argument("--device", default=os.environ.get("NNINTERACTIVE_DEVICE", "cuda:0"))
    nni.add_argument("--model-id", default=os.environ.get("NNINTERACTIVE_MODEL_ID"),
                     help="nnInteractive model id (default: the manifest's default model)")
    nni.add_argument("--model-dir", default=os.environ.get("NNINTERACTIVE_MODEL_FOLDER"),
                     help="use an already downloaded model folder instead of downloading")
    nni.add_argument("--torch-compile", action="store_true", help="faster predictions after a slow first one")
    nni.add_argument("--deterministic", action="store_true",
                     help="deterministic GPU kernels: replayed prompts give identical masks (slightly slower)")

    ts = sub.add_parser("totalsegmentator", help="TotalSegmentator: automatic segmentation of CT/MR structures")
    ts.add_argument("--task", default=os.environ.get("TOTALSEG_TASK", "total"),
                    help="TotalSegmentator task (default 'total': 117 CT structures; 'total_mr' for MR)")
    ts.add_argument("--device", default=os.environ.get("TOTALSEG_DEVICE", "gpu"), help="gpu, gpu:1, cpu or mps")
    ts.add_argument("--fast", action="store_true", help="3 mm model: faster and lighter, less precise")
    ts.add_argument("--license-number", default=os.environ.get("TOTALSEG_LICENSE"),
                    help="licence number for tasks that need one")
    ts.add_argument("--in-process", action="store_true",
                    help="run jobs in the bridge process (default: a child process per job, which returns "
                         "all GPU memory when the job ends and can be cancelled at once)")

    ml = sub.add_parser("monailabel", help="MONAI Label: serve a running MONAI Label server's models")
    ml.add_argument("--server", default=os.environ.get("MONAI_LABEL_URL", "http://127.0.0.1:8000"),
                    help="MONAI Label server URL (default http://127.0.0.1:8000)")
    ml.add_argument("--model", default=os.environ.get("MONAI_LABEL_MODEL"),
                    help="interactive model (deepedit/deepgrow/annotation type; default: the first one; '' = none)")
    ml.add_argument("--auto-model", default=os.environ.get("MONAI_LABEL_AUTO_MODEL"),
                    help="segmentation model for automatic jobs (default: the first one; '' = none)")
    ml.add_argument("--label", help="DeepEdit label the clicks segment (default: the model's first label)")
    ml.add_argument("--monai-token", default=os.environ.get("MONAI_LABEL_TOKEN"),
                    help="bearer token for a MONAI Label server with authentication")
    ml.add_argument("--clinical-weights", action="store_true",
                    help="do not mark the engine 'Research use only' (only if the models' licences allow it)")

    sub.add_parser("fake", help="model-free region growing and intensity bands (tests, set-up checks)")
    return p


def build_backend(args):
    """Creates the backend selected on the command line."""
    if args.engine == "nninteractive":
        from .nninteractive import NnInteractiveBackend

        return NnInteractiveBackend(args.device, args.model_id, args.model_dir, args.torch_compile, args.deterministic)
    if args.engine == "totalsegmentator":
        from .totalsegmentator import TotalSegmentatorBackend

        return TotalSegmentatorBackend(args.task, args.device, args.fast, args.license_number,
                                       isolate=not args.in_process)
    if args.engine == "monailabel":
        from .monailabel import MonaiLabelBackend

        return MonaiLabelBackend(args.server, args.model, args.auto_model, args.label,
                                 args.monai_token, research_only=not args.clinical_weights)
    from .backends import FakeBackend

    return FakeBackend()


def main(argv=None) -> None:
    args = _parser().parse_args(argv)
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s")
    backend = build_backend(args)
    info = backend.info()
    logging.info("serving %s %s on %s — %s", info["name"], info["version"], info["device"], info["license"])
    uvicorn.run(create_app(backend, args.token, args.session_ttl), host=args.host, port=args.port, log_level="info")


if __name__ == "__main__":
    main()
