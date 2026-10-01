//! Grouping of slice headers into series and ordering of slices in space.
//!
//! This module is pure: it operates on [`SliceHeader`] values only and is
//! tested without touching the file system.

use std::collections::BTreeMap;

use glam::DVec3;

use super::header::SliceHeader;

/// Key identifying slices that can be stacked into one volume: same series,
/// same in-plane size and (approximately) the same orientation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SeriesKey {
    /// Series Instance UID.
    pub series_uid: String,
    /// Rows × columns.
    pub size: (u32, u32),
    /// Orientation rounded to 1e-3.
    pub orientation: Option<[i32; 6]>,
}

impl SeriesKey {
    /// Computes the key of a header.
    pub fn of(h: &SliceHeader) -> Self {
        let q = |v: f64| (v * 1000.0).round() as i32;
        Self {
            series_uid: h.series_uid.clone(),
            size: (h.rows, h.columns),
            orientation: h.orientation.map(|(r, c)| [q(r.x), q(r.y), q(r.z), q(c.x), q(c.y), q(c.z)]),
        }
    }

    /// A stable textual id.
    pub fn id(&self) -> String {
        match self.orientation {
            Some(o) => format!("{}#{}x{}#{:?}", self.series_uid, self.size.1, self.size.0, o),
            None => format!("{}#{}x{}", self.series_uid, self.size.1, self.size.0),
        }
    }
}

/// Groups headers into series. The result is sorted by key for determinism.
pub fn group(headers: Vec<SliceHeader>) -> Vec<(SeriesKey, Vec<SliceHeader>)> {
    let mut map: BTreeMap<SeriesKey, Vec<SliceHeader>> = BTreeMap::new();
    for h in headers {
        map.entry(SeriesKey::of(&h)).or_default().push(h);
    }
    map.into_iter().collect()
}

/// One image plane of a series (a single-frame file or one frame of a
/// multi-frame file).
#[derive(Debug, Clone, PartialEq)]
pub struct PlaneRef {
    /// Index into the series' header list.
    pub header: usize,
    /// Frame number within the file.
    pub frame: u32,
}

/// Slices of a series sorted along the stacking axis.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderedSeries {
    /// Planes in ascending order along the slice normal.
    pub planes: Vec<PlaneRef>,
    /// Distance between consecutive planes in mm.
    pub slice_spacing: f64,
    /// How the order was determined.
    pub method: OrderingMethod,
}

/// Strategy used to order slices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderingMethod {
    /// Projection of Image Position (Patient) onto the plane normal.
    Position,
    /// Instance Number.
    InstanceNumber,
    /// Slice Location.
    SliceLocation,
    /// File order (frames of a multi-frame file).
    FileOrder,
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

fn fallback_spacing(headers: &[SliceHeader]) -> f64 {
    headers.iter().find_map(|h| h.spacing_between_slices.or(h.slice_thickness)).unwrap_or(1.0)
}

/// Orders the slices of one series.
///
/// Preference: position along the normal → Instance Number → Slice
/// Location → file order. Planes at duplicate positions are dropped (they
/// usually belong to a repeated acquisition).
pub fn order(headers: &[SliceHeader]) -> OrderedSeries {
    // Multi-frame (or single file): frames in file order.
    if headers.len() == 1 {
        let h = &headers[0];
        return OrderedSeries {
            planes: (0..h.frames).map(|f| PlaneRef { header: 0, frame: f }).collect(),
            slice_spacing: fallback_spacing(headers),
            method: OrderingMethod::FileOrder,
        };
    }

    let normal = headers.iter().find_map(SliceHeader::normal);
    let all_positions = headers.iter().all(|h| h.position.is_some());
    if let (Some(n), true) = (normal, all_positions) {
        let mut keyed: Vec<(f64, usize)> =
            headers.iter().enumerate().map(|(i, h)| (h.position.unwrap_or(DVec3::ZERO).dot(n), i)).collect();
        keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        keyed.dedup_by(|b, a| (b.0 - a.0).abs() < 1e-4);
        if keyed.len() > 1 {
            let gaps: Vec<f64> = keyed.windows(2).map(|w| w[1].0 - w[0].0).collect();
            let spacing = median(gaps).filter(|s| *s > 1e-4).unwrap_or_else(|| fallback_spacing(headers));
            return OrderedSeries {
                planes: keyed.into_iter().map(|(_, i)| PlaneRef { header: i, frame: 0 }).collect(),
                slice_spacing: spacing,
                method: OrderingMethod::Position,
            };
        }
    }

    let (method, mut keyed): (OrderingMethod, Vec<(f64, usize)>) =
        if headers.iter().all(|h| h.instance_number.is_some()) {
            (
                OrderingMethod::InstanceNumber,
                headers.iter().enumerate().map(|(i, h)| (h.instance_number.unwrap_or(0) as f64, i)).collect(),
            )
        } else if headers.iter().all(|h| h.slice_location.is_some()) {
            (
                OrderingMethod::SliceLocation,
                headers.iter().enumerate().map(|(i, h)| (h.slice_location.unwrap_or(0.0), i)).collect(),
            )
        } else {
            (OrderingMethod::FileOrder, headers.iter().enumerate().map(|(i, _)| (i as f64, i)).collect())
        };
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let spacing = if method == OrderingMethod::SliceLocation {
        median(keyed.windows(2).map(|w| w[1].0 - w[0].0).collect())
            .filter(|s| *s > 1e-4)
            .unwrap_or_else(|| fallback_spacing(headers))
    } else {
        fallback_spacing(headers)
    };
    OrderedSeries {
        planes: keyed.into_iter().map(|(_, i)| PlaneRef { header: i, frame: 0 }).collect(),
        slice_spacing: spacing,
        method,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::dicom::header::Photometric;
    use std::path::PathBuf;

    pub(crate) fn header(uid: &str, z: Option<f64>, inst: Option<i64>) -> SliceHeader {
        SliceHeader {
            path: PathBuf::from(format!("{uid}-{z:?}-{inst:?}.dcm")),
            series_uid: uid.into(),
            series_description: String::new(),
            modality: "CT".into(),
            rows: 4,
            columns: 4,
            frames: 1,
            pixel_spacing: None,
            slice_thickness: Some(2.5),
            spacing_between_slices: None,
            position: z.map(|z| DVec3::new(0.0, 0.0, z)),
            orientation: z.map(|_| (DVec3::X, DVec3::Y)),
            instance_number: inst,
            slice_location: None,
            window: None,
            photometric: Photometric::Monochrome2,
        }
    }

    #[test]
    fn groups_by_uid_and_geometry() {
        let mut other = header("a", Some(1.0), None);
        other.rows = 8;
        let g = group(vec![
            header("b", Some(0.0), None),
            header("a", Some(0.0), None),
            other,
            header("a", Some(1.0), None),
        ]);
        assert_eq!(g.len(), 3);
        assert_eq!(g[0].0.series_uid, "a");
        assert_eq!(g[0].1.len(), 2);
        assert_ne!(g[0].0.id(), g[1].0.id());
    }

    #[test]
    fn orders_by_position_with_median_spacing() {
        let hs: Vec<_> = [3.0, 0.0, 1.5, 4.5].iter().map(|&z| header("a", Some(z), None)).collect();
        let o = order(&hs);
        assert_eq!(o.method, OrderingMethod::Position);
        let zs: Vec<f64> = o.planes.iter().map(|p| hs[p.header].position.unwrap().z).collect();
        assert_eq!(zs, vec![0.0, 1.5, 3.0, 4.5]);
        assert!((o.slice_spacing - 1.5).abs() < 1e-9);
    }

    #[test]
    fn orders_along_oblique_normal() {
        // Sagittal acquisition: rows along y, columns along z -> normal along x.
        let mk = |x: f64| {
            let mut h = header("s", Some(0.0), None);
            h.position = Some(DVec3::new(x, 10.0, 20.0));
            h.orientation = Some((DVec3::Y, DVec3::Z));
            h
        };
        let hs = vec![mk(2.0), mk(-2.0), mk(0.0)];
        let o = order(&hs);
        let xs: Vec<f64> = o.planes.iter().map(|p| hs[p.header].position.unwrap().x).collect();
        assert_eq!(xs, vec![-2.0, 0.0, 2.0]);
        assert!((o.slice_spacing - 2.0).abs() < 1e-9);
    }

    #[test]
    fn duplicate_positions_are_dropped() {
        let hs = vec![header("a", Some(0.0), None), header("a", Some(0.0), None), header("a", Some(1.0), None)];
        assert_eq!(order(&hs).planes.len(), 2);
    }

    #[test]
    fn falls_back_to_instance_number_and_thickness() {
        let hs = vec![header("a", None, Some(3)), header("a", None, Some(1)), header("a", None, Some(2))];
        let o = order(&hs);
        assert_eq!(o.method, OrderingMethod::InstanceNumber);
        let inst: Vec<i64> = o.planes.iter().map(|p| hs[p.header].instance_number.unwrap()).collect();
        assert_eq!(inst, vec![1, 2, 3]);
        assert_eq!(o.slice_spacing, 2.5);
    }

    #[test]
    fn falls_back_to_slice_location() {
        let mut hs = vec![header("a", None, None), header("a", None, None)];
        hs[0].slice_location = Some(10.0);
        hs[1].slice_location = Some(7.0);
        let o = order(&hs);
        assert_eq!(o.method, OrderingMethod::SliceLocation);
        assert_eq!(o.planes[0].header, 1);
        assert!((o.slice_spacing - 3.0).abs() < 1e-9);
    }

    #[test]
    fn multiframe_single_file_uses_frames() {
        let mut h = header("m", None, None);
        h.frames = 5;
        h.spacing_between_slices = Some(0.7);
        let o = order(&[h]);
        assert_eq!(o.method, OrderingMethod::FileOrder);
        assert_eq!(o.planes.len(), 5);
        assert_eq!(o.planes[4].frame, 4);
        assert_eq!(o.slice_spacing, 0.7);
    }

    #[test]
    fn missing_everything_uses_file_order() {
        let o = order(&[header("a", None, None), header("a", None, None)]);
        assert_eq!(o.method, OrderingMethod::FileOrder);
        assert_eq!(o.slice_spacing, 2.5);
    }
}
