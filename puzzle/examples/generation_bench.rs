//! Release benchmark: fixed seed, 100px square cells, warmup + three samples.
//! cargo run --release --locked -p puzzella-puzzle --example generation_bench
#[path = "support/logging.rs"]
mod logging;
use bevy_asset::Assets;
use bevy_math::{UVec2, Vec2};
use bevy_mesh::{Mesh, MeshVertexAttribute, VertexFormat};
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillTessellator, FillVertex, StrokeOptions, StrokeTessellator,
    StrokeVertex, VertexBuffers,
};
use puzzella_core::{PieceId, PuzzleDefinition};
use puzzella_puzzle::generation::REFERENCE_GENERATOR_VERSION;
use puzzella_puzzle::{
    generate_pieces,
    generation::stroke_width,
    placement::generate_placement_grid,
    shapes::{contour_path, piece_edges},
    TessellationWorker,
};
use rayon::prelude::*;
use std::time::Instant;

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    logging::init();
    let available = std::thread::available_parallelism().map_or(1, usize::from);
    bevy_log::info!("available_threads={available} cell_size=100 seed=42 warmup=1 samples=3");
    let mut counts = vec![1, 4, available];
    counts.sort_unstable();
    counts.dedup();
    for threads in counts {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        for (columns, rows) in [(10, 10), (40, 25), (100, 50)] {
            let definition = PuzzleDefinition {
                generator_version: REFERENCE_GENERATOR_VERSION,
                seed: 42,
                grid_size: UVec2::new(columns, rows),
                image_size: UVec2::new(columns * 100, rows * 100),
                snap_distance: 50.0,
            };
            for run in 0..4 {
                let start = Instant::now();
                let pieces = pool.install(|| generate_pieces(&definition).unwrap());
                let cpu_ms = ms(start);
                let fill_vertices: usize = pieces
                    .pieces
                    .iter()
                    .map(|p| p.geometry.fill.positions.len())
                    .sum();
                let stroke_vertices: usize = pieces
                    .pieces
                    .iter()
                    .map(|p| p.geometry.stroke.positions.len())
                    .sum();
                let start = Instant::now();
                let mut meshes = Vec::with_capacity(pieces.pieces.len() * 2);
                for piece in pieces.pieces {
                    let mut fill = piece.geometry.fill.into_mesh();
                    // Same CPU attribute work as progressive main-thread registration.
                    fill.insert_attribute(
                        MeshVertexAttribute::new(
                            "PuzzlePieceId",
                            0x5055_5a5a,
                            VertexFormat::Uint32,
                        ),
                        vec![piece.piece_component.id.0; fill.count_vertices()],
                    );
                    meshes.push(fill);
                    meshes.push(piece.geometry.stroke.into_mesh());
                }
                let mesh_ms = ms(start);
                let start = Instant::now();
                let mut assets = Assets::<Mesh>::default();
                let handles: Vec<_> = meshes.into_iter().map(|mesh| assets.add(mesh)).collect();
                let asset_ms = ms(start);
                bevy_log::info!("NATIVE threads={threads} pieces={} run={run} cpu_ms={cpu_ms:.3} mesh_ms={mesh_ms:.3} asset_ms={asset_ms:.3} fill_vertices={fill_vertices} stroke_vertices={stroke_vertices}", definition.piece_count());
                std::hint::black_box((assets, handles));
            }
        }
    }
    compare_tolerances();
}

fn compare_tolerances() {
    let definition = PuzzleDefinition {
        generator_version: REFERENCE_GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(40, 25),
        image_size: UVec2::new(4000, 2500),
        snap_distance: 50.0,
    };
    let size = Vec2::splat(100.0);
    for (columns, rows) in [(10, 10), (40, 25), (100, 50)] {
        for run in 0..4 {
            let start = Instant::now();
            let positions = generate_placement_grid(
                columns,
                rows,
                100.0,
                100.0,
                columns as f32 * 100.0,
                rows as f32 * 100.0,
                42,
            );
            bevy_log::info!(
                "PLACEMENT pieces={} run={run} placement_ms={:.3}",
                columns * rows,
                ms(start)
            );
            std::hint::black_box(positions);
        }
    }
    let positions = generate_placement_grid(40, 25, 100.0, 100.0, 4000.0, 2500.0, 42);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    for tolerance in [0.1, 0.2, 0.25] {
        for run in 0..4 {
            let start = Instant::now();
            let pieces: Vec<_> = pool.install(|| {
                positions
                    .par_iter()
                    .enumerate()
                    .with_min_len(16)
                    .map_init(
                        || TessellationWorker::with_tolerance(tolerance),
                        |worker, (i, &position)| {
                            worker
                                .generate_piece(&definition, PieceId(i as u32), position)
                                .unwrap()
                        },
                    )
                    .collect()
            });
            let geometry_ms = ms(start);
            let vertices: usize = pieces.iter().map(|p| p.geometry.fill.positions.len()).sum();
            bevy_log::info!("TOLERANCE tolerance={tolerance} threads=4 run={run} geometry_ms={geometry_ms:.3} fill_vertices={vertices}");
            std::hint::black_box(pieces);
        }
    }
    // Isolated sequential stages; timing instrumentation lives in this example.
    let mut fill = FillTessellator::new();
    let mut stroke = StrokeTessellator::new();
    let mut fill_buffers: VertexBuffers<[f32; 2], u16> = VertexBuffers::new();
    let mut stroke_buffers: VertexBuffers<[f32; 2], u16> = VertexBuffers::new();
    let (mut contour_ms, mut fill_ms, mut stroke_ms) = (0.0, 0.0, 0.0);
    for i in 0..1000 {
        let start = Instant::now();
        let path = contour_path(&piece_edges(
            42,
            definition.grid_size,
            UVec2::new(i % 40, i / 40),
            size,
        ));
        contour_ms += ms(start);
        fill_buffers.vertices.clear();
        fill_buffers.indices.clear();
        let start = Instant::now();
        fill.tessellate_path(
            &path,
            &FillOptions::tolerance(0.25),
            &mut BuffersBuilder::new(&mut fill_buffers, |v: FillVertex| {
                [v.position().x, v.position().y]
            }),
        )
        .unwrap();
        fill_ms += ms(start);
        stroke_buffers.vertices.clear();
        stroke_buffers.indices.clear();
        let start = Instant::now();
        stroke
            .tessellate_path(
                &path,
                &StrokeOptions::tolerance(0.25).with_line_width(stroke_width(size)),
                &mut BuffersBuilder::new(&mut stroke_buffers, |v: StrokeVertex| {
                    [v.position().x, v.position().y]
                }),
            )
            .unwrap();
        stroke_ms += ms(start);
    }
    bevy_log::info!("STAGE pieces=1000 sequential_contour_ms={contour_ms:.3} sequential_fill_ms={fill_ms:.3} sequential_stroke_ms={stroke_ms:.3}");
}
