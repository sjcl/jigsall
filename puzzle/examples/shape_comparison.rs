//! A/B preview of v2 contours and current analytic silhouettes, plus six root close-ups.
use bevy_math::{UVec2, Vec2};
use puzzella_puzzle::{
    procedural::{
        decode_profile, edge_distance, piece_profiles, piece_signed_distance, raw_profile, EdgeId,
        EdgeOrientation, EdgeProfile, EdgeStyle,
    },
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
    let version = puzzella_core::GENERATOR_VERSION;
    let mut svg=format!("<svg xmlns='http://www.w3.org/2000/svg' width='880' height='390' viewBox='0 0 880 390'><rect width='880' height='390' fill='#faf9f6'/><g font-family='sans-serif' fill='#26343d'><text x='25' y='30' font-size='20'>v2 — native Bezier reference</text><text x='465' y='30' font-size='20'>v{version} — analytic GPU definition</text><text x='25' y='375' font-size='13'>Seed 42 · 4 × 3 · v{version} preview sampled at 1 px · generator versions intentionally differ</text></g>");
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
    let roots = path.with_file_name(format!(
        "{}-roots.svg",
        path.file_stem().unwrap().to_string_lossy()
    ));
    std::fs::write(&roots, root_comparison()).unwrap();
    println!("Wrote {}", roots.display());
}

fn root_comparison() -> String {
    let version = puzzella_core::GENERATOR_VERSION;
    let mut svg = format!("<svg xmlns='http://www.w3.org/2000/svg' width='1080' height='570' viewBox='0 0 1080 570'><rect width='1080' height='570' fill='#faf9f6'/><g font-family='sans-serif' fill='#26343d'><text x='20' y='28' font-size='20'>Six styles · tab and blank · 3× magnification</text><text x='20' y='51' font-size='14'>Red dashed: v2 Bezier · Blue fill / dark contour: v{version} · Same style, independently found seeds</text></g>");
    let edge = EdgeId {
        orientation: EdgeOrientation::Horizontal,
        x: 0,
        y: 1,
    };
    let styles = [
        EdgeStyle::Round,
        EdgeStyle::Wide,
        EdgeStyle::Narrow,
        EdgeStyle::Deep,
        EdgeStyle::Shallow,
        EdgeStyle::Pear,
    ];
    for (i, style) in styles.into_iter().enumerate() {
        let native_seed = (0..10000)
            .find(|&seed| {
                let p = EdgeProfile::from_seed(seed, edge);
                p.style == style && p.polarity > 0.0
            })
            .unwrap();
        let native_profile = EdgeProfile::from_seed(native_seed, edge);
        let seed = (0..10000)
            .find(|&seed| {
                let p = decode_profile(raw_profile(seed, edge));
                p.style == style && p.polarity > 0.0
            })
            .unwrap();
        let raw = raw_profile(seed, edge);
        let profile = decode_profile(raw);
        let panel_x = (i % 3) as f32 * 360.0;
        let panel_y = 75.0 + (i / 3) as f32 * 240.0;
        write!(svg, "<g font-family='sans-serif' fill='#26343d'><text x='{}' y='{panel_y}' font-size='18'>{style:?}</text><text x='{}' y='{}' font-size='11'>v2 seed {native_seed} / v{version} seed {seed}</text></g>", panel_x+20.0,panel_x+20.0,panel_y+18.0).unwrap();
        for blank in [false, true] {
            let id = i * 2 + usize::from(blank);
            let center = Vec2::new(panel_x + if blank { 260.0 } else { 100.0 }, panel_y + 110.0);
            let sign = if blank { -1.0 } else { 1.0 };
            let mut edge_raw = raw;
            if blank {
                edge_raw[0] ^= 8;
            }
            write!(svg,"<defs><clipPath id='root{id}'><rect x='{}' y='{}' width='150' height='138'/></clipPath></defs><g clip-path='url(#root{id})'>",center.x-75.0,center.y-69.0).unwrap();
            // Sample the complete signed edge, so a blank is an actual subtraction.
            for row in -69..69 {
                let mut run_start = 0;
                let mut previous = 0u8;
                for column in -75..=75 {
                    let q = Vec2::new(
                        profile.center * 100.0 + (column as f32 + 0.5) / 3.0,
                        -(row as f32 + 0.5) / 3.0,
                    );
                    let d = edge_distance(q, edge_raw, 100.0, 100.0);
                    let paint = if column == 75 || d > 0.0 {
                        0
                    } else if d.abs() < 0.22 {
                        2
                    } else {
                        1
                    };
                    if paint != previous {
                        if previous != 0 {
                            write!(
                                svg,
                                "<rect x='{}' y='{}' width='{}' height='1' fill='{}'/>",
                                center.x + run_start as f32,
                                center.y + row as f32,
                                column - run_start,
                                if previous == 2 { "#26343d" } else { "#cce4f4" }
                            )
                            .unwrap();
                        }
                        run_start = column;
                        previous = paint;
                    }
                }
            }
            let PieceEdge::Curved(curves) =
                piece_edges(native_seed, UVec2::new(1, 2), UVec2::Y, Vec2::splat(100.0))[0].clone()
            else {
                unreachable!()
            };
            let screen = |p: Vec2| {
                Vec2::new(
                    center.x + (p.x + 50.0 - native_profile.center * 100.0) * 3.0,
                    center.y - (p.y - 50.0) * sign * 3.0,
                )
            };
            let start = screen(curves[0].from);
            write!(svg, "<path d='M {} {} ", start.x, start.y).unwrap();
            for c in curves {
                let a = screen(c.control1);
                let b = screen(c.control2);
                let end = screen(c.to);
                write!(
                    svg,
                    "C {} {} {} {} {} {} ",
                    a.x, a.y, b.x, b.y, end.x, end.y
                )
                .unwrap();
            }
            write!(svg,"' fill='none' stroke='#bf493d' stroke-width='1.2' stroke-dasharray='4 3'/></g><text x='{}' y='{}' font-family='sans-serif' font-size='13' fill='#26343d'>{}</text>",center.x-15.0,center.y+85.0,if blank {"blank"} else {"tab"}).unwrap();
        }
    }
    svg.push_str("</svg>");
    svg
}
