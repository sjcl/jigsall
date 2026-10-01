// Baseline harness for ca56bf8; copy into game/src/baseline_release_bench.rs
// in an isolated checkout and include from interaction.rs. See docs/CONNECTED_SNAPPING.md.
use super::*;
use std::{hint::black_box, time::Instant};

fn frame(point: Vec2, pressed: bool, just_pressed: bool) -> PointerFrame {
    PointerFrame {
        position: Some(point),
        screen_position: Some(point),
        pressed,
        just_pressed,
        ctrl: false,
        over_ui: false,
        focused: true,
    }
}
#[test]
#[ignore = "release baseline with the same disconnected fixture as connected_snapping_cpu_benchmark"]
fn connected_baseline_release_benchmark() {
    assert!(!cfg!(debug_assertions));
    let mut csv = String::from("pieces,run,release_us\n");
    for count in [1_000, 10_000, 100_000, 1_000_000] {
        for run in 0..5 {
            let definition = PuzzleDefinition {
                generator_version: GENERATOR_VERSION,
                seed: 42,
                grid_size: match count {
                    1_000 => UVec2::new(40, 25),
                    10_000 => UVec2::splat(100),
                    100_000 => UVec2::new(400, 250),
                    _ => UVec2::splat(1000),
                },
                image_size: UVec2::splat(4096),
                snap_distance: 5.0,
            };
            let mut store = PieceDataStore::default();
            store.initialize(
                (0..count as u32)
                    .map(|id| {
                        definition.piece(id, Vec2::ZERO).correct_position
                            + Vec2::new(10_000.0 + id as f32 * 20.0, 10_000.0)
                    })
                    .collect(),
            );
            store.selected_pieces.fill();
            let mut gesture = PieceInteraction::default();
            let mut selection = PuzzleSelection::default();
            gesture.update(frame(Vec2::ZERO, true, true), &mut store, &mut selection);
            let request = selection.latest.unwrap();
            selection.completed = Some(SelectionResult {
                request_id: request.request_id,
                mode: request.mode,
                payload: SelectionPayload::Point(Some(PieceId(0))),
                error: None,
            });
            let commands =
                gesture.update(frame(Vec2::ZERO, true, false), &mut store, &mut selection);
            assert_eq!(commands.len(), 1);
            assert_eq!(
                store
                    .apply_command(LOCAL_PLAYER, &commands[0], Some(&definition))
                    .grabbed,
                count
            );
            store.dirty_pieces.clear();
            for step in 0..100_000 {
                assert!(black_box(gesture.update(
                    frame(Vec2::splat(step as f32), true, false),
                    &mut store,
                    &mut selection
                ))
                .is_empty());
            }
            let commands =
                gesture.update(frame(Vec2::ONE, false, false), &mut store, &mut selection);
            assert_eq!(commands.len(), 1);
            let start = Instant::now();
            let released = store.apply_command(LOCAL_PLAYER, &commands[0], Some(&definition));
            let time = start.elapsed().as_secs_f64() * 1e6;
            assert_eq!((released.released, released.placed), (count, 0));
            let row = format!("{count},{run},{time:.3}\n");
            print!("{row}");
            csv.push_str(&row);
        }
    }
    std::fs::create_dir_all("../target").unwrap();
    std::fs::write("../target/connected-baseline-release.csv", csv).unwrap();
}
