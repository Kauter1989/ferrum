"""Stage 23: unloading engines and giving GPU memory back.

`POST /v1/unload` over the protocol, the worker process that makes a full
unload possible, and the paths that close a session (and so free the
nnInteractive image): `DELETE` and expiry."""

import time

import numpy as np
from fastapi.testclient import TestClient
from ferrum_bridges.backends import FakeBackend
from ferrum_bridges.protocol import Engine, create_app
from ferrum_bridges.worker import WorkerBackend

DIMS = [20, 16, 12]  # nx, ny, nz


def ball() -> np.ndarray:
    z, y, x = np.mgrid[0:12, 0:16, 0:20]
    return np.where((x - 10) ** 2 + (y - 8) ** 2 + (z - 6) ** 2 < 16, 400, -1000).astype("<i2")


def open_session(client) -> str:
    r = client.post("/v1/sessions", json={"dims": DIMS, "dtype": "int16", "spacing": [1, 1, 2], "modality": "CT"})
    sid = r.json()["session_id"]
    headers = {"Content-Type": "application/octet-stream"}
    assert client.put(f"/v1/sessions/{sid}/volume", content=ball().tobytes(), headers=headers).status_code == 204
    return sid


class NoUnload(FakeBackend):
    def can_unload(self):
        return False


# covers 23.4-b
def test_unload_is_refused_while_a_session_is_open():
    backend = FakeBackend()
    client = TestClient(create_app(backend))
    assert client.get("/v1/info").json()["capabilities"]["unload"] is True
    sid = open_session(client)
    r = client.post("/v1/unload")
    assert r.status_code == 503 and r.json()["error"]["code"] == "busy"
    assert r.headers["retry-after"] == "5"
    assert getattr(backend, "unloads", 0) == 0, "nothing is stopped"
    assert client.delete(f"/v1/sessions/{sid}").status_code == 204
    assert client.post("/v1/unload").status_code == 204
    assert backend.unloads == 1
    assert open_session(client), "the next session loads again"


# covers 23.4-b
def test_an_engine_without_unload_answers_not_found():
    client = TestClient(create_app(NoUnload()))
    assert client.get("/v1/info").json()["capabilities"]["unload"] is False
    r = client.post("/v1/unload")
    assert r.status_code == 404 and r.json()["error"]["code"] == "not_found"


# covers 23.4-d
def test_peaks_and_resident_memory_reach_the_manifest():
    class Reporting(FakeBackend):
        def info(self):
            return {**super().info(), "expected_peak_mib": 5400, "resident_mib": 2100}

    info = TestClient(create_app(Reporting())).get("/v1/info").json()
    assert info["limits"]["expected_peak_mib"] == 5400
    assert info["resident_mib"] == 2100
    plain = TestClient(create_app(FakeBackend())).get("/v1/info").json()
    assert "expected_peak_mib" not in plain["limits"] and "resident_mib" not in plain


# covers 23.4-b
def test_the_worker_process_ends_on_unload_and_starts_again():
    backend = WorkerBackend(FakeBackend)
    try:
        first = backend.pid()
        assert first is not None
        assert backend.info()["unload"] is True
        session = backend.open(ball().astype(np.float32), (1, 1, 2), "CT")
        changed = session.point((6, 8, 10), True)
        assert changed is not None and session.mask(changed).any()
        try:
            backend.unload()
            raise AssertionError("unload must be refused while a session is open")
        except RuntimeError:
            pass
        session.close()
        backend.unload()
        assert backend.pid() is None and backend.resident_mib() == 0, "no process, no GPU memory"
        again = backend.open(ball().astype(np.float32), (1, 1, 2), "CT")
        assert backend.pid() not in (None, first), "a new worker"
        assert again.point((6, 8, 10), True) is not None
        again.close()
    finally:
        backend.unload()


# covers 23.4-b
def test_the_worker_serves_the_protocol():
    backend = WorkerBackend(FakeBackend)
    try:
        client = TestClient(create_app(backend))
        sid = open_session(client)
        r = client.post(f"/v1/sessions/{sid}/prompts", json={"type": "point", "voxel": [10, 8, 6], "positive": True})
        assert r.status_code == 200 and r.json()["empty"] is False
        assert client.post("/v1/unload").status_code == 503
        assert client.delete(f"/v1/sessions/{sid}").status_code == 204
        assert client.post("/v1/unload").status_code == 204
        assert backend.pid() is None
    finally:
        backend.unload()


class Spy(FakeBackend):
    """Counts closed sessions."""

    closed = 0

    def open(self, volume, spacing, modality, origin=None, direction=None):
        session = super().open(volume, spacing, modality, origin, direction)
        spy = self

        class Closing(type(session)):
            def close(self):
                spy.closed += 1

        session.__class__ = Closing
        return session


# covers 23.4-a
def test_sessions_close_on_delete_and_on_expiry():
    backend = Spy()
    client = TestClient(create_app(backend, session_ttl_s=3600))
    sid = open_session(client)
    client.delete(f"/v1/sessions/{sid}")
    assert backend.closed == 1, "DELETE closes the session (nnInteractive frees its image there)"

    engine = Engine(Spy(), session_ttl_s=60)
    sid = engine.create({"dims": DIMS, "dtype": "int16", "spacing": [1, 1, 2]})["session_id"]
    engine.upload(sid, ball().tobytes())
    engine.sessions[sid].last_used = time.monotonic() - 61  # idle past its lifetime
    engine.unload()  # expiry runs first: the expired session no longer blocks
    assert engine.backend.closed == 1, "an expired session is closed too"
    assert engine.sessions == {}
