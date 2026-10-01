# FERRUM engine bridges

Bridges serve segmentation engines over the
[FERRUM Engine Protocol](../docs/engine-protocol.md) (`ferrum-engine/1`), so
FERRUM's *AI segmentation* panel can use them. One Python package,
`ferrum_bridges`, holds the shared protocol server and one backend per
engine. **Set-up and walkthroughs: [docs/ai-demo.md](../docs/ai-demo.md).**

| Engine | Mode | Command | Docker | Licence |
|---|---|---|---|---|
| [nnInteractive](https://github.com/MIC-DKFZ/nnInteractive) | interactive (point, box, scribble, lasso, undo) | `ferrum-bridge nninteractive` | [`nninteractive/`](nninteractive) | code Apache-2.0, weights **CC BY-NC-SA 4.0** (research use only) |
| [TotalSegmentator](https://github.com/wasserth/TotalSegmentator) | automatic (117 CT structures, other tasks) | `ferrum-bridge totalsegmentator --task total` | [`totalsegmentator/`](totalsegmentator) | Apache-2.0; some tasks need a licence (free for non-commercial use) |
| MONAI Label | interactive and automatic | planned | | |
| fake | both, without a model | `ferrum-bridge fake` | | MIT |

```bash
pip install ".[nninteractive]"      # or ".[totalsegmentator]"
ferrum-bridge --port 8765 nninteractive
cd totalsegmentator && docker compose up --build    # GPU, published on 127.0.0.1:8766
```

| Part | Role |
|---|---|
| `ferrum_bridges/protocol.py` | protocol server (FastAPI): sessions, prompts, jobs, errors; engine independent |
| `ferrum_bridges/backends.py` | backend interface, job runner and the model-free fake |
| `ferrum_bridges/nninteractive.py` | nnInteractive 2.6 adapter |
| `ferrum_bridges/totalsegmentator.py` | TotalSegmentator 2.18 adapter (geometry → NIfTI affine, telemetry off) |

Tests: `pip install ".[test]" && pytest`, then FERRUM's conformance suite
against a running bridge:
`FERRUM_ENGINE_URL=http://127.0.0.1:8765 cargo test -p ferrum-engines --test conformance`.

A new engine needs only a backend: subclass `Backend` and `BackendSession`
(see `backends.py`), add a sub-command in `__main__.py`, and run the
conformance suite against it. The bridges are MIT; engines and weights are
never part of this repository.
