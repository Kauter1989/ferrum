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
    /// What could not be shown as stored (several images per position,
    /// irregular or assumed spacing), in plain words without identifiers.
    pub warnings: Vec<String>,
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

/// Slice spacing from the header, and where it came from.
fn fallback_spacing(headers: &[SliceHeader]) -> (f64, &'static str) {
    headers
        .iter()
        .find_map(|h| {
            h.spacing_between_slices
                .map(|s| (s, "Spacing Between Slices"))
                .or(h.slice_thickness.map(|s| (s, "Slice Thickness")))
        })
        .unwrap_or((1.0, "no header field (1 mm assumed)"))
}

/// Order of two images at the same slice position: the stack key, then
/// Instance Number, then file order (missing values last).
fn stack_order(headers: &[SliceHeader], a: usize, b: usize) -> std::cmp::Ordering {
    let (ha, hb) = (&headers[a], &headers[b]);
    ha.stack
        .order(&hb.stack)
        .then(ha.instance_number.unwrap_or(i64::MAX).cmp(&hb.instance_number.unwrap_or(i64::MAX)))
        .then(a.cmp(&b))
}

/// Largest deviation of a gap from the median gap that still counts as
/// regular: above the rounding noise of Image Position (Patient), far below
/// a missing slice.
fn gap_tolerance(median: f64) -> f64 {
    (0.02 * median).max(0.01)
}

/// Warns when the gaps between neighbouring positions are not all the
/// median gap.
fn irregular_gaps(gaps: &[f64], spacing: f64) -> Option<String> {
    let tol = gap_tolerance(spacing);
    if gaps.iter().all(|g| (g - spacing).abs() <= tol) {
        return None;
    }
    let lo = gaps.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = gaps.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some(format!(
        "slice spacing is irregular: gaps between neighbouring slices range from {lo:.3} to {hi:.3} mm; \
         the volume uses the median {spacing:.3} mm, so distances and volumes across slices may be off"
    ))
}

/// Warns when positions hold several images.
fn several_per_position(sizes: &[usize]) -> Option<String> {
    let lo = sizes.iter().copied().min().unwrap_or(1);
    let hi = sizes.iter().copied().max().unwrap_or(1);
    if hi <= 1 {
        return None;
    }
    let what = "echoes, b-values, phases or frames";
    let n = sizes.len();
    Some(if lo == hi {
        format!(
            "the series holds {hi} images at each of its {n} slice positions ({what}); \
             one volume of {hi} is shown: the first image at every position"
        )
    } else {
        format!(
            "the series holds {lo}–{hi} images per slice position at its {n} positions ({what}); \
             only the first image at every position is shown"
        )
    })
}

/// Orders planes by their projection on the slice normal, one plane per
/// position. `None` when fewer than two distinct positions exist.
fn order_by_position(headers: &[SliceHeader], n: DVec3) -> Option<OrderedSeries> {
    let mut keyed: Vec<(f64, usize)> =
        headers.iter().enumerate().map(|(i, h)| (h.position.unwrap_or(DVec3::ZERO).dot(n), i)).collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| stack_order(headers, a.1, b.1)));
    // One group per position: the first image of each group is shown.
    let mut firsts: Vec<(f64, usize)> = Vec::new();
    let mut sizes: Vec<usize> = Vec::new();
    for &(d, i) in &keyed {
        match firsts.last() {
            Some(&(f, _)) if (d - f).abs() < 1e-4 => {
                if let Some(s) = sizes.last_mut() {
                    *s += 1;
                }
            }
            _ => {
                firsts.push((d, i));
                sizes.push(1);
            }
        }
    }
    if firsts.len() < 2 {
        return None;
    }
    let gaps: Vec<f64> = firsts.windows(2).map(|w| w[1].0 - w[0].0).collect();
    let median_gap = median(gaps.clone()).filter(|s| *s > 1e-4);
    let spacing = median_gap.unwrap_or_else(|| fallback_spacing(headers).0);
    let warnings = several_per_position(&sizes).into_iter().chain(irregular_gaps(&gaps, spacing)).collect();
    Some(OrderedSeries {
        planes: firsts.into_iter().map(|(_, i)| PlaneRef { header: i, frame: 0 }).collect(),
        slice_spacing: spacing,
        method: OrderingMethod::Position,
        warnings,
    })
}

/// Orders the frames of one multi-frame (or single-frame) file.
fn order_frames(headers: &[SliceHeader]) -> OrderedSeries {
    let h = &headers[0];
    let (spacing, source) = fallback_spacing(headers);
    let warnings = if h.frames > 1 {
        vec![format!(
            "multi-frame file: per-frame positions are not read, so the frames are stacked in file order \
             with a slice spacing of {spacing:.3} mm from {source}"
        )]
    } else {
        Vec::new()
    };
    OrderedSeries {
        planes: (0..h.frames).map(|f| PlaneRef { header: 0, frame: f }).collect(),
        slice_spacing: spacing,
        method: OrderingMethod::FileOrder,
        warnings,
    }
}

/// Orders single-frame slices without usable positions: by Instance
/// Number, Slice Location or file order.
fn order_without_positions(headers: &[SliceHeader]) -> OrderedSeries {
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
    let from_locations = (method == OrderingMethod::SliceLocation)
        .then(|| median(keyed.windows(2).map(|w| w[1].0 - w[0].0).collect()))
        .flatten()
        .filter(|s| *s > 1e-4);
    let (spacing, source) = from_locations.map_or_else(|| fallback_spacing(headers), |s| (s, "Slice Location"));
    let by = match method {
        OrderingMethod::InstanceNumber => "Instance Number",
        OrderingMethod::SliceLocation => "Slice Location",
        _ => "file order",
    };
    let warnings = vec![format!(
        "the slices carry no usable image positions: they are ordered by {by}, with a slice spacing of \
         {spacing:.3} mm from {source}; distances across slices may be off"
    )];
    OrderedSeries {
        planes: keyed.into_iter().map(|(_, i)| PlaneRef { header: i, frame: 0 }).collect(),
        slice_spacing: spacing,
        method,
        warnings,
    }
}

/// Orders the slices of one series. Uses the projection of Image Position
/// (Patient) on the slice normal when available, then Instance Number,
/// Slice Location, and finally file order. Several images at one position
/// give one plane (the first by [`stack_order`]); what could not be shown
/// as stored is listed in [`OrderedSeries::warnings`].
pub fn order(headers: &[SliceHeader]) -> OrderedSeries {
    // Multi-frame (or single file): frames in file order.
    if headers.len() == 1 {
        return order_frames(headers);
    }
    let mut ordered = order_files(headers);
    let multi = headers.iter().filter(|h| h.frames > 1).count();
    if multi > 0 {
        ordered.warnings.push(format!(
            "{multi} of the series' {} files hold several frames; only the first frame of each file is read",
            headers.len()
        ));
    }
    ordered
}

/// Orders several files by their first frame.
fn order_files(headers: &[SliceHeader]) -> OrderedSeries {
    let normal = headers.iter().find_map(SliceHeader::normal);
    let all_positions = headers.iter().all(|h| h.position.is_some());
    if let (Some(n), true) = (normal, all_positions) {
        if let Some(ordered) = order_by_position(headers, n) {
            return ordered;
        }
        // Every image at one position (e.g. a cine series): stacked as
        // slices, which they are not.
        let mut ordered = order_without_positions(headers);
        ordered.warnings = vec![format!(
            "the series holds {} images at a single slice position (phases or frames); they are stacked as \
             slices in {} order, and the slice axis is not a distance",
            headers.len(),
            match ordered.method {
                OrderingMethod::InstanceNumber => "Instance Number",
                OrderingMethod::SliceLocation => "Slice Location",
                _ => "file",
            }
        )];
        return ordered;
    }
    order_without_positions(headers)
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
            stack: Default::default(),
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
        let o = order(&hs);
        assert_eq!(o.planes.len(), 2);
        assert_eq!(o.warnings.len(), 1, "{:?}", o.warnings);
        assert!(o.warnings[0].contains("1–2 images per slice position at its 2 positions"), "{:?}", o.warnings);
    }

    /// Two echoes at each of four positions, in reverse order. Echo `e`
    /// at position `k` has Instance Number `2k + e + 1` (interleaved).
    fn two_echoes(with_echo_numbers: bool) -> Vec<SliceHeader> {
        let mut hs: Vec<SliceHeader> = (0..4)
            .flat_map(|k| (0..2).map(move |e| (k, e)))
            .map(|(k, e)| {
                let mut h = header("mr", Some(f64::from(k) * 3.0), Some(i64::from(2 * k + e + 1)));
                if with_echo_numbers {
                    h.stack.echo_number = Some(i64::from(e + 1));
                    // Instance numbers that disagree with the echo order.
                    h.instance_number = Some(i64::from(10 - 2 * k - e));
                }
                h
            })
            .collect();
        // Reversed: echo 2 comes before echo 1 at every position, and the
        // positions run downwards.
        hs.reverse();
        hs
    }

    #[test]
    fn several_echoes_give_one_volume_never_a_mix() {
        // covers 2.7-a, 2.7-b
        for with_echo_numbers in [true, false] {
            let hs = two_echoes(with_echo_numbers);
            let o = order(&hs);
            assert_eq!(o.planes.len(), 4);
            assert_eq!(o.method, OrderingMethod::Position);
            assert!((o.slice_spacing - 3.0).abs() < 1e-9);
            for (k, p) in o.planes.iter().enumerate() {
                let h = &hs[p.header];
                assert_eq!(h.position.unwrap().z, k as f64 * 3.0);
                if with_echo_numbers {
                    assert_eq!(h.stack.echo_number, Some(1), "plane {k}");
                } else {
                    assert_eq!(h.instance_number, Some(2 * k as i64 + 1), "plane {k}");
                }
            }
            assert_eq!(o.warnings.len(), 1, "{:?}", o.warnings);
            assert!(o.warnings[0].contains("2 images at each of its 4 slice positions"), "{:?}", o.warnings);
            assert!(o.warnings[0].contains("one volume of 2 is shown"), "{:?}", o.warnings);
        }
    }

    #[test]
    fn the_stack_key_decides_before_the_instance_number() {
        // covers 2.7-a
        let mk = |z: f64, phase: Option<i64>, b: Option<f64>, inst: i64| {
            let mut h = header("dwi", Some(z), Some(inst));
            h.stack.temporal_position = phase;
            h.stack.b_value = b;
            h
        };
        let hs = vec![
            mk(0.0, Some(2), Some(0.0), 1),
            mk(0.0, Some(1), Some(1000.0), 2),
            mk(0.0, Some(1), Some(0.0), 3),
            mk(0.0, None, Some(0.0), 0),
            mk(1.0, Some(1), Some(0.0), 9),
            mk(1.0, Some(1), None, 4),
        ];
        let o = order(&hs);
        assert_eq!(o.planes.iter().map(|p| p.header).collect::<Vec<_>>(), vec![2, 4]);
    }

    #[test]
    fn regular_series_load_without_warnings() {
        // covers 2.7-c
        for spacing in [1.0, 5.0, 0.6] {
            // Image positions are rounded in DICOM: a little noise is regular.
            let hs: Vec<_> = (0..20)
                .map(|k| header("a", Some(f64::from(k) * spacing + if k % 3 == 0 { 0.004 } else { 0.0 }), None))
                .collect();
            let o = order(&hs);
            assert!(o.warnings.is_empty(), "{spacing}: {:?}", o.warnings);
        }
        let mk = |x: f64| {
            let mut h = header("s", Some(0.0), None);
            h.position = Some(DVec3::new(x, 10.0, 20.0) + DVec3::new(0.0, 0.3, 0.4) * x);
            h.orientation = Some((DVec3::Y, DVec3::Z));
            h
        };
        let o = order(&(0..6).map(|k| mk(f64::from(k) * 1.2)).collect::<Vec<_>>());
        assert!(o.warnings.is_empty(), "{:?}", o.warnings);
    }

    #[test]
    fn a_missing_slice_is_reported() {
        // covers 2.7-c
        let hs: Vec<_> = [0.0, 2.0, 4.0, 8.0, 10.0].iter().map(|&z| header("a", Some(z), None)).collect();
        let o = order(&hs);
        assert_eq!(o.planes.len(), 5);
        assert_eq!(o.slice_spacing, 2.0);
        assert_eq!(o.warnings.len(), 1, "{:?}", o.warnings);
        assert!(o.warnings[0].contains("range from 2.000 to 4.000 mm"), "{:?}", o.warnings);
        assert!(o.warnings[0].contains("median 2.000 mm"), "{:?}", o.warnings);
        // 3 % off the median is irregular, 1 % is not.
        let off = |d: f64| order(&[0.0, 2.0, 4.0 + d, 6.0 + d].map(|z| header("a", Some(z), None))).warnings.len();
        assert_eq!(off(0.06), 1);
        assert_eq!(off(0.02), 0);
    }

    #[test]
    fn one_position_with_many_images_is_not_a_volume() {
        // covers 2.7-b
        let hs: Vec<_> = (1..=5).map(|i| header("cine", Some(7.0), Some(i))).collect();
        let o = order(&hs);
        assert_eq!(o.planes.len(), 5);
        assert_eq!(o.warnings.len(), 1, "{:?}", o.warnings);
        assert!(o.warnings[0].contains("5 images at a single slice position"), "{:?}", o.warnings);
        assert!(o.warnings[0].contains("not a distance"), "{:?}", o.warnings);
    }

    #[test]
    fn falls_back_to_instance_number_and_thickness() {
        let hs = vec![header("a", None, Some(3)), header("a", None, Some(1)), header("a", None, Some(2))];
        let o = order(&hs);
        assert_eq!(o.method, OrderingMethod::InstanceNumber);
        let inst: Vec<i64> = o.planes.iter().map(|p| hs[p.header].instance_number.unwrap()).collect();
        assert_eq!(inst, vec![1, 2, 3]);
        assert_eq!(o.slice_spacing, 2.5);
        // covers 2.7-d
        assert_eq!(o.warnings.len(), 1, "{:?}", o.warnings);
        assert!(o.warnings[0].contains("ordered by Instance Number"), "{:?}", o.warnings);
        assert!(o.warnings[0].contains("2.500 mm from Slice Thickness"), "{:?}", o.warnings);
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
        assert!(o.warnings[0].contains("3.000 mm from Slice Location"), "{:?}", o.warnings);
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
        // covers 2.7-d
        assert_eq!(o.warnings.len(), 1, "{:?}", o.warnings);
        assert!(o.warnings[0].contains("0.700 mm from Spacing Between Slices"), "{:?}", o.warnings);
        let mut single = header("m", None, None);
        single.frames = 1;
        assert!(order(&[single]).warnings.is_empty(), "one image is no stack");
    }

    #[test]
    fn several_multiframe_files_say_that_only_first_frames_are_read() {
        // covers 2.7-d
        let mut hs: Vec<_> = (0..3).map(|k| header("e", Some(f64::from(k)), None)).collect();
        hs[1].frames = 4;
        hs[2].frames = 4;
        let o = order(&hs);
        assert_eq!(o.planes.len(), 3);
        assert_eq!(
            o.warnings,
            vec!["2 of the series' 3 files hold several frames; only the first frame of each file is read"]
        );
    }

    #[test]
    fn missing_everything_uses_file_order() {
        let o = order(&[header("a", None, None), header("a", None, None)]);
        assert_eq!(o.method, OrderingMethod::FileOrder);
        assert_eq!(o.slice_spacing, 2.5);
        assert!(o.warnings[0].contains("ordered by file order"), "{:?}", o.warnings);
    }
}
