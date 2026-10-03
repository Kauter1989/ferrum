"""Automatic segmentation through the shared protocol server, the job runner,
and the TotalSegmentator adapter (with TotalSegmentator itself stubbed)."""

import time

import nibabel as nib
import numpy as np
import pytest
from fastapi.testclient import TestClient

from ferrum_bridges.backends import BackendSession, FakeBackend, JobRunner
from ferrum_bridges.protocol import create_app
from ferrum_bridges.totalsegmentator import TotalSegmentatorBackend, TotalSegmentatorSession, affine_ras

DIMS = [20, 16, 12]  # nx, ny, nz


def volume() -> np.ndarray:
    z, y, x = np.mgrid[0:12, 0:16, 0:20]
    inside = (x - 10) ** 2 + (y - 8) ** 2 + (z - 6) ** 2 < 16
    return np.where(inside, 400, -1000).astype("<i2")


def open_session(client, **extra) -> str:
    body = {"dims": DIMS, "dtype": "int16", "spacing": [1, 1, 2], **extra}
    sid = client.post("/v1/sessions", json=body).json()["session_id"]
    assert client.put(f"/v1/sessions/{sid}/volume", content=volume().tobytes()).status_code == 204
    return sid


def wait(client, job: str) -> dict:
    for _ in range(500):
        s = client.get(f"/v1/jobs/{job}").json()
        if s["state"] not in ("queued", "running"):
            return s
        time.sleep(0.01)
    raise AssertionError("job did not finish")


def code(r) -> str:
    return r.json()["error"]["code"]


def test_jobs_through_the_protocol():
    client = TestClient(create_app(FakeBackend()))
    info = client.get("/v1/info").json()
    assert info["capabilities"]["automatic"] and [l["name"] for l in info["labels"]] == ["intermediate", "bright"]
    sid = open_session(client)
    assert code(client.get(f"/v1/sessions/{sid}/labelmap")) == "no_result"
    r = client.post(f"/v1/sessions/{sid}/segment", json={"labels": None})
    assert r.status_code == 202
    status = wait(client, r.json()["job_id"])
    assert status["state"] == "done" and status["progress"] == 1.0
    m = client.get(f"/v1/sessions/{sid}/labelmap", headers={"Accept-Encoding": "identity"})
    labels = np.frombuffer(m.content, "<u2").reshape(12, 16, 20)
    assert (labels == 2).sum() == (volume() == 400).sum()
    assert set(np.unique(labels)) <= {0, 2}
    # a subset, an unknown label, bad input, unknown jobs
    job = client.post(f"/v1/sessions/{sid}/segment", json={"labels": ["intermediate"]}).json()["job_id"]
    wait(client, job)
    assert not np.frombuffer(client.get(f"/v1/sessions/{sid}/labelmap").content, "<u2").any()
    assert code(client.post(f"/v1/sessions/{sid}/segment", json={"labels": ["liver"]})) == "bad_request"
    assert code(client.post(f"/v1/sessions/{sid}/segment", json={"labels": "liver"})) == "bad_request"
    assert code(client.get("/v1/jobs/nope")) == "not_found"
    assert client.delete(f"/v1/jobs/{job}").status_code == 204
    assert client.delete(f"/v1/sessions/{sid}").status_code == 204
    assert code(client.get(f"/v1/jobs/{job}")) == "not_found", "jobs go with their session"


def test_capabilities_decide_the_endpoints():
    class InteractiveOnly(FakeBackend):
        def info(self):
            return {**super().info(), "automatic": False}

    class AutomaticOnly(FakeBackend):
        def info(self):
            return {**super().info(), "interactive": False, "prompts": [], "undo": False}

        def open(self, volume, spacing, modality, origin=None, direction=None):
            s = super().open(volume, spacing, modality)

            class Auto(BackendSession):
                start_job, job_status, cancel_job, label_map = s.start_job, s.job_status, s.cancel_job, s.label_map

            return Auto()

    c = TestClient(create_app(InteractiveOnly()))
    sid = open_session(c)
    r = c.post(f"/v1/sessions/{sid}/segment", json={})
    assert (r.status_code, code(r)) == (404, "not_found")
    assert code(c.get(f"/v1/sessions/{sid}/labelmap")) == "not_found"

    c = TestClient(create_app(AutomaticOnly()))
    info = c.get("/v1/info").json()["capabilities"]
    assert (info["interactive"], info["automatic"]) == (False, True)
    sid = open_session(c)
    r = c.post(f"/v1/sessions/{sid}/prompts", json={"type": "point", "voxel": [1, 1, 1]})
    assert (r.status_code, code(r)) == (422, "unsupported_prompt")
    assert c.post(f"/v1/sessions/{sid}/reset").status_code == 204
    job = c.post(f"/v1/sessions/{sid}/segment", json={}).json()["job_id"]
    assert wait(c, job)["state"] == "done"


def test_job_runner_progress_cancel_failure_and_busy():
    r = JobRunner()
    gate = {"go": False}

    def slow(progress):
        progress(0.4, "half way")
        while not gate["go"]:
            time.sleep(0.005)
        return np.ones(2, np.uint16)

    job = r.start(slow, "starting")
    for _ in range(200):
        if r.status(job)["progress"] == 0.4:
            break
        time.sleep(0.005)
    assert r.status(job) == {"state": "running", "progress": 0.4, "message": "half way"}
    with pytest.raises(RuntimeError):
        r.start(slow, "second")
    r.cancel(job)
    gate["go"] = True
    assert r.wait(job)["state"] == "cancelled"
    assert r.result is None, "a cancelled job's result is discarded"

    def boom(progress):
        raise MemoryError("CUDA out of memory")

    failed = r.start(boom, "x")
    s = r.wait(failed)
    assert s["state"] == "failed" and "out of memory" in s["message"]
    with pytest.raises(KeyError):
        r.status("nope")


def test_affine_follows_the_patient_geometry():
    a = affine_ras((0.5, 0.75, 2.0), (-5.0, 2.0, 30.0), [[1, 0, 0], [0, 1, 0], [0, 0, 1]])
    assert np.allclose(a[:3, :3], np.diag([-0.5, -0.75, 2.0]))
    assert np.allclose(a[:3, 3], [5.0, -2.0, 30.0])
    # coronal stack: i → left, j → inferior, k → posterior (LPS)
    d = [[1, 0, 0], [0, 0, -1], [0, 1, 0]]
    a = affine_ras((1.0, 1.0, 3.0), (10.0, 20.0, 30.0), d)
    ijk = np.array([2.0, 1.0, 4.0, 1.0])
    lps = np.array([10.0, 20.0, 30.0]) + np.array(d, float).T @ (ijk[:3] * [1.0, 1.0, 3.0])
    assert np.allclose(a @ ijk, [-lps[0], -lps[1], lps[2], 1.0])


class StubTotalSegmentator(TotalSegmentatorBackend):
    """The real adapter with TotalSegmentator's model replaced by a threshold."""

    def __init__(self, task="total"):  # skip the TotalSegmentator imports
        self.task, self.device, self.fast, self.license_number = task, "cpu", True, None
        self.classes = {1: "spleen", 5: "liver"}
        self.licensed = task != "total"
        self.modality = "CT"
        self.calls = []

    def run(self, image, roi_subset):
        self.calls.append((image.affine.copy(), image.shape, roi_subset))
        data = np.asanyarray(image.dataobj)
        return nib.Nifti1Image(np.where(data > 0, 5, 0).astype(np.uint8), image.affine)


def test_totalsegmentator_adapter_round_trips_the_grid():
    backend = StubTotalSegmentator()
    info = backend.info()
    assert (info["interactive"], info["automatic"], info["research_only"]) == (False, True, False)
    assert info["labels"] == [{"value": 1, "name": "spleen"}, {"value": 5, "name": "liver"}]
    assert info["modalities"] == ["CT"] and "Apache-2.0" in info["license"]
    assert StubTotalSegmentator("appendicular_bones").info()["research_only"]

    client = TestClient(create_app(backend))
    sid = open_session(client, origin=[-5.0, 2.0, 30.0])
    job = client.post(f"/v1/sessions/{sid}/segment", json={"labels": ["liver"]}).json()["job_id"]
    assert wait(client, job)["state"] == "done", client.get(f"/v1/jobs/{job}").json()
    affine, shape, roi = backend.calls[-1]
    assert shape == (20, 16, 12), "NIfTI data is x, y, z"
    assert np.allclose(affine[:3, 3], [5.0, -2.0, 30.0]) and roi == ["liver"]
    labels = np.frombuffer(client.get(f"/v1/sessions/{sid}/labelmap").content, "<u2").reshape(12, 16, 20)
    assert np.array_equal(labels == 5, volume() > 0), "back on the [z, y, x] grid"
    # a subset that excludes the result yields an empty map; unknown names are rejected
    job = client.post(f"/v1/sessions/{sid}/segment", json={"labels": ["spleen"]}).json()["job_id"]
    wait(client, job)
    assert not np.frombuffer(client.get(f"/v1/sessions/{sid}/labelmap").content, "<u2").any()
    assert code(client.post(f"/v1/sessions/{sid}/segment", json={"labels": ["brain"]})) == "bad_request"


def test_totalsegmentator_shape_mismatch_fails_the_job():
    backend = StubTotalSegmentator()
    backend.run = lambda image, roi: nib.Nifti1Image(np.zeros((2, 2, 2), np.uint8), image.affine)
    s = TotalSegmentatorSession(backend, volume().astype(np.float32), (1, 1, 1), (0, 0, 0), ((1, 0, 0), (0, 1, 0), (0, 0, 1)))
    job = s.start_job(None)
    status = s.runner.wait(job)
    assert status["state"] == "failed" and "differs" in status["message"]
    with pytest.raises(ValueError):
        s.label_map()


FAKE_TOTALSEGMENTATOR = """
import time
import numpy as np
import nibabel as nib


def totalsegmentator(image, output, **options):
    if options["task"] == "slow":
        time.sleep(60)
    data = np.asanyarray(image.dataobj)
    return nib.Nifti1Image(np.where(data > 0, 5, 0).astype(np.uint8), image.affine)
"""


@pytest.fixture
def fake_totalsegmentator(tmp_path, monkeypatch):
    """A ``totalsegmentator`` package the spawned job process can import."""
    pkg = tmp_path / "totalsegmentator"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("")
    (pkg / "python_api.py").write_text(FAKE_TOTALSEGMENTATOR)
    (pkg / "config.py").write_text("def set_config_key(key, value):\n    pass\n")
    monkeypatch.syspath_prepend(str(tmp_path))


def test_jobs_run_in_a_child_process(fake_totalsegmentator):
    backend = StubTotalSegmentator()
    backend.isolate = True
    s = TotalSegmentatorSession(backend, volume().astype(np.float32), (1, 1, 2), (0, 0, 0), ((1, 0, 0), (0, 1, 0), (0, 0, 1)))
    status = s.runner.wait(s.start_job(["liver"]), timeout=60)
    assert status["state"] == "done", status
    assert np.array_equal(s.label_map() == 5, volume() > 0)
    assert backend.calls == [], "the model ran in the child, not in the bridge"


def test_cancelling_a_job_ends_its_process(fake_totalsegmentator):
    import threading

    backend = StubTotalSegmentator("slow")
    backend.isolate = True
    s = TotalSegmentatorSession(backend, volume().astype(np.float32), (1, 1, 2), (0, 0, 0), ((1, 0, 0), (0, 1, 0), (0, 0, 1)))
    job = s.start_job(None)
    for _ in range(200):
        if "running (" in s.runner.status(job)["message"]:
            break
        time.sleep(0.05)
    s.cancel_job(job)
    assert s.runner.status(job)["state"] == "cancelled"
    for _ in range(200):
        if not any(t.name == f"job-{job}" for t in threading.enumerate()):
            break
        time.sleep(0.05)
    assert not any(t.name == f"job-{job}" for t in threading.enumerate()), "the job stopped with its process"
    assert s.runner.result is None


def test_a_failing_child_fails_the_job(fake_totalsegmentator, tmp_path):
    (tmp_path / "totalsegmentator" / "python_api.py").write_text("def totalsegmentator(*a, **k):\n    raise SystemExit(3)\n")
    backend = StubTotalSegmentator()
    backend.isolate = True
    s = TotalSegmentatorSession(backend, volume().astype(np.float32), (1, 1, 2), (0, 0, 0), ((1, 0, 0), (0, 1, 0), (0, 0, 1)))
    status = s.runner.wait(s.start_job(None), timeout=60)
    assert status["state"] == "failed" and "exit code 3" in status["message"]
