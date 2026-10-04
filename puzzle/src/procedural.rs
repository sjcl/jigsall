//! Release generator v1 (pre-release v5). Integer operations and shapes mirror puzzle_shape.wgsl.
use bevy_math::{UVec2, Vec2};
use puzzella_core::{PieceId, PuzzleDefinition};

pub const MAX_TAB_DEPTH: f32 = 0.22;
// Keep these constants identical to puzzle_shape.wgsl.
pub const ROOT_WIDTH_FACTOR: f32 = 0.60;
pub const ROOT_HEIGHT_FACTOR: f32 = 0.25;
pub const ROOT_BLEND_FACTOR: f32 = 0.04;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeOrientation {
    Horizontal,
    Vertical,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeId {
    pub orientation: EdgeOrientation,
    pub x: u32,
    pub y: u32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeStyle {
    Round,
    Wide,
    Narrow,
    Deep,
    Shallow,
    Pear,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeProfile {
    pub polarity: f32,
    pub style: EdgeStyle,
    pub center: f32,
    pub width: f32,
    pub depth: f32,
    pub neck_width: f32,
    pub head_width: f32,
    pub asymmetry: f32,
}

/// lowbias32, https://github.com/skeeto/hash-prospector (public domain).
pub fn mix32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}
fn edge_base(seed: u64, edge: EdgeId) -> u32 {
    let mut h = mix32(seed as u32 ^ 0x9e37_79b9);
    h = mix32(h ^ (seed >> 32) as u32);
    h = mix32(
        h ^ match edge.orientation {
            EdgeOrientation::Horizontal => 0x484f_5249,
            EdgeOrientation::Vertical => 0x5645_5254,
        },
    );
    h = mix32(h ^ edge.x);
    h = mix32(h ^ edge.y);
    h
}
pub fn edge_hash(seed: u64, edge: EdgeId, domain: u32) -> u32 {
    mix32(edge_base(seed, edge) ^ domain.wrapping_mul(0x9e37_79b9))
}
/// Six 8-bit variation samples plus 3-bit style and polarity. Zero marks an outer edge.
pub fn raw_profile(seed: u64, edge: EdgeId) -> [u32; 2] {
    let base = edge_base(seed, edge);
    let h = mix32(base);
    let sample = |d: u32| mix32(base ^ d.wrapping_mul(0x9e37_79b9)) >> 24;
    [
        ((h >> 1) % 6 + 1)
            | ((h & 1) << 3)
            | (sample(1) << 4)
            | (sample(2) << 12)
            | (sample(3) << 20),
        sample(4) | (sample(5) << 8) | (sample(6) << 16),
    ]
}
/// Uniform macro bins, with a small signed residual within each bin.
pub(crate) fn class_sample(word: u32, shift: u32, count: u32) -> (u32, f32) {
    let scaled = ((word >> shift) & 255) * count;
    (scaled >> 8, (scaled & 255) as f32 / 255.0 * 2.0 - 1.0)
}
/// Conservative half envelope including both smooth unions, in edge-length units.
pub fn profile_envelope(p: EdgeProfile) -> f32 {
    let root = p.neck_width * 0.5 + (p.width - p.neck_width) * 0.5 * ROOT_WIDTH_FACTOR;
    (root + 0.00215).max(p.head_width * (0.5347222 + p.asymmetry.abs()))
}
pub fn decode_profile(raw: [u32; 2]) -> EdgeProfile {
    let style = (raw[0] & 7).saturating_sub(1);
    let (width, depth, neck, head, style) = match style {
        0 => (0.46, 0.18, 0.14, 0.28, EdgeStyle::Round),
        1 => (0.52, 0.17, 0.18, 0.34, EdgeStyle::Wide),
        2 => (0.40, 0.18, 0.12, 0.23, EdgeStyle::Narrow),
        3 => (0.46, 0.21, 0.14, 0.27, EdgeStyle::Deep),
        4 => (0.46, 0.14, 0.16, 0.28, EdgeStyle::Shallow),
        _ => (0.48, 0.19, 0.12, 0.32, EdgeStyle::Pear),
    };
    let dimension = |word, shift| {
        let (class, micro) = class_sample(word, shift, 4);
        (0.86 + class as f32 * (0.28 / 3.0)) * (1.0 + micro * 0.01)
    };
    let base_width = width;
    let width = base_width * dimension(raw[0], 12);
    let head_width = width * (head / base_width) * dimension(raw[1], 8);
    let (nc, nm) = class_sample(raw[1], 0, 4);
    let neck_width = head_width * (neck / head) * (0.70 + nc as f32 * 0.16) * (1.0 + nm * 0.01);
    let (dc, dm) = class_sample(raw[0], 20, 4);
    let low = depth * 0.85 / 0.99;
    let high = (depth * 1.15_f32).min(0.215) / 1.01;
    let depth = (low + (high - low) * (dc as f32 / 3.0)) * (1.0 + dm * 0.01);
    let (sc, sm) = class_sample(raw[1], 16, 7);
    let asymmetry = -0.10 + sc as f32 * (0.20 / 6.0) + sm * 0.005;
    let mut profile = EdgeProfile {
        polarity: if raw[0] & 8 == 0 { 1.0 } else { -1.0 },
        style,
        center: 0.5,
        width,
        depth,
        neck_width,
        head_width,
        asymmetry,
    };
    // Scale all seven classes together; clamping individual centers would collapse bins.
    let span = 0.08_f32.min((0.5 - 0.185 - profile_envelope(profile)) / 1.05);
    let (cc, cm) = class_sample(raw[0], 4, 7);
    profile.center = 0.5 + span * (-1.0 + cc as f32 / 3.0 + cm * 0.05);
    profile
}
pub fn piece_profiles(seed: u64, grid: UVec2, cell: UVec2) -> [[u32; 2]; 4] {
    [
        (EdgeOrientation::Horizontal, cell.x, cell.y, cell.y == 0),
        (
            EdgeOrientation::Vertical,
            cell.x + 1,
            cell.y,
            cell.x + 1 == grid.x,
        ),
        (
            EdgeOrientation::Horizontal,
            cell.x,
            cell.y + 1,
            cell.y + 1 == grid.y,
        ),
        (EdgeOrientation::Vertical, cell.x, cell.y, cell.x == 0),
    ]
    .map(|(orientation, x, y, outer)| {
        if outer {
            [0, 0]
        } else {
            raw_profile(seed, EdgeId { orientation, x, y })
        }
    })
}
fn sd_box(p: Vec2, half: Vec2, radius: f32) -> f32 {
    let q = p.abs() - half + Vec2::splat(radius);
    q.max(Vec2::ZERO).length() + q.max_element().min(0.0) - radius
}
fn smooth_min(a: f32, b: f32, k: f32) -> f32 {
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}
/// A quarter-ellipse fillet outside the ellipse centered at (root_half, height).
/// Its contour is horizontal at the baseline and vertical at the neck.
fn sd_root(q: Vec2, neck_half: f32, root_half: f32, height: f32) -> f32 {
    let radii = Vec2::new(root_half - neck_half, height);
    let outside_ellipse = (1.0 - (Vec2::new(q.x.abs() - root_half, q.y - height) / radii).length())
        * radii.min_element();
    outside_ellipse
        .max(q.x.abs() - root_half)
        .max(q.y - height)
        .max(-q.y)
}
/// Rounded neck, elliptical head and concave root fillets. Ellipse distance is
/// a conservative approximation; the zero contour and complement are exact.
pub fn sd_tab(q: Vec2, p: EdgeProfile, length: f32, short: f32) -> f32 {
    let depth = p.depth * short;
    let x = q.x - p.center * length;
    let neck = p.neck_width * length;
    let radii = Vec2::new(p.head_width * length * 0.5, depth * 0.36);
    let head = ((Vec2::new(x - p.asymmetry * p.head_width * length, q.y - depth * 0.62) / radii)
        .length()
        - 1.0)
        * radii.min_element();
    let stem = sd_box(
        Vec2::new(x, q.y - depth * 0.23),
        Vec2::new(neck * 0.5, depth * 0.28),
        neck.min(depth) * 0.18,
    );
    let neck_half = neck * 0.5;
    let root_half = neck_half + (p.width * length * 0.5 - neck_half) * ROOT_WIDTH_FACTOR;
    let root = sd_root(
        Vec2::new(x, q.y),
        neck_half,
        root_half,
        depth * ROOT_HEIGHT_FACTOR,
    );
    smooth_min(
        smooth_min(head, stem, depth * 0.06),
        root,
        depth * ROOT_BLEND_FACTOR,
    )
    .max(-q.y)
}
pub fn edge_distance(q: Vec2, raw: [u32; 2], length: f32, short: f32) -> f32 {
    // sd_tab(q) >= -q.y. On this half-plane the existing min/max complement
    // is identically the baseline; avoid decoding parameters that cannot affect it.
    if raw[0] == 0 || (raw[0] & 8 == 0 && q.y <= 0.0) || (raw[0] & 8 != 0 && q.y >= 0.0) {
        return q.y;
    }
    let p = decode_profile(raw);
    if p.polarity > 0.0 {
        q.y.min(sd_tab(q, p, length, short))
    } else {
        q.y.max(-sd_tab(Vec2::new(q.x, -q.y), p, length, short))
    }
}
pub fn piece_signed_distance(local: Vec2, size: Vec2, profiles: [[u32; 2]; 4]) -> f32 {
    let h = size * 0.5;
    let s = size.min_element();
    [
        edge_distance(
            Vec2::new(local.x + h.x, local.y - h.y),
            profiles[0],
            size.x,
            s,
        ),
        edge_distance(
            Vec2::new(h.y - local.y, local.x - h.x),
            profiles[1],
            size.y,
            s,
        ),
        -edge_distance(
            Vec2::new(local.x + h.x, local.y + h.y),
            profiles[2],
            size.x,
            s,
        ),
        -edge_distance(
            Vec2::new(h.y - local.y, local.x + h.x),
            profiles[3],
            size.y,
            s,
        ),
    ]
    .into_iter()
    .fold(f32::NEG_INFINITY, f32::max)
}
pub fn piece_uv(def: &PuzzleDefinition, id: PieceId, local: Vec2) -> Vec2 {
    let cell = UVec2::new(id.0 % def.grid_size.x, id.0 / def.grid_size.x);
    let size = def.image_size.as_vec2() / def.grid_size.as_vec2();
    ((cell.as_vec2() + Vec2::splat(0.5)) * size + Vec2::new(local.x, -local.y))
        / def.image_size.as_vec2()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tab_half_width(p: EdgeProfile, length: f32, short: f32, y: f32) -> f32 {
        let mut inside = 0.0;
        let mut outside = p.width * length;
        for _ in 0..32 {
            let x = (inside + outside) * 0.5;
            if sd_tab(Vec2::new(p.center * length + x, y), p, length, short) <= 0.0 {
                inside = x;
            } else {
                outside = x;
            }
        }
        (inside + outside) * 0.5
    }
    #[test]
    fn root_width_decreases_smoothly_without_a_shelf() {
        let edge = EdgeId {
            orientation: EdgeOrientation::Horizontal,
            x: 1,
            y: 2,
        };
        for seed in 0..64 {
            let p = decode_profile(raw_profile(seed, edge));
            for (length, short) in [(100.0, 100.0), (200.0, 50.0), (50.0, 50.0)] {
                let height = p.depth * short * ROOT_HEIGHT_FACTOR;
                let neck_half = p.neck_width * length * 0.5;
                let root_half =
                    neck_half + (p.width * length * 0.5 - neck_half) * ROOT_WIDTH_FACTOR;
                // Avoid y=0, where the whole canonical baseline is a zero contour.
                let mut previous = tab_half_width(p, length, short, height * 0.00001);
                let mut previous_root = root_half;
                assert!((previous - root_half).abs() < (root_half - neck_half) * 0.01);
                for step in 1..=128 {
                    let y = height * step as f32 / 128.0;
                    let width = tab_half_width(p, length, short, y);
                    let t = y / height;
                    let root_width = root_half - (root_half - neck_half) * (t * (2.0 - t)).sqrt();
                    assert!(root_width <= previous_root);
                    assert!(
                        sd_root(Vec2::new(root_width, y), neck_half, root_half, height).abs()
                            < short * 1e-5
                    );
                    // Even the first, steepest width sample is bounded and continuous.
                    // The unchanged head/stem union can start widening at the neck.
                    assert!((previous - width).abs() < (root_half - neck_half) * 0.15);
                    previous = width;
                    previous_root = root_width;
                }
                let low = tab_half_width(p, length, short, height * 0.01);
                let mid = tab_half_width(p, length, short, height * 0.5);
                assert!(root_half - low > (root_half - neck_half) * 0.10);
                assert!(
                    low - mid > (root_half - neck_half) * 0.60,
                    "flat shoulder remains"
                );
            }
        }
    }
    #[test]
    fn root_contour_leaves_baseline_horizontally_and_joins_neck_vertically() {
        let neck = 7.0;
        let root = 16.6;
        let height = 4.5;
        let span = root - neck;
        // Parametrize the quarter ellipse without numerical contour solving.
        // At the root dy/dx approaches zero; at the neck dx/dy approaches zero.
        for fraction in [0.01_f32, 0.02, 0.04] {
            let dx = span * fraction;
            let y = height * (1.0 - (1.0 - fraction * fraction).sqrt());
            assert!(y / dx < 0.02);
            let boundary = Vec2::new(root - dx, y);
            assert!(sd_root(boundary, neck, root, height).abs() < 1e-5);
            assert!(
                sd_root(
                    boundary + Vec2::new(0.0, height * 0.001),
                    neck,
                    root,
                    height
                ) > 0.0
            );
            assert!(sd_root(boundary - Vec2::new(0.0, y * 0.5), neck, root, height) < 0.0);
            let dy = height * fraction;
            let x = root - span * (1.0 - fraction * fraction).sqrt();
            assert!((x - neck) / dy < 0.05);
            assert!(sd_root(Vec2::new(x, height - dy), neck, root, height).abs() < 1e-5);
        }
    }
    #[test]
    fn profiles_are_safe_and_cover_styles_and_all_hash_inputs() {
        let e = EdgeId {
            orientation: EdgeOrientation::Horizontal,
            x: 1,
            y: 2,
        };
        let mut styles = [false; 6];
        for seed in 0..1024 {
            let raw = raw_profile(seed, e);
            styles[((raw[0] & 7) - 1) as usize] = true;
            let p = decode_profile(raw);
            assert!(p.neck_width < p.head_width && p.head_width < p.width);
            assert!(
                p.depth < MAX_TAB_DEPTH
                    && p.center - profile_envelope(p) >= 0.185 - 1e-6
                    && p.center + profile_envelope(p) <= 0.815 + 1e-6
            );
            assert!(sd_tab(Vec2::ONE, p, 100.0, 100.0).is_finite());
            for changed in [
                raw_profile(seed ^ (1 << 32), e),
                raw_profile(seed ^ 1, e),
                raw_profile(seed, EdgeId { x: 2, ..e }),
                raw_profile(seed, EdgeId { y: 3, ..e }),
                raw_profile(
                    seed,
                    EdgeId {
                        orientation: EdgeOrientation::Vertical,
                        ..e
                    },
                ),
            ] {
                assert_ne!(raw, changed);
            }
        }
        assert!(styles.into_iter().all(|s| s));
    }
    #[test]
    fn shared_edges_complement_and_outer_edges_are_straight() {
        let size = Vec2::splat(100.0);
        let grid = UVec2::splat(2);
        for seed in 0..30 {
            let a = piece_profiles(seed, grid, UVec2::ZERO);
            let b = piece_profiles(seed, grid, UVec2::X);
            let c = piece_profiles(seed, grid, UVec2::Y);
            assert_eq!(a[1], b[3]);
            assert_eq!(a[2], c[0]);
            assert_eq!(
                crate::fingerprint::EdgeFingerprint::from_raw(a[1]),
                crate::fingerprint::EdgeFingerprint::from_raw(b[3])
            );
            assert_eq!(
                crate::fingerprint::EdgeFingerprint::from_raw(a[2]),
                crate::fingerprint::EdgeFingerprint::from_raw(c[0])
            );
            assert_eq!(a[0], [0, 0]);
            assert_eq!(a[3], [0, 0]);
            for x in -25..26 {
                for y in -25..26 {
                    let da = piece_signed_distance(Vec2::new(50.0 + x as f32, y as f32), size, a);
                    let db = piece_signed_distance(Vec2::new(-50.0 + x as f32, y as f32), size, b);
                    assert!(da * db <= 0.0, "{seed} {x} {y}: {da} {db}");
                    let q = Vec2::new(50.0 - y as f32, x as f32);
                    assert_eq!(
                        edge_distance(q, a[1], 100.0, 100.0),
                        edge_distance(q, b[3], 100.0, 100.0)
                    );
                }
            }
            assert_eq!(piece_signed_distance(Vec2::new(0.0, 51.0), size, a), 1.0);
        }
    }
    #[test]
    fn assembled_shape_tiles_image_and_uv_matches_v2() {
        let def = PuzzleDefinition {
            generator_version: puzzella_core::GENERATOR_VERSION,
            seed: 42,
            grid_size: UVec2::splat(3),
            image_size: UVec2::new(99, 63),
            snap_distance: 5.0,
            rotation_enabled: true,
        };
        let size = def.image_size.as_vec2() / def.grid_size.as_vec2();
        for y in 0..63 {
            for x in 0..99 {
                let image_point = Vec2::new(x as f32 + 0.37, y as f32 + 0.41);
                let world = Vec2::new(image_point.x - 49.5, 31.5 - image_point.y);
                let mut coverage = 0;
                for id in 0..9 {
                    let piece = def.piece(id, Vec2::ZERO);
                    let local = world - piece.correct_position;
                    let edges = piece_profiles(def.seed, def.grid_size, piece.grid_position);
                    let d = piece_signed_distance(local, size, edges);
                    assert!(d.is_finite());
                    if d <= 0.0 {
                        coverage += 1;
                        let uv = piece_uv(&def, PieceId(id), local);
                        let center = (piece.grid_position.as_vec2() + Vec2::splat(0.5)) * size;
                        let v2 = (center + Vec2::new(local.x, -local.y)) / def.image_size.as_vec2();
                        assert!(uv.abs_diff_eq(v2, 1e-6));
                        assert!(uv.cmpge(Vec2::ZERO).all() && uv.cmple(Vec2::ONE).all());
                    }
                }
                assert_eq!(coverage, 1, "image point {image_point}");
            }
        }
    }
}
