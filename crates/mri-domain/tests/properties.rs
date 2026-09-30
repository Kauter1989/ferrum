#![allow(missing_docs, clippy::expect_used)]
//! Property-based tests of domain invariants.

use glam::{Vec2, Vec3};
use mri_domain::clip::ClipSettings;
use mri_domain::{
    Aabb, ClipBox, ControlPoint, Dims3, IntensityRange, OrbitCamera, Ray, Rgb, SliceAxis, SliceView, TransferFunction,
    Volume, WindowLevel,
};
use proptest::prelude::*;

fn unit() -> impl Strategy<Value = f32> {
    0.0f32..=1.0
}

fn transfer_function() -> impl Strategy<Value = TransferFunction> {
    prop::collection::vec((unit(), unit()), 0..8).prop_map(|mut inner| {
        inner.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut pts = vec![ControlPoint::new(0.0, Rgb::BLACK, 0.0)];
        pts.extend(inner.into_iter().map(|(p, a)| ControlPoint::new(p, Rgb::new(p, a, 1.0 - p), a)));
        pts.push(ControlPoint::new(1.0, Rgb::WHITE, 1.0));
        TransferFunction::new(pts).expect("generated function is valid")
    })
}

proptest! {
    #[test]
    fn tf_sample_is_bounded_by_neighbouring_points(tf in transfer_function(), x in unit()) {
        let (c, a) = tf.sample(x);
        prop_assert!((0.0..=1.0).contains(&a));
        prop_assert!((0.0..=1.0).contains(&c.r));
        let max = tf.points().iter().map(|p| p.opacity).fold(0.0, f32::max);
        prop_assert!(a <= max + 1e-6);
    }

    #[test]
    fn tf_max_opacity_dominates_samples(tf in transfer_function(), a in unit(), b in unit(), t in unit()) {
        let (lo, hi) = (a.min(b), a.max(b));
        let x = lo + (hi - lo) * t;
        prop_assert!(tf.max_opacity_in(lo, hi) + 1e-5 >= tf.sample(x).1);
    }

    #[test]
    fn tf_edits_preserve_invariants(tf in transfer_function(), idx in 0usize..10, p in -1.0f32..2.0, o in -1.0f32..2.0) {
        let mut tf = tf;
        let _ = tf.move_point(idx, p, o);
        let i = tf.insert_point(p);
        let _ = tf.remove_point(i);
        prop_assert!(TransferFunction::new(tf.points().to_vec()).is_ok());
    }

    #[test]
    fn window_apply_is_monotonic(c in -2000.0f32..2000.0, w in 0.0f32..4000.0, x in -3000.0f32..3000.0, dx in 0.0f32..100.0) {
        let win = WindowLevel::new(c, w);
        let a = win.apply(x);
        prop_assert!((0.0..=1.0).contains(&a));
        prop_assert!(win.apply(x + dx) >= a);
    }

    #[test]
    fn storage_roundtrip_error_is_bounded(min in -5000.0f32..0.0, span in 1.0f32..10000.0, t in unit()) {
        let r = IntensityRange::new(min, min + span).unwrap();
        let v = min + span * t;
        let err = (r.from_storage(r.to_storage(v)) - v).abs();
        prop_assert!(err <= span / 65535.0 + 1e-3 * span.max(1.0) / 1000.0 + 1e-3);
    }

    #[test]
    fn ray_box_interval_points_are_inside(ox in -3.0f32..3.0, oy in -3.0f32..3.0, oz in -3.0f32..3.0,
                                          dx in -1.0f32..1.0, dy in -1.0f32..1.0, dz in -1.0f32..1.0) {
        prop_assume!(Vec3::new(dx, dy, dz).length() > 1e-3);
        let b = Aabb::centered(Vec3::new(1.0, 0.7, 0.4));
        let r = Ray::new(Vec3::new(ox, oy, oz), Vec3::new(dx, dy, dz));
        if let Some((n, f)) = b.intersect(&r) {
            let grown = Aabb::new(b.min - Vec3::splat(1e-3), b.max + Vec3::splat(1e-3));
            prop_assert!(grown.contains(r.at(n)));
            prop_assert!(grown.contains(r.at(f)));
            prop_assert!(grown.contains(r.at((n + f) * 0.5)));
        }
    }

    #[test]
    fn clipping_never_extends_segment(lo in prop::array::uniform3(unit()), hi in prop::array::uniform3(unit()),
                                      cut in unit(), ox in -2.0f32..2.0, oy in -2.0f32..2.0) {
        let mut s = ClipSettings::default();
        s.clip_box = ClipBox { min: Vec3::from(lo), max: Vec3::from(hi) };
        s.view_cut = cut;
        let r = Ray::new(Vec3::new(ox, oy, 3.0), Vec3::new(-ox * 0.2, -oy * 0.2, -1.0));
        let hs = s.half_spaces(Vec3::ONE, r.direction);
        if let Some((n, f)) = Aabb::centered(Vec3::ONE).intersect(&r) {
            if let Some(seg) = ClipSettings::clip_segment(&hs, &r, n, f) {
                prop_assert!(seg.t_near >= n - 1e-6 && seg.t_far <= f + 1e-6);
                let mid = r.at((seg.t_near + seg.t_far) * 0.5);
                for h in &hs {
                    prop_assert!(h.normal.dot(mid) <= h.offset + 1e-4);
                }
            }
        }
    }

    #[test]
    fn camera_orbit_keeps_distance(moves in prop::collection::vec((-1.0f32..1.0, -1.0f32..1.0), 1..50)) {
        let mut c = OrbitCamera::default();
        for (dx, dy) in moves {
            c.rotate(dx, dy);
        }
        prop_assert!(((c.eye() - c.target).length() - c.distance).abs() < 1e-3);
        prop_assert!((c.view_dir().length() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn slice_tex_coord_stays_in_unit_cube(axis_idx in 0usize..3, u in unit(), v in unit(), s in unit()) {
        let t = SliceAxis::ALL[axis_idx].tex_coord(Vec2::new(u, v), s);
        prop_assert!(t.cmpge(Vec3::ZERO).all() && t.cmple(Vec3::ONE).all());
    }

    #[test]
    fn slice_view_roundtrip(zoom in 0.25f32..32.0, px in -500.0f32..500.0, py in -500.0f32..500.0,
                            x in 0.0f32..800.0, y in 0.0f32..600.0) {
        let view = SliceView { zoom, pan: Vec2::new(px, py) };
        let vp = Vec2::new(800.0, 600.0);
        let img = Vec2::new(240.0, 180.0);
        let mm = view.screen_to_mm(Vec2::new(x, y), vp, img);
        let back = view.mm_to_screen(mm, vp, img);
        prop_assert!((back - Vec2::new(x, y)).length() < 1e-2);
    }

    #[test]
    fn trilinear_sample_within_data_range(vals in prop::collection::vec(0.0f32..100.0, 27), t in prop::array::uniform3(-0.5f32..1.5)) {
        let v = Volume::from_physical(Dims3::new(3, 3, 3), Vec3::ONE, &vals).unwrap();
        let s = v.sample(Vec3::from(t));
        prop_assert!((-1e-6..=1.0 + 1e-6).contains(&s));
    }
}
