//! Native, order-independent shared edges and piece-local cubic contours.
use bevy_math::{UVec2, Vec2};
use lyon::{math::point, path::Path};

pub use crate::procedural::{EdgeId, EdgeOrientation, EdgeProfile, EdgeStyle};

fn mix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

impl EdgeProfile {
    pub fn from_seed(seed: u64, edge: EdgeId) -> Self {
        let tag = match edge.orientation {
            EdgeOrientation::Horizontal => 0x484f_5249_5a4f_4e54,
            EdgeOrientation::Vertical => 0x5645_5254_4943_414c,
        };
        let hash = mix64(seed ^ tag ^ mix64((u64::from(edge.x) << 32) | u64::from(edge.y)));
        let style = match (hash >> 1) % 6 {
            0 => EdgeStyle::Round,
            1 => EdgeStyle::Wide,
            2 => EdgeStyle::Narrow,
            3 => EdgeStyle::Deep,
            4 => EdgeStyle::Shallow,
            _ => EdgeStyle::Pear,
        };
        let (width, depth, neck, head) = match style {
            EdgeStyle::Round => (0.46, 0.18, 0.14, 0.28),
            EdgeStyle::Wide => (0.52, 0.17, 0.18, 0.34),
            EdgeStyle::Narrow => (0.40, 0.18, 0.12, 0.23),
            EdgeStyle::Deep => (0.46, 0.21, 0.14, 0.27),
            EdgeStyle::Shallow => (0.46, 0.14, 0.16, 0.28),
            EdgeStyle::Pear => (0.48, 0.19, 0.12, 0.32),
        };
        // Each parameter has its own stable hash domain. No shared RNG state,
        // platform-sized hashes, transcendental functions or scheduling input.
        let variation = |domain: u64| {
            let bits = (mix64(hash.wrapping_add(domain * 0x9e37_79b9)) >> 40) as u32;
            bits as f32 / 16_777_215.0 * 2.0 - 1.0
        };
        Self {
            polarity: if hash & 1 == 0 { 1.0 } else { -1.0 },
            style,
            center: 0.5 + variation(1) * 0.025,
            width: width * (1.0 + variation(2) * 0.035),
            depth: depth * (1.0 + variation(3) * 0.035),
            neck_width: neck * (1.0 + variation(4) * 0.035),
            head_width: head * (1.0 + variation(5) * 0.035),
            asymmetry: variation(6) * 0.025,
        }
    }

    fn cubics(self, length: f32, short_side: f32) -> [CubicBezier; 8] {
        let c = self.center * length;
        let w = self.width * length;
        let n = self.neck_width * length;
        let h = self.head_width * length;
        let d = self.depth * short_side * self.polarity;
        let skew = self.asymmetry * h;
        let anchors = [
            Vec2::ZERO,
            Vec2::new(c - w * 0.5, 0.0),
            Vec2::new(c - n * 0.5, d * 0.23),
            Vec2::new(c - h * 0.5 + skew, d * 0.65),
            Vec2::new(c + skew, d),
            Vec2::new(c + h * 0.5 + skew, d * 0.65),
            Vec2::new(c + n * 0.5, d * 0.23),
            Vec2::new(c + w * 0.5, 0.0),
            Vec2::new(length, 0.0),
        ];
        let shoulder = (w - n) * 0.18;
        let tangents = [
            Vec2::new(anchors[1].x / 3.0, 0.0),
            Vec2::new(shoulder, 0.0),
            Vec2::new(0.0, d * 0.11),
            Vec2::new(0.0, d * 0.21),
            Vec2::new(h * 0.33, 0.0),
            Vec2::new(0.0, -d * 0.21),
            Vec2::new(0.0, -d * 0.11),
            Vec2::new(shoulder, 0.0),
            Vec2::new((length - anchors[7].x) / 3.0, 0.0),
        ];
        // Identical tangents on both sides of every join give C1 continuity.
        // Disjoint height bands and left/right halves prevent intersections.
        // Shoulders stay >20% from corners; depth is <22% of the short side.
        std::array::from_fn(|i| CubicBezier {
            from: anchors[i],
            control1: anchors[i] + tangents[i],
            control2: anchors[i + 1] - tangents[i + 1],
            to: anchors[i + 1],
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier {
    pub from: Vec2,
    pub control1: Vec2,
    pub control2: Vec2,
    pub to: Vec2,
}
impl CubicBezier {
    pub fn reversed(self) -> Self {
        Self {
            from: self.to,
            control1: self.control2,
            control2: self.control1,
            to: self.from,
        }
    }
    fn transformed(self, orientation: EdgeOrientation, origin: Vec2) -> Self {
        let transform = |v: Vec2| {
            origin
                + match orientation {
                    EdgeOrientation::Horizontal => v,
                    EdgeOrientation::Vertical => Vec2::new(v.y, -v.x),
                }
        };
        Self {
            from: transform(self.from),
            control1: transform(self.control1),
            control2: transform(self.control2),
            to: transform(self.to),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // Bounded stack contour avoids four per-piece heap allocations.
pub enum PieceEdge {
    Straight { from: Vec2, to: Vec2 },
    Curved([CubicBezier; 8]),
}
impl PieceEdge {
    pub fn reversed(self) -> Self {
        match self {
            Self::Straight { from, to } => Self::Straight { from: to, to: from },
            Self::Curved(curves) => Self::Curved(std::array::from_fn(|i| curves[7 - i].reversed())),
        }
    }
    fn from(&self) -> Vec2 {
        match self {
            Self::Straight { from, .. } => *from,
            Self::Curved(curves) => curves[0].from,
        }
    }
}

/// Top/right/bottom/left in clockwise order. Both sides recompute the same
/// canonical edge, then reverse its commands. No edge table or barrier.
pub fn piece_edges(seed: u64, grid: UVec2, cell: UVec2, size: Vec2) -> [PieceEdge; 4] {
    let half = size * 0.5;
    let specs = [
        (
            EdgeOrientation::Horizontal,
            cell.x,
            cell.y,
            Vec2::new(-half.x, half.y),
            cell.y == 0,
            false,
        ),
        (
            EdgeOrientation::Vertical,
            cell.x + 1,
            cell.y,
            half,
            cell.x + 1 == grid.x,
            false,
        ),
        (
            EdgeOrientation::Horizontal,
            cell.x,
            cell.y + 1,
            -half,
            cell.y + 1 == grid.y,
            true,
        ),
        (
            EdgeOrientation::Vertical,
            cell.x,
            cell.y,
            Vec2::new(-half.x, half.y),
            cell.x == 0,
            true,
        ),
    ];
    specs.map(|(orientation, x, y, origin, outer, reverse)| {
        let length = match orientation {
            EdgeOrientation::Horizontal => size.x,
            EdgeOrientation::Vertical => size.y,
        };
        let edge = if outer {
            let delta = match orientation {
                EdgeOrientation::Horizontal => Vec2::new(length, 0.0),
                EdgeOrientation::Vertical => Vec2::new(0.0, -length),
            };
            PieceEdge::Straight {
                from: origin,
                to: origin + delta,
            }
        } else {
            let profile = EdgeProfile::from_seed(seed, EdgeId { orientation, x, y });
            PieceEdge::Curved(
                profile
                    .cubics(length, size.min_element())
                    .map(|curve| curve.transformed(orientation, origin)),
            )
        };
        if reverse {
            edge.reversed()
        } else {
            edge
        }
    })
}

pub fn contour_path(edges: &[PieceEdge; 4]) -> Path {
    let mut builder = Path::builder();
    let start = edges[0].from();
    builder.begin(point(start.x, start.y));
    for edge in edges {
        match edge {
            PieceEdge::Straight { to, .. } => {
                builder.line_to(point(to.x, to.y));
            }
            PieceEdge::Curved(curves) => {
                for (index, curve) in curves.iter().enumerate() {
                    // The first/last cubics are exactly collinear baselines.
                    // A line avoids redundant flattening and rounding vertices.
                    if index == 0 || index == 7 {
                        builder.line_to(point(curve.to.x, curve.to.y));
                        continue;
                    }
                    builder.cubic_bezier_to(
                        point(curve.control1.x, curve.control1.y),
                        point(curve.control2.x, curve.control2.y),
                        point(curve.to.x, curve.to.y),
                    );
                }
            }
        }
    }
    builder.close();
    builder.build()
}

#[cfg(test)]
mod tests;
