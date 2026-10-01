//! 2D measurements and text annotations attached to slices.
//!
//! All coordinates are in-plane millimetres measured from the top-left corner
//! of the displayed slice image, so measurements are physically correct for
//! anisotropic voxels.

use std::collections::BTreeMap;

use glam::Vec2;

use crate::slice::SliceAxis;

/// Identifies the slice an annotation belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SliceKey {
    /// Slice orientation id (see [`SliceAxis::id`]).
    pub axis: u32,
    /// Slice index along the orientation's normal.
    pub index: u32,
}

impl SliceKey {
    /// Creates a key.
    pub fn new(axis: SliceAxis, index: u32) -> Self {
        Self { axis: axis.id(), index }
    }
}

/// Unique id of an annotation within an [`AnnotationSet`].
pub type AnnotationId = u64;

/// A measurement or note drawn on a slice.
#[derive(Debug, Clone, PartialEq)]
pub enum Annotation {
    /// Straight-line distance.
    Distance {
        /// Start point.
        a: Vec2,
        /// End point.
        b: Vec2,
    },
    /// Angle at `vertex` between rays to `a` and `b`.
    Angle {
        /// First arm end.
        a: Vec2,
        /// Angle vertex.
        vertex: Vec2,
        /// Second arm end.
        b: Vec2,
    },
    /// Closed polygon (free-form area).
    Polygon {
        /// Vertices in drawing order.
        points: Vec<Vec2>,
    },
    /// Axis-aligned rectangle given by two opposite corners.
    Rect {
        /// First corner.
        a: Vec2,
        /// Opposite corner.
        b: Vec2,
    },
    /// Free text label.
    Text {
        /// Anchor position.
        pos: Vec2,
        /// Label contents.
        text: String,
    },
}

/// Distance from `p` to segment `ab`.
pub fn point_segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 == 0.0 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Signed area of a polygon (shoelace formula), positive for
/// counter-clockwise order in a y-up frame.
pub fn polygon_signed_area(points: &[Vec2]) -> f32 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut acc = 0.0;
    for (i, p) in points.iter().enumerate() {
        let q = points[(i + 1) % points.len()];
        acc += p.x * q.y - q.x * p.y;
    }
    acc * 0.5
}

/// Angle in degrees at `vertex` between `a` and `b` (`0..=180`).
pub fn angle_degrees(a: Vec2, vertex: Vec2, b: Vec2) -> f32 {
    let u = a - vertex;
    let v = b - vertex;
    if u.length_squared() == 0.0 || v.length_squared() == 0.0 {
        return 0.0;
    }
    let cos = (u.dot(v) / (u.length() * v.length())).clamp(-1.0, 1.0);
    cos.acos().to_degrees()
}

impl Annotation {
    /// Primary numeric value: mm, degrees or mm². `None` for text.
    pub fn value(&self) -> Option<f32> {
        match self {
            Annotation::Distance { a, b } => Some(a.distance(*b)),
            Annotation::Angle { a, vertex, b } => Some(angle_degrees(*a, *vertex, *b)),
            Annotation::Polygon { points } => Some(polygon_signed_area(points).abs()),
            Annotation::Rect { a, b } => {
                let d = (*b - *a).abs();
                Some(d.x * d.y)
            }
            Annotation::Text { .. } => None,
        }
    }

    /// Human-readable measurement label.
    pub fn label(&self) -> String {
        match (self, self.value()) {
            (Annotation::Distance { .. }, Some(v)) => format!("{v:.1} mm"),
            (Annotation::Angle { .. }, Some(v)) => format!("{v:.1}°"),
            (Annotation::Polygon { .. } | Annotation::Rect { .. }, Some(v)) => {
                format!("{:.2} cm²", v / 100.0)
            }
            (Annotation::Text { text, .. }, _) => text.clone(),
            _ => String::new(),
        }
    }

    /// All control points (for rendering handles and hit testing).
    pub fn points(&self) -> Vec<Vec2> {
        match self {
            Annotation::Distance { a, b } | Annotation::Rect { a, b } => vec![*a, *b],
            Annotation::Angle { a, vertex, b } => vec![*a, *vertex, *b],
            Annotation::Polygon { points } => points.clone(),
            Annotation::Text { pos, .. } => vec![*pos],
        }
    }

    /// Point where the label is drawn.
    pub fn label_anchor(&self) -> Vec2 {
        match self {
            Annotation::Distance { a, b } | Annotation::Rect { a, b } => (*a + *b) * 0.5,
            Annotation::Angle { vertex, .. } => *vertex,
            Annotation::Polygon { points } => points.iter().copied().sum::<Vec2>() / points.len().max(1) as f32,
            Annotation::Text { pos, .. } => *pos,
        }
    }

    /// Distance from `p` to the annotation's outline.
    pub fn distance_to(&self, p: Vec2) -> f32 {
        let segs = |pts: &[Vec2], closed: bool| -> f32 {
            let n = pts.len();
            if n == 1 {
                return p.distance(pts[0]);
            }
            let count = if closed { n } else { n - 1 };
            (0..count).map(|i| point_segment_distance(p, pts[i], pts[(i + 1) % n])).fold(f32::INFINITY, f32::min)
        };
        match self {
            Annotation::Distance { a, b } => point_segment_distance(p, *a, *b),
            Annotation::Angle { a, vertex, b } => segs(&[*a, *vertex, *b], false),
            Annotation::Polygon { points } if !points.is_empty() => segs(points, true),
            Annotation::Polygon { .. } => f32::INFINITY,
            Annotation::Rect { a, b } => {
                let c = [*a, Vec2::new(b.x, a.y), *b, Vec2::new(a.x, b.y)];
                segs(&c, true)
            }
            Annotation::Text { pos, .. } => p.distance(*pos),
        }
    }

    /// Translates the whole annotation.
    pub fn translate(&mut self, d: Vec2) {
        match self {
            Annotation::Distance { a, b } | Annotation::Rect { a, b } => {
                *a += d;
                *b += d;
            }
            Annotation::Angle { a, vertex, b } => {
                *a += d;
                *vertex += d;
                *b += d;
            }
            Annotation::Polygon { points } => points.iter_mut().for_each(|p| *p += d),
            Annotation::Text { pos, .. } => *pos += d,
        }
    }
}

/// Collection of annotations indexed by slice.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnnotationSet {
    next_id: AnnotationId,
    items: BTreeMap<AnnotationId, (SliceKey, Annotation)>,
}

impl AnnotationSet {
    /// Adds an annotation and returns its id.
    pub fn add(&mut self, key: SliceKey, annotation: Annotation) -> AnnotationId {
        let id = self.next_id;
        self.next_id += 1;
        self.items.insert(id, (key, annotation));
        id
    }

    /// Removes an annotation; returns it if it existed.
    pub fn remove(&mut self, id: AnnotationId) -> Option<Annotation> {
        self.items.remove(&id).map(|(_, a)| a)
    }

    /// Mutable access to an annotation.
    pub fn get_mut(&mut self, id: AnnotationId) -> Option<&mut Annotation> {
        self.items.get_mut(&id).map(|(_, a)| a)
    }

    /// Immutable access to an annotation.
    pub fn get(&self, id: AnnotationId) -> Option<&Annotation> {
        self.items.get(&id).map(|(_, a)| a)
    }

    /// Annotations on the given slice.
    pub fn on_slice(&self, key: SliceKey) -> impl Iterator<Item = (AnnotationId, &Annotation)> {
        self.items.iter().filter(move |(_, (k, _))| *k == key).map(|(id, (_, a))| (*id, a))
    }

    /// Nearest annotation on `key` within `tolerance` mm of `p`.
    pub fn hit_test(&self, key: SliceKey, p: Vec2, tolerance: f32) -> Option<AnnotationId> {
        self.on_slice(key)
            .map(|(id, a)| (id, a.distance_to(p)))
            .filter(|(_, d)| *d <= tolerance)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// Removes everything.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Total number of annotations.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// `true` if there are no annotations.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_value_and_label() {
        let a = Annotation::Distance { a: Vec2::ZERO, b: Vec2::new(3.0, 4.0) };
        assert_eq!(a.value(), Some(5.0));
        assert_eq!(a.label(), "5.0 mm");
    }

    #[test]
    fn right_angle() {
        let a = Annotation::Angle { a: Vec2::X, vertex: Vec2::ZERO, b: Vec2::Y };
        assert!((a.value().unwrap() - 90.0).abs() < 1e-4);
        assert_eq!(angle_degrees(Vec2::X, Vec2::ZERO, Vec2::ZERO), 0.0);
        assert!((angle_degrees(Vec2::X, Vec2::ZERO, -Vec2::X) - 180.0).abs() < 1e-4);
    }

    #[test]
    fn polygon_area_square_and_orientation_independent() {
        let sq = vec![Vec2::ZERO, Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)];
        let mut rev = sq.clone();
        rev.reverse();
        assert_eq!(Annotation::Polygon { points: sq }.value(), Some(100.0));
        assert_eq!(Annotation::Polygon { points: rev.clone() }.value(), Some(100.0));
        assert_eq!(Annotation::Polygon { points: rev }.label(), "1.00 cm²");
        assert_eq!(polygon_signed_area(&[Vec2::ZERO, Vec2::X]), 0.0);
    }

    #[test]
    fn rect_area_any_corner_order() {
        let r = Annotation::Rect { a: Vec2::new(5.0, 5.0), b: Vec2::new(1.0, 3.0) };
        assert_eq!(r.value(), Some(8.0));
    }

    #[test]
    fn segment_distance() {
        assert_eq!(point_segment_distance(Vec2::new(0.5, 1.0), Vec2::ZERO, Vec2::X), 1.0);
        assert_eq!(point_segment_distance(Vec2::new(2.0, 0.0), Vec2::ZERO, Vec2::X), 1.0);
        assert_eq!(point_segment_distance(Vec2::new(0.0, 2.0), Vec2::ZERO, Vec2::ZERO), 2.0);
    }

    #[test]
    fn set_hit_test_filters_by_slice() {
        let mut set = AnnotationSet::default();
        let k1 = SliceKey::new(SliceAxis::Axial, 3);
        let k2 = SliceKey::new(SliceAxis::Axial, 4);
        let id = set.add(k1, Annotation::Distance { a: Vec2::ZERO, b: Vec2::new(10.0, 0.0) });
        set.add(k2, Annotation::Text { pos: Vec2::new(5.0, 0.0), text: "x".into() });
        assert_eq!(set.hit_test(k1, Vec2::new(5.0, 0.5), 1.0), Some(id));
        assert_eq!(set.hit_test(k1, Vec2::new(5.0, 5.0), 1.0), None);
        assert_eq!(set.on_slice(k2).count(), 1);
        set.get_mut(id).unwrap().translate(Vec2::new(0.0, 5.0));
        assert_eq!(set.hit_test(k1, Vec2::new(5.0, 5.0), 1.0), Some(id));
        assert!(set.remove(id).is_some());
        assert_eq!(set.len(), 1);
        set.clear();
        assert!(set.is_empty());
    }

    #[test]
    fn ids_are_unique_after_removal() {
        let mut set = AnnotationSet::default();
        let k = SliceKey::new(SliceAxis::Coronal, 0);
        let a = set.add(k, Annotation::Text { pos: Vec2::ZERO, text: String::new() });
        set.remove(a);
        let b = set.add(k, Annotation::Text { pos: Vec2::ZERO, text: String::new() });
        assert_ne!(a, b);
    }
}
