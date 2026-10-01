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
            // Every singleton may choose a different neighbor under single-snap
            // semantics. Assert the movement bound for ALL pieces, not one last ID.
            for (id, state) in store.states.iter().enumerate() {
                let released_position = Vec2::new(id as f32 + 10.0, 10_020.0);
                assert!(
                    puzzella_core::offset_distance_squared(state.position, released_position)
                        < f64::from(definition.snap_distance).powi(2)
                );
            }
            assert_eq!(ranges, 1);
            let row=format!("{count},{run},{fill:.3},{receive:.3},{commit:.3},{highlight:.3},{snapshot_ns:.3},{copy:.3},{hashclone:.3},{down:.3},{members:.3},{grab:.3},{grab_upload:.3},{pointer:.3},{release_command:.3},{release:.3},{release_upload:.3},1,1,{},{},{grab_bytes},{release_bytes},{ranges}\n",count.div_ceil(32)*4,count*8+count.div_ceil(32)*4);
            print!("{row}");
            csv.push_str(&row);
        }
    }
    std::fs::create_dir_all("../target").unwrap();
    std::fs::write("../target/million-selection-cpu.csv", csv).unwrap();
}

fn connected_definition(count: usize) -> PuzzleDefinition {
    PuzzleDefinition {
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
    }
}

#[test]
#[ignore = "release CPU benchmark; writes target/connected-snapping-cpu.csv"]
fn connected_snapping_cpu_benchmark() {
    if cfg!(debug_assertions) {
        panic!("run with --release");
    }
    let mut csv = String::from("pieces,scenario,run,connectivity_init_us,union_chain_us,iteration_us,expansion_us,grab_us,pointer_ns,release_us,connectivity_bytes,released,component_size\n");
    for count in [1_000, 10_000, 100_000, 1_000_000] {
        for scenario in [
            "disconnected",
            "single_snap",
            "closure",
            "board",
            "connected",
        ] {
            for run in 0..5 {
                let d = connected_definition(count);
                let start = Instant::now();
                let connectivity = PieceConnectivity::new(count);
                let init = micros(start);
                let mut s = PieceDataStore::default();
                s.initialize(
                    (0..count as u32)
                        .map(|id| {
                            if scenario == "board" {
                                return d.correct_position(PieceId(id))
                                    + Vec2::new(
                                        if (id % d.grid_size.x + id / d.grid_size.x)
                                            .is_multiple_of(2)
                                        {
                                            4.0
                                        } else {
                                            -4.0
                                        },
                                        0.0,
                                    );
                            }
                            d.correct_position(PieceId(id))
                                + Vec2::new(
                                    10_000.0
                                        + if scenario == "disconnected"
                                            || (scenario == "single_snap" && id > 1)
                                        {
                                            id as f32 * 20.0
                                        } else {
                                            0.0
                                        },
                                    10_000.0,
                                )
                        })
                        .collect(),
                );
                s.connectivity = connectivity;
                let start = Instant::now();
                if scenario == "connected" {
                    for id in 1..count as u32 {
                        s.connectivity.union(PieceId(id - 1), PieceId(id));
                    }
                }
                let union = micros(start);
                let start = Instant::now();
                assert_eq!(
                    black_box(s.connectivity.iter_component(PieceId(0)).count()),
                    if scenario == "connected" { count } else { 1 }
                );
                let iteration = micros(start);
                let mut requested = PieceBitSet::new(count);
                requested.insert(PieceId(0));
                let start = Instant::now();
                let expanded = black_box(s.connectivity.expand(&requested));
                let expansion = micros(start);
                assert_eq!(
                    expanded.count(),
                    if scenario == "connected" { count } else { 1 }
                );
                if !matches!(scenario, "single_snap" | "closure") {
                    s.selected_pieces.fill();
                } else {
                    s.selected_pieces = requested;
                }
                let mut gesture = PieceInteraction::default();
                let mut selection = PuzzleSelection::default();
                gesture.update(frame(Vec2::ZERO, true, true), &mut s, &mut selection);
                let request = selection.latest.unwrap();
                selection.completed = Some(SelectionResult {
                    request_id: request.request_id,
                    mode: request.mode,
                    payload: SelectionPayload::Point(Some(PieceId(0))),
                    error: None,
                });
                let commands =
                    gesture.update(frame(Vec2::ZERO, true, false), &mut s, &mut selection);
                assert_eq!(commands.len(), 1);
                let start = Instant::now();
                let grabbed = s.apply_command(LOCAL_PLAYER, &commands[0], Some(&d));
                let grab = micros(start);
                assert_eq!(
                    grabbed.grabbed,
                    if matches!(scenario, "single_snap" | "closure") {
                        1
                    } else {
                        count
                    }
                );
                s.dirty_pieces.clear();
                let frozen = s.drag.members.clone();
                let states = s.states.as_ptr();
                let start = Instant::now();
                for step in 0..100_000 {
                    assert!(black_box(gesture.update(
                        frame(Vec2::splat(step as f32), true, false),
                        &mut s,
                        &mut selection
                    ))
                    .is_empty());
                }
                let pointer = start.elapsed().as_secs_f64() * 1e9 / 100_000.0;
                assert!(Arc::ptr_eq(&frozen, &s.drag.members));
                assert_eq!(s.states.as_ptr(), states);
                assert!(s.dirty_pieces.is_empty());
                let commands = gesture.update(
                    frame(
                        if scenario == "board" {
                            Vec2::ZERO
                        } else {
                            Vec2::ONE
                        },
                        false,
                        false,
                    ),
                    &mut s,
                    &mut selection,
                );
                assert_eq!(commands.len(), 1);
                let start = Instant::now();
                let released = s.apply_command(LOCAL_PLAYER, &commands[0], Some(&d));
                let release = micros(start);
                assert_eq!(released.released, grabbed.grabbed);
                assert_eq!(released.placed, if scenario == "board" { count } else { 0 });
                assert!(s.held_by.is_empty());
                let component_size = s.connectivity.component_size(PieceId(0));
                assert_eq!(
                    component_size,
                    match scenario {
                        "disconnected" => 1,
                        "single_snap" => 2,
                        _ => count,
                    }
                );
                let row = format!("{count},{scenario},{run},{init:.3},{union:.3},{iteration:.3},{expansion:.3},{grab:.3},{pointer:.3},{release:.3},{},{},{component_size}\n", s.connectivity.storage_bytes(), released.released);
                print!("{row}");
                csv.push_str(&row);
            }
        }
    }
    std::fs::create_dir_all("../target").unwrap();
    std::fs::write("../target/connected-snapping-cpu.csv", csv).unwrap();
}

#[test]
#[ignore = "release small-operation benchmark; writes target/small-release-cpu.csv"]
fn small_component_release_cpu_benchmark() {
    assert!(!black_box(cfg!(debug_assertions)), "run with --release");
    let mut csv = String::from("pieces,members,command,run,release_us,released,component_size\n");
    for count in [1_000, 10_000, 100_000, 1_000_000] {
        for members in [1, 8, 32] {
            for command in ["group", "scalar"] {
                for run in 0..10 {
                    let d = connected_definition(count);
                    let mut store = PieceDataStore::default();
                    store.initialize(
                        (0..count as u32)
                            .map(|id| {
                                d.correct_position(PieceId(id))
                                    + Vec2::new(
                                        10_000.0
                                            + if id > members { id as f32 * 20.0 } else { 0.0 },
                                        10_000.0,
                                    )
                            })
                            .collect(),
                    );
                    for id in 1..members {
                        store.connectivity.union(PieceId(0), PieceId(id));
                    }
                    let id = PieceId(members / 2);
                    assert_eq!(
                        store
                            .apply_command(LOCAL_PLAYER, &PieceCommand::Grab(id), Some(&d))
                            .grabbed,
                        members as usize
                    );
                    let mut requested = PieceBitSet::new(count);
                    requested.insert(id);
                    let release = if command == "group" {
                        PieceCommand::ReleaseGroup {
                            members: requested,
                            delta: Vec2::ONE,
                        }
                    } else {
                        store.apply_command(
                            LOCAL_PLAYER,
                            &PieceCommand::Move {
                                id,
                                position: store.states[id.0 as usize].position + Vec2::ONE,
                            },
                            Some(&d),
                        );
                        PieceCommand::Release(id)
                    };
                    let target = store.states[members as usize].position;
                    store.dirty_pieces.clear();
                    let start = Instant::now();
                    let outcome = black_box(store.apply_command(LOCAL_PLAYER, &release, Some(&d)));
                    let elapsed = micros(start);
                    assert_eq!(outcome.released, members as usize);
                    assert_eq!(outcome.placed, 0);
                    assert!(store.held_by.is_empty());
                    let size = store.connectivity.component_size(PieceId(0));
                    assert_eq!(size, members as usize + 1);
                    assert_eq!(store.states[members as usize].position, target);
                    let row = format!(
                        "{count},{members},{command},{run},{elapsed:.3},{},{size}\n",
                        outcome.released
                    );
                    print!("{row}");
                    csv.push_str(&row);
                }
            }
        }
    }
    std::fs::create_dir_all("../target").unwrap();
    std::fs::write("../target/small-release-cpu.csv", csv).unwrap();
}
