//! A/B preview of the preserved v2 contour and sampled v3 analytic silhouette.
use bevy_math::{UVec2, Vec2};
use puzzella_puzzle::{
    procedural::{piece_profiles, piece_signed_distance},
    shapes::{piece_edges, PieceEdge},
};
use std::{fmt::Write, path::Path};

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/shape-comparison.svg".into());
    let grid = UVec2::new(4, 3);
    let size = Vec2::splat(100.0);
    let seed = 42;
    let colors = [
        "#d3e7f5", "#edcdb9", "#d4dfb2", "#d6c2e9", "#f3dfaa", "#b5dbd1",
    ];
    let mut svg=String::from("<svg xmlns='http://www.w3.org/2000/svg' width='880' height='390' viewBox='0 0 880 390'><rect width='880' height='390' fill='#faf9f6'/><g font-family='sans-serif' fill='#26343d'><text x='25' y='30' font-size='20'>v2 — native Bezier reference</text><text x='465' y='30' font-size='20'>v3 — analytic GPU definition</text><text x='25' y='375' font-size='13'>Seed 42 · 4 × 3 · v3 preview sampled at 1 px · generator versions intentionally differ</text></g>");
    for y in 0..grid.y {
        for x in 0..grid.x {
            let cell = UVec2::new(x, y);
            let color = colors[((y * grid.x + x) % 6) as usize];
            let center = Vec2::new(75.0 + x as f32 * 100.0, 115.0 + y as f32 * 100.0);
            let edges = piece_edges(seed, grid, cell, size);
            let mut path = String::new();
            write!(path, "M -50 50 ").unwrap();
            for edge in edges {
                match edge {
                    PieceEdge::Straight { to, .. } => write!(path, "L {} {} ", to.x, to.y).unwrap(),
                    PieceEdge::Curved(curves) => {
                        for c in curves {
                            write!(
                                path,
                                "C {} {} {} {} {} {} ",
                                c.control1.x,
                                c.control1.y,
                                c.control2.x,
                                c.control2.y,
                                c.to.x,
                                c.to.y
                            )
                            .unwrap();
                        }
                    }
                }
            }
            write!(svg,"<path transform='translate({} {}) scale(1 -1)' d='{path}Z' fill='{color}' stroke='#26343d' stroke-width='1'/>",center.x,center.y).unwrap();
            let profiles = piece_profiles(seed, grid, cell);
            for row in -72..72 {
                let mut run_start = 0;
                let mut previous = 0u8;
                for column in -72..=72 {
                    let d = piece_signed_distance(
                        Vec2::new(column as f32 + 0.5, -row as f32 - 0.5),
                        size,
                        profiles,
                    );
                    let paint = if column == 72 || d > 0.0 {
                        0
                    } else if d.abs() < 0.7 {
                        2
                    } else {
                        1
                    };
                    if paint != previous {
                        if previous != 0 {
                            write!(
                                svg,
                                "<rect x='{}' y='{}' width='{}' height='1' fill='{}'/>",
                                center.x + 440.0 + run_start as f32,
                                center.y + row as f32,
                                column - run_start,
                                if previous == 2 { "#26343d" } else { color }
                            )
                            .unwrap();
                        }
                        run_start = column;
                        previous = paint;
                    }
                }
            }
        }
    }
    svg.push_str("</svg>");
    let path = Path::new(&output);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, svg).unwrap();
    println!("Wrote {}", path.display());
}
