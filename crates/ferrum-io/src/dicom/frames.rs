//! Functional groups of Enhanced multi-frame DICOM (MR, PET, CT): the
//! geometry and the stack key of every frame, and the pixel measures they
//! share.
//!
//! A functional group macro (Plane Position, Plane Orientation, Pixel
//! Measures, Frame Content, MR Echo, MR Diffusion) sits either in the item
//! of the Per-Frame Functional Groups Sequence for that frame or in the
//! single item of the Shared Functional Groups Sequence; the per-frame one
//! wins.

use dicom_dictionary_std::tags;
use dicom_object::{mem::InMemDicomObject, Tag};
use glam::{DVec3, Vec2};

use super::header::{f64_of, f64s_of, int_of, StackKey};

/// Geometry and stack key of one frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameInfo {
    /// Image Position (Patient) of the frame's first pixel.
    pub position: Option<DVec3>,
    /// Image Orientation (Patient): row and column direction cosines.
    pub orientation: Option<(DVec3, DVec3)>,
    /// What tells this frame apart from others at the same position.
    pub stack: StackKey,
}

/// What the functional groups of a file say.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FunctionalGroups {
    /// One entry per frame, in file order; empty without a Per-Frame
    /// Functional Groups Sequence.
    pub frames: Vec<FrameInfo>,
    /// Pixel spacing `(column, row)` in mm from the Pixel Measures macro.
    pub pixel_spacing: Option<Vec2>,
    /// Slice thickness in mm from the Pixel Measures macro.
    pub slice_thickness: Option<f64>,
    /// Orientation from the Plane Orientation macro (shared, else of the
    /// first frame).
    pub orientation: Option<(DVec3, DVec3)>,
}

/// The first item of the sequence `tag` in `obj`.
fn first_item(obj: &InMemDicomObject, tag: Tag) -> Option<&InMemDicomObject> {
    obj.get(tag)?.items()?.first()
}

/// The items of the sequence `tag` in `obj` (empty if absent).
fn items(obj: &InMemDicomObject, tag: Tag) -> &[InMemDicomObject] {
    obj.get(tag).and_then(|e| e.items()).unwrap_or(&[])
}

/// The macro `tag` of a frame: from its per-frame item, else shared.
fn group<'a>(
    frame: Option<&'a InMemDicomObject>,
    shared: Option<&'a InMemDicomObject>,
    tag: Tag,
) -> Option<&'a InMemDicomObject> {
    frame.and_then(|f| first_item(f, tag)).or_else(|| shared.and_then(|s| first_item(s, tag)))
}

pub(crate) fn position_of(obj: &InMemDicomObject) -> Option<DVec3> {
    f64s_of(obj, tags::IMAGE_POSITION_PATIENT).filter(|v| v.len() >= 3).map(|v| DVec3::new(v[0], v[1], v[2]))
}

pub(crate) fn orientation_of(obj: &InMemDicomObject) -> Option<(DVec3, DVec3)> {
    f64s_of(obj, tags::IMAGE_ORIENTATION_PATIENT)
        .filter(|v| v.len() >= 6)
        .map(|v| (DVec3::new(v[0], v[1], v[2]), DVec3::new(v[3], v[4], v[5])))
        .filter(|(r, c)| r.length() > 0.5 && c.length() > 0.5)
}

pub(crate) fn pixel_spacing_of(obj: &InMemDicomObject) -> Option<Vec2> {
    f64s_of(obj, tags::PIXEL_SPACING)
        .filter(|v| v.len() >= 2 && v[0] > 0.0 && v[1] > 0.0)
        // DICOM order is (row spacing, column spacing)
        .map(|v| Vec2::new(v[1] as f32, v[0] as f32))
}

fn frame_info(frame: Option<&InMemDicomObject>, shared: Option<&InMemDicomObject>) -> FrameInfo {
    let content = group(frame, shared, tags::FRAME_CONTENT_SEQUENCE);
    let echo = group(frame, shared, tags::MR_ECHO_SEQUENCE);
    let diffusion = group(frame, shared, tags::MR_DIFFUSION_SEQUENCE);
    FrameInfo {
        position: group(frame, shared, tags::PLANE_POSITION_SEQUENCE).and_then(position_of),
        orientation: group(frame, shared, tags::PLANE_ORIENTATION_SEQUENCE).and_then(orientation_of),
        stack: StackKey {
            temporal_position: content.and_then(|c| int_of(c, tags::TEMPORAL_POSITION_INDEX)),
            echo_time: echo.and_then(|e| f64_of(e, tags::EFFECTIVE_ECHO_TIME)),
            b_value: diffusion.and_then(|d| f64_of(d, tags::DIFFUSION_B_VALUE)),
            ..StackKey::default()
        },
    }
}

/// Reads the functional groups of `obj`; `None` when it has neither a
/// shared nor a per-frame sequence (a classic file).
pub fn read(obj: &InMemDicomObject) -> Option<FunctionalGroups> {
    let shared = first_item(obj, tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE);
    let per_frame = items(obj, tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE);
    if shared.is_none() && per_frame.is_empty() {
        return None;
    }
    let measures = group(per_frame.first(), shared, tags::PIXEL_MEASURES_SEQUENCE);
    let frames: Vec<FrameInfo> = per_frame.iter().map(|f| frame_info(Some(f), shared)).collect();
    Some(FunctionalGroups {
        pixel_spacing: measures.and_then(pixel_spacing_of),
        slice_thickness: measures.and_then(|m| f64_of(m, tags::SLICE_THICKNESS)).filter(|v| *v > 0.0),
        orientation: group(per_frame.first(), shared, tags::PLANE_ORIENTATION_SEQUENCE).and_then(orientation_of),
        frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom_core::value::DataSetSequence;
    use dicom_core::{DataElement, PrimitiveValue, VR};

    fn seq(tag: Tag, items: Vec<InMemDicomObject>) -> DataElement<InMemDicomObject> {
        DataElement::new(tag, VR::SQ, DataSetSequence::from(items))
    }

    fn one(elements: Vec<DataElement<InMemDicomObject>>) -> InMemDicomObject {
        InMemDicomObject::from_element_iter(elements)
    }

    fn fd(tag: Tag, v: f64) -> DataElement<InMemDicomObject> {
        DataElement::new(tag, VR::FD, PrimitiveValue::from(v))
    }

    fn ds(tag: Tag, v: &str) -> DataElement<InMemDicomObject> {
        // several values, as a file read back gives them
        DataElement::new(tag, VR::DS, PrimitiveValue::Strs(v.split('\\').map(String::from).collect()))
    }

    #[test]
    fn per_frame_groups_override_shared_ones() {
        // covers 20.1-b, 20.1-c
        let shared = one(vec![
            seq(tags::PIXEL_MEASURES_SEQUENCE, vec![one(vec![ds(tags::PIXEL_SPACING, "0.5\\0.25")])]),
            seq(tags::MR_ECHO_SEQUENCE, vec![one(vec![fd(tags::EFFECTIVE_ECHO_TIME, 30.0)])]),
            seq(
                tags::PLANE_ORIENTATION_SEQUENCE,
                vec![one(vec![ds(tags::IMAGE_ORIENTATION_PATIENT, "0\\1\\0\\0\\0\\-1")])],
            ),
        ]);
        let frame = |z: &str, echo: Option<f64>, b: f64| {
            let mut groups = vec![
                seq(tags::PLANE_POSITION_SEQUENCE, vec![one(vec![ds(tags::IMAGE_POSITION_PATIENT, z)])]),
                seq(tags::MR_DIFFUSION_SEQUENCE, vec![one(vec![fd(tags::DIFFUSION_B_VALUE, b)])]),
            ];
            if let Some(e) = echo {
                groups.push(seq(tags::MR_ECHO_SEQUENCE, vec![one(vec![fd(tags::EFFECTIVE_ECHO_TIME, e)])]));
            }
            one(groups)
        };
        let obj = one(vec![
            seq(tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE, vec![shared]),
            seq(
                tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
                vec![frame("1\\2\\3", Some(90.0), 0.0), frame("4\\5\\6", None, 1000.0)],
            ),
        ]);
        let g = read(&obj).unwrap();
        // DICOM (row, column) = (0.5, 0.25) → (x, y) = (0.25, 0.5)
        assert_eq!(g.pixel_spacing, Some(Vec2::new(0.25, 0.5)));
        assert_eq!(g.orientation, Some((DVec3::Y, DVec3::NEG_Z)));
        assert_eq!(g.frames.len(), 2);
        assert_eq!(g.frames[0].position, Some(DVec3::new(1.0, 2.0, 3.0)));
        assert_eq!((g.frames[0].stack.echo_time, g.frames[0].stack.b_value), (Some(90.0), Some(0.0)));
        assert_eq!((g.frames[1].stack.echo_time, g.frames[1].stack.b_value), (Some(30.0), Some(1000.0)));
        assert_eq!(g.frames[1].orientation, Some((DVec3::Y, DVec3::NEG_Z)), "shared orientation");
        assert_eq!(read(&one(vec![ds(tags::PIXEL_SPACING, "1\\1")])), None, "a classic file has no groups");
    }
}
