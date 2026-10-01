//! Annotation report: everything needed to reproduce the annotations of a
//! study outside the viewer. The data layer serialises it (JSON).

use std::path::PathBuf;

use glam::{Vec2, Vec3};

use crate::annotation::{AnnotationId, AnnotationSet};
use crate::repository::StudyInfo;
use crate::slice::SliceAxis;
use crate::volume::Volume;

/// One annotation, described independently of the viewer state.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotationRecord {
    /// Id within the study's annotation set.
    pub id: AnnotationId,
    /// User-given name.
    pub name: String,
    /// Annotation type (`"Distance"`, `"Angle"`, `"Area"`, `"Rectangle"`,
    /// `"Text"`).
    pub kind: &'static str,
    /// Slice orientation.
    pub plane: SliceAxis,
    /// Zero-based slice index along the plane normal.
    pub slice_index: u32,
    /// Measured value in [`AnnotationRecord::unit`]; `None` for text.
    pub value: Option<f32>,
    /// `"mm"`, `"deg"`, `"mm2"`, or empty for text.
    pub unit: &'static str,
    /// Text of a text annotation.
    pub text: Option<String>,
    /// Control points in in-plane millimetres from the top-left corner of
    /// the displayed slice (x right, y down).
    pub points_mm: Vec<Vec2>,
    /// The same points as continuous voxel coordinates `(i, j, k)` of the
    /// loaded volume; integer values are voxel centres.
    pub points_voxel: Vec<Vec3>,
}

/// All annotations of a study plus what identifies the study.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnotationReport {
    /// Opened folder (DICOM) or file (NIfTI).
    pub source: PathBuf,
    /// Study identification from the DICOM header.
    pub study: StudyInfo,
    /// Volume size in voxels `(i, j, k)`.
    pub dims: [u32; 3],
    /// Voxel spacing in millimetres.
    pub spacing: Vec3,
    /// Annotations in creation order.
    pub annotations: Vec<AnnotationRecord>,
}

impl AnnotationReport {
    /// Builds the report of `set` for a study shown with `volume`.
    /// Annotations with an unknown slice orientation are skipped.
    pub fn build(source: PathBuf, study: StudyInfo, volume: &Volume, set: &AnnotationSet) -> Self {
        let annotations = set
            .iter()
            .filter_map(|(id, key, a, name)| {
                let plane = key.slice_axis()?;
                let points_mm = a.points();
                let points_voxel = points_mm.iter().map(|p| mm_to_voxel(volume, plane, key.index, *p)).collect();
                Some(AnnotationRecord {
                    id,
                    name: name.to_string(),
                    kind: a.kind(),
                    plane,
                    slice_index: key.index,
                    value: a.value(),
                    unit: a.unit(),
                    text: match a {
                        crate::annotation::Annotation::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    },
                    points_mm,
                    points_voxel,
                })
            })
            .collect();
        let d = volume.dims();
        Self { source, study, dims: [d.x, d.y, d.z], spacing: volume.spacing(), annotations }
    }
}

/// Continuous voxel coordinates of in-plane point `mm` on slice `index`.
pub fn mm_to_voxel(volume: &Volume, plane: SliceAxis, index: u32, mm: Vec2) -> Vec3 {
    let uv = mm / plane.plane_size_mm(volume);
    let t = plane.tex_coord(uv, plane.slice_position(volume, index));
    let dims = volume.dims().as_uvec3().as_vec3();
    t * dims - Vec3::splat(0.5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::{Annotation, SliceKey};
    use crate::geometry::Dims3;

    fn volume() -> Volume {
        let dims = Dims3::new(10, 20, 5);
        Volume::from_physical(dims, Vec3::new(1.0, 0.5, 2.0), &vec![0.0; dims.voxel_count()]).unwrap()
    }

    #[test]
    fn voxel_coordinates_of_slice_points() {
        let v = volume();
        // axial slice 3: in-plane mm map to (i, j), k is the slice index
        let p = mm_to_voxel(&v, SliceAxis::Axial, 3, Vec2::new(0.5, 0.25));
        assert!((p - Vec3::new(0.0, 0.0, 3.0)).length() < 1e-4, "{p}");
        let q = mm_to_voxel(&v, SliceAxis::Axial, 3, Vec2::new(9.5, 9.75));
        assert!((q - Vec3::new(9.0, 19.0, 3.0)).length() < 1e-4, "{q}");
        // coronal: vertical screen axis runs from superior (top) to inferior
        let c = mm_to_voxel(&v, SliceAxis::Coronal, 4, Vec2::new(0.5, 1.0));
        assert!((c - Vec3::new(0.0, 4.0, 4.0)).length() < 1e-4, "{c}");
    }

    #[test]
    fn report_lists_named_annotations() {
        let v = volume();
        let mut set = AnnotationSet::default();
        let id =
            set.add(SliceKey::new(SliceAxis::Axial, 2), Annotation::Distance { a: Vec2::ZERO, b: Vec2::new(3.0, 4.0) });
        set.rename(id, "Nodule");
        set.add(SliceKey::new(SliceAxis::Sagittal, 1), Annotation::Text { pos: Vec2::ONE, text: "note".into() });
        let study = StudyInfo { study_date: "20240428".into(), ..Default::default() };
        let r = AnnotationReport::build(PathBuf::from("/data/ct"), study, &v, &set);
        assert_eq!(r.dims, [10, 20, 5]);
        assert_eq!(r.study.study_date, "20240428");
        assert_eq!(r.annotations.len(), 2);
        let d = &r.annotations[0];
        assert_eq!(
            (d.name.as_str(), d.kind, d.plane, d.slice_index, d.unit),
            ("Nodule", "Distance", SliceAxis::Axial, 2, "mm")
        );
        assert_eq!(d.value, Some(5.0));
        assert_eq!(d.points_mm.len(), 2);
        assert_eq!(d.points_voxel.len(), 2);
        let t = &r.annotations[1];
        assert_eq!((t.kind, t.text.as_deref(), t.value), ("Text", Some("note"), None));
    }
}
