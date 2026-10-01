//! Offline fingerprint inspection; never stored or uploaded by the renderer.
use crate::procedural::{class_sample, edge_distance, sd_tab, EdgeProfile, EdgeStyle};
use bevy_math::Vec2;

pub mod assessment;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EdgeFingerprint {
    pub style: u8,
    pub center_class: u8,
    pub width_class: u8,
    pub depth_class: u8,
    pub neck_class: u8,
    pub head_class: u8,
    pub skew_class: u8,
}
impl EdgeFingerprint {
    /// Internal edges only. Polarity is deliberately absent: both owners share a fingerprint.
    pub fn from_raw(raw: [u32; 2]) -> Self {
        assert_ne!(raw[0] & 7, 0);
        Self {
            style: ((raw[0] & 7) - 1) as u8,
            center_class: class_sample(raw[0], 4, 7).0 as u8,
            width_class: class_sample(raw[0], 12, 4).0 as u8,
            depth_class: class_sample(raw[0], 20, 4).0 as u8,
            neck_class: class_sample(raw[1], 0, 4).0 as u8,
            head_class: class_sample(raw[1], 8, 4).0 as u8,
            skew_class: class_sample(raw[1], 16, 7).0 as u8,
        }
    }
    pub fn classes(self) -> [u8; 7] {
        [
            self.style,
            self.center_class,
            self.width_class,
            self.depth_class,
            self.neck_class,
            self.head_class,
            self.skew_class,
        ]
    }
}

/// Fixed canonical square-cell tab, 64 columns across the full edge, 32 rows over
/// the positive 0..MAX_TAB_DEPTH band. No recentering: center is part of the fingerprint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeSilhouetteDescriptor(pub [u64; 32]);
impl EdgeSilhouetteDescriptor {
    pub fn from_raw(raw: [u32; 2]) -> Self {
        Self::raster(|q| edge_distance(q, [raw[0] & !8, raw[1]], 100.0, 100.0))
    }
    pub fn v4_reference(raw: [u32; 2]) -> Self {
        let p = decode_v4_reference(raw);
        Self::raster(|q| q.y.min(sd_tab(q, p, 100.0, 100.0)))
    }
    fn raster(distance: impl Fn(Vec2) -> f32) -> Self {
        let mut rows = [0; 32];
        for (y, row) in rows.iter_mut().enumerate() {
            for x in 0..64 {
                let q = Vec2::new(
                    (x as f32 + 0.5) * 100.0 / 64.0,
                    (y as f32 + 0.5) * 22.0 / 32.0,
                );
                if distance(q) <= 0.0 {
                    *row |= 1 << x;
                }
            }
        }
        Self(rows)
    }
    pub fn hamming(&self, other: &Self) -> u32 {
        self.0
            .iter()
            .zip(other.0)
            .map(|(a, b)| (a ^ b).count_ones())
            .sum()
    }
    pub fn iou(&self, other: &Self) -> f64 {
        let intersection: u32 = self
            .0
            .iter()
            .zip(other.0)
            .map(|(a, b)| (a & b).count_ones())
            .sum();
        let union: u32 = self
            .0
            .iter()
            .zip(other.0)
            .map(|(a, b)| (a | b).count_ones())
            .sum();
        intersection as f64 / union.max(1) as f64
    }
}

/// Frozen v4 decoder for measurement only. v5 gameplay rejects v4 definitions.
pub fn decode_v4_reference(raw: [u32; 2]) -> EdgeProfile {
    let (w, d, n, h, style) = match raw[0] & 7 {
        2 => (0.52, 0.17, 0.18, 0.34, EdgeStyle::Wide),
        3 => (0.40, 0.18, 0.12, 0.23, EdgeStyle::Narrow),
        4 => (0.46, 0.21, 0.14, 0.27, EdgeStyle::Deep),
        5 => (0.46, 0.14, 0.16, 0.28, EdgeStyle::Shallow),
        6 => (0.48, 0.19, 0.12, 0.32, EdgeStyle::Pear),
        _ => (0.46, 0.18, 0.14, 0.28, EdgeStyle::Round),
    };
    let v = |word, shift| ((word >> shift) & 255u32) as f32 / 255.0 * 2.0 - 1.0;
    EdgeProfile {
        polarity: if raw[0] & 8 == 0 { 1.0 } else { -1.0 },
        style,
        center: 0.5 + v(raw[0], 4) * 0.025,
        width: w * (1.0 + v(raw[0], 12) * 0.035),
        depth: d * (1.0 + v(raw[0], 20) * 0.035),
        neck_width: n * (1.0 + v(raw[1], 0) * 0.035),
        head_width: h * (1.0 + v(raw[1], 8) * 0.035),
        asymmetry: v(raw[1], 16) * 0.025,
    }
}

/// Controlled byte fixture in order center, width, depth, neck, head, skew.
pub fn sample_profile(style: u32, samples: [u32; 6]) -> [u32; 2] {
    [
        style | samples[0] << 4 | samples[1] << 12 | samples[2] << 20,
        samples[3] | samples[4] << 8 | samples[5] << 16,
    ]
}
/// Center extreme + widest width/head + extreme lean, for each neck/depth class.
pub fn worst_case_profiles() -> Vec<[u32; 2]> {
    let mut result = Vec::new();
    for style in 1..=6 {
        for center in [0, 255] {
            for skew in [0, 255] {
                for neck in [0, 85, 170, 255] {
                    for depth in [0, 85, 170, 255] {
                        result.push(sample_profile(style, [center, 255, depth, neck, 255, skew]));
                    }
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procedural::{decode_profile, profile_envelope};
    #[test]
    fn macro_bins_are_uniform_ordered_and_do_not_overlap() {
        for count in [4, 7] {
            let mut bins = vec![0; count as usize];
            let mut previous = -2.0;
            for sample in 0..256 {
                let (class, micro) = class_sample(sample, 0, count);
                bins[class as usize] += 1;
                let value = class as f32 + micro * 0.1;
                assert!(value > previous);
                previous = value;
            }
            assert!(bins
                .iter()
                .all(|&n| n == 256 / count || n == 256 / count + 1));
        }
        // Check actual decoded dimensions as well, with the other axes held fixed.
        for style in 1..=6 {
            for axis in 0..6 {
                let mut last = -1.0;
                for sample in 0..256 {
                    let mut samples = [128; 6];
                    samples[axis] = sample;
                    let p = decode_profile(sample_profile(style, samples));
                    let value = [
                        p.center,
                        p.width,
                        p.depth,
                        p.neck_width / p.head_width,
                        p.head_width / p.width,
                        p.asymmetry,
                    ][axis];
                    assert!(value > last, "{style} {axis} {sample}: {value} <= {last}");
                    last = value;
                }
            }
        }
    }
    #[test]
    fn all_macro_combinations_preserve_parameter_safety() {
        let seven = [0, 43, 85, 128, 170, 213, 255];
        let four = [0, 85, 170, 255];
        for style in 1..=6 {
            for c in seven {
                for w in four {
                    for d in four {
                        for n in four {
                            for h in four {
                                for s in seven {
                                    let raw = sample_profile(style, [c, w, d, n, h, s]);
                                    let p = decode_profile(raw);
                                    assert!(
                                        p.neck_width > 0.0
                                            && p.neck_width < p.head_width
                                            && p.head_width < p.width
                                    );
                                    assert!(p.depth > 0.0 && p.depth <= 0.215 + 1e-6);
                                    let envelope = profile_envelope(p);
                                    assert!(
                                        p.center - envelope >= 0.185 - 1e-6
                                            && p.center + envelope <= 0.815 + 1e-6
                                    );
                                    assert_eq!(
                                        EdgeFingerprint::from_raw(raw),
                                        EdgeFingerprint::from_raw([raw[0] ^ 8, raw[1]])
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn worst_case_contours_are_connected_and_stay_inside_clearance_and_quad() {
        for raw in worst_case_profiles() {
            for (length, short) in [(100.0, 100.0), (200.0, 50.0)] {
                let mut previous = None;
                for row in 0..64 {
                    let y = (row as f32 + 0.5) * 0.23 * short / 64.0;
                    let mut first = None;
                    let mut last = None;
                    let mut gap = false;
                    for col in 0..256 {
                        let x = (col as f32 + 0.5) * length / 256.0;
                        let q = Vec2::new(x, y);
                        let d = edge_distance(q, raw, length, short);
                        assert!(d.is_finite());
                        assert_eq!(
                            d,
                            -edge_distance(Vec2::new(x, -y), [raw[0] ^ 8, raw[1]], length, short)
                        );
                        if d <= 0.0 {
                            assert!(!gap, "disconnected row {raw:?} {row}");
                            assert!((0.18 * length..=0.82 * length).contains(&x));
                            assert!(y < 0.22 * short);
                            first.get_or_insert(col);
                            last = Some(col);
                        } else if first.is_some() {
                            gap = true;
                        }
                    }
                    if let (Some(a), Some(b)) = (first, last) {
                        if let Some((pa, pb)) = previous {
                            assert!(a <= pb && b >= pa, "disconnected head {raw:?}");
                        }
                        previous = Some((a, b));
                    } else if previous.is_some() {
                        // No later island may appear above the finished head.
                        for r in row + 1..64 {
                            for col in 0..256 {
                                assert!(
                                    edge_distance(
                                        Vec2::new(
                                            (col as f32 + 0.5) * length / 256.0,
                                            (r as f32 + 0.5) * 0.23 * short / 64.0
                                        ),
                                        raw,
                                        length,
                                        short
                                    ) > 0.0
                                );
                            }
                        }
                        break;
                    }
                }
            }
        }
    }
    #[test]
    fn descriptors_preserve_matching_edges_and_distinguish_macro_changes() {
        let a = sample_profile(1, [0, 128, 128, 128, 128, 128]);
        let b = sample_profile(1, [255, 128, 128, 128, 128, 128]);
        let da = EdgeSilhouetteDescriptor::from_raw(a);
        assert_eq!(da, EdgeSilhouetteDescriptor::from_raw([a[0] ^ 8, a[1]]));
        let db = EdgeSilhouetteDescriptor::from_raw(b);
        assert!(da.hamming(&db) > 100);
        assert!(da.iou(&db) < 0.6);
    }
    #[test]
    fn baseline_shortcut_is_identical_to_the_original_complement() {
        for raw in worst_case_profiles() {
            for polarity in [0, 8] {
                let raw = [raw[0] | polarity, raw[1]];
                let p = decode_profile(raw);
                for y in [-100.0, -22.0, -1.0, -0.001, 0.0, 0.001, 1.0, 22.0, 100.0] {
                    for x in [0.0, 18.0, p.center * 100.0, 82.0, 100.0] {
                        let q = Vec2::new(x, y);
                        let original = if p.polarity > 0.0 {
                            q.y.min(sd_tab(q, p, 100.0, 100.0))
                        } else {
                            q.y.max(-sd_tab(Vec2::new(q.x, -q.y), p, 100.0, 100.0))
                        };
                        assert_eq!(edge_distance(q, raw, 100.0, 100.0), original);
                    }
                }
            }
        }
    }
}
