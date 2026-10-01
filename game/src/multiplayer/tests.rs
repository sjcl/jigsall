use super::*;
use crate::resources::{
    pieces::{prepare_piece_upload, ENABLED, HELD, MAX_Z, PLACED, PREVIEW, SELECTED},
    PieceUpload,
};
use bevy::prelude::*;
use puzzella_core::{
    apply_piece_command,
    session::{
        AuthorityEpoch, AuthorityEventEnvelope, ClientCommandEnvelope, ClientCommandSequence,
        CommandSequenceStatus, ImageHash, MigrationState, RecoverySource, SessionDefinition,
        SessionId,
    },
    CommandOutcome, PieceCommand, GENERATOR_VERSION,
};
use std::collections::HashSet;

const SESSION: SessionId = SessionId(55);
const SESSION_DEFINITION: SessionDefinition = SessionDefinition {
    id: SESSION,
    image_hash: ImageHash([0x42; 32]),
};
const A: PlayerId = PlayerId(1);
const B: PlayerId = PlayerId(2);
const C: PlayerId = PlayerId(3);
const D: PlayerId = PlayerId(4);

fn definition() -> PuzzleDefinition {
    PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(2, 2),
        image_size: UVec2::splat(200),
        snap_distance: 5.0,
    }
}
fn fixture() -> PieceDataStore {
    let mut store = PieceDataStore::default();
    store.initialize(vec![
        Vec2::new(-50.0, 50.0),
        Vec2::new(301.0, 402.0),
        Vec2::new(503.0, 604.0),
        Vec2::new(705.0, 806.0),
    ]);
    let mut placed = store.state(PieceId(0)).unwrap();
    placed.placed = true;
    store.set_state(PieceId(0), placed);
    for (id, player) in [(PieceId(1), A), (PieceId(2), B)] {
        let mut held = store.state(id).unwrap();
        held.held_by = Some(player);
        store.set_state(id, held);
        store.bring_piece_to_front(id);
    }
    store.selected_pieces.insert(PieceId(1));
    store.highlights_dirty = true;
    store.sync_highlights(); // Exercise private previous highlight caches too.
    store
}
fn dragging_fixture() -> PieceDataStore {
    let mut store = fixture();
    store.drag.members = vec![0b110].into(); // Membership bitset for pieces 1 and 2.
    store.drag.delta = Vec2::new(11.0, 22.0);
    store
}
fn envelope(epoch: u64, player: PlayerId, sequence: u64) -> ClientCommandEnvelope {
    ClientCommandEnvelope {
        session: SESSION,
        authority_epoch: AuthorityEpoch(epoch),
        player,
        sequence: ClientCommandSequence::Control(sequence),
        command: PieceCommand::Grab(PieceId(1)),
    }
}
fn assert_restored(snapshot: &GameSnapshot, store: &PieceDataStore) {
    assert_eq!(store.len(), snapshot.pieces.len());
    for (index, expected) in snapshot.pieces.iter().enumerate() {
        let actual = &store.states[index];
        assert_eq!(actual.position, expected.position);
        assert_eq!(actual.z_order, expected.z_order);
        assert_eq!(
            actual.flags,
            ENABLED
                | if expected.flags & SNAPSHOT_PLACED != 0 {
                    PLACED
                } else {
                    0
                }
        );
    }
    assert_eq!(store.next_z_order, snapshot.next_z_order);
    assert_eq!(
        store.placed_count,
        snapshot
            .pieces
            .iter()
            .filter(|state| state.flags & SNAPSHOT_PLACED != 0)
            .count()
    );
    assert!(store.held_by.is_empty());
    assert!(store.selected_pieces.is_empty());
    assert!(store.drag.members.is_empty());
    assert_eq!(store.drag.delta, Vec2::ZERO);
    assert!(store.dirty_pieces.is_empty());
    assert!(!store.highlights_dirty);
}

#[test]
fn graceful_a_to_b_preserves_dense_authority_and_refreshes_upload_for_b_and_c() {
    let definition = definition();
    let mut a_store = fixture();
    // Cached progress is deliberately stale: it is absent from the protocol.
    a_store.placed_count = 99;
    let mut session = AuthoritySession::new(SESSION_DEFINITION, A, AuthorityCursor::new(3, 100));
    session.accept_command(&envelope(3, B, 0)).unwrap();
    session.begin_graceful(B).unwrap();
    let snapshot =
        GameSnapshot::capture(&a_store, &definition, SESSION_DEFINITION, session.cursor()).unwrap();
    assert!(snapshot
        .pieces
        .iter()
        .all(|state| state.flags & !SNAPSHOT_PLACED == 0));
    assert_eq!(a_store.held_by.len(), 2); // Capturing does not change the old host.
    assert!(a_store.selected_pieces.contains(&PieceId(1)));
    assert_eq!(a_store.states[1].flags & SELECTED, 0);
    assert!(a_store.drag.members.is_empty());
    assert_eq!(a_store.drag.delta, Vec2::ZERO);

    let mut app = App::new();
    app.insert_resource(dragging_fixture())
        .insert_resource(definition.clone())
        .init_resource::<PieceUpload>()
        .add_systems(Update, prepare_piece_upload);
    app.update(); // B already has GPU presentation state.
    let old_gpu_epoch = app.world().resource::<PieceUpload>().epoch;
    // B validates and retains the final snapshot before sending its ACK.
    snapshot
        .validate(SnapshotExpectation {
            session: SESSION,
            image_hash: SESSION_DEFINITION.image_hash,
            cursor: session.cursor(),
            definition: &definition,
        })
        .unwrap();
    session
        .acknowledge_snapshot(SESSION, B, snapshot.cursor)
        .unwrap();
    // Validation/ACK must not restore B's store or start another GPU upload.
    assert_eq!(
        app.world().resource::<PieceDataStore>().epoch,
        old_gpu_epoch
    );
    assert_eq!(app.world().resource::<PieceDataStore>().held_by.len(), 2);
    assert_eq!(
        app.world().resource::<PieceDataStore>().drag.delta,
        Vec2::new(11.0, 22.0)
    );
    app.update();
    assert!(app.world().resource::<PieceUpload>().initial.is_none());
    assert!(app.world().resource::<PieceUpload>().ranges.is_empty());
    session.host_changed(B).unwrap();
    assert!(!session.is_active());
    let frozen_session = session.clone();
    let new_cursor = install_migration_snapshot(
        &mut session,
        &mut app.world_mut().resource_mut::<PieceDataStore>(),
        &definition,
        &snapshot,
    )
    .unwrap();
    assert_eq!(new_cursor, AuthorityCursor::new(4, 0));
    assert_eq!(snapshot.image_hash, SESSION_DEFINITION.image_hash);
    assert_eq!(session.session_definition(), SESSION_DEFINITION);
    assert_eq!(session.host(), B);
    let store = app.world().resource::<PieceDataStore>();
    assert_restored(&snapshot, store);
    assert!(store.epoch > old_gpu_epoch);
    app.update();
    let upload = app.world().resource::<PieceUpload>();
    assert_eq!(upload.epoch, app.world().resource::<PieceDataStore>().epoch);
    assert_eq!(upload.definition.as_ref(), Some(&definition));
    assert!(upload.drag.members.is_empty());
    assert_eq!(upload.drag.delta, Vec2::ZERO);
    assert_eq!(
        upload.initial.as_ref().unwrap().as_ref(),
        app.world().resource::<PieceDataStore>().states.as_ref()
    );
    assert!(upload.ranges.is_empty());

    // A second client also activates the same authority generation.
    let mut c_session = frozen_session;
    let mut c_store = dragging_fixture();
    install_migration_snapshot(&mut c_session, &mut c_store, &definition, &snapshot).unwrap();
    assert_restored(&snapshot, &c_store);
    assert_eq!(c_session.cursor(), session.cursor());
    assert_eq!(
        session.accept_command(&envelope(3, A, 1)),
        Err(ProtocolError::WrongEpoch)
    );
    assert_eq!(
        session.validate_event(&AuthorityEventEnvelope {
            session: SESSION,
            host: A,
            cursor: AuthorityCursor::new(3, 101),
            event: (),
        }),
        Err(ProtocolError::WrongEpoch)
    );
    assert_eq!(
        session.accept_command(&envelope(4, B, 0)),
        Ok(CommandSequenceStatus::InOrder)
    );
    // Freed pieces can be grabbed again.
    let mut state = c_store.state(PieceId(1)).unwrap();
    assert_eq!(
        apply_piece_command(&mut state, C, &PieceCommand::Grab(PieceId(1))),
        Some(CommandOutcome::Grabbed)
    );
    // The next idle frame has no full upload and no dirty range.
    app.update();
    assert!(app.world().resource::<PieceUpload>().initial.is_none());
    assert!(app.world().resource::<PieceUpload>().ranges.is_empty());
    // Previous selection caches are reset; new local highlighting touches just one piece.
    let mut store = app.world_mut().resource_mut::<PieceDataStore>();
    store.selected_pieces.insert(PieceId(3));
    store.highlights_dirty = true;
    app.update();
    let upload = app.world().resource::<PieceUpload>();
    assert!(upload.ranges.is_empty());
    assert_eq!(&*upload.selected, &[1 << 3]);
}

#[test]
fn abrupt_a_loss_recovers_latest_cursor_then_activates_externally_chosen_c() {
    let definition = definition();
    let candidates = [
        RecoverySource {
            session: SESSION,
            player: B,
            cursor: AuthorityCursor::new(4, 801),
        },
        RecoverySource {
            session: SESSION,
            player: C,
            cursor: AuthorityCursor::new(4, 805),
        },
        RecoverySource {
            session: SESSION,
            player: D,
            cursor: AuthorityCursor::new(4, 805),
        },
    ];
    let mut session = AuthoritySession::new(SESSION_DEFINITION, A, AuthorityCursor::new(4, 801));
    session.host_lost().unwrap();
    assert_eq!(
        session.accept_command(&envelope(4, B, 1)),
        Err(ProtocolError::Frozen)
    );
    session.host_changed(C).unwrap();
    let chosen = session
        .choose_recovery_source(candidates.into_iter().rev())
        .unwrap();
    assert_eq!(chosen, candidates[1]);
    let mut peer = AuthoritySession::new(SESSION_DEFINITION, A, AuthorityCursor::new(4, 801));
    peer.host_lost().unwrap();
    peer.host_changed(C).unwrap();
    assert_eq!(peer.choose_recovery_source(candidates).unwrap(), chosen);

    let mut source_store = dragging_fixture();
    let authoritative_states = source_store.states.clone();
    // An abrupt recovery source discards prediction without committing its delta.
    source_store.drag = default();
    let snapshot = GameSnapshot::capture(
        &source_store,
        &definition,
        SESSION_DEFINITION,
        chosen.cursor,
    )
    .unwrap();
    assert_eq!(source_store.states, authoritative_states);
    for (piece, authoritative) in snapshot.pieces.iter().zip(&authoritative_states) {
        assert_eq!(piece.position, authoritative.position);
    }
    let mut recovered_store = dragging_fixture();
    let old_epoch = recovered_store.epoch;
    assert_eq!(
        install_migration_snapshot(&mut session, &mut recovered_store, &definition, &snapshot)
            .unwrap(),
        AuthorityCursor::new(5, 0)
    );
    assert_restored(&snapshot, &recovered_store);
    assert!(recovered_store.epoch > old_epoch);
    assert_eq!(session.host(), C);
    assert_eq!(session.session_definition(), SESSION_DEFINITION);
    assert_eq!(
        session.accept_command(&envelope(4, D, 0)),
        Err(ProtocolError::WrongEpoch)
    );
    assert_eq!(
        session.validate_event(&AuthorityEventEnvelope {
            session: SESSION,
            host: A,
            cursor: AuthorityCursor::new(4, 806),
            event: (),
        }),
        Err(ProtocolError::WrongEpoch)
    );
    assert_eq!(
        session.validate_event(&AuthorityEventEnvelope {
            session: SESSION,
            host: C,
            cursor: AuthorityCursor::new(5, 1),
            event: (),
        }),
        Ok(())
    );
}

#[test]
fn ordinary_disconnect_releases_only_b_holds_marks_dirty_without_moving_or_snapping() {
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..30)
            .map(|index| Vec2::new(index as f32, 100.0))
            .collect(),
    );
    for id in [PieceId(10), PieceId(11), PieceId(25), PieceId(12)] {
        let mut state = store.state(id).unwrap();
        state.held_by = Some(if id == PieceId(12) { C } else { B });
        store.set_state(id, state);
    }
    // Even malformed placed+held data is cleaned without changing placed.
    let mut placed = store.state(PieceId(25)).unwrap();
    placed.placed = true;
    store.set_state(PieceId(25), placed);
    store.dirty_pieces.clear();
    let before = store.states.clone();
    let epoch = store.epoch;
    let next_z = store.next_z_order;
    let placed_count = store.placed_count;
    assert_eq!(
        release_player_holds(&mut store, B),
        vec![PieceId(10), PieceId(11), PieceId(25)]
    );
    assert_eq!(
        store.dirty_pieces.iter().collect::<HashSet<_>>(),
        HashSet::from([PieceId(10), PieceId(11), PieceId(25)])
    );
    for (index, original) in before.iter().enumerate() {
        assert_eq!(store.states[index].position, original.position);
        assert_eq!(store.states[index].z_order, original.z_order);
        assert_eq!(store.states[index].flags & PLACED, original.flags & PLACED);
    }
    for id in [PieceId(10), PieceId(11), PieceId(25)] {
        assert!(store.state(id).unwrap().held_by.is_none());
        assert_eq!(store.states[id.0 as usize].flags & HELD, 0);
    }
    assert_eq!(store.state(PieceId(12)).unwrap().held_by, Some(C));
    assert_ne!(store.states[12].flags & HELD, 0);
    assert_eq!(
        (store.epoch, store.next_z_order, store.placed_count),
        (epoch, next_z, placed_count)
    );
    assert!(release_player_holds(&mut store, B).is_empty());
}

#[test]
fn capture_rejects_active_local_drag_without_changing_the_store() {
    let mut store = dragging_fixture();
    store.drag.delta = Vec2::new(100.0, 200.0);
    let original = dragging_fixture();
    let epoch = store.epoch;
    let members = store.drag.members.clone();
    assert_eq!(
        GameSnapshot::capture(
            &store,
            &definition(),
            SESSION_DEFINITION,
            AuthorityCursor::new(3, 100),
        ),
        Err(SnapshotError::ActiveLocalDrag)
    );
    assert_eq!(store.states, original.states);
    assert_eq!(store.epoch, epoch);
    assert_eq!(store.drag.members, members);
    assert_eq!(store.drag.delta, Vec2::new(100.0, 200.0));
    assert_eq!(store.held_by, original.held_by);
    assert_eq!(store.selected_pieces, original.selected_pieces);
    assert_eq!(store.dirty_pieces, original.dirty_pieces);
    assert_eq!(store.placed_count, original.placed_count);
    assert_eq!(store.next_z_order, original.next_z_order);
    assert_eq!(store.highlights_dirty, original.highlights_dirty);
    store.highlights_dirty = true;
    store.sync_highlights();
    assert_eq!(store.states, original.states); // Private highlight cache remains valid.
}

#[test]
fn invalid_snapshots_are_rejected_atomically_without_panics() {
    let definition = definition();
    let cursor = AuthorityCursor::new(3, 100);
    let snapshot =
        GameSnapshot::capture(&fixture(), &definition, SESSION_DEFINITION, cursor).unwrap();
    let mut cases = Vec::new();
    let mut invalid = snapshot.clone();
    invalid.session = SessionId(999);
    cases.push((invalid, SnapshotError::WrongSession));
    // Same session, dimensions and piece states; a different image is still rejected.
    let mut invalid = snapshot.clone();
    invalid.image_hash = ImageHash([0x43; 32]);
    cases.push((invalid, SnapshotError::WrongImageHash));
    let mut invalid = snapshot.clone();
    invalid.pieces.pop();
    cases.push((
        invalid,
        SnapshotError::WrongPieceCount {
            expected: 4,
            actual: 3,
        },
    ));
    for coordinate in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut invalid = snapshot.clone();
        invalid.pieces[1].position.x = coordinate;
        cases.push((invalid, SnapshotError::NonFinitePosition(PieceId(1))));
    }
    let mut invalid = snapshot.clone();
    invalid.definition.generator_version = GENERATOR_VERSION + 1;
    cases.push((
        invalid,
        SnapshotError::InvalidDefinition("Unsupported puzzle generator version"),
    ));
    for wrong_cursor in [
        AuthorityCursor::new(2, 100),
        AuthorityCursor::new(4, 0),
        AuthorityCursor::new(3, 99),
        AuthorityCursor::new(3, 101),
    ] {
        let mut invalid = snapshot.clone();
        invalid.cursor = wrong_cursor;
        cases.push((invalid, SnapshotError::WrongCursor));
    }
    for schema in [1, SNAPSHOT_SCHEMA_VERSION + 1] {
        let mut invalid = snapshot.clone();
        invalid.schema_version = schema;
        cases.push((invalid, SnapshotError::UnsupportedSchema(schema)));
    }
    let mut invalid = snapshot.clone();
    invalid.definition.seed += 1;
    cases.push((invalid, SnapshotError::WrongDefinition));
    for grid in [UVec2::ZERO, UVec2::splat(u32::MAX)] {
        let mut invalid = snapshot.clone();
        invalid.definition.grid_size = grid;
        cases.push((
            invalid,
            SnapshotError::InvalidDefinition("Grid dimensions must be between 1 and 1000"),
        ));
    }
    for next_z in [0, MAX_Z + 1] {
        let mut invalid = snapshot.clone();
        invalid.next_z_order = next_z;
        cases.push((invalid, SnapshotError::InvalidNextZOrder));
    }
    for z_order in [snapshot.next_z_order, u32::MAX] {
        let mut invalid = snapshot.clone();
        invalid.pieces[1].z_order = z_order;
        cases.push((invalid, SnapshotError::InvalidZOrder(PieceId(1))));
    }
    for flags in [SELECTED, PREVIEW, HELD, ENABLED, 1 << 31] {
        let mut invalid = snapshot.clone();
        invalid.pieces[1].flags |= flags;
        cases.push((invalid, SnapshotError::InvalidFlags(PieceId(1))));
    }
    for (invalid, error) in cases {
        let mut store = dragging_fixture();
        let original = dragging_fixture(); // private caches are checked by continued highlight sync below.
        let states = store.states.clone();
        let epoch = store.epoch;
        let result = invalid.install(
            &mut store,
            SnapshotExpectation {
                session: SESSION,
                image_hash: SESSION_DEFINITION.image_hash,
                cursor,
                definition: &definition,
            },
        );
        assert_eq!(result, Err(error));
        assert_eq!(store.states, states);
        assert_eq!(store.epoch, epoch);
        assert_eq!(store.held_by, original.held_by);
        assert_eq!(store.selected_pieces, original.selected_pieces);
        assert_eq!(store.drag.members, original.drag.members);
        assert_eq!(store.drag.delta, original.drag.delta);
        assert_eq!(store.dirty_pieces, original.dirty_pieces);
        assert_eq!(
            (store.placed_count, store.next_z_order),
            (original.placed_count, original.next_z_order)
        );
        store.highlights_dirty = true;
        store.sync_highlights();
        assert_eq!(store.states, states);
    }
}

#[test]
fn failed_migration_never_activates_authority_or_changes_gpu_state() {
    let definition = definition();
    let mut session = AuthoritySession::new(SESSION_DEFINITION, A, AuthorityCursor::new(3, 100));
    session.host_lost().unwrap();
    session.host_changed(B).unwrap();
    session
        .choose_recovery_source([RecoverySource {
            session: SESSION,
            player: C,
            cursor: session.cursor(),
        }])
        .unwrap();
    let mut store = dragging_fixture();
    let states = store.states.clone();
    let epoch = store.epoch;
    let mut snapshot = GameSnapshot::capture(
        &fixture(),
        &definition,
        SESSION_DEFINITION,
        session.cursor(),
    )
    .unwrap();
    let original_hash = snapshot.image_hash;
    snapshot.image_hash = ImageHash([0x43; 32]);
    assert_eq!(
        install_migration_snapshot(&mut session, &mut store, &definition, &snapshot),
        Err(MigrationInstallError::Snapshot(
            SnapshotError::WrongImageHash
        ))
    );
    assert_eq!(session.cursor(), AuthorityCursor::new(3, 100));
    assert_eq!(session.host(), A);
    assert_eq!(session.session_definition(), SESSION_DEFINITION);
    assert!(!session.is_active());
    assert_eq!(store.states, states);
    assert_eq!(store.epoch, epoch);
    snapshot.image_hash = original_hash;
    snapshot.pieces[0].position.y = f32::NAN;
    assert_eq!(
        install_migration_snapshot(&mut session, &mut store, &definition, &snapshot),
        Err(MigrationInstallError::Snapshot(
            SnapshotError::NonFinitePosition(PieceId(0))
        ))
    );
    assert_eq!(session.cursor(), AuthorityCursor::new(3, 100));
    assert_eq!(session.host(), A);
    assert!(matches!(
        session.migration(),
        MigrationState::Recovering { .. }
    ));
    assert_eq!(store.states, states);
    assert_eq!(store.epoch, epoch);
    snapshot.cursor = AuthorityCursor::new(3, 99);
    assert_eq!(
        install_migration_snapshot(&mut session, &mut store, &definition, &snapshot),
        Err(MigrationInstallError::Protocol(
            ProtocolError::WrongSnapshotCursor
        ))
    );
    assert_eq!(store.states, states);
    assert_eq!(store.epoch, epoch);
}

#[test]
fn million_piece_snapshot_has_sixteen_byte_dense_states_and_round_trips() {
    assert_eq!(std::mem::size_of::<SnapshotPieceState>(), 16);
    let mut definition = definition();
    definition.grid_size = UVec2::splat(1000);
    let mut source = PieceDataStore::default();
    source.initialize(vec![Vec2::ONE; 1_000_000]);
    let snapshot = GameSnapshot::capture(
        &source,
        &definition,
        SESSION_DEFINITION,
        AuthorityCursor::new(8, 123),
    )
    .unwrap();
    assert_eq!(snapshot.pieces.len(), 1_000_000);
    assert_eq!(snapshot.image_hash, SESSION_DEFINITION.image_hash);
    let mut target = PieceDataStore::default();
    snapshot
        .install(
            &mut target,
            SnapshotExpectation {
                session: SESSION,
                image_hash: SESSION_DEFINITION.image_hash,
                cursor: snapshot.cursor,
                definition: &definition,
            },
        )
        .unwrap();
    assert_eq!(source.states, target.states);
    assert_eq!(target.next_z_order, 1_000_000);
    assert_eq!(target.placed_count, 0);
    assert!(target.held_by.is_empty() && target.dirty_pieces.is_empty());
}

#[test]
fn reordered_moves_never_suppress_reliable_release_or_move_a_regrabbed_piece() {
    let mut store = fixture();
    release_player_holds(&mut store, A);
    let mut session = AuthoritySession::new(SESSION_DEFINITION, A, AuthorityCursor::new(4, 0));
    let move_request = |control, tick, position| ClientCommandEnvelope {
        sequence: ClientCommandSequence::Move {
            after_control_sequence: control,
            tick,
        },
        command: PieceCommand::Move {
            id: PieceId(1),
            position,
        },
        ..envelope(4, B, 0)
    };
    let mut piece = store.state(PieceId(1)).unwrap();
    let early_move = move_request(0, 100, Vec2::new(10.0, 20.0));
    assert_eq!(
        session.accept_command(&early_move),
        Err(ProtocolError::ControlNotProcessed { required: 0 })
    );
    assert_eq!(
        session.accept_command(&envelope(4, B, 0)),
        Ok(CommandSequenceStatus::InOrder)
    );
    assert_eq!(
        apply_piece_command(&mut piece, B, &PieceCommand::Grab(PieceId(1))),
        Some(CommandOutcome::Grabbed)
    );
    assert_eq!(
        session.accept_command(&early_move),
        Ok(CommandSequenceStatus::Gap { expected: 0 })
    );
    assert_eq!(
        apply_piece_command(&mut piece, B, &early_move.command),
        Some(CommandOutcome::Moved)
    );
    let release = ClientCommandEnvelope {
        command: PieceCommand::Release(PieceId(1)),
        ..envelope(4, B, 1)
    };
    assert_eq!(
        session.accept_command(&release),
        Ok(CommandSequenceStatus::InOrder)
    );
    assert_eq!(
        apply_piece_command(&mut piece, B, &release.command),
        Some(CommandOutcome::Released)
    );
    store.set_state(PieceId(1), piece);
    assert!(store.state(PieceId(1)).unwrap().held_by.is_none());
    assert_eq!(piece.position, Vec2::new(10.0, 20.0));

    session.accept_command(&envelope(4, B, 2)).unwrap();
    apply_piece_command(&mut piece, B, &PieceCommand::Grab(PieceId(1))).unwrap();
    let stale_move = move_request(0, 101, Vec2::splat(999.0));
    assert_eq!(
        session.accept_command(&stale_move),
        Err(ProtocolError::StaleMoveContext)
    );
    assert_eq!(piece.position, Vec2::new(10.0, 20.0));
    let current_move = move_request(2, 0, Vec2::new(30.0, 40.0));
    assert_eq!(
        session.accept_command(&current_move),
        Ok(CommandSequenceStatus::InOrder)
    );
    apply_piece_command(&mut piece, B, &current_move.command).unwrap();
    store.set_state(PieceId(1), piece);
    assert_eq!(
        store.state(PieceId(1)).unwrap().position,
        Vec2::new(30.0, 40.0)
    );
    assert_eq!(store.state(PieceId(1)).unwrap().held_by, Some(B));
}
