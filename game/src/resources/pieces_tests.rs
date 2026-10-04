use super::*;
use crate::resources::{AppState, GameData, PerformanceMonitor};
use puzzella_core::{GENERATOR_VERSION, LOCAL_PLAYER};
#[test]
fn local_selection_tracks_authority_changes_and_keeps_local_holds() {
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::ZERO; 4]);
    store.selected_pieces.fill();
    let mut state = store.state(PieceId(0)).unwrap();
    state.held_by = Some(LOCAL_PLAYER);
    store.set_state(PieceId(0), state, puzzella_core::LOCAL_PLAYER);
    assert!(store.selected_pieces.contains(&PieceId(0)));
    store.sync_highlights();
    state.held_by = Some(PlayerId(1));
    store.set_state(PieceId(0), state, puzzella_core::LOCAL_PLAYER);
    assert!(!store.selected_pieces.contains(&PieceId(0)));
    assert!(store.highlights_dirty);
    state.held_by = None;
    store.set_state(PieceId(0), state, puzzella_core::LOCAL_PLAYER);
    assert!(!store.selected_pieces.contains(&PieceId(0)));
    let mut placed = store.state(PieceId(1)).unwrap();
    placed.placed = true;
    store.set_state(PieceId(1), placed, puzzella_core::LOCAL_PLAYER);
    let disabled = store.state(PieceId(2)).unwrap();
    store.states[2].flags &= !ENABLED;
    store.set_state(PieceId(2), disabled, puzzella_core::LOCAL_PLAYER);
    assert_eq!(
        store.selected_pieces.iter().collect::<Vec<_>>(),
        [PieceId(3)]
    );
}

#[test]
fn local_selection_survives_local_grab_but_not_other_players_grab() {
    for bulk in [false, true] {
        let mut store = PieceDataStore::default();
        store.initialize(vec![Vec2::ZERO; 4]);
        store.selected_pieces.fill();
        let original = store.selected_pieces.clone();
        let mut local = PieceBitSet::new(4);
        local.insert(PieceId(0));
        let command = if bulk {
            PieceCommand::GrabGroup { members: local }
        } else {
            PieceCommand::Grab(PieceId(0))
        };
        assert_eq!(
            store
                .apply_command(LOCAL_PLAYER, &command, None, puzzella_core::LOCAL_PLAYER)
                .grabbed,
            1
        );
        assert_eq!(store.selected_pieces.count(), 4);
        store.sync_highlights();
        let mut remote = PieceBitSet::new(4);
        remote.extend([PieceId(0), PieceId(1)]);
        let command = if bulk {
            PieceCommand::GrabGroup { members: remote }
        } else {
            PieceCommand::Grab(PieceId(1))
        };
        assert_eq!(
            store
                .apply_command(PlayerId(1), &command, None, puzzella_core::LOCAL_PLAYER)
                .grabbed,
            1
        );
        assert!(store.highlights_dirty);
        assert_eq!(
            store.selected_pieces.iter().collect::<Vec<_>>(),
            [PieceId(0), PieceId(2), PieceId(3)]
        );
        assert_eq!(original.count(), 4, "rollback snapshot stays immutable");
        store.apply_command(
            PlayerId(1),
            &PieceCommand::Grab(PieceId(0)),
            None,
            puzzella_core::LOCAL_PLAYER,
        );
        assert!(store.selected_pieces.contains(&PieceId(0)));
        store.apply_command(
            PlayerId(1),
            &PieceCommand::Release(PieceId(1)),
            None,
            puzzella_core::LOCAL_PLAYER,
        );
        assert!(!store.selected_pieces.contains(&PieceId(1)));
    }
}

#[test]
fn local_selection_additive_original_is_revalidated_after_delayed_readback() {
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::ZERO; 6]);
    store.selected_pieces.extend((0..4).map(PieceId));
    let original = store.selected_pieces.clone();
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::Grab(PieceId(0)),
        None,
        puzzella_core::LOCAL_PLAYER,
    );
    store.apply_command(
        PlayerId(1),
        &PieceCommand::Grab(PieceId(1)),
        None,
        puzzella_core::LOCAL_PLAYER,
    );
    let mut placed = store.state(PieceId(2)).unwrap();
    placed.placed = true;
    store.set_state(PieceId(2), placed, puzzella_core::LOCAL_PLAYER);
    let disabled = store.state(PieceId(3)).unwrap();
    store.states[3].flags &= !ENABLED;
    store.set_state(PieceId(3), disabled, puzzella_core::LOCAL_PLAYER);
    let mut members = PieceBitSet::new(6);
    members.extend((0..5).map(PieceId));
    store.commit_selection(members, Some(&original), puzzella_core::LOCAL_PLAYER);
    assert_eq!(
        store.selected_pieces.iter().collect::<Vec<_>>(),
        [PieceId(0), PieceId(4)]
    );
    assert_eq!(original.count(), 4);
}

#[test]
fn dirty_mask_reuses_its_allocation_for_single_piece_and_bulk_uploads() {
    let mut app = App::new();
    app.init_resource::<PieceDataStore>()
        .init_resource::<PieceUpload>()
        .add_systems(Update, prepare_piece_upload);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO; 1_000_000]);
    app.update();
    app.update();
    let allocation = app
        .world()
        .resource::<PieceDataStore>()
        .dirty_pieces
        .words()
        .as_ptr();
    for id in [PieceId(10), PieceId(999_999), PieceId(33)] {
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .dirty_pieces
            .insert(id);
        app.update();
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(store.dirty_pieces.words().as_ptr(), allocation);
        assert!(store.dirty_pieces.is_empty());
        let upload = app.world().resource::<PieceUpload>();
        assert_eq!(upload.ranges.len(), 1);
        assert_eq!(upload.ranges[0].start, id.0);
        assert_eq!(upload.ranges[0].states.len(), 1);
    }
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .dirty_pieces
        .fill();
    app.update();
    assert_eq!(
        app.world()
            .resource::<PieceDataStore>()
            .dirty_pieces
            .words()
            .as_ptr(),
        allocation
    );
    assert!(app
        .world()
        .resource::<PieceDataStore>()
        .dirty_pieces
        .is_empty());
    assert_eq!(
        app.world().resource::<PieceUpload>().ranges[0].states.len(),
        1_000_000
    );
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .dirty_pieces
        .insert(PieceId(99));
    app.update();
    let upload = app.world().resource::<PieceUpload>();
    assert_eq!(upload.ranges.len(), 1);
    assert_eq!(upload.ranges[0].start, 99);
    assert_eq!(upload.ranges[0].states.len(), 1);
    assert_eq!(
        app.world()
            .resource::<PieceDataStore>()
            .dirty_pieces
            .words()
            .as_ptr(),
        allocation
    );
}

#[test]
fn rejected_grab_and_authority_release_cannot_drag_another_players_hold() {
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::ZERO]);
    let other = PlayerId(1);
    let mut members = PieceBitSet::new(1);
    members.fill();
    store.apply_command(
        other,
        &PieceCommand::Grab(PieceId(0)),
        None,
        puzzella_core::LOCAL_PLAYER,
    );
    store.drag = DragTransform {
        members: members.words().clone(),
        delta: Vec2::ONE,
    };
    assert_eq!(
        store
            .apply_command(
                LOCAL_PLAYER,
                &PieceCommand::GrabGroup {
                    members: members.clone()
                },
                None,
                puzzella_core::LOCAL_PLAYER
            )
            .grabbed,
        0
    );
    assert!(store.drag.members.is_empty());
    store.apply_command(
        other,
        &PieceCommand::Release(PieceId(0)),
        None,
        puzzella_core::LOCAL_PLAYER,
    );
    store.drag = DragTransform {
        members: members.words().clone(),
        delta: Vec2::ONE,
    };
    store.apply_command(
        other,
        &PieceCommand::GrabGroup {
            members: members.clone(),
        },
        None,
        puzzella_core::LOCAL_PLAYER,
    );
    assert_eq!(store.drag.members[0], 0);
    store.apply_command(
        other,
        &PieceCommand::Release(PieceId(0)),
        None,
        puzzella_core::LOCAL_PLAYER,
    );
    for bulk in [false, true] {
        store.drag = DragTransform {
            members: members.words().clone(),
            delta: Vec2::ONE,
        };
        store.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::GrabGroup {
                members: members.clone(),
            },
            None,
            puzzella_core::LOCAL_PLAYER,
        );
        let command = if bulk {
            PieceCommand::ReleaseGroup {
                members: members.clone(),
                delta: Vec2::ZERO,
            }
        } else {
            PieceCommand::Release(PieceId(0))
        };
        store.apply_command(LOCAL_PLAYER, &command, None, puzzella_core::LOCAL_PLAYER);
        store.apply_command(
            other,
            &PieceCommand::Grab(PieceId(0)),
            None,
            puzzella_core::LOCAL_PLAYER,
        );
        assert_eq!(store.drag.members[0], 0);
        assert_eq!(store.state(PieceId(0)).unwrap().position, Vec2::ZERO);
        store.apply_command(
            other,
            &PieceCommand::Release(PieceId(0)),
            None,
            puzzella_core::LOCAL_PLAYER,
        );
    }
}
#[test]
fn fragmented_bulk_uploads_have_a_fixed_range_allocation_budget() {
    let mut app = App::new();
    app.init_resource::<PieceDataStore>()
        .init_resource::<PieceUpload>()
        .add_systems(Update, prepare_piece_upload);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO; 1_000_000]);
    app.update();
    app.update();
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .dirty_pieces
        .extend((0..1_000_000).step_by(2).map(PieceId));
    app.update();
    let upload = app.world().resource::<PieceUpload>();
    assert_eq!(upload.ranges.len(), 1);
    assert_eq!(upload.ranges[0].start, 0);
    assert_eq!(upload.ranges[0].states.len(), 999_999);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .dirty_pieces
        .insert(PieceId(33));
    app.update();
    let upload = app.world().resource::<PieceUpload>();
    assert_eq!(upload.ranges.len(), 1);
    assert_eq!(upload.ranges[0].states.len(), 1);
    assert_eq!(upload.ranges[0].start, 33);
}

fn definition() -> PuzzleDefinition {
    PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::splat(2),
        image_size: UVec2::splat(100),
        snap_distance: 5.0,
        rotation_enabled: true,
    }
}
#[test]
fn bulk_ownership_snap_threshold_and_exactly_once_commit() {
    let def = definition();
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..4)
            .map(|id| def.piece(id, Vec2::ZERO).correct_position)
            .collect(),
    );
    let mut placed = store.state(PieceId(1)).unwrap();
    placed.placed = true;
    store.set_state(PieceId(1), placed, puzzella_core::LOCAL_PLAYER);
    let other = PlayerId(u64::MAX);
    let mut held = store.state(PieceId(2)).unwrap();
    held.held_by = Some(other);
    store.set_state(PieceId(2), held, puzzella_core::LOCAL_PLAYER);
    let mut members = PieceBitSet::new(4);
    members.fill();
    store.selected_pieces = members.clone();
    let mut loose = store.state(PieceId(3)).unwrap();
    loose.position.x += 4.0;
    store.set_state(PieceId(3), loose, puzzella_core::LOCAL_PLAYER);
    let grabbed = store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::GrabGroup {
            members: members.clone(),
        },
        Some(&def),
        puzzella_core::LOCAL_PLAYER,
    );
    assert_eq!(grabbed.grabbed, 2);
    assert_eq!(store.state(PieceId(2)).unwrap().held_by, Some(other));
    assert_eq!(
        store
            .apply_command(
                other,
                &PieceCommand::Release(PieceId(0)),
                Some(&def),
                puzzella_core::LOCAL_PLAYER
            )
            .released,
        0
    );
    let command = PieceCommand::ReleaseGroup {
        members,
        delta: Vec2::X,
    };
    let released = store.apply_command(
        LOCAL_PLAYER,
        &command,
        Some(&def),
        puzzella_core::LOCAL_PLAYER,
    );
    assert_eq!((released.released, released.placed), (2, 1));
    assert_eq!(store.placed_count, 2);
    assert!(store.state(PieceId(0)).unwrap().placed);
    assert!(!store.selected_pieces.contains(&PieceId(0)));
    // Exactly at the threshold remains loose, just as single release does.
    assert!(!store.state(PieceId(3)).unwrap().placed);
    let before = store.states.clone();
    assert_eq!(
        store.apply_command(
            LOCAL_PLAYER,
            &command,
            Some(&def),
            puzzella_core::LOCAL_PLAYER
        ),
        AppliedCommand::default()
    );
    assert_eq!(store.states, before);
    assert_eq!(store.held_by.len(), 1);
    assert!(!store.held_by.has_player(LOCAL_PLAYER));
    assert!(store.held_by.has_player(other));
}
#[test]
fn nonexistent_ids_wrong_dimensions_and_invalid_delta_cannot_corrupt_state() {
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::ZERO; 4]);
    for command in [
        PieceCommand::Grab(PieceId(u32::MAX)),
        PieceCommand::GrabGroup {
            members: PieceBitSet::new(5),
        },
    ] {
        assert_eq!(
            store.apply_command(LOCAL_PLAYER, &command, None, puzzella_core::LOCAL_PLAYER),
            AppliedCommand::default()
        );
    }
    let mut members = PieceBitSet::new(4);
    members.fill();
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::GrabGroup {
            members: members.clone(),
        },
        None,
        puzzella_core::LOCAL_PLAYER,
    );
    let before = store.states.clone();
    for delta in [Vec2::splat(f32::NAN), Vec2::splat(f32::INFINITY)] {
        assert_eq!(
            store.apply_command(
                LOCAL_PLAYER,
                &PieceCommand::ReleaseGroup {
                    members: members.clone(),
                    delta
                },
                None,
                puzzella_core::LOCAL_PLAYER
            ),
            AppliedCommand::default()
        );
        assert_eq!(store.states, before);
        assert_eq!(store.held_by.len(), 4);
    }
    assert_eq!(
        store.apply_command(
            PlayerId(1),
            &PieceCommand::ReleaseGroup {
                members,
                delta: Vec2::ONE
            },
            None,
            puzzella_core::LOCAL_PLAYER
        ),
        AppliedCommand::default()
    );
    assert_eq!(store.states, before);
}
#[test]
fn bulk_grab_checks_authoritative_owner_even_if_render_mirror_is_stale() {
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::ZERO]);
    store.held_by.insert(PieceId(0), PlayerId(1));
    let mut members = PieceBitSet::new(1);
    members.fill();
    assert_eq!(
        store
            .apply_command(
                LOCAL_PLAYER,
                &PieceCommand::GrabGroup { members },
                None,
                puzzella_core::LOCAL_PLAYER
            )
            .grabbed,
        0
    );
    assert_eq!(store.state(PieceId(0)).unwrap().held_by, Some(PlayerId(1)));
}
#[test]
fn all_selected_upload_is_only_a_mask_and_idle_shares_it() {
    let mut app = App::new();
    app.init_resource::<PieceDataStore>()
        .init_resource::<PieceUpload>()
        .add_systems(Update, prepare_piece_upload);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO; 1_000_000]);
    app.update();
    app.update();
    let states = app.world().resource::<PieceDataStore>().states.as_ptr();
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .selected_pieces
        .fill();
    app.update();
    let words = app.world().resource::<PieceUpload>().selected.clone();
    assert_eq!(words.len() * 4, 125_000);
    assert!(app.world().resource::<PieceUpload>().ranges.is_empty());
    for _ in 0..3 {
        app.update();
        assert!(Arc::ptr_eq(
            &words,
            &app.world().resource::<PieceUpload>().selected
        ));
        assert_eq!(
            app.world().resource::<PieceDataStore>().states.as_ptr(),
            states
        );
    }
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .selected_pieces
        .clear();
    app.update();
    assert!(app.world().resource::<PieceUpload>().ranges.is_empty());
    assert!(app
        .world()
        .resource::<PieceUpload>()
        .selected
        .iter()
        .all(|word| *word == 0));
}
#[test]
fn final_mask_revalidates_delayed_ownership_and_placed_state() {
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::ZERO; 4]);
    let mut original = PieceBitSet::new(4);
    original.insert(PieceId(0));
    let mut mask = PieceBitSet::new(4);
    mask.fill();
    store.apply_command(
        PlayerId(1),
        &PieceCommand::Grab(PieceId(1)),
        None,
        puzzella_core::LOCAL_PLAYER,
    );
    let mut placed = store.state(PieceId(2)).unwrap();
    placed.placed = true;
    store.set_state(PieceId(2), placed, puzzella_core::LOCAL_PLAYER);
    store.commit_selection(mask, Some(&original), puzzella_core::LOCAL_PLAYER);
    assert_eq!(
        store.selected_pieces.iter().collect::<Vec<_>>(),
        [PieceId(0), PieceId(3)]
    );
    store.commit_selection(PieceBitSet::new(4), None, puzzella_core::LOCAL_PLAYER);
    assert!(store.selected_pieces.is_empty());
    store.selected_pieces = original.clone();
    assert!(store.selected_pieces.contains(&PieceId(0)));
}
#[test]
fn bulk_snap_completion_updates_progress_without_per_piece_events() {
    use crate::{components::*, systems::game_logic::*};
    let def = definition();
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..4)
            .map(|id| def.piece(id, Vec2::ZERO).correct_position)
            .collect(),
    );
    let mut members = PieceBitSet::new(4);
    members.fill();
    let mut app = App::new();
    app.add_plugins(bevy::state::app::StatesPlugin)
        .init_state::<AppState>()
        .insert_resource(store)
        .insert_resource(def)
        .init_resource::<GameData>()
        .init_resource::<crate::resources::LocalPlayerId>()
        .init_resource::<PerformanceMonitor>()
        .add_message::<puzzella_core::ClientCommand>()
        .add_message::<PieceMoveCompleted>()
        .add_message::<PiecePlacedEvent>()
        .add_systems(
            Update,
            (apply_piece_commands, update_game_state_event_driven).chain(),
        );
    let mut messages = app
        .world_mut()
        .resource_mut::<Messages<puzzella_core::ClientCommand>>();
    messages.write(puzzella_core::ClientCommand {
        player: LOCAL_PLAYER,
        command: PieceCommand::GrabGroup {
            members: members.clone(),
        },
    });
    messages.write(puzzella_core::ClientCommand {
        player: LOCAL_PLAYER,
        command: PieceCommand::ReleaseGroup {
            members,
            delta: Vec2::ZERO,
        },
    });
    app.update();
    assert!(app.world().resource::<GameData>().puzzle_completed);
    assert_eq!(app.world().resource::<GameData>().puzzle_progress, 1.0);
    assert!(app
        .world()
        .resource::<Messages<PieceMoveCompleted>>()
        .is_empty());
    assert!(app
        .world()
        .resource::<Messages<PiecePlacedEvent>>()
        .is_empty());
    app.update();
    assert_eq!(
        *app.world().resource::<State<AppState>>().get(),
        AppState::GameComplete
    );
}
