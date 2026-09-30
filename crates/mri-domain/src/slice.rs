//! Planar (2D) slices through a volume and the viewport mapping used to
//! display them.

use glam::{UVec3, Vec2, Vec3};

use crate::volume::Volume;

/// Orientation of a 2D slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SliceAxis {
    /// Plane of constant column index `i` (YZ plane).
    Sagittal,
    /// Plane of constant row index `j` (XZ plane).
    Coronal,
    /// Plane of constant slice index `k` (XY plane) — the acquisition plane.
    #[default]
    Axial,
}

impl SliceAxis {
    /// All orientations in display order.
    pub const ALL: [SliceAxis; 3] = [SliceAxis::Sagittal, SliceAxis::Coronal, SliceAxis::Axial];

    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            SliceAxis::Sagittal => "Sagittal",
            SliceAxis::Coronal => "Coronal",
            SliceAxis::Axial => "Axial",
        }
    }

    /// Index of the volume axis normal to the plane (0 = x, 1 = y, 2 = z).
    pub fn normal_axis(&self) -> usize {
        match self {
            SliceAxis::Sagittal => 0,
            SliceAxis::Coronal => 1,
            SliceAxis::Axial => 2,
        }
    }

    /// Stable numeric id, used by the shader.
    pub fn id(&self) -> u32 {
        self.normal_axis() as u32
    }

    /// Number of slices along the normal.
    pub fn slice_count(&self, volume: &Volume) -> u32 {
        volume.dims().as_uvec3()[self.normal_axis()]
    }

    /// `(horizontal, vertical)` volume axes shown on screen.
    fn plane_axes(&self) -> (usize, usize) {
        match self {
            SliceAxis::Sagittal => (1, 2),
            SliceAxis::Coronal => (0, 2),
            SliceAxis::Axial => (0, 1),
        }
    }

    /// `true` if the vertical screen axis runs against the volume axis
    /// (so that the head is at the top of coronal/sagittal images).
    fn vertical_flipped(&self) -> bool {
        !matches!(self, SliceAxis::Axial)
    }

    /// In-plane size in voxels `(width, height)`.
    pub fn plane_dims(&self, volume: &Volume) -> (u32, u32) {
        let d = volume.dims().as_uvec3();
        let (h, v) = self.plane_axes();
        (d[h], d[v])
    }

    /// In-plane pixel spacing in millimetres `(horizontal, vertical)`.
    pub fn plane_spacing(&self, volume: &Volume) -> Vec2 {
        let s = volume.spacing();
        let (h, v) = self.plane_axes();
        Vec2::new(s[h], s[v])
    }

    /// In-plane physical size in millimetres.
    pub fn plane_size_mm(&self, volume: &Volume) -> Vec2 {
        let (w, h) = self.plane_dims(volume);
        Vec2::new(w as f32, h as f32) * self.plane_spacing(volume)
    }

    /// Maps view coordinates `uv ∈ [0, 1]²` (`u` right, `v` down) and the
    /// normalised slice position `s` to a 3D texture coordinate.
    pub fn tex_coord(&self, uv: Vec2, s: f32) -> Vec3 {
        let (h, v) = self.plane_axes();
        let mut t = Vec3::ZERO;
        t[self.normal_axis()] = s;
        t[h] = uv.x;
        t[v] = if self.vertical_flipped() { 1.0 - uv.y } else { uv.y };
        t
    }

    /// Inverse of [`SliceAxis::tex_coord`] for the in-plane components:
    /// view coordinates of the projection of texture point `t`.
    pub fn uv_of_tex(&self, t: Vec3) -> Vec2 {
        let (h, v) = self.plane_axes();
        Vec2::new(t[h], if self.vertical_flipped() { 1.0 - t[v] } else { t[v] })
    }

    /// Volume axis shown horizontally (`0`) or vertically (`1`) on screen.
    pub fn screen_axis(&self, which: usize) -> usize {
        let (h, v) = self.plane_axes();
        if which == 0 {
            h
        } else {
            v
        }
    }

    /// Normalised position of the centre of slice `index`.
    pub fn slice_position(&self, volume: &Volume, index: u32) -> f32 {
        (index as f32 + 0.5) / self.slice_count(volume) as f32
    }

    /// Voxel shown at pixel `(px, py)` of slice `index` (pixel `(0, 0)` is
    /// the top-left corner of the displayed image).
    pub fn voxel_at(&self, volume: &Volume, index: u32, px: u32, py: u32) -> Option<UVec3> {
        let (w, hgt) = self.plane_dims(volume);
        if px >= w || py >= hgt || index >= self.slice_count(volume) {
            return None;
        }
        let (h, v) = self.plane_axes();
        let mut vox = UVec3::ZERO;
        vox[self.normal_axis()] = index;
        vox[h] = px;
        vox[v] = if self.vertical_flipped() { hgt - 1 - py } else { py };
        Some(vox)
    }
}

/// A 2D image extracted from a volume (normalised `u16` samples, row-major,
/// top row first).
#[derive(Debug, Clone, PartialEq)]
pub struct SliceImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel spacing in millimetres.
    pub spacing: Vec2,
    /// Samples, `width * height` elements.
    pub pixels: Vec<u16>,
}

impl SliceImage {
    /// Extracts slice `index` along `axis` at native resolution.
    ///
    /// Axial slices are contiguous in memory and are copied row by row; the
    /// other orientations gather with a stride.
    pub fn extract(volume: &Volume, axis: SliceAxis, index: u32) -> Option<Self> {
        if index >= axis.slice_count(volume) {
            return None;
        }
        let (w, h) = axis.plane_dims(volume);
        let dims = volume.dims();
        let data = volume.data();
        let pixels = match axis {
            SliceAxis::Axial => {
                let start = dims.index(0, 0, index);
                data[start..start + dims.slice_len()].to_vec()
            }
            _ => {
                let mut out = Vec::with_capacity(w as usize * h as usize);
                for py in 0..h {
                    for px in 0..w {
                        // voxel_at cannot fail for in-range px/py/index.
                        let v = axis.voxel_at(volume, index, px, py).unwrap_or(UVec3::ZERO);
                        out.push(data[dims.index(v.x, v.y, v.z)]);
                    }
                }
                out
            }
        };
        Some(Self { width: w, height: h, spacing: axis.plane_spacing(volume), pixels })
    }
}

/// Pan / zoom state of a 2D slice viewport together with the math that maps
/// between screen points and in-plane millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliceView {
    /// Magnification relative to "fit to viewport" (`1.0`).
    pub zoom: f32,
    /// Offset of the image centre from the viewport centre, in points.
    pub pan: Vec2,
}

impl Default for SliceView {
    fn default() -> Self {
        Self { zoom: 1.0, pan: Vec2::ZERO }
    }
}

impl SliceView {
    /// Minimum zoom.
    pub const MIN_ZOOM: f32 = 0.25;
    /// Maximum zoom.
    pub const MAX_ZOOM: f32 = 32.0;

    /// Screen points per millimetre for an image of `image_mm` shown in a
    /// viewport of `viewport` points.
    pub fn scale(&self, viewport: Vec2, image_mm: Vec2) -> f32 {
        let fit = (viewport.x / image_mm.x).min(viewport.y / image_mm.y);
        fit * self.zoom
    }

    /// Screen rectangle `(min, max)` covered by the image, relative to the
    /// viewport's top-left corner.
    pub fn image_rect(&self, viewport: Vec2, image_mm: Vec2) -> (Vec2, Vec2) {
        let half = image_mm * self.scale(viewport, image_mm) * 0.5;
        let c = viewport * 0.5 + self.pan;
        (c - half, c + half)
    }

    /// Screen point → in-plane millimetres from the image's top-left corner.
    pub fn screen_to_mm(&self, p: Vec2, viewport: Vec2, image_mm: Vec2) -> Vec2 {
        let (min, _) = self.image_rect(viewport, image_mm);
        (p - min) / self.scale(viewport, image_mm)
    }

    /// In-plane millimetres → screen point.
    pub fn mm_to_screen(&self, mm: Vec2, viewport: Vec2, image_mm: Vec2) -> Vec2 {
        let (min, _) = self.image_rect(viewport, image_mm);
        min + mm * self.scale(viewport, image_mm)
    }

    /// Zooms by `factor` keeping the image point under `anchor` fixed.
    pub fn zoom_about(&mut self, factor: f32, anchor: Vec2, viewport: Vec2, image_mm: Vec2) {
        let before = self.screen_to_mm(anchor, viewport, image_mm);
        self.zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        let after = self.mm_to_screen(before, viewport, image_mm);
        self.pan += anchor - after;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Dims3;
    use crate::volume::IntensityRange;

    fn indexed_volume() -> Volume {
        // value = linear index so extraction can be verified exactly
        let dims = Dims3::new(4, 3, 2);
        let data: Vec<u16> = (0..dims.voxel_count() as u16).collect();
        Volume::new(dims, Vec3::new(1.0, 2.0, 3.0), IntensityRange::new(0.0, 1.0).unwrap(), data).unwrap()
    }

    #[test]
    fn plane_dims_and_spacing() {
        let v = indexed_volume();
        assert_eq!(SliceAxis::Axial.plane_dims(&v), (4, 3));
        assert_eq!(SliceAxis::Coronal.plane_dims(&v), (4, 2));
        assert_eq!(SliceAxis::Sagittal.plane_dims(&v), (3, 2));
        assert_eq!(SliceAxis::Sagittal.plane_spacing(&v), Vec2::new(2.0, 3.0));
        assert_eq!(SliceAxis::Axial.slice_count(&v), 2);
        assert_eq!(SliceAxis::Sagittal.plane_size_mm(&v), Vec2::new(6.0, 6.0));
    }

    #[test]
    fn axial_extraction_is_contiguous_copy() {
        let v = indexed_volume();
        let img = SliceImage::extract(&v, SliceAxis::Axial, 1).unwrap();
        assert_eq!(img.pixels, (12..24).collect::<Vec<u16>>());
    }

    #[test]
    fn coronal_extraction_has_last_slice_on_top() {
        let v = indexed_volume();
        let img = SliceImage::extract(&v, SliceAxis::Coronal, 1).unwrap();
        // top row is k = 1, row j = 1 -> indices 16..20
        assert_eq!(&img.pixels[0..4], &[16, 17, 18, 19]);
        assert_eq!(&img.pixels[4..8], &[4, 5, 6, 7]);
    }

    #[test]
    fn sagittal_extraction() {
        let v = indexed_volume();
        let img = SliceImage::extract(&v, SliceAxis::Sagittal, 2).unwrap();
        // top row k=1: j = 0..3 at i=2 -> 14, 18, 22
        assert_eq!(&img.pixels[0..3], &[14, 18, 22]);
        assert_eq!(&img.pixels[3..6], &[2, 6, 10]);
        assert!(SliceImage::extract(&v, SliceAxis::Sagittal, 4).is_none());
    }

    #[test]
    fn tex_coord_agrees_with_voxel_at() {
        let v = indexed_volume();
        for axis in SliceAxis::ALL {
            let (w, h) = axis.plane_dims(&v);
            for idx in 0..axis.slice_count(&v) {
                for py in 0..h {
                    for px in 0..w {
                        let uv = Vec2::new((px as f32 + 0.5) / w as f32, (py as f32 + 0.5) / h as f32);
                        let t = axis.tex_coord(uv, axis.slice_position(&v, idx));
                        assert_eq!(v.tex_to_voxel(t), axis.voxel_at(&v, idx, px, py), "{axis:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn uv_of_tex_inverts_tex_coord() {
        for axis in SliceAxis::ALL {
            let uv = Vec2::new(0.2, 0.7);
            assert!((axis.uv_of_tex(axis.tex_coord(uv, 0.4)) - uv).length() < 1e-6);
            assert_ne!(axis.screen_axis(0), axis.normal_axis());
            assert_ne!(axis.screen_axis(1), axis.normal_axis());
        }
    }

    #[test]
    fn view_mapping_roundtrip_and_fit() {
        let view = SliceView::default();
        let vp = Vec2::new(800.0, 400.0);
        let img = Vec2::new(200.0, 100.0);
        let (min, max) = view.image_rect(vp, img);
        assert_eq!(min, Vec2::ZERO);
        assert_eq!(max, vp);
        let p = Vec2::new(123.0, 45.0);
        let mm = view.screen_to_mm(p, vp, img);
        assert!((view.mm_to_screen(mm, vp, img) - p).length() < 1e-3);
    }

    #[test]
    fn zoom_about_keeps_anchor_fixed() {
        let mut view = SliceView::default();
        let vp = Vec2::new(500.0, 500.0);
        let img = Vec2::new(250.0, 250.0);
        let anchor = Vec2::new(100.0, 300.0);
        let before = view.screen_to_mm(anchor, vp, img);
        view.zoom_about(2.0, anchor, vp, img);
        assert_eq!(view.zoom, 2.0);
        let after = view.screen_to_mm(anchor, vp, img);
        assert!((before - after).length() < 1e-3);
        view.zoom_about(1000.0, anchor, vp, img);
        assert_eq!(view.zoom, SliceView::MAX_ZOOM);
    }
}
