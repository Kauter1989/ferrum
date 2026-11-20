"""Orthanc integration: STOW a de-identified SEG/SR, retrieve it, validate with
highdicom (Stage 32, PR 4, #76).

A synthetic CT phantom (generated at run time, no patient data) is stored in a
real Orthanc PACS over DICOMweb. FERRUM opens it, segments a blob and exports a
de-identified SEG/SR (Stage 31). The export is STOW'd back to Orthanc; the
objects that come back over WADO-RS are validated with highdicom (MIT) — the
SEG structure (segments, frames, geometry, referenced images) and the SR
measurement report. The STOW response is also checked for the success-sequence
tag (0008,1190), confirming the PR 3 ``RETRIEVE_URL`` dictionary quirk against a
real PACS.

Orthanc's DICOMweb plugin is **AGPL**: Orthanc is a separate Docker service
reached over HTTP, never linked into FERRUM (ADR 0021 §6). In CI the ``orthanc``
job provides it; locally run ``docker run -p 8042:8042 orthancteam/orthanc``.

Skips when highdicom/pydicom/requests are absent, no ``ferrum-cli`` is built, or
no Orthanc is reachable (``FERRUM_ORTHANC_URL`` or the default :8042). Build the
CLI with ``cargo build -p ferrum-cli`` or point ``FERRUM_CLI`` at one.

# covers 32.5-a
# covers 32.5-b
# covers 32.2-a
"""

import io
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

import pytest

pydicom = pytest.importorskip("pydicom")
hd = pytest.importorskip("highdicom")
requests = pytest.importorskip("requests")
np = pytest.importorskip("numpy")

from pydicom.dataset import Dataset, FileDataset  # noqa: E402
from pydicom.sr.codedict import codes  # noqa: E402
from pydicom.uid import ExplicitVRLittleEndian, generate_uid  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
SECRETS = [b"DOE^JANE", b"MRN-4711", b"19700101", b"ACC-0815", b"Secret Hospital", b"DR^SECRET"]


def orthanc() -> tuple[str, tuple[str, str]]:
    """The DICOMweb root and basic auth, or skip when no PACS is reachable."""
    auth = (os.environ.get("FERRUM_ORTHANC_USER", "orthanc"), os.environ.get("FERRUM_ORTHANC_PASS", "orthanc"))
    explicit = os.environ.get("FERRUM_ORTHANC_URL", "").strip()
    root = explicit.rstrip("/") if explicit else "http://127.0.0.1:8042/dicom-web"
    try:
        resp = requests.get(f"{root}/system", auth=auth, timeout=3)
        resp.raise_for_status()
    except requests.RequestException:
        if explicit:
            raise  # a configured CI PACS that is down is a real failure
        pytest.skip(f"no Orthanc at {root} (start Docker Orthanc or set FERRUM_ORTHANC_URL)")
    return root, auth


def ferrum_cli() -> str:
    env = os.environ.get("FERRUM_CLI")
    if env:
        return env
    for cand in [
        ROOT / "target/release/ferrum-cli",
        ROOT / "target/debug/ferrum-cli",
        ROOT / "target/release/ferrum-cli.exe",
        ROOT / "target/debug/ferrum-cli.exe",
    ]:
        if cand.exists():
            return str(cand)
    found = shutil.which("ferrum-cli")
    if found:
        return found
    pytest.skip("ferrum-cli is not built (cargo build -p ferrum-cli)")


def run(cli: str, *args: str) -> str:
    # The CLI path and arguments are built by this test, not untrusted input.
    proc = subprocess.run([cli, *args], capture_output=True, text=True)  # noqa: S603
    if proc.returncode != 0:
        pytest.fail(f"ferrum-cli {' '.join(args)} failed:\n{proc.stdout[-800:]}\n{proc.stderr[-400:]}")
    return proc.stdout


def write_series(directory: Path) -> tuple[Path, str, str]:
    """A 16x16x8 CT with a 400 HU blob in -1000 HU background, identifiers set."""
    directory.mkdir(parents=True, exist_ok=True)
    nx, ny, nz = 16, 16, 8
    study_uid = generate_uid()
    series_uid = generate_uid()
    for k in range(nz):
        meta = Dataset()
        meta.MediaStorageSOPClassUID = "1.2.840.10008.5.1.4.1.1.2"
        meta.MediaStorageSOPInstanceUID = generate_uid()
        meta.TransferSyntaxUID = ExplicitVRLittleEndian
        path = directory / f"slice{k:02d}.dcm"
        ds = FileDataset(str(path), {}, file_meta=meta, preamble=b"\0" * 128)
        ds.PatientName = "DOE^JANE"
        ds.PatientID = "MRN-4711"
        ds.PatientBirthDate = "19700101"
        ds.PatientSex = "F"
        ds.StudyDate = "20240428"
        ds.StudyTime = "102417"
        ds.AccessionNumber = "ACC-0815"
        ds.InstitutionName = "Secret Hospital"
        ds.ReferringPhysicianName = "DR^SECRET"
        ds.StudyDescription = "Chest CT"
        ds.StudyInstanceUID = study_uid
        ds.SeriesInstanceUID = series_uid
        ds.SOPInstanceUID = meta.MediaStorageSOPInstanceUID
        ds.SOPClassUID = meta.MediaStorageSOPClassUID
        ds.Modality = "CT"
        ds.InstanceNumber = k + 1
        ds.ImagePositionPatient = [0.0, 0.0, float(k) * 2.5]
        ds.ImageOrientationPatient = [1, 0, 0, 0, 1, 0]
        ds.PixelSpacing = [1.0, 1.0]
        ds.SliceThickness = 2.5
        ds.Rows, ds.Columns = ny, nx
        ds.SamplesPerPixel = 1
        ds.PhotometricInterpretation = "MONOCHROME2"
        ds.BitsAllocated = 16
        ds.BitsStored = 16
        ds.HighBit = 15
        ds.PixelRepresentation = 1
        ds.RescaleSlope = 1
        ds.RescaleIntercept = 0
        arr = np.full((ny, nx), -1000, dtype=np.int16)
        arr[6:10, 6:10] = 400  # the blob
        ds.PixelData = arr.tobytes()
        ds.save_as(str(path), enforce_file_format=True)
    return directory, study_uid, series_uid


def stow(root: str, auth: tuple[str, str], paths: list[Path]) -> dict:
    """STOW-RS store of Part 10 files; returns the parsed response dataset."""
    boundary = "ferrum-orthanc-boundary"
    body = b""
    for path in paths:
        body += f"--{boundary}\r\nContent-Type: application/dicom\r\n\r\n".encode()
        body += path.read_bytes() + b"\r\n"
    body += f"--{boundary}--\r\n".encode()
    headers = {"Content-Type": f'multipart/related; type="application/dicom"; boundary={boundary}'}
    resp = requests.post(f"{root}/studies", data=body, headers=headers, auth=auth, timeout=120)
    resp.raise_for_status()
    return resp.json()


def success_uids(response: dict) -> set[str]:
    """SOP Instance UIDs in the STOW success sequence (0008,1190).

    The 0.10 DICOM dictionary names this tag RETRIEVE_URL; its value is the
    Referenced SOP Sequence of stored instances (PS3.18 Annex I). Reading it
    here confirms a real Orthanc reports success under (0008,1190).
    """
    seq = response.get("00081190", {}).get("Value", [])
    return {item["00081155"]["Value"][0] for item in seq}


def wado_instance(root: str, auth: tuple[str, str], study: str, series: str, sop: str) -> bytes:
    """WADO-RS instance retrieval: the Part 10 bytes of one object."""
    url = f"{root}/studies/{study}/series/{series}/instances/{sop}"
    resp = requests.get(url, headers={"Accept": 'multipart/related; type="application/dicom"'}, auth=auth, timeout=120)
    resp.raise_for_status()
    content_type = resp.headers.get("Content-Type", "")
    if not content_type.startswith("multipart/"):
        return resp.content
    boundary = content_type.split("boundary=")[1].split(";")[0].strip().strip('"')
    delim = f"--{boundary}".encode()
    for part in resp.content.split(delim):
        sep = part.find(b"\r\n\r\n")
        if sep < 0:
            continue
        payload = part[sep + 4 :]
        # Remove exactly the one CRLF that precedes the next boundary — never
        # rstrip, which would eat pixel-data bytes ending in 0x0D 0x0A.
        if payload.endswith(b"\r\n"):
            payload = payload[:-2]
        if payload and payload != b"--":
            return payload
    pytest.fail("WADO response carried no DICOM part")


@pytest.fixture(scope="module")
def stowed_export() -> tuple[bytes, bytes]:
    """Export a de-identified SEG/SR, STOW it, retrieve both objects back."""
    root, auth = orthanc()
    cli = ferrum_cli()
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        src, _study_uid, _series_uid = write_series(tmp / "src")
        # The source study lives in the PACS (STOW the CT phantom).
        ct_response = stow(root, auth, sorted(src.glob("*.dcm")))
        assert success_uids(ct_response), "the CT phantom stored in Orthanc"

        ws = tmp / "ws"
        run(cli, "study", "open", "-w", str(ws), str(src), "--modality", "CT")
        run(
            cli,
            "segment",
            "threshold",
            "-w",
            str(ws),
            "--seed",
            "v:8,8,4",
            "--min",
            "200",
            "--max",
            "600",
            "--name",
            "lesion",
        )
        run(cli, "export", "bundle", "-w", str(ws), "--format", "dicom", "--deidentify", "basic")
        seg_path = ws / "export" / "segmentation.dcm"
        sr_path = ws / "export" / "measurements.dcm"
        assert seg_path.exists() and sr_path.exists(), "the de-identified SEG/SR exported"

        seg_ds = pydicom.dcmread(str(seg_path))
        sr_ds = pydicom.dcmread(str(sr_path))
        # STOW the export back to Orthanc; confirm the success-sequence tag.
        response = stow(root, auth, [seg_path, sr_path])
        stored = success_uids(response)
        assert str(seg_ds.SOPInstanceUID) in stored, "SEG in the (0008,1190) success sequence"
        assert str(sr_ds.SOPInstanceUID) in stored, "SR in the (0008,1190) success sequence"

        # Retrieve what comes back over WADO-RS — the objects highdicom checks.
        seg_bytes = wado_instance(
            root, auth, str(seg_ds.StudyInstanceUID), str(seg_ds.SeriesInstanceUID), str(seg_ds.SOPInstanceUID)
        )
        sr_bytes = wado_instance(
            root, auth, str(sr_ds.StudyInstanceUID), str(sr_ds.SeriesInstanceUID), str(sr_ds.SOPInstanceUID)
        )
        return seg_bytes, sr_bytes


def test_stowed_seg_is_validated_by_highdicom(stowed_export: tuple[bytes, bytes]) -> None:
    # covers 32.5-a: the SEG that comes back from Orthanc is a valid highdicom
    # Segmentation — segments, frames, orthonormal geometry, referenced images.
    seg_bytes, _ = stowed_export
    seg = hd.seg.Segmentation.from_dataset(pydicom.dcmread(io.BytesIO(seg_bytes)))
    assert seg.segmentation_type == hd.seg.SegmentationTypeValues.BINARY
    numbers = list(seg.segment_numbers)
    assert numbers == list(range(1, len(numbers) + 1)), numbers
    for n in numbers:
        desc = seg.get_segment_description(n)
        assert desc.segment_label and desc.algorithm_type is not None
    volume = seg.get_volume(combine_segments=False)
    array = volume.array
    assert array.shape[-1] == len(numbers) and array.any(), array.shape
    assert np.allclose(np.linalg.norm(volume.direction, axis=0), 1.0, atol=1e-4), "orthonormal axes"
    assert list(seg.get_source_image_uids()), "the SEG references its source images"


def test_stowed_sr_is_validated_by_highdicom(stowed_export: tuple[bytes, bytes]) -> None:
    # covers 32.5-a: the SR that comes back is a Comprehensive 3D SR with a
    # TID 1500 measurement report (segment volume measured).
    _, sr_bytes = stowed_export
    sr = hd.sr.Comprehensive3DSR.from_dataset(pydicom.dcmread(io.BytesIO(sr_bytes)))
    assert sr.VerificationFlag == "UNVERIFIED"
    report = hd.sr.MeasurementReport.from_sequence([sr])
    groups = report.get_planar_roi_measurement_groups() + report.get_volumetric_roi_measurement_groups()
    assert groups, "the SR carries ROI measurement groups"
    volumes = report.get_volumetric_roi_measurement_groups()
    for group in volumes:
        assert group.referenced_segment is not None, "volume groups reference a segment"
        assert group.get_measurements(name=codes.SCT.Volume), "segment volume measured"


def test_no_identifier_leaks_into_the_stowed_seg(stowed_export: tuple[bytes, bytes]) -> None:
    # covers 32.5-a, 32.2-a: the de-identified SEG that round-trips through the
    # PACS carries none of the source patient identifiers.
    seg_bytes, _ = stowed_export
    for secret in SECRETS:
        assert secret not in seg_bytes, f"{secret!r} leaks into the retrieved SEG"
    seg = pydicom.dcmread(io.BytesIO(seg_bytes))
    assert str(seg.PatientIdentityRemoved).strip() == "YES"
