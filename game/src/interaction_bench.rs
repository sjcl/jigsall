//! Release CPU timings. Explicit transitions may enumerate members; idle/pointer never do.
use super::*;
use bevy::ecs::system::RunSystemOnce;
use std::{collections::HashSet, hint::black_box, sync::Arc, time::Instant};

fn frame(position: Vec2, pressed: bool, just_pressed: bool) -> PointerFrame {
    PointerFrame {
        position: Some(position),
        screen_position: Some(position),
        pressed,
        just_pressed,
        ctrl: false,
        over_ui: false,
        focused: true,
    }
}
fn micros(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1e6
}
fn upload(app: &mut App, store: PieceDataStore) -> (PieceDataStore, f64, usize, usize) {
    app.insert_resource(store);
    let start = Instant::now();
    app.world_mut()
        .run_system_once(crate::resources::pieces::prepare_piece_upload)
        .unwrap();
    let time = micros(start);
    let upload = app.world().resource::<PieceUpload>();
    let bytes = upload.ranges.iter().map(|r| r.states.len() * 16).sum();
    let ranges = upload.ranges.len();
    (
        app.world_mut().remove_resource::<PieceDataStore>().unwrap(),
        time,
        bytes,
        ranges,
    )
}

#[test]
#[ignore = "release CPU benchmark; writes target/million-selection-cpu.csv"]
fn million_selection_cpu_benchmark() {
    if cfg!(debug_assertions) {
        panic!("run with --release");
    }
    let mut csv=String::from("pieces,run,fill_us,receive_us,selection_commit_us,highlight_prepare_us,snapshot_ns,snapshot_copy_us,hashset_clone_us,pointer_down_us,drag_members_and_command_us,grab_ownership_z_us,grab_upload_prepare_us,pointer_ns,release_command_us,release_delta_ownership_snap_us,release_upload_prepare_us,grab_commands,release_commands,mask_bytes,owner_bytes,grab_state_bytes,release_state_bytes,release_ranges\n");
    for count in [1_000, 10_000, 100_000, 1_000_000] {
        for run in 0..5 {
            let mut app = App::new();
            app.init_resource::<PieceUpload>();
            let mut store = PieceDataStore::default();
            store.initialize(
                (0..count)
                    .map(|id| Vec2::new(id as f32, 10_000.0))
                    .collect(),
            );
            let (store, _, _, _) = upload(&mut app, store);
            let (mut store, _, _, _) = upload(&mut app, store);
            let start = Instant::now();
            store.selected_pieces.fill();
            let fill = micros(start);
            let bytes: Vec<_> = store
                .selected_pieces
                .words()
                .iter()
                .flat_map(|word| word.to_le_bytes())
                .collect();
            let start = Instant::now();
            let SelectionPayload::Rectangle(mask) =
                crate::selection::decode_payload(SelectionMode::Rectangle, &bytes, count).unwrap()
            else {
                panic!()
            };
            let receive = micros(start);
            let start = Instant::now();
            store.commit_selection(mask, None);
            let commit = micros(start);
            let (mut store, highlight, highlight_bytes, _) = upload(&mut app, store);
            assert_eq!(highlight_bytes, 0);
            assert_eq!(store.selected_pieces.count(), count);
            let start = Instant::now();
            for _ in 0..10_000 {
                black_box(store.selected_pieces.clone());
            }
            let snapshot_ns = start.elapsed().as_secs_f64() * 1e9 / 10_000.0;
            let start = Instant::now();
            black_box(
                PieceBitSet::from_words(count, store.selected_pieces.words().to_vec()).unwrap(),
            );
            let copy = micros(start);
            let old: HashSet<_> = (0..count).map(|id| PieceId(id as u32)).collect();
            let start = Instant::now();
            black_box(old.clone());
            let hashclone = micros(start);
            drop(old);
            let mut gesture = PieceInteraction::default();
            let mut selection = PuzzleSelection::default();
            let start = Instant::now();
            assert!(gesture
                .update(frame(Vec2::ZERO, true, true), &mut store, &mut selection)
                .is_empty());
            let down = micros(start);
            let request = selection.latest.unwrap();
            selection.completed = Some(SelectionResult {
                request_id: request.request_id,
                mode: request.mode,
                payload: SelectionPayload::Point(Some(PieceId(0))),
                error: None,
            });
            let start = Instant::now();
            let commands =
                gesture.update(frame(Vec2::ZERO, true, false), &mut store, &mut selection);
            let members = micros(start);
            assert_eq!(commands.len(), 1);
            let start = Instant::now();
            let result = store.apply_command(LOCAL_PLAYER, &commands[0], None);
            let grab = micros(start);
            assert_eq!(result.grabbed, count);
            drop(commands);
            let (mut store, grab_upload, grab_bytes, _) = upload(&mut app, store);
            let frozen = store.drag.members.clone();
            let start = Instant::now();
            for step in 0..100_000 {
                assert!(black_box(gesture.update(
                    frame(Vec2::splat(step as f32), true, false),
                    &mut store,
                    &mut selection
                ))
                .is_empty());
            }
            let pointer = start.elapsed().as_secs_f64() * 1e9 / 100_000.0;
            assert!(Arc::ptr_eq(&frozen, &store.drag.members));
            assert!(store.dirty_pieces.is_empty());
            let start = Instant::now();
            let commands = gesture.update(
                frame(Vec2::new(10.0, 20.0), false, false),
                &mut store,
                &mut selection,
            );
            let release_command = micros(start);
            assert_eq!(commands.len(), 1);
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
            let start = Instant::now();
            let result = store.apply_command(LOCAL_PLAYER, &commands[0], Some(&definition));
            let release = micros(start);
            assert_eq!(result.released, count);
            assert_eq!(result.placed, 0);
            assert!(store.held_by.is_empty());
            let (store, release_upload, release_bytes, ranges) = upload(&mut app, store);
            assert_eq!(
                store.states[count - 1].position,
                Vec2::new(count as f32 + 9.0, 10_020.0)
            );
            assert_eq!(ranges, 1);
            let row=format!("{count},{run},{fill:.3},{receive:.3},{commit:.3},{highlight:.3},{snapshot_ns:.3},{copy:.3},{hashclone:.3},{down:.3},{members:.3},{grab:.3},{grab_upload:.3},{pointer:.3},{release_command:.3},{release:.3},{release_upload:.3},1,1,{},{},{grab_bytes},{release_bytes},{ranges}\n",count.div_ceil(32)*4,count*8+count.div_ceil(32)*4);
            print!("{row}");
            csv.push_str(&row);
        }
    }
    std::fs::create_dir_all("../target").unwrap();
    std::fs::write("../target/million-selection-cpu.csv", csv).unwrap();
}
