//! cargo run --release --locked -p puzzella-game --example initialization_bench
use bevy::prelude::*;
use puzzella_core::{PieceId, PuzzleDefinition, GENERATOR_VERSION};
use puzzella_game::resources::{
    pieces::prepare_piece_upload, DensePieceStates, LocalPlayerId, PieceDataStore, PieceUpload,
};
use std::{hint::black_box, time::Instant};

fn main() {
    println!("pieces,run,worker_ms,handoff_ms,upload_prep_ms,state_bytes");
    for grid in [
        UVec2::new(40, 25),
        UVec2::splat(100),
        UVec2::new(400, 250),
        UVec2::splat(1000),
    ] {
        let definition = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: grid,
            image_size: UVec2::splat(4096),
            snap_distance: 5.0,
        };
        for run in 0..5 {
            let mut app = App::new();
            app.init_resource::<LocalPlayerId>()
                .init_resource::<PieceDataStore>()
                .init_resource::<PieceUpload>()
                .insert_resource(definition.clone())
                .add_systems(Update, prepare_piece_upload);
            // Warm up schedule initialization outside the measured upload.
            app.update();
            let start = Instant::now();
            let states = DensePieceStates::generate(black_box(&definition));
            let worker_ms = start.elapsed().as_secs_f64() * 1000.0;
            let allocation = states.as_ptr();
            let start = Instant::now();
            app.world_mut()
                .resource_mut::<PieceDataStore>()
                .initialize_dense(states);
            let handoff_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            app.update();
            let prep_ms = start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(
                app.world().resource::<PieceDataStore>().states.as_ptr(),
                allocation
            );
            assert_eq!(
                app.world()
                    .resource::<PieceUpload>()
                    .initial
                    .as_ref()
                    .unwrap()
                    .as_ptr(),
                allocation
            );
            app.update();
            let local_player = app.world().resource::<LocalPlayerId>().0;
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            let mut state = store.state(PieceId(0)).unwrap();
            state.position += Vec2::ONE;
            store.set_state(PieceId(0), state, local_player);
            assert_eq!(store.states.as_ptr(), allocation);
            println!(
                "{},{run},{worker_ms:.4},{handoff_ms:.4},{prep_ms:.4},{}",
                definition.piece_count(),
                definition.piece_count() * 16
            );
        }
    }
}
