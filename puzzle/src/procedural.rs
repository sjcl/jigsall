//! Generator v3. All integer operations and shape equations mirror puzzle_shape.wgsl.
use bevy_math::{UVec2, Vec2};
use puzzella_core::{PieceId, PuzzleDefinition};

pub const MAX_TAB_DEPTH: f32 = 0.22;
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
    let v = |word: u32, shift: u32| ((word >> shift) & 255u32) as f32 / 255.0 * 2.0 - 1.0;
    EdgeProfile {
        polarity: if raw[0] & 8 == 0 { 1.0 } else { -1.0 },
        style,
        center: 0.5 + v(raw[0], 4) * 0.025,
        width: width * (1.0 + v(raw[0], 12) * 0.035),
        depth: depth * (1.0 + v(raw[0], 20) * 0.035),
        neck_width: neck * (1.0 + v(raw[1], 0) * 0.035),
        head_width: head * (1.0 + v(raw[1], 8) * 0.035),
        asymmetry: v(raw[1], 16) * 0.025,
    }
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
/// Rounded neck, elliptical head and rounded shoulder. The ellipse distance is
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
    let shoulder = sd_box(
        Vec2::new(x, q.y),
        Vec2::new(p.width * length * 0.5, depth * 0.06),
        depth * 0.06,
    );
    smooth_min(smooth_min(head, stem, depth * 0.06), shoulder, depth * 0.04).max(-q.y)
}
pub fn edge_distance(q: Vec2, raw: [u32; 2], length: f32, short: f32) -> f32 {
    if raw[0] == 0 {
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
                    && p.center - p.width * 0.5 > 0.2
                    && p.center + p.width * 0.5 < 0.8
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
            assert_eq!(a[1], b[3]);
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
