# Coordinates

| Form | Meaning |
|---|---|
| `voxel [i, j, k]` | 0-based index in the canonical LPS grid: `i` towards patient left, `j` posterior, `k` superior. Integers are voxel centres, and fractions are allowed. |
| `slice_number` | 1-based number of a slice in a plane, as shown in the viewer. `slice_index` is the 0-based form. Axial slice `n` holds voxels with `k = n − 1`. |
| `patient_mm [x, y, z]` | LPS millimetres from the series geometry: `x` towards left, `y` posterior, `z` superior. |
| `render` + `pixel [x, y]` | A pixel of an earlier render. Pixel centres lie at `+0.5`; `(0, 0)` is the top-left corner. |

- **Outputs:** every point appears in all forms: `voxel` (continuous),
  `voxel_index` (nearest) and `patient_mm`.
- **Image orientation:** radiological convention.
  - Axial images show the patient's right on the left (`orientation` in
    the sidecar).
  - Coronal and sagittal images have the head at the top.
- **Tiled renders** (`view montage`, `view mpr`): each tile has its own
  `origin` and maps. A render pixel resolves through the tile under it.
- **3D renders** have no mapping. Measure on slices.
- **Values:** taken from the nearest voxel; nothing is interpolated.
  - **Distances** use patient millimetres.
  - **Volumes** count voxels × voxel volume.
