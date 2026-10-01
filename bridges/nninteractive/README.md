# nnInteractive bridge for FERRUM

Serves [nnInteractive](https://github.com/MIC-DKFZ/nnInteractive) over the
[FERRUM Engine Protocol](../../docs/engine-protocol.md) (`ferrum-engine/1`),
so FERRUM's *AI segmentation* tools can use it.

**Deployment and walkthrough: [docs/ai-demo.md](../../docs/ai-demo.md).**

```bash
docker compose up --build                          # NVIDIA GPU, ~10 GB VRAM recommended
# or without Docker:
pip install ".[nninteractive]" && ferrum-nninteractive --port 8765
# check the set-up without a GPU or model:
pip install . && ferrum-nninteractive --backend fake
```

| Part | Role |
|---|---|
| `ferrum_nninteractive/protocol.py` | the protocol server (FastAPI), independent of the engine |
| `ferrum_nninteractive/nninteractive_backend.py` | nnInteractive 2.6 adapter (one GPU session at a time) |
| `ferrum_nninteractive/backends.py` | backend interface and a model-free fake for tests |

Tests: `pip install ".[test]" && pytest`, plus FERRUM's conformance suite
against a running bridge:
`FERRUM_ENGINE_URL=http://127.0.0.1:8765 cargo test -p ferrum-engines --test conformance`.

Licences: this bridge is MIT. nnInteractive's code is Apache-2.0; its model
weights are **CC BY-NC-SA 4.0 (non-commercial)**. FERRUM shows a *Research use
only* badge for them. Weights are downloaded at run time and are never part of
this repository or the image.
