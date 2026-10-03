"""Validates DICOM objects exported by FERRUM with highdicom.

Usage: python scripts/validate_dicom_export.py DIR

Every ``segmentation.dcm`` below DIR is read as a highdicom Segmentation
(segments, frames, geometry) and every ``measurements.dcm`` as a
Comprehensive 3D SR with a TID 1500 measurement report. The Rust test
``crates/ferrum-io/tests/dicom_export.rs`` writes the samples when
``FERRUM_DICOM_EXPORT_DIR`` is set.
"""

import sys
from pathlib import Path

import highdicom as hd
import numpy as np
import pydicom
from pydicom.sr.codedict import codes


def check_seg(path: Path) -> str:
    seg = hd.seg.Segmentation.from_dataset(pydicom.dcmread(path))
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
    referenced = [s for s in seg.get_source_image_uids()]
    return f"{len(numbers)} segments, {seg.NumberOfFrames} frames, {len(referenced)} source images"


def check_sr(path: Path) -> str:
    ds = pydicom.dcmread(path)
    sr = hd.sr.Comprehensive3DSR.from_dataset(ds)
    assert sr.VerificationFlag == "UNVERIFIED"
    report = hd.sr.MeasurementReport.from_sequence([sr])
    groups = report.get_planar_roi_measurement_groups() + report.get_volumetric_roi_measurement_groups()
    measurements = 0
    for item in sr.ContentSequence:
        if item.ConceptNameCodeSequence[0].CodeValue != "126010":
            continue
        for group in item.ContentSequence:
            nums = [c for c in group.ContentSequence if c.ValueType == "NUM"]
            assert nums, "every group has a measurement"
            for c in nums:
                float(c.MeasuredValueSequence[0].NumericValue)
                measurements += 1
            ids = [c for c in group.ContentSequence if c.ConceptNameCodeSequence[0].CodeValue == "112040"]
            assert len(ids) == 1, "tracking UID"
    volumes = report.get_volumetric_roi_measurement_groups()
    for g in volumes:
        ref = g.referenced_segment
        assert ref is not None, "volume groups reference a segment"
        assert g.get_measurements(name=codes.SCT.Volume), "segment volume"
    return f"{measurements} measurements, {len(volumes)} segment volumes ({len(groups)} ROI groups)"


def main(root: str) -> int:
    paths = sorted(Path(root).rglob("*.dcm"))
    if not paths:
        print(f"no DICOM files under {root}")
        return 1
    for path in paths:
        name = path.name
        if name.startswith("segmentation"):
            print(f"{path}: {check_seg(path)}")
        elif name.startswith("measurements"):
            print(f"{path}: {check_sr(path)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "."))
