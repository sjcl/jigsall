//! Export native contour commands and tessellation for raster visual inspection.
//! cargo run --release --locked -p puzzella-puzzle --example geometry_preview > target/geometry-preview.txt
use bevy_math::{UVec2, Vec2};
use puzzella_core::{PieceId, PuzzleDefinition, GENERATOR_VERSION};
use puzzella_puzzle::{
    shapes::{piece_edges, EdgeId, EdgeOrientation, EdgeProfile, PieceEdge},
    TessellationWorker,
};
use std::io::{self, Write};

fn main() {
    let mut out = io::BufWriter::new(io::stdout().lock());
    let grid = UVec2::new(4, 3);
    let definition = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: grid,
        image_size: UVec2::new(400, 300),
        snap_distance: 50.0,
    };
    for tolerance in [0.1, 0.2, 0.25] {
        let mut worker = TessellationWorker::with_tolerance(tolerance);
        for i in 0..12 {
            let piece = worker
                .generate_piece(&definition, PieceId(i), Vec2::ZERO)
                .unwrap();
            writeln!(
                out,
                "piece,{tolerance},{i},{},{}",
                piece.piece_component.correct_position.x, piece.piece_component.correct_position.y
            )
            .unwrap();
            for p in piece.geometry.fill.positions {
                writeln!(out, "vertex,{},{}", p[0], p[1]).unwrap();
            }
            for triangle in piece.geometry.fill.indices.chunks_exact(3) {
                writeln!(
                    out,
                    "triangle,{},{},{}",
                    triangle[0], triangle[1], triangle[2]
                )
                .unwrap();
            }
            write_edges(
                &mut out,
                &piece_edges(
                    42,
                    grid,
                    piece.piece_component.grid_position,
                    Vec2::splat(100.0),
                ),
            );
        }
    }
    let mut found = [false; 6];
    for seed in 0..1000 {
        let edge = EdgeId {
            orientation: EdgeOrientation::Horizontal,
            x: 0,
            y: 1,
        };
        let profile = EdgeProfile::from_seed(seed, edge);
        let index = profile.style as usize;
        if found[index] || profile.polarity < 0.0 {
            continue;
        }
        found[index] = true;
        writeln!(out, "style,{index},{:?},{seed}", profile.style).unwrap();
        write_edges(
            &mut out,
            &piece_edges(seed, UVec2::splat(2), UVec2::new(0, 1), Vec2::splat(100.0)),
        );
        if found.iter().all(|&f| f) {
            break;
        }
    }
}

fn write_edges(out: &mut impl Write, edges: &[PieceEdge; 4]) {
    for edge in edges {
        match edge {
            PieceEdge::Straight { from, to } => {
                writeln!(out, "line,{},{},{},{}", from.x, from.y, to.x, to.y).unwrap()
            }
            PieceEdge::Curved(curves) => {
                for curve in curves {
                    writeln!(
                        out,
                        "cubic,{},{},{},{},{},{},{},{}",
                        curve.from.x,
                        curve.from.y,
                        curve.control1.x,
                        curve.control1.y,
                        curve.control2.x,
                        curve.control2.y,
                        curve.to.x,
                        curve.to.y
                    )
                    .unwrap();
                }
            }
        }
    }
}
