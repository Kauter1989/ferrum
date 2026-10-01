"""Command line: ``python -m ferrum_nninteractive [--backend nninteractive|fake] ...``."""

from __future__ import annotations

import argparse
import logging
import os

import uvicorn

from .protocol import create_app


def main(argv=None) -> None:
    p = argparse.ArgumentParser(prog="ferrum-nninteractive", description=__doc__)
    p.add_argument("--host", default=os.environ.get("FERRUM_BRIDGE_HOST", "127.0.0.1"),
                   help="interface to listen on (default 127.0.0.1; use an SSH tunnel for remote access)")
    p.add_argument("--port", type=int, default=int(os.environ.get("FERRUM_BRIDGE_PORT", "8765")))
    p.add_argument("--backend", choices=["nninteractive", "fake"], default="nninteractive",
                   help="'fake' is a model-free region grower for testing the setup")
    p.add_argument("--device", default=os.environ.get("NNINTERACTIVE_DEVICE", "cuda:0"))
    p.add_argument("--model-id", default=os.environ.get("NNINTERACTIVE_MODEL_ID"),
                   help="nnInteractive model id (default: the manifest's default model)")
    p.add_argument("--model-dir", default=os.environ.get("NNINTERACTIVE_MODEL_FOLDER"),
                   help="use an already downloaded model folder instead of downloading")
    p.add_argument("--torch-compile", action="store_true", help="faster predictions after a slow first one")
    p.add_argument("--token", default=os.environ.get("FERRUM_ENGINE_TOKEN"),
                   help="require 'Authorization: Bearer <token>' (env FERRUM_ENGINE_TOKEN)")
    p.add_argument("--session-ttl", type=int, default=3600)
    args = p.parse_args(argv)
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s")

    if args.backend == "fake":
        from .backends import FakeBackend

        backend = FakeBackend()
    else:
        from .nninteractive_backend import NnInteractiveBackend

        backend = NnInteractiveBackend(args.device, args.model_id, args.model_dir, args.torch_compile)
    info = backend.info()
    logging.info("serving %s %s on %s — %s", info["name"], info["version"], info["device"], info["license"])
    uvicorn.run(create_app(backend, args.token, args.session_ttl), host=args.host, port=args.port, log_level="info")


if __name__ == "__main__":
    main()
