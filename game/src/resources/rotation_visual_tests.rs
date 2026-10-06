use super::*;
use crate::resources::pieces::{without_piece_state_access, HELD};
use jigsall_core::{
    protocol::{ComponentRef, PieceTarget},
    PieceBitSet, PieceCommand, PuzzleDefinition, GENERATOR_VERSION, LOCAL_PLAYER,
};

fn fixture() -> (PieceDataStore, PuzzleDefinition) {
    let def = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(3, 1),
        image_size: UVec2::new(120, 30),
        snap_distance: 5.0,
        rotation_enabled: true,
    };
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..3)
            .map(|id| def.correct_position(PieceId(id)) + Vec2::new(70.0, 40.0))
            .collect(),
    );
    store.connectivity.union(PieceId(0), PieceId(1));
    (store, def)
}
fn command(store: &PieceDataStore, turns: i8) -> PieceCommand {
    PieceCommand::Rotate {
        target: PieceTarget::Component(
            ComponentRef::from_member(&store.connectivity, PieceId(0)).unwrap(),
        ),
        quarter_turns: turns,
    }
}
fn turn(store: &mut PieceDataStore, def: &PuzzleDefinition, turns: i8) {
    let command = command(store, turns);
    let boundary = store.capture_rotation_command(&command, Some(def));
    assert_eq!(
        store
            .apply_command(LOCAL_PLAYER, &command, Some(def), LOCAL_PLAYER)
            .rotated,
        2
    );
    store.finish_rotation_boundary(boundary);
}
fn displayed(store: &PieceDataStore, id: PieceId) -> (Vec2, f32) {
    let state = store.presentation_state(id);
    let root = store.connectivity.minimum_member(id);
    let a = store.rotation_visual.animation(root).unwrap();
    (
        a.position(state.position, store.rotation_visual.clock) + store.visual_delta(id),
        a.target_angle + a.angle(store.rotation_visual.clock),
    )
}
fn close(a: f32, b: f32) {
    assert!((a - b).abs() < 0.0001, "{a} != {b}");
}

#[test]
fn elevation_envelope_scales_with_residual_and_stays_normalized() {
    for (residual, start_elevation, peak) in [
        (-QUARTER, 0.0, 1.0),
        (QUARTER * 0.5, 0.0, 0.5),
        (QUARTER / 9.0, 0.0, 1.0 / 9.0),
        (QUARTER * 2.0, 0.0, 1.0),
        (QUARTER * 0.1, 0.8, 0.8),
        (0.0, 0.8, 0.8),
    ] {
        let a = RotationAnimation {
            pivot: Vec2::ZERO,
            offset: Vec2::ZERO,
            residual,
            target_angle: 0.0,
            start: 2.0,
            duration: 1.0,
            start_elevation,
        };
        assert_eq!(a.elevation(1.0), start_elevation);
        assert_eq!(a.elevation(a.start), start_elevation);
        assert_eq!(a.elevation(a.start + a.duration * 0.5), peak);
        for now in [3.0, 3.1, 10.0] {
            assert_eq!(a.elevation(now), 0.0);
        }
        for i in 0..=1000 {
            let p = i as f64 / 1000.0;
            let elevation = a.elevation(a.start + a.duration * p);
            let range = if p < 0.5 {
                start_elevation..=peak
            } else {
                0.0..=peak
            };
            assert!(range.contains(&elevation), "{p}: {elevation}");
        }
        // Both half-curves have zero slope at the start, peak and settle.
        let step = 0.0001;
        for p in [0.0, 0.5, 1.0] {
            let now = a.start + a.duration * p;
            let slope = (a.elevation(now + step) - a.elevation(now - step)) / (2.0 * step as f32);
            assert!(slope.abs() < 0.003, "slope at {p}: {slope}");
        }
    }
}

#[test]
fn gpu_rotation_record_keeps_32_bytes_and_publishes_inherited_elevation() {
    assert_eq!(std::mem::size_of::<GpuRotationAnimation>(), 32);
    assert_eq!(
        std::mem::offset_of!(GpuRotationAnimation, start_elevation),
        28
    );
    let (mut store, def) = fixture();
    turn(&mut store, &def, 1);
    store.rotation_visual.clock = 0.050;
    let elevation = store
        .rotation_visual
        .animation(PieceId(0))
        .unwrap()
        .elevation(0.050);
    turn(&mut store, &def, -1);
    assert_eq!(store.rotation_visual.records.len(), 1);
    assert_eq!(store.rotation_visual.records[0].start_elevation, elevation);
    let a = store.rotation_visual.animation(PieceId(0)).unwrap();
    assert_eq!(a.start_elevation, elevation);
    store.rotation_visual.clock = a.start + a.duration;
    assert!(!store
        .capture_rotation_boundary()
        .items
        .contains_key(&PieceId(0)));
    // Expired records are collected at the next existing control boundary.
    turn(&mut store, &def, 1);
    assert_eq!(store.rotation_visual.records.len(), 1);
    assert_eq!(store.rotation_visual.records[0].start_elevation, 0.0);
}

#[test]
fn continuous_rotation_start_midpoint_end_and_identity_preserve_rigid_body() {
    let (mut store, def) = fixture();
    let old = [store.states[0].position, store.states[1].position];
    turn(&mut store, &def, 1);
    let committed = store.states.clone();
    close(displayed(&store, PieceId(0)).1, 0.0);
    assert!(displayed(&store, PieceId(0)).0.distance(old[0]) < 0.0001);
    store.rotation_visual.clock = 0.060;
    close(displayed(&store, PieceId(0)).1, QUARTER * 0.5);
    let a = displayed(&store, PieceId(0)).0;
    let b = displayed(&store, PieceId(1)).0;
    assert!((b - a).distance(rotate(old[1] - old[0], QUARTER * 0.5)) < 0.0001);
    store.rotation_visual.clock = 0.120;
    let animation = store.rotation_visual.animation(PieceId(0)).unwrap();
    assert_eq!(animation.angle(0.120), 0.0);
    assert_eq!(animation.elevation(0.0), 0.0);
    assert_eq!(animation.elevation(0.060), 1.0);
    assert_eq!(animation.elevation(0.120), 0.0);
    assert_eq!(
        animation.position(store.states[0].position, 0.120),
        store.states[0].position
    );
    assert_eq!(store.states, committed);
}

#[test]
fn repeated_q_e_retarget_from_displayed_angle_including_reversal_and_wrap() {
    for (first, second) in [(1, 1), (-1, -1), (1, -1), (-1, 1)] {
        let (mut store, def) = fixture();
        turn(&mut store, &def, first);
        store.rotation_visual.clock = 0.050;
        let before = displayed(&store, PieceId(0));
        let elevation = store
            .rotation_visual
            .animation(PieceId(0))
            .unwrap()
            .elevation(0.050);
        assert!(elevation > 0.0);
        turn(&mut store, &def, second);
        let after = displayed(&store, PieceId(0));
        close(before.1, after.1);
        assert!(before.0.distance(after.0) < 0.0001);
        let a = store.rotation_visual.animation(PieceId(0)).unwrap();
        assert_eq!(a.start_elevation, elevation);
        assert_eq!(a.elevation(0.050), elevation);
        assert!(a.elevation(a.start + a.duration * 0.1) >= elevation);
        assert_eq!(a.elevation(a.start + a.duration), 0.0);
        store.rotation_visual.clock += 0.020;
        let next = displayed(&store, PieceId(0)).1;
        assert!((next - after.1) * f32::from(second) > 0.0);
    }
    let (mut store, def) = fixture();
    for _ in 0..3 {
        turn(&mut store, &def, 1);
        store.rotation_visual.clock += 0.121;
    }
    turn(&mut store, &def, 1);
    let animation = store.rotation_visual.animation(PieceId(0)).unwrap();
    close(animation.residual, -QUARTER);
    close(animation.target_angle, 4.0 * QUARTER);
}

#[test]
fn prediction_ack_retires_override_without_finishing_or_restarting_animation() {
    let (mut store, def) = fixture();
    let cmd = command(&store, 1);
    let boundary = store.capture_rotation_command(&cmd, Some(&def));
    let PieceCommand::Rotate {
        target,
        quarter_turns,
    } = &cmd
    else {
        unreachable!()
    };
    let mut poses = HashMap::new();
    store.predict_rotation(target, *quarter_turns, &def, &mut poses);
    store.set_local_rotation(poses);
    store.finish_rotation_boundary(boundary);
    store.rotation_visual.clock = 0.030;
    let before = displayed(&store, PieceId(0));
    let animation = store.rotation_visual.animation(PieceId(0)).unwrap();
    let ack = store.capture_rotation_boundary();
    store.apply_command(LOCAL_PLAYER, &cmd, Some(&def), LOCAL_PLAYER);
    store.clear_local_rotation();
    store.finish_rotation_boundary(ack);
    assert!(store.local_rotation.poses.is_empty());
    close(displayed(&store, PieceId(0)).1, before.1);
    assert!(displayed(&store, PieceId(0)).0.distance(before.0) < 0.0001);
    let after = store.rotation_visual.animation(PieceId(0)).unwrap();
    assert_eq!(
        (after.start, after.duration, after.start_elevation),
        (
            animation.start,
            animation.duration,
            animation.start_elevation
        )
    );
    for now in [0.030, 0.060, 0.100, 0.120] {
        assert_eq!(after.elevation(now), animation.elevation(now));
    }
    store.rotation_visual.clock = 0.060;
    close(displayed(&store, PieceId(0)).1, QUARTER * 0.5);
}

#[test]
fn rejection_rebases_continuously_to_canonical_and_reset_discards_visuals() {
    let (mut store, def) = fixture();
    let cmd = command(&store, 1);
    let boundary = store.capture_rotation_command(&cmd, Some(&def));
    let PieceCommand::Rotate {
        target,
        quarter_turns,
    } = &cmd
    else {
        unreachable!()
    };
    let mut poses = HashMap::new();
    store.predict_rotation(target, *quarter_turns, &def, &mut poses);
    store.set_local_rotation(poses);
    store.finish_rotation_boundary(boundary);
    store.rotation_visual.clock = 0.060;
    let before = displayed(&store, PieceId(0));
    let elevation = store
        .rotation_visual
        .animation(PieceId(0))
        .unwrap()
        .elevation(0.060);
    store.clear_local_rotation();
    close(displayed(&store, PieceId(0)).1, before.1);
    assert!(displayed(&store, PieceId(0)).0.distance(before.0) < 0.0001);
    let a = store.rotation_visual.animation(PieceId(0)).unwrap();
    assert_eq!(a.start_elevation, elevation);
    assert_eq!(a.elevation(0.060), elevation);
    assert_eq!(a.elevation(a.start + a.duration), 0.0);
    store.rotation_visual.clock = 0.190;
    assert_eq!(displayed(&store, PieceId(0)).0, store.states[0].position);
    close(displayed(&store, PieceId(0)).1, 0.0);
    store.clear_rotation_visual();
    assert!(store.rotation_visual.animations.is_empty());
    turn(&mut store, &def, 1);
    store.initialize(vec![Vec2::ZERO]);
    assert_eq!(store.rotation_visual.clock, 0.190);
    assert!(store.rotation_visual.animations.is_empty());
    assert!(store.rotation_visual.records.is_empty());
}

#[test]
fn drag_pointer_and_ack_basis_rebase_use_rigid_visual_without_changing_delta() {
    let (mut store, def) = fixture();
    let mut members = PieceBitSet::new(store.len());
    members.insert(PieceId(0));
    members.insert(PieceId(1));
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::GrabGroup {
            members: members.clone(),
        },
        Some(&def),
        LOCAL_PLAYER,
    );
    store.drag.members = members.words().clone();
    store.drag.delta = Vec2::new(13.0, 17.0);
    let cmd = PieceCommand::RotateDrag {
        members: members.clone(),
        delta: store.drag.delta,
        quarter_turns: 1,
    };
    let boundary = store.capture_rotation_command(&cmd, Some(&def));
    let old = store.states[0].position + store.drag.delta;
    let applied = store.apply_command(LOCAL_PLAYER, &cmd, Some(&def), LOCAL_PLAYER);
    assert!(applied.drag_rebased);
    store.finish_rotation_boundary(boundary);
    assert_eq!(store.drag.delta, Vec2::ZERO);
    assert!(displayed(&store, PieceId(0)).0.distance(old) < 0.0001);
    // Give the ACK basis rebase a nonzero inherited start elevation.
    store.rotation_visual.clock = 0.020;
    let retarget = PieceCommand::RotateDrag {
        members: members.clone(),
        delta: Vec2::ZERO,
        quarter_turns: 1,
    };
    let boundary = store.capture_rotation_command(&retarget, Some(&def));
    assert!(
        store
            .apply_command(LOCAL_PLAYER, &retarget, Some(&def), LOCAL_PLAYER)
            .drag_rebased
    );
    store.finish_rotation_boundary(boundary);
    store.rotation_visual.clock = 0.030;
    let before = displayed(&store, PieceId(0));
    store.drag.delta = Vec2::new(4.0, 6.0);
    assert!(
        displayed(&store, PieceId(0))
            .0
            .distance(before.0 + store.drag.delta)
            < 0.0001
    );
    let before = displayed(&store, PieceId(0));
    let animation = store.rotation_visual.animation(PieceId(0)).unwrap();
    assert!(animation.start_elevation > 0.0);
    let boundary = store.capture_rotation_boundary();
    let consumed = store.drag.delta;
    for id in members.iter() {
        store.states[id.0 as usize].position += consumed;
    }
    store.drag.delta = Vec2::ZERO;
    store.finish_rotation_boundary(boundary);
    assert!(displayed(&store, PieceId(0)).0.distance(before.0) < 0.0001);
    close(displayed(&store, PieceId(0)).1, before.1);
    let after = store.rotation_visual.animation(PieceId(0)).unwrap();
    assert_eq!(
        (after.start, after.duration, after.start_elevation),
        (
            animation.start,
            animation.duration,
            animation.start_elevation
        )
    );
    for now in [0.030, 0.060, 0.120, 0.240] {
        assert_eq!(after.elevation(now), animation.elevation(now));
    }
    assert_eq!(store.states[0].flags & HELD, HELD);
}

#[test]
fn million_piece_animation_frames_only_share_records_and_advance_clock() {
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::ZERO; 1_000_000]);
    store.rotation_visual.animations.insert(
        PieceId(0),
        RotationAnimation {
            pivot: Vec2::ZERO,
            offset: Vec2::ZERO,
            residual: -QUARTER,
            target_angle: QUARTER,
            start: 0.0,
            duration: ROTATION_SECONDS_PER_QUARTER,
            start_elevation: 0.0,
        },
    );
    store.rotation_visual.publish(&store.connectivity);
    let mut app = App::new();
    app.insert_resource(store)
        .init_resource::<RotationVisualUpload>()
        .add_systems(Last, prepare_rotation_visual_upload);
    app.update();
    let records = app
        .world()
        .resource::<RotationVisualUpload>()
        .records
        .clone();
    let revision = app.world().resource::<RotationVisualUpload>().revision;
    for i in 1..=20 {
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .rotation_visual
            .clock = f64::from(i) * 0.010;
        without_piece_state_access(|| app.update());
        let upload = app.world().resource::<RotationVisualUpload>();
        assert_eq!(upload.revision, revision);
        assert!(Arc::ptr_eq(&records, &upload.records));
        assert_eq!(upload.active, i < 12);
    }
}

#[test]
fn release_final_delta_handoffs_without_visual_translation_jump() {
    let (mut store, def) = fixture();
    let mut members = PieceBitSet::new(store.len());
    members.insert(PieceId(0));
    members.insert(PieceId(1));
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::GrabGroup {
            members: members.clone(),
        },
        Some(&def),
        LOCAL_PLAYER,
    );
    store.drag.members = members.words().clone();
    let cmd = PieceCommand::RotateDrag {
        members: members.clone(),
        delta: Vec2::ZERO,
        quarter_turns: 1,
    };
    let boundary = store.capture_rotation_command(&cmd, Some(&def));
    store.apply_command(LOCAL_PLAYER, &cmd, Some(&def), LOCAL_PLAYER);
    store.finish_rotation_boundary(boundary);
    store.rotation_visual.clock = 0.050;
    store.drag.delta = Vec2::new(7.0, 11.0);
    let before = displayed(&store, PieceId(0));
    let release = PieceCommand::ReleaseGroup {
        members,
        delta: store.drag.delta,
    };
    store.drag = Default::default(); // finish_drag runs before command dispatch
    let boundary = store.capture_rotation_command(&release, Some(&def));
    assert_eq!(
        store
            .apply_command(LOCAL_PLAYER, &release, Some(&def), LOCAL_PLAYER)
            .released,
        2
    );
    store.finish_rotation_boundary(boundary);
    assert!(displayed(&store, PieceId(0)).0.distance(before.0) < 0.0001);
    close(displayed(&store, PieceId(0)).1, before.1);
}

#[test]
fn dynamic_component_table_uses_dsu_roots_and_independent_planner_pivots() {
    let count = 256;
    let def = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(count, 1),
        image_size: UVec2::new(count * 10, 10),
        snap_distance: 1.0,
        rotation_enabled: true,
    };
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..count)
            .map(|id| def.correct_position(PieceId(id)) + Vec2::new(0.0, 40.0))
            .collect(),
    );
    // Deliberately make DSU root 1, with stable minimum 0.
    store.connectivity.union(PieceId(1), PieceId(2));
    store.connectivity.union(PieceId(0), PieceId(1));
    assert_eq!(store.connectivity.find_root(PieceId(0)), PieceId(1));
    let mut members = PieceBitSet::new(count as usize);
    members.fill();
    let cmd = PieceCommand::Rotate {
        target: PieceTarget::from_selection(&store.connectivity, &members).unwrap(),
        quarter_turns: 1,
    };
    let boundary = store.capture_rotation_command(&cmd, Some(&def));
    assert_eq!(
        store
            .apply_command(LOCAL_PLAYER, &cmd, Some(&def), LOCAL_PLAYER)
            .rotated,
        count as usize
    );
    store.finish_rotation_boundary(boundary);
    assert_eq!(store.rotation_visual.records.len(), count as usize - 2);
    assert!(store.rotation_visual.slots.contains_key(&PieceId(1)));
    assert!(!store.rotation_visual.slots.contains_key(&PieceId(0)));
    assert_eq!(
        store.rotation_visual.animation(PieceId(3)).unwrap().pivot,
        store.states[3].position
    );
    assert_eq!(
        store
            .rotation_visual
            .animation(PieceId(count - 1))
            .unwrap()
            .pivot,
        store.states[count as usize - 1].position
    );
}

#[test]
fn rejected_identity_rotation_never_starts_a_visual_turn() {
    let (mut store, def) = fixture();
    for turns in [0, 4, -4] {
        let cmd = command(&store, turns);
        let boundary = store.capture_rotation_command(&cmd, Some(&def));
        assert_eq!(
            store
                .apply_command(LOCAL_PLAYER, &cmd, Some(&def), LOCAL_PLAYER)
                .rotated,
            0
        );
        store.finish_rotation_boundary(boundary);
        assert!(store.rotation_visual.animations.is_empty());
    }
}

#[test]
fn authority_cancel_restores_logical_pose_and_settles_visual_without_leaking_delta() {
    use crate::resources::pieces::local_rotation::PredictionDrag;
    use jigsall_core::protocol::ActiveDragTarget;
    let (mut store, def) = fixture();
    let canonical = store.states.clone();
    let mut members = PieceBitSet::new(store.len());
    members.insert(PieceId(0));
    members.insert(PieceId(1));
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::GrabGroup {
            members: members.clone(),
        },
        Some(&def),
        LOCAL_PLAYER,
    );
    store.drag.members = members.words().clone();
    store.drag.delta = Vec2::new(9.0, 11.0);
    let cmd = PieceCommand::RotateDrag {
        members: members.clone(),
        delta: store.drag.delta,
        quarter_turns: 1,
    };
    let boundary = store.capture_rotation_command(&cmd, Some(&def));
    let mut poses = HashMap::new();
    let delta = store
        .predict_drag_rotation(
            PredictionDrag {
                members: &members,
                player: LOCAL_PLAYER,
                optimistic_grab: false,
            },
            store.drag.delta,
            1,
            &def,
            &mut poses,
        )
        .unwrap();
    for pose in poses.values_mut() {
        pose.position -= delta;
    }
    store.set_local_rotation(poses);
    store.finish_rotation_boundary(boundary);
    store.rotation_visual.clock = 0.050;
    let before = displayed(&store, PieceId(0));
    let elevation = store
        .rotation_visual
        .animation(PieceId(0))
        .unwrap()
        .elevation(0.050);
    let boundary = store.capture_rotation_boundary();
    let target = ActiveDragTarget::Sparse(vec![ComponentRef::from_member(
        &store.connectivity,
        PieceId(0),
    )
    .unwrap()]);
    assert_eq!(
        store
            .cancel_drag_target(LOCAL_PLAYER, &target)
            .unwrap()
            .released,
        2
    );
    store.clear_local_rotation();
    store.drag = Default::default();
    store.finish_rotation_boundary(boundary);
    assert!(displayed(&store, PieceId(0)).0.distance(before.0) < 0.0001);
    close(displayed(&store, PieceId(0)).1, before.1);
    let a = store.rotation_visual.animation(PieceId(0)).unwrap();
    assert_eq!(a.start_elevation, elevation);
    assert_eq!(a.elevation(0.050), elevation);
    assert_eq!(a.elevation(a.start + a.duration), 0.0);
    store.rotation_visual.clock = 1.0;
    assert_eq!(displayed(&store, PieceId(0)).0, canonical[0].position);
    assert!(store.local_rotation.poses.is_empty());
    assert!(store.drag.members.is_empty());
}

#[test]
fn snapshot_install_discards_the_previous_epoch_animation() {
    let (mut store, def) = fixture();
    turn(&mut store, &def, 1);
    store.rotation_visual.clock = 0.030;
    let boundary = store.capture_rotation_boundary();
    let old_epoch = store.epoch;
    store.replace_snapshot_states(
        store.states.to_vec(),
        store.next_z_order,
        store.connectivity.clone(),
    );
    store.finish_rotation_boundary(boundary);
    assert_ne!(store.epoch, old_epoch);
    assert_eq!(store.rotation_visual.clock, 0.030);
    assert!(store.rotation_visual.animations.is_empty());
    assert!(store.rotation_visual.records.is_empty());
}
