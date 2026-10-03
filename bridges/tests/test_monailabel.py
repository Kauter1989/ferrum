"""The MONAI Label adapter against a fake MONAI Label server that mimics its
REST API (``/info/``, ``/session/``, ``/infer/{model}``)."""

import gzip
import json
import time

import nibabel as nib
import numpy as np
import pytest
from fastapi import FastAPI, File, Form, UploadFile
from fastapi.responses import Response
from fastapi.testclient import TestClient

from ferrum_bridges.monailabel import MonaiLabelBackend, label_values, read_label_image
from ferrum_bridges.protocol import create_app

DIMS = [20, 16, 12]  # nx, ny, nz


def volume() -> np.ndarray:
    z, y, x = np.mgrid[0:12, 0:16, 0:20]
    inside = (x - 6) ** 2 + (y - 8) ** 2 + (z - 6) ** 2 < 16
    second = (x - 15) ** 2 + (y - 8) ** 2 + (z - 6) ** 2 < 6
    return np.where(inside | second, 400, -1000).astype("<i2")


MODELS = {
    "deepedit": {"type": "deepedit", "labels": {"spleen": 1, "liver": 2, "background": 0}, "dimension": 3},
    "deepgrow_3d": {"type": "deepgrow", "labels": [], "dimension": 3},
    "sam2": {"type": "annotation", "labels": [], "dimension": 2},
    "segmentation": {"type": "segmentation", "labels": {"spleen": 1, "liver": 6}, "dimension": 3},
}


def fake_monai_label(models=None):
    """Answers clicks with balls of bright voxels around the foreground
    points; the segmentation model labels all bright voxels ``liver``."""
    app = FastAPI()
    app.state.sessions, app.state.requests = {}, []

    @app.get("/info/")
    def info():
        return {"name": "radiology", "version": "0.8.5", "labels": [], "models": models or MODELS}

    @app.put("/session/")
    async def create_session(files: list[UploadFile] = File(...)):
        img = nib.Nifti1Image.from_bytes(gzip.decompress(await files[0].read()))
        sid = f"s{len(app.state.sessions)}"
        app.state.sessions[sid] = img
        return {"session_id": sid, "session_info": {}}

    @app.delete("/session/{sid}")
    def remove_session(sid: str):
        app.state.sessions.pop(sid)
        return {}

    @app.post("/infer/{model}")
    def infer(model: str, session_id: str = "", output: str = "", params: str = Form("{}")):
        p = json.loads(params)
        app.state.requests.append((model, session_id, output, p))
        img = app.state.sessions[session_id]
        data = np.asanyarray(img.dataobj)  # x, y, z
        out = np.zeros(data.shape, np.uint8)
        x, y, z = np.mgrid[0 : data.shape[0], 0 : data.shape[1], 0 : data.shape[2]]
        if model == "segmentation":
            out[data > 0] = 6
        else:
            value = MODELS["deepedit"]["labels"][p["label"]] if model == "deepedit" else 1
            for px, py, pz in p.get("foreground", []):
                out[((x - px) ** 2 + (y - py) ** 2 + (z - pz) ** 2 < 9) & (data > 0)] = value
            for px, py, pz in p.get("background", []):
                out[(x - px) ** 2 + (y - py) ** 2 + (z - pz) ** 2 < 4] = 0
            if "roi" in p:
                x0, y0, x1, y1, z0, z1 = p["roi"]
                out[x0:x1, y0:y1, z0:z1][data[x0:x1, y0:y1, z0:z1] > 0] = 1
        result = nib.Nifti1Image(out, img.affine).to_bytes()
        return Response(gzip.compress(result), media_type="application/gzip")

    return app


def bridge(**kwargs):
    server = fake_monai_label(kwargs.pop("models", None))
    backend = MonaiLabelBackend(client=TestClient(server), **kwargs)
    return server, backend, TestClient(create_app(backend))


def open_session(client) -> str:
    body = {"dims": DIMS, "dtype": "int16", "spacing": [1, 1, 2], "origin": [-5.0, 2.0, 30.0]}
    sid = client.post("/v1/sessions", json=body).json()["session_id"]
    assert client.put(f"/v1/sessions/{sid}/volume", content=volume().tobytes()).status_code == 204
    return sid


def full_mask(client, sid) -> np.ndarray:
    r = client.get(f"/v1/sessions/{sid}/mask", params={"box": "0,0,0,20,16,12"}, headers={"Accept-Encoding": "identity"})
    return np.frombuffer(r.content, np.uint8).reshape(12, 16, 20)


def test_labels_and_results():
    assert label_values({"background": 0, "spleen": 1, "liver": 6}) == {"spleen": 1, "liver": 6}
    assert label_values(["spleen", "liver"]) == {"spleen": 1, "liver": 2}
    img = nib.Nifti1Image(np.ones((4, 3, 2, 1), np.float32), np.eye(4))
    labels = read_label_image(img.to_bytes(), (2, 3, 4))  # plain NIfTI, trailing channel axis
    assert labels.dtype == np.uint16 and labels.shape == (2, 3, 4) and labels.all()
    with pytest.raises(RuntimeError, match="result"):
        read_label_image(gzip.compress(img.to_bytes()), (4, 3, 2))


def test_deepedit_clicks_undo_reset_and_cleanup():
    server, backend, client = bridge()
    info = client.get("/v1/info").json()
    caps = info["capabilities"]
    assert (caps["interactive"], caps["automatic"], caps["prompts"], caps["undo"]) == (True, True, ["point"], True)
    assert backend.model == "deepedit" and backend.label == "spleen"
    assert [x["name"] for x in info["labels"]] == ["spleen", "liver"] and info["research_only"]

    sid = open_session(client)
    uploaded = next(iter(server.state.sessions.values()))
    assert uploaded.shape == (20, 16, 12), "uploaded as x, y, z"
    assert np.allclose(uploaded.affine[:3, 3], [5.0, -2.0, 30.0]), "patient geometry in RAS"

    r = client.post(f"/v1/sessions/{sid}/prompts", json={"type": "point", "voxel": [6, 8, 6]}).json()
    assert r["revision"] == 1 and not r["empty"]
    model, msid, output, params = server.state.requests[-1]
    assert (model, output) == ("deepedit", "image") and msid in server.state.sessions
    assert params == {"foreground": [[6, 8, 6]], "background": [], "spleen": [[6, 8, 6]], "label": "spleen"}
    first = full_mask(client, sid)
    assert first[6, 8, 6] and not first[6, 8, 15], "only the clicked object"

    r = client.post(f"/v1/sessions/{sid}/prompts", json={"type": "point", "voxel": [6, 8, 7], "positive": False}).json()
    assert server.state.requests[-1][3]["background"] == [[6, 8, 7]], "the whole history is sent"
    assert not full_mask(client, sid)[7, 8, 6]
    r = client.post(f"/v1/sessions/{sid}/undo").json()
    assert r["changed"] is not None and np.array_equal(full_mask(client, sid), first)
    assert client.post(f"/v1/sessions/{sid}/undo").json()["empty"]
    assert client.post(f"/v1/sessions/{sid}/undo").json()["changed"] is None, "nothing left to undo"
    r = client.post(f"/v1/sessions/{sid}/prompts", json={"type": "box", "min": [0, 0, 0], "max": [5, 5, 5]})
    assert r.status_code == 422

    client.post(f"/v1/sessions/{sid}/prompts", json={"type": "point", "voxel": [15, 8, 6]})
    assert client.post(f"/v1/sessions/{sid}/reset").status_code == 204
    assert not full_mask(client, sid).any()
    assert client.delete(f"/v1/sessions/{sid}").status_code == 204
    assert not server.state.sessions, "the MONAI Label session is removed"


def test_automatic_segmentation_with_label_filter():
    server, backend, client = bridge(model="")
    caps = client.get("/v1/info").json()["capabilities"]
    assert (caps["interactive"], caps["automatic"], caps["prompts"]) == (False, True, [])
    sid = open_session(client)
    for labels, expected in ((None, 6), (["liver"], 6), (["spleen"], 0)):
        job = client.post(f"/v1/sessions/{sid}/segment", json={"labels": labels}).json()["job_id"]
        for _ in range(500):
            status = client.get(f"/v1/jobs/{job}").json()
            if status["state"] not in ("queued", "running"):
                break
            time.sleep(0.01)
        assert status["state"] == "done", status
        m = np.frombuffer(client.get(f"/v1/sessions/{sid}/labelmap").content, "<u2").reshape(12, 16, 20)
        assert np.array_equal(m, np.where(volume() > 0, expected, 0))
    assert server.state.requests[-1][:3] == ("segmentation", next(iter(server.state.sessions)), "image")
    r = client.post(f"/v1/sessions/{sid}/segment", json={"labels": ["brain"]})
    assert r.json()["error"]["code"] == "bad_request"


def test_sam2_boxes_and_model_choice():
    server, backend, client = bridge(model="sam2", auto_model="")
    caps = client.get("/v1/info").json()["capabilities"]
    assert (caps["prompts"], caps["automatic"]) == (["point", "box"], False)
    assert caps["deterministic"] is False, "inference runs on the MONAI Label server"
    sid = open_session(client)
    r = client.post(f"/v1/sessions/{sid}/prompts", json={"type": "box", "min": [12, 4, 3], "max": [19, 13, 10]}).json()
    assert server.state.requests[-1][3]["roi"] == [12, 4, 19, 13, 3, 10], "roi = x0, y0, x1, y1, z0, z1"
    assert r["changed"] == {"min": [13, 6, 4], "max": [18, 11, 9]}
    r = client.post(f"/v1/sessions/{sid}/prompts", json={"type": "box", "min": [0, 0, 0], "max": [5, 5, 5], "positive": False})
    assert r.status_code == 422

    with pytest.raises(RuntimeError, match="not on the server"):
        bridge(model="nope")
    with pytest.raises(RuntimeError, match="no interactive or segmentation model"):
        bridge(models={"cls": {"type": "classification"}})
    _, deepgrow, _ = bridge(models={"deepgrow_3d": MODELS["deepgrow_3d"]})
    assert (deepgrow.model, deepgrow.auto_model, deepgrow.label) == ("deepgrow_3d", None, None)
