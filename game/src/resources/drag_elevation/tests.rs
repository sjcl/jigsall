use super::*;
use jigsall_core::{PieceBitSet, PieceId};

fn mask(count: usize, ids: impl IntoIterator<Item = u32>) -> PieceBitSet {
    let mut mask = PieceBitSet::new(count);
    mask.extend(ids.into_iter().map(PieceId));
    mask
}
fn fixture(
    count: usize,
) -> (
    PieceDataStore,
    RemoteDragPresentation,
    DragElevationPresentation,
    DragElevationUpload,
) {
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::ZERO; count]);
    let mut remote = RemoteDragPresentation::default();
    remote.reset(store.epoch, count);
    let mut presentation = DragElevationPresentation::default();
    let mut upload = DragElevationUpload::default();
    presentation.prepare(&store, &remote, &mut upload);
    (store, remote, presentation, upload)
}
fn at(
    store: &mut PieceDataStore,
    remote: &RemoteDragPresentation,
    presentation: &mut DragElevationPresentation,
    upload: &mut DragElevationUpload,
    now: f64,
) {
    store.rotation_visual.clock = now;
    presentation.prepare(store, remote, upload);
}

#[test]
fn drag_elevation_component_grab_hold_release_and_early_cancel_are_continuous() {
    let (mut store, remote, mut presentation, mut upload) = fixture(3);
    assert!(!upload.active);
    assert_eq!(presentation.value(0, 0.0), 0.0);
    store.drag.members = mask(3, [0, 1]).words().clone();
    at(&mut store, &remote, &mut presentation, &mut upload, 0.0);
    for (time, expected) in [(0.0, 0.0), (0.04, 0.5), (0.08, 1.0), (0.5, 1.0)] {
        at(&mut store, &remote, &mut presentation, &mut upload, time);
        assert_eq!(presentation.value(0, time), expected);
        assert_eq!(presentation.value(1, time), expected);
        assert_eq!(presentation.value(2, time), 0.0);
        assert_eq!(presentation.mapping[0], presentation.mapping[1]);
    }
    store.drag = default();
    at(&mut store, &remote, &mut presentation, &mut upload, 0.5);
    assert_eq!(presentation.value(0, 0.5), 1.0);
    at(&mut store, &remote, &mut presentation, &mut upload, 0.54);
    assert!((presentation.value(0, 0.54) - 0.5).abs() < 1e-6);
    at(&mut store, &remote, &mut presentation, &mut upload, 0.581);
    assert_eq!(presentation.value(0, 0.581), 0.0);
    assert!(!upload.active);
    // Cancel before the grab finishes reverses from the current height.
    store.drag.members = mask(3, [0, 1]).words().clone();
    at(&mut store, &remote, &mut presentation, &mut upload, 1.0);
    at(&mut store, &remote, &mut presentation, &mut upload, 1.04);
    let before = presentation.value(0, 1.04);
    store.drag = default();
    at(&mut store, &remote, &mut presentation, &mut upload, 1.04);
    assert_eq!(presentation.value(0, 1.04), before);
    at(&mut store, &remote, &mut presentation, &mut upload, 1.121);
    assert_eq!(presentation.value(0, 1.121), 0.0);
}

#[test]
fn drag_elevation_regrab_and_partial_membership_keep_fades_and_reused_slots_safe() {
    let (mut store, remote, mut presentation, mut upload) = fixture(3);
    store.drag.members = mask(3, [0, 1]).words().clone();
    at(&mut store, &remote, &mut presentation, &mut upload, 0.0);
    at(&mut store, &remote, &mut presentation, &mut upload, 0.1);
    store.drag.members = mask(3, [0]).words().clone();
    at(&mut store, &remote, &mut presentation, &mut upload, 0.1);
    assert_eq!(presentation.value(0, 0.14), 1.0);
    assert!((presentation.value(1, 0.14) - 0.5).abs() < 1e-6);
    store.drag = default();
    at(&mut store, &remote, &mut presentation, &mut upload, 0.14);
    at(&mut store, &remote, &mut presentation, &mut upload, 0.18);
    let before = presentation.value(0, 0.18);
    store.drag.members = mask(3, [0]).words().clone();
    at(&mut store, &remote, &mut presentation, &mut upload, 0.18);
    assert_eq!(presentation.value(0, 0.18), before);
    at(&mut store, &remote, &mut presentation, &mut upload, 0.3);
    assert_eq!(presentation.value(0, 0.3), 1.0);
    assert_eq!(presentation.value(1, 0.3), 0.0);
    store.drag.members = mask(3, [2]).words().clone();
    at(&mut store, &remote, &mut presentation, &mut upload, 0.3);
    at(&mut store, &remote, &mut presentation, &mut upload, 0.4);
    assert_eq!(presentation.value(0, 0.4), 0.0);
    assert_eq!(presentation.value(1, 0.4), 0.0);
    assert_eq!(presentation.value(2, 0.4), 1.0);
    // Some existing end paths leave a full-length zero mask behind.
    store.drag.members = vec![0].into();
    at(&mut store, &remote, &mut presentation, &mut upload, 0.4);
    assert_eq!(presentation.value(2, 0.4), 1.0);
    at(&mut store, &remote, &mut presentation, &mut upload, 0.481);
    assert!(!upload.active);
}

#[test]
fn drag_elevation_remote_release_reuse_disconnect_and_epoch_reset() {
    let (mut store, mut remote, mut presentation, mut upload) = fixture(3);
    let slot = remote.allocate(mask(3, [0, 1]), Vec2::ZERO).unwrap();
    at(&mut store, &remote, &mut presentation, &mut upload, 0.0);
    at(&mut store, &remote, &mut presentation, &mut upload, 0.1);
    remote.release(slot);
    let reused = remote.allocate(mask(3, [2]), Vec2::ZERO).unwrap();
    assert_eq!(slot, reused);
    at(&mut store, &remote, &mut presentation, &mut upload, 0.1);
    assert_eq!(presentation.value(0, 0.1), 1.0);
    assert_eq!(presentation.value(2, 0.1), 0.0);
    at(&mut store, &remote, &mut presentation, &mut upload, 0.2);
    assert_eq!(presentation.value(0, 0.2), 0.0);
    assert_eq!(presentation.value(2, 0.2), 1.0);
    remote.reset(store.epoch, 3); // disconnect/teardown in the same puzzle epoch
    at(&mut store, &remote, &mut presentation, &mut upload, 0.2);
    assert_eq!(presentation.value(2, 0.2), 1.0);
    at(&mut store, &remote, &mut presentation, &mut upload, 0.281);
    assert!(!upload.active);
    store.drag.members = mask(3, [0]).words().clone();
    at(&mut store, &remote, &mut presentation, &mut upload, 0.3);
    store.initialize(vec![Vec2::ZERO; 3]);
    remote.reset(store.epoch, 3);
    at(&mut store, &remote, &mut presentation, &mut upload, 0.4);
    assert!(!upload.active);
    assert!(upload.records.is_empty());
    assert!(upload.ranges.is_empty());
}

#[test]
fn drag_elevation_rotation_max_is_bounded_and_continuous_at_control_boundaries() {
    let rotation = super::super::rotation_visual::RotationAnimation {
        pivot: Vec2::ZERO,
        offset: Vec2::ZERO,
        residual: std::f32::consts::FRAC_PI_2,
        target_angle: 0.0,
        start: 0.0,
        duration: 0.120,
        start_elevation: 0.0,
    };
    let (mut store, remote, mut presentation, mut upload) = fixture(1);
    let grab = 0.03;
    let release = 0.09;
    let before = rotation.elevation(grab);
    store.drag.members = mask(1, [0]).words().clone();
    at(&mut store, &remote, &mut presentation, &mut upload, grab);
    assert_eq!(
        rotation.elevation(grab).max(presentation.value(0, grab)),
        before
    );
    at(&mut store, &remote, &mut presentation, &mut upload, release);
    let before = rotation
        .elevation(release)
        .max(presentation.value(0, release));
    store.drag = default();
    at(&mut store, &remote, &mut presentation, &mut upload, release);
    assert_eq!(
        rotation
            .elevation(release)
            .max(presentation.value(0, release)),
        before
    );
    let mut previous = 0.0f32;
    for i in 0..=2000 {
        let time = i as f64 / 10000.0;
        let lift = if time < grab {
            0.0
        } else if time < release {
            let t = ((time - grab) / DRAG_ELEVATION_SECONDS) as f32;
            t * t * (3.0 - 2.0 * t)
        } else {
            presentation.value(0, time)
        };
        let elevation = rotation.elevation(time).max(lift);
        assert!((0.0..=1.0).contains(&elevation));
        assert!((elevation - previous).abs() < 0.01);
        previous = elevation;
    }
    assert_eq!(previous, 0.0);
}

#[test]
fn drag_elevation_observes_actual_local_release_and_disconnect_cleanup() {
    use jigsall_core::{PieceCommand, LOCAL_PLAYER};
    for disconnect in [false, true] {
        let (mut store, remote, mut presentation, mut upload) = fixture(2);
        store.connectivity.union(PieceId(0), PieceId(1));
        // Input installs the frozen local mask before authority dispatch. The
        // command adapter synchronizes only this exact gesture's shared Arc.
        let requested = mask(2, [0, 1]);
        store.drag.members = requested.words().clone();
        assert_eq!(
            store
                .apply_command(
                    LOCAL_PLAYER,
                    &PieceCommand::GrabGroup { members: requested },
                    None,
                    LOCAL_PLAYER
                )
                .grabbed,
            2
        );
        let grabbed = store.states.clone();
        at(&mut store, &remote, &mut presentation, &mut upload, 0.0);
        at(&mut store, &remote, &mut presentation, &mut upload, 0.1);
        assert_eq!(store.states, grabbed);
        assert_eq!(presentation.value(0, 0.1), 1.0);
        if disconnect {
            assert_eq!(store.clear_player_holds(LOCAL_PLAYER).len(), 2);
        } else {
            assert_eq!(
                store
                    .apply_command(
                        LOCAL_PLAYER,
                        &PieceCommand::ReleaseGroup {
                            members: mask(2, [0, 1]),
                            delta: Vec2::ZERO
                        },
                        None,
                        LOCAL_PLAYER
                    )
                    .released,
                2
            );
        }
        let released = store.states.clone();
        at(&mut store, &remote, &mut presentation, &mut upload, 0.1);
        assert_eq!(presentation.value(0, 0.1), 1.0);
        assert_eq!(presentation.value(1, 0.1), 1.0);
        at(&mut store, &remote, &mut presentation, &mut upload, 0.181);
        assert!(!upload.active);
        assert_eq!(store.states, released);
    }
}

#[test]
fn drag_elevation_million_pointer_frames_keep_membership_records_and_uploads_unchanged() {
    let (mut store, remote, mut presentation, mut upload) = fixture(1_000_000);
    assert!(presentation.mapping.is_empty()); // idle allocates no per-piece lift data
    let mut mask = PieceBitSet::new(1_000_000);
    mask.fill();
    store.drag.members = mask.words().clone();
    at(&mut store, &remote, &mut presentation, &mut upload, 0.0);
    let revision = upload.revision;
    let records = upload.records.clone();
    let ranges = upload.ranges.clone();
    let mapping = presentation.mapping.as_ptr();
    for i in 1..10_000 {
        super::super::pieces::without_piece_state_access(|| {
            store.drag.delta = Vec2::splat(i as f32);
            at(
                &mut store,
                &remote,
                &mut presentation,
                &mut upload,
                i as f64 / 60.0,
            );
        });
        assert_eq!(upload.revision, revision);
        assert!(Arc::ptr_eq(&upload.records, &records));
        assert!(Arc::ptr_eq(&upload.ranges, &ranges));
        assert_eq!(presentation.mapping.as_ptr(), mapping);
        assert!(presentation.dirty.is_empty());
    }
    assert_eq!(presentation.value(999_999, 1000.0), 1.0);
    assert_eq!(std::mem::size_of::<GpuDragElevation>(), 16);
}
