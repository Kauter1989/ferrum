"""Bridge tests without a GPU: the protocol over the fake backend, and the
nnInteractive adapter against a stub of nnInteractive's session."""

import base64
import gzip

import numpy as np
import pytest
from fastapi.testclient import TestClient

from ferrum_nninteractive.backends import FakeBackend, diff_box, grow
from ferrum_nninteractive.nninteractive_backend import NnInteractiveSession, _research_only
from ferrum_nninteractive.protocol import create_app

DIMS = [20, 16, 12]  # nx, ny, nz


def ball() -> np.ndarray:
    z, y, x = np.mgrid[0:12, 0:16, 0:20]
    inside = (x - 10) ** 2 + (y - 8) ** 2 + (z - 6) ** 2 < 16
    return np.where(inside, 400, -1000).astype("<i2")


@pytest.fixture
def client():
    return TestClient(create_app(FakeBackend()))


def open_session(client, gz=False) -> str:
    r = client.post("/v1/sessions", json={"dims": DIMS, "dtype": "int16", "spacing": [1, 1, 2], "modality": "CT"})
    assert r.status_code == 201, r.text
    sid = r.json()["session_id"]
    body = ball().tobytes()
    headers = {"Content-Type": "application/octet-stream"}
    if gz:
        body = gzip.compress(body)
        headers["Content-Encoding"] = "gzip"
    assert client.put(f"/v1/sessions/{sid}/volume", content=body, headers=headers).status_code == 204
    return sid


def code(r) -> str:
    return r.json()["error"]["code"]


def test_info(client):
    info = client.get("/v1/info").json()
    assert info["protocol"] == "ferrum-engine/1"
    assert info["capabilities"]["interactive"] is True
    assert set(info["capabilities"]["prompts"]) == {"point", "box", "scribble", "lasso"}


def test_point_mask_undo_reset_delete(client):
    sid = open_session(client, gz=True)
    r = client.post(f"/v1/sessions/{sid}/prompts", json={"type": "point", "positive": True, "voxel": [10, 8, 6]}).json()
    assert r["revision"] == 1 and not r["empty"]
    ch = r["changed"]
    assert ch == {"min": [7, 5, 3], "max": [14, 12, 10]}
    box = ",".join(map(str, ch["min"] + ch["max"]))
    m = client.get(f"/v1/sessions/{sid}/mask", params={"box": box}, headers={"Accept-Encoding": "identity"})
    assert m.headers["x-ferrum-revision"] == "1"
    data = np.frombuffer(m.content, np.uint8).reshape(7, 7, 7)  # z, y, x of the box
    assert data[3, 3, 3] == 1 and data.sum() == (ball()[3:10, 5:12, 7:14] == 400).sum()
    full = client.get(f"/v1/sessions/{sid}/mask", headers={"Accept-Encoding": "gzip"})
    assert len(full.content) == 20 * 16 * 12  # transparently decompressed
    u = client.post(f"/v1/sessions/{sid}/undo").json()
    assert u["empty"] and u["revision"] == 2
    assert client.post(f"/v1/sessions/{sid}/undo").json()["changed"] is None
    assert client.post(f"/v1/sessions/{sid}/reset").status_code == 204
    assert client.delete(f"/v1/sessions/{sid}").status_code == 204
    assert code(client.delete(f"/v1/sessions/{sid}")) == "not_found"


def test_box_scribble_lasso_and_exclude(client):
    sid = open_session(client)
    url = f"/v1/sessions/{sid}/prompts"
    r = client.post(url, json={"type": "box", "min": [4, 2, 6], "max": [17, 14, 7]}).json()
    assert r["changed"]["min"][2] == 6 and r["changed"]["max"][2] == 7  # planar result
    mask = base64.b64encode(bytes([1, 0, 1, 1])).decode()
    r = client.post(url, json={"type": "scribble", "min": [0, 0, 0], "max": [2, 2, 1], "mask": mask}).json()
    assert r["changed"] == {"min": [0, 0, 0], "max": [2, 2, 1]}
    r = client.post(url, json={"type": "lasso", "positive": False, "min": [0, 0, 0], "max": [2, 2, 1], "mask": mask}).json()
    assert r["changed"] == {"min": [0, 0, 0], "max": [2, 2, 1]}
    r = client.post(url, json={"type": "point", "positive": False, "voxel": [10, 8, 6]}).json()
    assert r["empty"]


def test_errors(client):
    assert code(client.get("/v1/sessions/nope/mask")) == "not_found"
    assert code(client.get("/v1/nothing")) == "not_found"
    r = client.post("/v1/sessions", json={"dims": [0, 1, 1], "dtype": "int16"})
    assert (r.status_code, code(r)) == (400, "bad_request")
    assert code(client.post("/v1/sessions", json={"dims": [2, 2, 2], "dtype": "int8"})) == "bad_request"
    assert code(client.post("/v1/sessions", content=b"[1]")) == "bad_request"
    assert code(client.post("/v1/sessions", content=b"{bad")) == "bad_request"
    r = client.post("/v1/sessions", json={"dims": [2, 2, 2], "dtype": "uint16"})
    sid = r.json()["session_id"]
    r = client.post(f"/v1/sessions/{sid}/prompts", json={"type": "point", "voxel": [0, 0, 0]})
    assert (r.status_code, code(r)) == (409, "no_volume")
    assert code(client.put(f"/v1/sessions/{sid}/volume", content=b"123")) == "bad_request"
    bad_gzip = client.put(f"/v1/sessions/{sid}/volume", content=b"xx", headers={"Content-Encoding": "gzip"})
    assert code(bad_gzip) == "bad_request"
    sid = open_session(client)
    url = f"/v1/sessions/{sid}/prompts"
    for body in [
        {"type": "polygon"},
        {"type": "point", "voxel": [20, 0, 0]},
        {"type": "point", "voxel": [1, -1, 0]},
        {"type": "box", "min": [0, 0, 0], "max": [21, 1, 1]},
        {"type": "box", "min": [3, 0, 0], "max": [3, 1, 1]},
        {"type": "lasso", "min": [0, 0, 0], "max": [2, 2, 1], "mask": "AAA="},
        {"type": "lasso", "min": [0, 0, 0], "max": [2, 2, 1], "mask": "!!"},
    ]:
        r = client.post(url, json=body)
        assert (r.status_code, code(r)) == (400, "bad_request"), body
    for q in ["1,2,3", "a,b,c,d,e,f", "0,0,0,21,1,1", "-1,0,0,1,1,1"]:
        assert code(client.get(f"/v1/sessions/{sid}/mask", params={"box": q})) == "bad_request", q


def test_capabilities_limits_and_token():
    class Limited(FakeBackend):
        max_sessions = 1

        def info(self):
            return {**super().info(), "prompts": ["point", "box"], "planar_boxes_only": True, "undo": False,
                    "max_voxels": 4000}

    client = TestClient(create_app(Limited(), token="s3cret"))
    assert code(client.get("/v1/info")) == "unauthorized"
    client.headers["Authorization"] = "Bearer s3cret"
    sid = open_session(client)
    url = f"/v1/sessions/{sid}/prompts"
    r = client.post(url, json={"type": "lasso", "min": [0, 0, 0], "max": [1, 1, 1], "mask": "AQ=="})
    assert (r.status_code, code(r)) == (422, "unsupported_prompt")
    r = client.post(url, json={"type": "box", "min": [0, 0, 0], "max": [5, 5, 5]})
    assert (r.status_code, code(r)) == (422, "unsupported_prompt")
    assert code(client.post(f"/v1/sessions/{sid}/undo")) == "unsupported_prompt"
    r = client.post("/v1/sessions", json={"dims": [100, 100, 100], "dtype": "int16"})
    assert (r.status_code, code(r)) == (413, "too_large")
    second = open_session(client)
    assert code(client.get(f"/v1/sessions/{sid}/mask")) == "not_found", "capacity 1: the old session was closed"
    assert client.get(f"/v1/sessions/{second}/mask").status_code == 200


def test_helpers():
    a = np.zeros((3, 3, 3), np.uint8)
    assert diff_box(a, a) is None
    b = a.copy()
    b[1, 2, 0] = 1
    assert diff_box(a, b) == [[1, 2], [2, 3], [0, 1]]
    v = np.zeros((5, 5, 5), np.float32)
    v[0, :, :] = 1
    assert grow(v, (2, 2, 2), 0.5).sum() == 100, "growth stops at the edge without wrapping"
    assert grow(v, (2, 2, 2), 0.5, [[2, 3], [0, 5], [0, 5]]).sum() == 25
    assert grow(v, (0, 0, 0), 0.5, [[2, 3], [0, 5], [0, 5]]).sum() == 0
    assert _research_only("CC BY-NC-SA 4.0")
    assert _research_only("!!MISSING!!")
    assert not _research_only("Apache-2.0")


class StubSession:
    """The subset of nnInteractiveInferenceSession the adapter uses."""

    def __init__(self):
        self.calls = []
        self.undone = False
        self._last_paste_bbox = [[0, 1], [0, 1], [0, 1]]

    def add_point_interaction(self, c, include_interaction):
        self.calls.append(("point", c, include_interaction))
        return [[0, 1], [0, 2], [0, 3]]

    def add_bbox_interaction(self, b, include_interaction):
        self.calls.append(("box", b, include_interaction))
        return None

    def add_scribble_interaction(self, m, include_interaction, interaction_bbox):
        self.calls.append(("scribble", m.shape, interaction_bbox))
        return [[0, 1], [0, 1], [0, 1]]

    def add_lasso_interaction(self, m, include_interaction, interaction_bbox):
        self.calls.append(("lasso", m.shape, interaction_bbox))
        return [[0, 1], [0, 1], [0, 1]]

    def undo(self):
        self.undone = not self.undone
        return self.undone

    def reset_interactions(self):
        self.calls.append(("reset",))


def test_nninteractive_adapter():
    stub = StubSession()
    target = np.zeros((2, 3, 4), np.uint8)
    s = NnInteractiveSession(stub, target)
    assert s.point((1, 2, 3), True) == [[0, 1], [0, 2], [0, 3]]
    assert s.box([[0, 1], [0, 3], [0, 4]], False) is None
    m = np.ones((1, 2, 2), np.uint8)
    s.scribble(m, [[0, 1], [0, 2], [0, 2]], True)
    s.lasso(m, [[0, 1], [0, 2], [0, 2]], False)
    assert [c[0] for c in stub.calls] == ["point", "box", "scribble", "lasso"]
    assert stub.calls[2][2] == [[0, 1], [0, 2], [0, 2]], "masks are passed cropped to their box"
    assert s.undo() == (True, [[0, 1], [0, 1], [0, 1]])
    assert s.undo() == (False, None)
    stub._last_paste_bbox = None
    assert s.undo() == (True, [[0, 2], [0, 3], [0, 4]])
    assert s.is_empty()
    target[1, 2, 3] = 1
    assert not s.is_empty()
    assert s.mask([[1, 2], [2, 3], [3, 4]]).tolist() == [[[1]]]
    s.reset()
    s.close()
    assert stub.calls[-2:] == [("reset",), ("reset",)]
