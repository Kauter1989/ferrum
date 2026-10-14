//! The volumes of a series that holds several images per position:
//! echoes, diffusion b-values, dynamic phases, PET frames (Stage 20.4).
//!
//! Images at one position are told apart by the fields of their
//! [`StackKey`] that differ there; each distinct value of those fields is a
//! volume. When no field differs, the k-th image at every position (in
//! [`stack_order`](super::series::stack_order)) forms volume k.

use std::collections::BTreeSet;

use super::header::{SliceHeader, StackKey};
use super::series::{position_groups, PlaneRef};

/// One field that can tell volumes apart, in the order volumes are sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Temporal,
    EchoNumber,
    EchoTime,
    BValue,
    Trigger,
    FrameTime,
    Acquisition,
}

const FIELDS: [Field; 7] = [
    Field::Temporal,
    Field::EchoNumber,
    Field::EchoTime,
    Field::BValue,
    Field::Trigger,
    Field::FrameTime,
    Field::Acquisition,
];

impl Field {
    fn value(self, k: &StackKey) -> Option<f64> {
        match self {
            Field::Temporal => k.temporal_position.map(|v| v as f64),
            Field::EchoNumber => k.echo_number.map(|v| v as f64),
            Field::EchoTime => k.echo_time,
            Field::BValue => k.b_value,
            Field::Trigger => k.trigger_time,
            Field::FrameTime => k.frame_reference_time,
            Field::Acquisition => k.acquisition_number.map(|v| v as f64),
        }
    }

    fn id_prefix(self) -> &'static str {
        match self {
            Field::Temporal => "t",
            Field::EchoNumber => "e",
            Field::EchoTime => "te",
            Field::BValue => "b",
            Field::Trigger => "tr",
            Field::FrameTime => "f",
            Field::Acquisition => "a",
        }
    }
}

/// One volume of a series: what it is called and which images it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeSpec {
    /// Stable key within the series (`b1000`, `t3`, `te80-e2`, `n2`).
    pub key: String,
    /// Name for people (`b = 1000`, `TE = 80 ms`, `phase 3/20`, …).
    pub label: String,
    /// Stack key of the volume's first image (b-value, echo time, …).
    pub stack: StackKey,
    /// The images of the volume.
    pub members: BTreeSet<PlaneRef>,
    /// The volume has an image at every position of the series.
    pub complete: bool,
}

/// A number without a useless fraction: `1000`, `80.5`.
fn num(v: f64) -> String {
    format!("{v}")
}

/// The fields whose values differ between images at one position.
fn distinguishing(groups: &[Vec<StackKey>]) -> Vec<Field> {
    FIELDS
        .into_iter()
        .filter(|f| {
            groups.iter().any(|g| {
                let first = g.first().map(|k| f.value(k));
                g.iter().any(|k| Some(f.value(k)) != first)
            })
        })
        .collect()
}

/// Rank of `v` among the distinct values `all` (1-based) and their count.
fn ordinal(v: Option<f64>, all: &[Option<f64>]) -> (usize, usize) {
    let distinct: Vec<f64> = {
        let mut d: Vec<f64> = all.iter().flatten().copied().collect();
        d.sort_by(f64::total_cmp);
        d.dedup();
        d
    };
    let rank = v.and_then(|v| distinct.iter().position(|d| *d == v)).map_or(0, |p| p + 1);
    (rank, distinct.len())
}

fn label_part(f: Field, v: Option<f64>, all: &[Option<f64>]) -> String {
    let Some(x) = v else {
        return format!("{} unknown", f.id_prefix());
    };
    match f {
        Field::Temporal => {
            let (r, n) = ordinal(v, all);
            format!("phase {r}/{n}")
        }
        Field::EchoNumber => format!("echo {}", num(x)),
        Field::EchoTime => format!("TE = {} ms", num(x)),
        Field::BValue => format!("b = {}", num(x)),
        Field::Trigger => format!("trigger {} ms", num(x)),
        Field::FrameTime => {
            let (r, n) = ordinal(v, all);
            format!("frame {r}/{n}, t = {} s", num(x / 1000.0))
        }
        Field::Acquisition => format!("acquisition {}", num(x)),
    }
}

/// The volumes of a series, in order; empty when it holds one image per
/// position (or has no positions).
pub fn partition(headers: &[SliceHeader]) -> Vec<VolumeSpec> {
    let Some(groups) = position_groups(headers) else {
        return Vec::new();
    };
    if groups.iter().all(|g| g.len() <= 1) {
        return Vec::new();
    }
    let positions = groups.len();
    let keys: Vec<Vec<StackKey>> = groups.iter().map(|g| g.iter().map(|im| im.stack.clone()).collect()).collect();
    let fields = distinguishing(&keys);
    if fields.is_empty() {
        return by_rank(&groups, positions);
    }
    // One volume per distinct tuple of the distinguishing fields.
    let tuple = |k: &StackKey| fields.iter().map(|f| f.value(k)).collect::<Vec<_>>();
    let mut volumes: Vec<(Vec<Option<f64>>, VolumeSpec)> = Vec::new();
    for im in groups.iter().flatten() {
        let t = tuple(im.stack);
        match volumes.iter_mut().find(|(k, _)| *k == t) {
            Some((_, spec)) => {
                spec.members.insert(im.plane.clone());
            }
            None => volumes.push((
                t,
                VolumeSpec {
                    key: String::new(),
                    label: String::new(),
                    stack: im.stack.clone(),
                    members: BTreeSet::from([im.plane.clone()]),
                    complete: false,
                },
            )),
        }
    }
    volumes.sort_by(|a, b| a.1.stack.order(&b.1.stack));
    let all: Vec<Vec<Option<f64>>> =
        fields.iter().map(|f| volumes.iter().map(|(t, _)| f.value(&t_stack(&fields, t))).collect()).collect();
    volumes
        .into_iter()
        .map(|(t, mut spec)| {
            spec.key = fields
                .iter()
                .zip(&t)
                .map(|(f, v)| format!("{}{}", f.id_prefix(), v.map_or("x".into(), num)))
                .collect::<Vec<_>>()
                .join("-");
            spec.label =
                fields.iter().enumerate().map(|(i, f)| label_part(*f, t[i], &all[i])).collect::<Vec<_>>().join(", ");
            spec.complete = spec.members.len() == positions;
            spec
        })
        .collect()
}

/// A stack key holding the tuple `t` of `fields` (for [`Field::value`]).
fn t_stack(fields: &[Field], t: &[Option<f64>]) -> StackKey {
    let mut k = StackKey::default();
    for (f, v) in fields.iter().zip(t) {
        match f {
            Field::Temporal => k.temporal_position = v.map(|x| x as i64),
            Field::EchoNumber => k.echo_number = v.map(|x| x as i64),
            Field::EchoTime => k.echo_time = *v,
            Field::BValue => k.b_value = *v,
            Field::Trigger => k.trigger_time = *v,
            Field::FrameTime => k.frame_reference_time = *v,
            Field::Acquisition => k.acquisition_number = v.map(|x| x as i64),
        }
    }
    k
}

/// Volume k = the k-th image at every position.
fn by_rank(groups: &[Vec<super::series::Image<'_>>], positions: usize) -> Vec<VolumeSpec> {
    let count = groups.iter().map(Vec::len).max().unwrap_or(0);
    (0..count)
        .map(|r| {
            let members: BTreeSet<PlaneRef> =
                groups.iter().filter_map(|g| g.get(r)).map(|im| im.plane.clone()).collect();
            let stack = groups.iter().find_map(|g| g.get(r)).map(|im| im.stack.clone()).unwrap_or_default();
            VolumeSpec {
                key: format!("n{}", r + 1),
                label: format!("volume {}/{count}", r + 1),
                stack,
                complete: members.len() == positions,
                members,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dicom::series::tests::header;

    /// 3 positions; at each, one image per value of `set`.
    fn series(values: &[f64], set: impl Fn(&mut SliceHeader, f64)) -> Vec<SliceHeader> {
        let mut hs = Vec::new();
        for z in 0..3 {
            for (n, &v) in values.iter().enumerate() {
                let mut h = header("s", Some(f64::from(z) * 2.0), Some(i64::from(z) * 10 + n as i64));
                // a field that is the same everywhere never names a volume
                h.stack.acquisition_number = Some(1);
                set(&mut h, v);
                hs.push(h);
            }
        }
        hs.reverse();
        hs
    }

    fn labels(v: &[VolumeSpec]) -> Vec<(&str, &str, usize, bool)> {
        v.iter().map(|s| (s.key.as_str(), s.label.as_str(), s.members.len(), s.complete)).collect()
    }

    #[test]
    fn one_image_per_position_is_one_volume() {
        // covers 20.4-a
        let hs: Vec<_> = (0..3).map(|z| header("s", Some(f64::from(z)), None)).collect();
        assert!(partition(&hs).is_empty());
    }

    #[test]
    fn b_values_echoes_and_phases_name_the_volumes() {
        // covers 20.4-a
        let b = partition(&series(&[1000.0, 0.0], |h, v| h.stack.b_value = Some(v)));
        assert_eq!(labels(&b), vec![("b0", "b = 0", 3, true), ("b1000", "b = 1000", 3, true)]);
        let echoes = partition(&series(&[2.0, 1.0], |h, v| {
            h.stack.echo_number = Some(v as i64);
            h.stack.echo_time = Some(v * 40.0 - 20.0);
        }));
        assert_eq!(
            labels(&echoes),
            vec![("e1-te20", "echo 1, TE = 20 ms", 3, true), ("e2-te60", "echo 2, TE = 60 ms", 3, true)]
        );
        let phases = partition(&series(&[9.0, 5.0, 7.0], |h, v| h.stack.temporal_position = Some(v as i64)));
        assert_eq!(
            labels(&phases),
            vec![("t5", "phase 1/3", 3, true), ("t7", "phase 2/3", 3, true), ("t9", "phase 3/3", 3, true)]
        );
    }

    #[test]
    fn pet_frames_are_named_by_their_start() {
        // covers 20.4-b
        let frames = partition(&series(&[60000.0, 0.0, 30000.0], |h, v| h.stack.frame_reference_time = Some(v)));
        assert_eq!(
            labels(&frames),
            vec![
                ("f0", "frame 1/3, t = 0 s", 3, true),
                ("f30000", "frame 2/3, t = 30 s", 3, true),
                ("f60000", "frame 3/3, t = 60 s", 3, true)
            ]
        );
        // every image of a volume has its frame time
        let hs = series(&[60000.0, 0.0, 30000.0], |h, v| h.stack.frame_reference_time = Some(v));
        for v in &frames {
            let t = v.stack.frame_reference_time;
            assert!(v.members.iter().all(|p| hs[p.header].stack.frame_reference_time == t), "{}", v.label);
        }
    }

    #[test]
    fn without_a_distinguishing_field_the_rank_decides() {
        // covers 20.4-a
        let hs = series(&[0.0, 0.0], |_, _| {});
        let v = partition(&hs);
        assert_eq!(labels(&v), vec![("n1", "volume 1/2", 3, true), ("n2", "volume 2/2", 3, true)]);
        // volume 1 holds the lower Instance Number at every position
        for p in &v[0].members {
            let inst = hs[p.header].instance_number.unwrap();
            assert_eq!(inst % 10, 0, "{inst}");
        }
    }

    #[test]
    fn a_volume_missing_a_position_is_incomplete() {
        // covers 20.4-a
        let mut hs = series(&[1.0, 2.0], |h, v| h.stack.temporal_position = Some(v as i64));
        hs.retain(|h| !(h.stack.temporal_position == Some(2) && h.position.unwrap().z == 4.0));
        let v = partition(&hs);
        assert_eq!(labels(&v), vec![("t1", "phase 1/2", 3, true), ("t2", "phase 2/2", 2, false)]);
    }
}
