//! Measurements and random fixtures only; generator v5 is never altered here.
use super::decode_v4_reference;
use crate::procedural::{
    decode_profile, edge_distance, raw_profile, sd_tab, EdgeId, EdgeOrientation, MAX_TAB_DEPTH,
};
use bevy_math::Vec2;
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use std::collections::HashSet;

pub const SEEDS: [u64; 4] = [1, 42, 2026, 0x1234_5678_9abc_def0];
pub const ASPECTS: [(&str, Vec2); 5] = [
    ("1:1", Vec2::new(100.0, 100.0)),
    ("2:1", Vec2::new(200.0, 100.0)),
    ("4:1", Vec2::new(400.0, 100.0)),
    ("1:2", Vec2::new(100.0, 200.0)),
    ("1:4", Vec2::new(100.0, 400.0)),
];
pub const AXES: [(&str, u32); 6] = [
    ("center", 7),
    ("width", 4),
    ("depth", 4),
    ("neck", 4),
    ("head", 4),
    ("skew", 7),
];
pub const ORIENTATIONS: [EdgeOrientation; 2] =
    [EdgeOrientation::Horizontal, EdgeOrientation::Vertical];
pub fn orientation_name(o: EdgeOrientation) -> &'static str {
    match o {
        EdgeOrientation::Horizontal => "H",
        EdgeOrientation::Vertical => "V",
    }
}
pub fn edge_geometry(size: Vec2, o: EdgeOrientation) -> (f32, f32) {
    (
        match o {
            EdgeOrientation::Horizontal => size.x,
            EdgeOrientation::Vertical => size.y,
        },
        size.min_element(),
    )
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampledEdge {
    pub id: EdgeId,
    pub raw: [u32; 2],
}
/// Uniform draws without replacement over the 1,998,000 internal edges of a
/// 1000x1000 grid. No shape/profile filtering, even when two shapes look alike.
pub fn random_edges(
    seed: u64,
    salt: u64,
    count: usize,
    orientation: Option<EdgeOrientation>,
) -> Vec<SampledEdge> {
    let total = if orientation.is_some() {
        999_000
    } else {
        1_998_000
    };
    assert!(count <= total as usize);
    let mut rng = ChaCha8Rng::seed_from_u64(seed ^ salt ^ 0x5341_4d50_4c45_5635);
    let mut seen = HashSet::new();
    let mut edges = Vec::with_capacity(count);
    while edges.len() < count {
        let index = rng.random_range(0..total);
        if !seen.insert(index) {
            continue;
        }
        let o = orientation.unwrap_or(if index < 999_000 {
            EdgeOrientation::Horizontal
        } else {
            EdgeOrientation::Vertical
        });
        let i = if orientation.is_none() {
            index % 999_000
        } else {
            index
        };
        let id = match o {
            EdgeOrientation::Horizontal => EdgeId {
                orientation: o,
                x: i % 1000,
                y: i / 1000 + 1,
            },
            EdgeOrientation::Vertical => EdgeId {
                orientation: o,
                x: i % 999 + 1,
                y: i / 999,
            },
        };
        edges.push(SampledEdge {
            id,
            raw: raw_profile(seed, id),
        });
    }
    edges
}
/// A separate RNG stream: permutation depends on seed/salt, never fingerprint.
pub fn blank_permutation(seed: u64, salt: u64, count: usize) -> Vec<usize> {
    let mut indices: Vec<_> = (0..count).collect();
    indices.shuffle(&mut ChaCha8Rng::seed_from_u64(
        seed ^ salt ^ 0x424c_414e_4b53_5635,
    ));
    indices
}
/// Uniform target/distractors in a random pool, then a separate permutation for
/// each candidate count. Never truncate a shuffled list while forcing the answer in.
pub fn trial_indices(seed: u64, round: u64, pool_size: usize, count: usize) -> (usize, Vec<usize>) {
    assert!(count > 0 && count <= pool_size);
    let mut pool: Vec<_> = (0..pool_size).collect();
    pool.shuffle(&mut ChaCha8Rng::seed_from_u64(
        seed ^ 0x0054_5249_414c ^ round,
    ));
    pool.truncate(count);
    let target = pool[0];
    let order = blank_permutation(
        seed,
        0x0048_554d_414e ^ round ^ (count as u64).wrapping_mul(0x9e37_79b9),
        count,
    );
    (target, order.into_iter().map(|i| pool[i]).collect())
}
pub fn profile_samples(raw: [u32; 2]) -> [u32; 6] {
    [
        (raw[0] >> 4) & 255,
        (raw[0] >> 12) & 255,
        (raw[0] >> 20) & 255,
        raw[1] & 255,
        (raw[1] >> 8) & 255,
        (raw[1] >> 16) & 255,
    ]
}
/// Closest available byte to the original residual, in the requested bin.
/// Four-class axes preserve residual exactly; seven-class axes differ by <=6
/// integer residual units (signed micro <=12/255).
pub fn replace_class(raw: [u32; 2], axis: usize, class: u32) -> [u32; 2] {
    let count = AXES[axis].1;
    assert!(class < count);
    let mut samples = profile_samples(raw);
    let residual = (samples[axis] * count) & 255;
    samples[axis] = (0..256)
        .filter(|&s| (s * count) >> 8 == class)
        .min_by_key(|&s| (((s * count) & 255).abs_diff(residual), s))
        .unwrap();
    let mut result = super::sample_profile(raw[0] & 7, samples);
    result[0] |= raw[0] & 8;
    result
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RasterMode {
    Normalized64,
    Display64,
    Display256,
}
impl RasterMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Normalized64 => "normalized64",
            Self::Display64 => "display64",
            Self::Display256 => "display256",
        }
    }
    pub fn dimensions(self) -> (usize, usize) {
        match self {
            Self::Display256 => (256, 128),
            _ => (64, 32),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SilhouetteMask {
    pub words: Vec<u64>,
    pub pixels: usize,
}
impl SilhouetteMask {
    pub fn raster(raw: [u32; 2], length: f32, short: f32, mode: RasterMode, v4: bool) -> Self {
        assert!(length.is_finite() && short > 0.0 && short <= length);
        let (width, height) = mode.dimensions();
        let dy = if mode == RasterMode::Normalized64 {
            MAX_TAB_DEPTH * short / height as f32
        } else {
            length / width as f32
        };
        let raw = [raw[0] & !8, raw[1]];
        let p = decode_v4_reference(raw);
        let mut words = vec![0; (width * height).div_ceil(64)];
        for y in 0..height {
            for x in 0..width {
                let q = Vec2::new(
                    (x as f32 + 0.5) * length / width as f32,
                    (y as f32 + 0.5) * dy,
                );
                let d = if v4 {
                    q.y.min(sd_tab(q, p, length, short))
                } else {
                    edge_distance(q, raw, length, short)
                };
                if d <= 0.0 {
                    let bit = y * width + x;
                    words[bit / 64] |= 1 << (bit % 64);
                }
            }
        }
        Self {
            words,
            pixels: width * height,
        }
    }
    pub fn hamming(&self, other: &Self) -> u32 {
        assert_eq!(self.pixels, other.pixels);
        self.words
            .iter()
            .zip(&other.words)
            .map(|(a, b)| (a ^ b).count_ones())
            .sum()
    }
    pub fn iou(&self, other: &Self) -> f64 {
        let intersection: u32 = self
            .words
            .iter()
            .zip(&other.words)
            .map(|(a, b)| (a & b).count_ones())
            .sum();
        let union: u32 = self
            .words
            .iter()
            .zip(&other.words)
            .map(|(a, b)| (a | b).count_ones())
            .sum();
        intersection as f64 / union.max(1) as f64
    }
}
/// Actual SDF contour; offline solver, never compiled into normal gameplay.
/// Points are in world units, canonical x along the full edge, y outward.
pub fn tab_contour(raw: [u32; 2], length: f32, short: f32, sections: u32) -> Vec<Vec2> {
    assert!(sections > 0);
    let mut p = decode_profile(raw);
    p.polarity = 1.0;
    let depth = p.depth * short;
    let mut left = Vec::new();
    let mut right = Vec::new();
    for row in 0..=sections {
        let y = if row == 0 {
            depth * 1e-6
        } else {
            depth * 0.98 * row as f32 / sections as f32
        };
        let center = (p.center
            + p.asymmetry * p.head_width * if y > depth * 0.51 { 1.0 } else { 0.0 })
            * length;
        let mut limits = [0.0; 2];
        for (i, sign) in [-1.0, 1.0].into_iter().enumerate() {
            let mut inside = center;
            let mut outside = center + sign * length;
            for _ in 0..28 {
                let x = (inside + outside) * 0.5;
                if sd_tab(Vec2::new(x, y), p, length, short) <= 0.0 {
                    inside = x;
                } else {
                    outside = x;
                }
            }
            limits[i] = (inside + outside) * 0.5;
        }
        left.push(Vec2::new(limits[0], y));
        right.push(Vec2::new(limits[1], y));
    }
    let mut points = vec![Vec2::ZERO];
    points.extend(left);
    points.extend(right.into_iter().rev());
    points.push(Vec2::new(length, 0.0));
    points
}

#[cfg(test)]
mod tests {
    use super::super::EdgeFingerprint;
    use super::*;
    use crate::procedural::piece_profiles;
    use bevy_math::UVec2;
    #[test]
    fn random_sampling_is_reproducible_unique_internal_and_shape_unfiltered() {
        for seed in SEEDS {
            for orientation in [
                None,
                Some(EdgeOrientation::Horizontal),
                Some(EdgeOrientation::Vertical),
            ] {
                let a = random_edges(seed, 0, 32, orientation);
                assert_eq!(a, random_edges(seed, 0, 32, orientation));
                assert_ne!(a, random_edges(seed, 1, 32, orientation));
                let mut ids = HashSet::new();
                for e in a {
                    assert!(ids.insert((orientation_name(e.id.orientation), e.id.x, e.id.y)));
                    let cell = UVec2::new(e.id.x, e.id.y);
                    let p = piece_profiles(seed, UVec2::splat(1000), cell);
                    match e.id.orientation {
                        EdgeOrientation::Horizontal => assert_eq!(p[0], e.raw),
                        EdgeOrientation::Vertical => assert_eq!(p[3], e.raw),
                    }
                }
                let mut permutation = blank_permutation(seed, 0, 32);
                assert_eq!(permutation, blank_permutation(seed, 0, 32));
                permutation.sort_unstable();
                assert_eq!(permutation, (0..32).collect::<Vec<_>>());
            }
        }
    }
    #[test]
    fn changing_one_class_keeps_all_other_samples_and_classes_fixed() {
        for e in random_edges(42, 0, 32, None) {
            for axis in 0..6 {
                for class in 0..AXES[axis].1 {
                    let raw = replace_class(e.raw, axis, class);
                    let before = profile_samples(e.raw);
                    let after = profile_samples(raw);
                    let a = EdgeFingerprint::from_raw(e.raw).classes();
                    let b = EdgeFingerprint::from_raw(raw).classes();
                    assert_eq!(b[axis + 1], class as u8);
                    for i in 0..6 {
                        if i != axis {
                            assert_eq!(before[i], after[i]);
                            assert_eq!(a[i + 1], b[i + 1]);
                        }
                    }
                    let count = AXES[axis].1;
                    assert!(
                        ((before[axis] * count) & 255).abs_diff((after[axis] * count) & 255) <= 6
                    );
                    assert_eq!(raw[0] & 15, e.raw[0] & 15);
                }
            }
        }
    }
    #[test]
    fn geometry_mapping_and_masks_use_both_actual_edge_lengths() {
        assert_eq!(
            edge_geometry(ASPECTS[2].1, EdgeOrientation::Horizontal),
            (400.0, 100.0)
        );
        assert_eq!(
            edge_geometry(ASPECTS[4].1, EdgeOrientation::Vertical),
            (400.0, 100.0)
        );
        let raw = raw_profile(
            42,
            EdgeId {
                orientation: EdgeOrientation::Horizontal,
                x: 1,
                y: 2,
            },
        );
        let normalized = SilhouetteMask::raster(raw, 100.0, 100.0, RasterMode::Normalized64, false);
        assert_eq!(
            normalized.words,
            super::super::EdgeSilhouetteDescriptor::from_raw(raw).0
        );
        for mode in [
            RasterMode::Normalized64,
            RasterMode::Display64,
            RasterMode::Display256,
        ] {
            let a = SilhouetteMask::raster(raw, 400.0, 100.0, mode, false);
            assert_eq!(
                a,
                SilhouetteMask::raster([raw[0] ^ 8, raw[1]], 400.0, 100.0, mode, false)
            );
            assert_eq!(a.hamming(&a), 0);
            assert_eq!(a.iou(&a), 1.0);
        }
    }
    #[test]
    fn every_candidate_count_has_one_answer_and_unique_unfiltered_distractors() {
        for seed in SEEDS {
            for round in 0..20 {
                for count in [8, 10, 12] {
                    let (target, indices) = trial_indices(seed, round, 64, count);
                    assert_eq!(indices.len(), count);
                    assert_eq!(indices.iter().filter(|&&i| i == target).count(), 1);
                    assert_eq!(indices.iter().copied().collect::<HashSet<_>>().len(), count);
                    assert!(indices.iter().all(|&i| i < 64));
                    assert_eq!((target, indices), trial_indices(seed, round, 64, count));
                }
            }
        }
    }
}
