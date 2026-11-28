"""Oracle diff: FERRUM's PS3.15 Basic Profile vs pydicom/deid (Stage 31, #75).

A synthetic DICOM series (generated at run time, no patient data) is
de-identified two ways: FERRUM (`export bundle --deidentify basic`, which
applies the PS3.15 Annex E Table E.1-1) and pydicom/deid (MIT) with a recipe
expressing the same Basic Profile core actions. The test pins the agreement on
the core identifiers and names the documented differences.

Skips when `deid` is not installed or no `ferrum-cli` binary is built (build it
with `cargo build -p ferrum-cli`, or point `FERRUM_CLI` at one). The RSNA/CTP
anonymizer (Java) is a best-effort follow-up; the deid oracle covers the core.

# covers 31.4-a
# covers 31.4-b
"""

import os
import shutil
import subprocess
import tempfile
from pathlib import Path

import pytest

pydicom = pytest.importorskip("pydicom")
deid_dicom = pytest.importorskip("deid.dicom")

from pydicom.dataset import Dataset, FileDataset  # noqa: E402
from pydicom.uid import ExplicitVRLittleEndian, generate_uid  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]

# The core identifiers both profiles must remove or anonymise. FERRUM follows
# Table E.1-1 strictly (PatientName D, PatientID Z->pseudonym, PatientBirthDate
# Z, AccessionNumber Z, InstitutionName X, ReferringPhysicianName X); the deid
# recipe below expresses the same intent.
RECIPE = """FORMAT dicom
%header
REPLACE PatientName ANONYMOUS
REPLACE PatientID ANONYMOUS
REMOVE PatientBirthDate
REMOVE AccessionNumber
REMOVE InstitutionName
REMOVE ReferringPhysicianName
REMOVE StudyDescription
REMOVE StudyDate
"""


def ferrum_cli() -> str:
    env = os.environ.get("FERRUM_CLI")
    if env:
        return env
    for cand in [
        ROOT / "target/release/ferrum-cli.exe",
        ROOT / "target/debug/ferrum-cli.exe",
        ROOT / "target/release/ferrum-cli",
        ROOT / "target/debug/ferrum-cli",
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


def write_series(directory: Path) -> Path:
    """A 16x16x8 CT with a 400 HU blob in -1000 HU background, identifiers set."""
    import numpy as np

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
        ds.PatientName = "DOE^JOHN"
        ds.PatientID = "123456"
        ds.PatientBirthDate = "19800101"
        ds.PatientSex = "M"
        ds.PatientAge = "045Y"
        ds.StudyDate = "20240428"
        ds.StudyTime = "102417"
        ds.AccessionNumber = "ACC-999"
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
    return directory


def value_gone(ds, attr: str) -> bool:
    """The attribute is absent or zero-length (FERRUM Z) — its value is gone."""
    if attr not in ds:
        return True
    v = ds.data_element(attr).value
    return v is None or str(v).strip("\0 ").strip() == ""


def test_ferrum_basic_profile_agrees_with_deid_on_core_identifiers():
    cli = ferrum_cli()
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        src = write_series(tmp / "src")
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
        assert seg_path.exists(), "the de-identified SEG was exported"
        ferrum_seg = pydicom.dcmread(str(seg_path))

        # deid oracle on the same source
        recipe = tmp / "basic.deid"
        recipe.write_text(RECIPE)
        source_file = str(next(src.glob("*.dcm")))
        deid_clean = deid_dicom.replace_identifiers([source_file], deid=str(recipe))[0]

        # Agreement: both anonymise the patient name and remove the identifiers.
        assert str(ferrum_seg.PatientName) == "ANONYMOUS"
        assert str(deid_clean.PatientName) == "ANONYMOUS"
        assert str(ferrum_seg.PatientID) != "123456", "FERRUM pseudonymises PatientID"
        assert str(deid_clean.PatientID) != "123456", "deid anonymises PatientID"
        for attr in ["PatientBirthDate", "AccessionNumber", "InstitutionName", "ReferringPhysicianName"]:
            assert value_gone(ferrum_seg, attr), f"FERRUM keeps {attr}"
            assert value_gone(deid_clean, attr), f"deid keeps {attr}"
        # FERRUM marks the object de-identified (PS3.15 E.1.1).
        assert ferrum_seg.PatientIdentityRemoved == "YES"
        # No source identifier leaks into the FERRUM SEG bytes.
        text = seg_path.read_bytes()
        for secret in [b"DOE^JOHN", b"123456", b"19800101", b"ACC-999", b"Secret Hospital", b"DR^SECRET"]:
            assert secret not in text, f"{secret!r} leaks into the SEG"


def test_ferrum_stricter_than_deid_on_dates_and_characteristics_by_default():
    # covers 31.4-b — the documented difference: FERRUM's default Basic Profile
    # zeroes the study date and removes patient characteristics (age X, sex Z),
    # while a deid recipe keeps them unless told otherwise. Both are valid; the
    # difference is named, not hidden.
    cli = ferrum_cli()
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        src = write_series(tmp / "src")
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
        seg = pydicom.dcmread(str(ws / "export" / "segmentation.dcm"))
        assert value_gone(seg, "StudyDate"), "FERRUM zeroes StudyDate by default"
        assert value_gone(seg, "PatientAge"), "FERRUM removes PatientAge by default (X)"
        # deid with a minimal recipe keeps them (the documented difference).
        recipe = tmp / "min.deid"
        recipe.write_text("FORMAT dicom\n%header\nREPLACE PatientName ANONYMOUS\n")
        clean = deid_dicom.replace_identifiers([str(next(src.glob("*.dcm")))], deid=str(recipe))[0]
        assert not value_gone(clean, "StudyDate"), "deid keeps StudyDate unless told"
        assert not value_gone(clean, "PatientAge"), "deid keeps PatientAge unless told"
