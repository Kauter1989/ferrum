//! Clean Pixel Data: masking burned-in regions (Stage 31, #75, criterion
//! 31.3-a; ADR 0020 decision 5; `docs/deidentification.md` §2).
//!
//! Burned-in annotation — patient names, IDs and dates rendered into the
//! pixels themselves — survives every attribute-level de-identification
//! ([`super::deid`]). This module masks it: the pixels of a region are
//! replaced by a solid fill, the rest of the image is unchanged. Masking is
//! a **pixel transform**, not image formation: it edits stored pixel data
//! and never touches a renderer.
//!
//! Region sources:
//! - configured rectangular regions ([`CleanPixelSpec::regions`]), always
//!   masked, whether or not the flag is set;
//! - the `BurnedInAnnotation` flag (`(0028,0301)` = `YES`): with no
//!   configured region the text's location is unknown, so the conservative
//!   default masks the **whole image** (the profile is conservative by
//!   default, ADR 0020 decision 6). After masking, the flag is set to `NO`.
//!
//! Supported pixel data: native (uncompressed) monochrome 8/16-bit and
//! RGB 8-bit (planar configuration 0 or 1), single- or multi-frame. Anything
//! else (encapsulated/compressed data, other sample layouts) is refused with
//! [`CleanPixelError::Unsupported`] — a burned-in region that cannot be
//! masked is an error, never a silent pass.
//!
//! The render-side application of the same region mask is a pure voxel
//! transform on the loaded volume ([`ferrum_domain::Volume::masked_copy`]);
//! the `ferrum-app` use case wires it for masked renders (criterion 31.3-c).

use std::path::{Path, PathBuf};

use dicom_core::header::Header;
use dicom_core::value::Value;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_dictionary_std::tags;
use dicom_object::{InMemDicomObject, OpenFileOptions};

/// A rectangular region in stored pixel coordinates: `x` along columns,
/// `y` along rows, 0-based, half-open (`x ≤ col < x + w`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRegion {
    /// First column (0-based).
    pub x: u32,
    /// First row (0-based).
    pub y: u32,
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
}

impl PixelRegion {
    /// A region at `(x, y)` of `w × h` pixels.
    pub const fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// The region clipped to a `cols × rows` image; `None` when it lies
    /// entirely outside (or is empty).
    pub fn clamped(self, cols: u32, rows: u32) -> Option<Self> {
        let x = self.x.min(cols);
        let y = self.y.min(rows);
        let w = self.w.min(cols - x);
        let h = self.h.min(rows - y);
        (w > 0 && h > 0).then_some(Self { x, y, w, h })
    }
}

/// What to mask: the configured regions and the solid fill that replaces
/// their pixels (a stored value; the default `0` is black/neutral).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanPixelSpec {
    /// Configured rectangular regions, always masked (clipped to the image).
    pub regions: Vec<PixelRegion>,
    /// Solid fill written into every masked pixel (stored bit pattern).
    pub fill: u16,
}

/// What [`clean_pixel_data`] did, so a caller can report it (never silently
/// passed): the flag as found, whether the whole image was masked, and the
/// regions actually applied (after clipping).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanPixelReport {
    /// `true` when pixels were replaced.
    pub masked: bool,
    /// The `BurnedInAnnotation` flag as found (`YES`).
    pub burned_in_flag: bool,
    /// `true` when the flag's conservative whole-image default was applied.
    pub whole_image: bool,
    /// The regions applied, clipped to the image.
    pub regions: Vec<PixelRegion>,
}

/// Everything masking a data set can fail at.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CleanPixelError {
    /// The data set carries no pixel data to mask.
    #[error("the data set has no pixel data")]
    NoPixelData,
    /// A required pixel-layout attribute is missing or inconsistent.
    #[error("pixel layout: {0}")]
    Layout(String),
    /// The pixel data cannot be masked by this transform (compressed or an
    /// unsupported sample layout); it must not pass as cleaned.
    #[error("unsupported pixel data: {0}")]
    Unsupported(String),
}

/// The `BurnedInAnnotation` flag (`(0028,0301)`) reads `YES`.
pub fn burned_in_annotation(obj: &InMemDicomObject) -> bool {
    obj.element(tags::BURNED_IN_ANNOTATION)
        .ok()
        .and_then(|e| e.to_str().ok())
        .is_some_and(|s| s.trim().eq_ignore_ascii_case("YES"))
}

/// Masks the burned-in regions of `obj`'s pixel data per `spec` and reports
/// what was done. With configured regions those are masked; with none and
/// the flag `YES` the whole image is masked (the conservative default);
/// with none and no flag the object is unchanged (`masked: false`). After
/// masking, `BurnedInAnnotation` is set to `NO`. On error the object is left
/// as it was.
pub fn clean_pixel_data(
    obj: &mut InMemDicomObject,
    spec: &CleanPixelSpec,
) -> Result<CleanPixelReport, CleanPixelError> {
    let (cols, rows) = image_dims(obj)?;
    let flag = burned_in_annotation(obj);
    let mut regions: Vec<PixelRegion> = spec.regions.iter().filter_map(|r| r.clamped(cols, rows)).collect();
    let whole_image = regions.is_empty() && flag;
    if whole_image {
        regions.push(PixelRegion::new(0, 0, cols, rows));
    }
    if regions.is_empty() {
        return Ok(CleanPixelReport { masked: false, burned_in_flag: flag, whole_image: false, regions });
    }
    let layout = PixelLayout::read(obj, cols, rows)?;
    let elem = obj.take_element(tags::PIXEL_DATA).map_err(|_| CleanPixelError::NoPixelData)?;
    let vr = elem.vr();
    let mut value = elem.into_value();
    let masked = mask_value(&mut value, &layout, &regions, spec.fill);
    // Put the pixel data back even when masking failed: the object stays as
    // it was (mask_value only writes after every check passed).
    obj.put(DataElement::new(tags::PIXEL_DATA, vr, value));
    masked?;
    obj.put(DataElement::new(tags::BURNED_IN_ANNOTATION, VR::CS, PrimitiveValue::from("NO")));
    Ok(CleanPixelReport { masked: true, burned_in_flag: flag, whole_image, regions })
}

/// `true` when any of the source files carries `BurnedInAnnotation = YES`
/// (the render-side flag check: a masked render of a flagged study masks the
/// whole volume when no region is configured). Files that are not DICOM or
/// cannot be read contribute `false`; only headers up to the pixel data are
/// parsed.
pub fn sources_burned_in(paths: &[PathBuf]) -> bool {
    paths.iter().any(|p| source_burned_in(p))
}

/// The flag of one file; `false` when it cannot be read as DICOM.
fn source_burned_in(path: &Path) -> bool {
    OpenFileOptions::new()
        .read_until(tags::PIXEL_DATA)
        .open_file(path)
        .map(|obj| burned_in_annotation(&obj))
        .unwrap_or(false)
}

/// `(columns, rows)` of the data set; both attributes are required.
fn image_dims(obj: &InMemDicomObject) -> Result<(u32, u32), CleanPixelError> {
    let dim = |tag: Tag, name: &str| -> Result<u32, CleanPixelError> {
        obj.element(tag)
            .ok()
            .and_then(|e| e.to_int::<u16>().ok())
            .map(u32::from)
            .filter(|v| *v > 0)
            .ok_or_else(|| CleanPixelError::Layout(format!("{name} is missing or zero")))
    };
    Ok((dim(tags::COLUMNS, "Columns")?, dim(tags::ROWS, "Rows")?))
}

/// The stored pixel layout the mask walks.
struct PixelLayout {
    /// Columns of the image.
    cols: u32,
    /// Rows of the image.
    rows: u32,
    /// Samples per pixel (1 monochrome, 3 RGB).
    spp: usize,
    /// Frames (NumberOfFrames, default 1).
    frames: usize,
    /// Planar configuration (0 interleaved, 1 separate planes; RGB only).
    planar: u16,
    /// Bits allocated (8 or 16).
    bits: u16,
}

impl PixelLayout {
    fn read(obj: &InMemDicomObject, cols: u32, rows: u32) -> Result<Self, CleanPixelError> {
        let uint = |tag: Tag, default: u16| -> Result<u16, CleanPixelError> {
            match obj.element(tag) {
                Ok(e) => e.to_int::<u16>().map_err(|_| CleanPixelError::Layout(format!("{} is not a uint16", e.tag()))),
                Err(_) => Ok(default),
            }
        };
        let spp = uint(tags::SAMPLES_PER_PIXEL, 1)?;
        let bits = uint(tags::BITS_ALLOCATED, 0)?;
        let planar = uint(tags::PLANAR_CONFIGURATION, 0)?;
        let frames = obj
            .element(tags::NUMBER_OF_FRAMES)
            .ok()
            .and_then(|e| e.to_str().ok())
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(1);
        let ok = matches!((spp, bits), (1, 8) | (1, 16) | (3, 8)) && (spp == 1 || planar <= 1) && frames > 0;
        if !ok {
            return Err(CleanPixelError::Unsupported(format!(
                "samples per pixel {spp}, bits allocated {bits}, planar configuration {planar}"
            )));
        }
        Ok(Self { cols, rows, spp: usize::from(spp), frames, planar, bits })
    }

    /// Pixels per frame.
    fn frame_len(&self) -> usize {
        self.cols as usize * self.rows as usize
    }

    /// Elements the pixel data must hold (`frames × rows × cols × spp`).
    fn len(&self) -> usize {
        self.frame_len() * self.spp * self.frames
    }
}

/// Replaces the pixels of `regions` in one pixel value. Writes happen only
/// after the layout and length checks passed, so an error leaves the value
/// unchanged.
fn mask_value(
    value: &mut Value<InMemDicomObject>,
    layout: &PixelLayout,
    regions: &[PixelRegion],
    fill: u16,
) -> Result<(), CleanPixelError> {
    match value {
        Value::Primitive(PrimitiveValue::U16(d)) if layout.bits == 16 && layout.spp == 1 => {
            check_len(d.len(), layout)?;
            mask_gray(d, layout, regions, fill);
        }
        Value::Primitive(PrimitiveValue::I16(d)) if layout.bits == 16 && layout.spp == 1 => {
            check_len(d.len(), layout)?;
            mask_gray(d, layout, regions, fill as i16);
        }
        Value::Primitive(PrimitiveValue::U8(d)) if layout.bits == 8 => {
            check_len(d.len(), layout)?;
            mask_samples(d, layout, regions, fill as u8);
        }
        Value::Primitive(_) => {
            return Err(CleanPixelError::Unsupported("pixel data is not 8/16-bit native samples".into()));
        }
        Value::PixelSequence(_) => {
            return Err(CleanPixelError::Unsupported("encapsulated (compressed) pixel data".into()));
        }
        Value::Sequence(_) => {
            return Err(CleanPixelError::Unsupported("pixel data is a sequence".into()));
        }
    }
    Ok(())
}

fn check_len(actual: usize, layout: &PixelLayout) -> Result<(), CleanPixelError> {
    if actual == layout.len() {
        Ok(())
    } else {
        Err(CleanPixelError::Unsupported(format!("pixel data holds {actual} elements, expected {}", layout.len())))
    }
}

/// Monochrome samples of any integer type: each masked row is one run.
fn mask_gray<T: Copy>(data: &mut [T], layout: &PixelLayout, regions: &[PixelRegion], fill: T) {
    let cols = layout.cols as usize;
    let frame_len = layout.frame_len();
    for f in 0..layout.frames {
        for r in regions {
            for row in r.y..r.y + r.h {
                let start = f * frame_len + row as usize * cols + r.x as usize;
                data[start..start + r.w as usize].fill(fill);
            }
        }
    }
}

/// 8-bit samples: monochrome runs, or RGB with the fill in every channel.
fn mask_samples<T: Copy>(data: &mut [T], layout: &PixelLayout, regions: &[PixelRegion], fill: T) {
    if layout.spp == 1 {
        mask_gray(data, layout, regions, fill);
    } else {
        mask_rgb(data, layout, regions, fill);
    }
}

/// RGB 8-bit: planar 0 interleaved (`rgb rgb …`), planar 1 separate planes
/// (`rr… gg… bb…`), per frame.
fn mask_rgb<T: Copy>(data: &mut [T], layout: &PixelLayout, regions: &[PixelRegion], fill: T) {
    let cols = layout.cols as usize;
    let frame_len = layout.frame_len();
    for f in 0..layout.frames {
        for r in regions {
            for row in r.y..r.y + r.h {
                let in_frame = row as usize * cols + r.x as usize;
                let line = f * frame_len + in_frame;
                if layout.planar == 0 {
                    for px in 0..r.w as usize {
                        let base = (line + px) * 3;
                        data[base..base + 3].fill(fill);
                    }
                } else {
                    // separate planes per frame: frame f, plane c, pixel in_frame
                    for plane in 0..3 {
                        let start = f * frame_len * 3 + plane * frame_len + in_frame;
                        data[start..start + r.w as usize].fill(fill);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamping_keeps_the_inside_part() {
        let r = PixelRegion::new(28, 20, 10, 10);
        assert_eq!(r.clamped(32, 24), Some(PixelRegion::new(28, 20, 4, 4)));
        assert_eq!(PixelRegion::new(0, 0, 32, 24).clamped(32, 24), Some(PixelRegion::new(0, 0, 32, 24)));
        assert_eq!(PixelRegion::new(32, 0, 4, 4).clamped(32, 24), None);
        assert_eq!(PixelRegion::new(0, 24, 4, 4).clamped(32, 24), None);
        assert_eq!(PixelRegion::new(0, 0, 0, 4).clamped(32, 24), None);
        // an origin beyond the image collapses to an empty region
        assert_eq!(PixelRegion::new(40, 40, 4, 4).clamped(32, 24), None);
    }
}
