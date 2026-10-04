use super::*;
use crate::{
    multiplayer::{GameSnapshot, SnapshotExpectation},
    resources::pieces::{HELD, PLACED},
    selection::SelectionPayload,
};
use jigsall_core::{
    session::{AuthorityCursor, ImageHash, SessionDefinition, SessionId},
    PieceBitSet, PieceCommand, PlayerId,
};

fn restore_components(app: &mut App, def: &PuzzleDefinition, unions: &[(u32, u32)]) {
    let mut source = PieceDataStore::default();
    source.initialize(
        (0..def.piece_count() as u32)
            .map(|id| def.correct_position(PieceId(id)) + Vec2::splat(10_000.0))
            .collect(),
    );
    for &(a, b) in unions {
        source.connectivity.union(PieceId(a), PieceId(b));
    }
    let session = SessionDefinition {
        id: SessionId(12),
        image_hash: ImageHash([3; 32]),
    };
    let cursor = AuthorityCursor::new(1, 0);
    let snapshot = GameSnapshot::capture(&source, def, session, cursor).unwrap();
    snapshot
        .install(
            &mut app.world_mut().resource_mut::<PieceDataStore>(),
            SnapshotExpectation {
                session: session.id,
                image_hash: session.image_hash,
                cursor,
                definition: def,
            },
        )
        .unwrap();
    wait_ready(app);
}
fn preview(app: &mut App, rect: Rect) {
    // Settle any preceding CPU authority/selection edit before measuring the
    // preview request itself (which must upload no selection/root metadata).
    update_gpu(app);
    let id = app
        .world_mut()
        .resource_mut::<PuzzleSelection>()
        .request_preview(rect);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        update_gpu(app);
        if app
            .sub_app(RenderApp)
            .world()
            .resource::<GpuRenderer>()
            .last_submitted
            == id
        {
            break;
        }
        assert!(Instant::now() < deadline);
    }
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert!(gpu.maps.is_empty(), "preview must not map a readback");
    assert!(app
        .world()
        .resource::<PuzzleSelection>()
        .completed
        .is_none());
    assert_eq!(gpu.root_upload_bytes, 0);
    assert_eq!(gpu.selection_upload_bytes, 0, "preview request {rect:?}");
    if app
        .sub_app(RenderApp)
        .world()
        .resource::<ExtractedPuzzle>()
        .region
        .is_some()
    {
        assert_eq!(gpu.uniform.get().preview_active, 1);
        assert_eq!(
            gpu.pick_uniform.get().preview_active,
            0,
            "picking must not load preview roots while the main draw previews"
        );
    }
    assert_eq!(
        gpu.preview_dispatches,
        usize::from(
            app.sub_app(RenderApp)
                .world()
                .resource::<ExtractedPuzzle>()
                .region
                .is_some()
        ),
        "one collapse pass for a nonempty preview, none for empty/clipped"
    );
}
fn masks(app: &App) -> (u32, u32) {
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    let buffers = gpu.buffers.as_ref().unwrap();
    let word = |buffer| u32::from_le_bytes(read_buffer(app, buffer, 4).try_into().unwrap());
    (word(&buffers.direct_hits), word(&buffers.preview))
}
fn roots(app: &App) -> Vec<u32> {
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    bytemuck::cast_slice::<u8, u32>(&read_buffer(
        app,
        &gpu.buffers.as_ref().unwrap().component_roots,
        32,
    ))
    .to_vec()
}
fn final_rectangle(app: &mut App, rect: Rect) -> PieceBitSet {
    let id = app
        .world_mut()
        .resource_mut::<PuzzleSelection>()
        .request(rect, SelectionMode::Rectangle);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        update_gpu(app);
        if let Some(result) = app
            .world_mut()
            .resource_mut::<PuzzleSelection>()
            .take_result(id)
        {
            assert!(result.error.is_none());
            let SelectionPayload::Rectangle(mask) = result.payload else {
                panic!()
            };
            assert_eq!(mask.bit_len(), 8);
            assert_eq!(
                mask.words().len() * 4,
                4,
                "readback stays ceil(N/32)*4 bytes"
            );
            return mask;
        }
        assert!(Instant::now() < deadline);
    }
}
fn pixel(pixels: &[u8], x: usize, y: usize) -> &[u8] {
    &pixels[(y * 384 + x) * 4..(y * 384 + x) * 4 + 4]
}
fn assert_blue(color: &[u8]) {
    assert!(
        color[0] < 170 && color[1] > 180 && color[2] == 255,
        "{color:?}"
    );
}
fn assert_yellow(color: &[u8]) {
    assert!(
        color[0] == 255 && color[1] > 200 && color[2] < 30,
        "{color:?}"
    );
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_component_rectangle_preview_matches_final_selection_without_readback() {
    let (mut app, camera, target) = gpu_app(384);
    let mut def = definition(UVec2::new(4, 2), 256, 42);
    def.image_size.y = 128;
    app.world_mut().insert_resource(def.clone());
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation = Vec2::splat(10_000.0).extend(0.0);
    restore_components(&mut app, &def, &[(0, 1), (2, 6)]);
    assert_eq!(roots(&app), [0, 0, 2, 3, 4, 5, 2, 7]);
    let a = Rect::new(92.0, 156.0, 100.0, 164.0);
    let b = Rect::new(220.0, 156.0, 228.0, 164.0);
    let singleton = Rect::new(284.0, 156.0, 292.0, 164.0);
    preview(&mut app, a);
    assert_eq!(masks(&app), (1, 1), "one member hit, component root marked");
    assert!(app
        .sub_app(RenderApp)
        .world()
        .resource::<GpuRenderer>()
        .slots
        .is_empty());
    let pixels = rendered_pixels(&mut app, target.clone());
    assert_blue(pixel(&pixels, 96, 128));
    assert_blue(pixel(&pixels, 160, 128)); // A's unhit second member.
    assert_eq!(
        pixel(&pixels, 128, 140),
        &[255, 255, 255, 255],
        "no internal blue border"
    );
    assert_eq!(
        pixel(&pixels, 224, 128),
        &[255, 255, 255, 255],
        "unhit component"
    );

    preview(&mut app, Rect::new(92.0, 156.0, 228.0, 164.0));
    assert_eq!(
        masks(&app),
        (7, 5),
        "multiple component roots collapse independently"
    );
    let pixels = rendered_pixels(&mut app, target.clone());
    assert_blue(pixel(&pixels, 224, 255)); // Unhit B member 6.
    assert_eq!(pixel(&pixels, 288, 128), &[255, 255, 255, 255]);
    preview(&mut app, b);
    assert_eq!(
        masks(&app),
        (4, 4),
        "moving/shrinking removes old component bits"
    );
    preview(&mut app, singleton);
    assert_eq!(
        masks(&app),
        (8, 8),
        "disconnected singleton stays piece-local"
    );
    let pixels = rendered_pixels(&mut app, target.clone());
    assert_blue(pixel(&pixels, 288, 128));
    assert_eq!(pixel(&pixels, 96, 128), &[255, 255, 255, 255]);
    for rect in [
        Rect::new(400.0, 400.0, 410.0, 410.0),
        Rect::new(0.0, 0.0, 0.0, 0.0),
    ] {
        preview(&mut app, rect);
        assert_eq!(masks(&app), (0, 0), "empty/clipped clears both masks");
        assert_eq!(
            app.sub_app(RenderApp)
                .world()
                .resource::<GpuRenderer>()
                .preview_dispatches,
            0
        );
    }
    preview(&mut app, a);
    let direct = final_rectangle(&mut app, a);
    assert_eq!(direct.iter().collect::<Vec<_>>(), [PieceId(0)]);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .commit_selection(direct, None, jigsall_core::LOCAL_PLAYER);
    assert_eq!(
        app.world()
            .resource::<PieceDataStore>()
            .selected_pieces
            .iter()
            .collect::<Vec<_>>(),
        [PieceId(0), PieceId(1)]
    );
    let pixels = rendered_pixels(&mut app, target.clone());
    assert_yellow(pixel(&pixels, 96, 128));
    assert_yellow(pixel(&pixels, 160, 128)); // Selected beats blue preview.
    assert_eq!(pixel(&pixels, 128, 140), &[255, 255, 255, 255]);

    // A hold arriving after readback is still rejected by final CPU authority.
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .selected_pieces
        .clear();
    update_gpu(&mut app);
    let stale_direct = final_rectangle(&mut app, a);
    let owner = PlayerId(2);
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        assert_eq!(
            store
                .apply_command(
                    owner,
                    &PieceCommand::Grab(PieceId(0)),
                    None,
                    jigsall_core::LOCAL_PLAYER
                )
                .grabbed,
            2
        );
        store.commit_selection(stale_direct, None, jigsall_core::LOCAL_PLAYER);
        assert!(store.selected_pieces.is_empty());
        assert!(store
            .connectivity
            .iter_component(PieceId(0))
            .all(|id| store.states[id.0 as usize].flags & HELD != 0));
    }
    preview(&mut app, a);
    assert_eq!(masks(&app), (0, 0), "held components have no direct hits");
    assert!(final_rectangle(&mut app, a).is_empty());
    assert_eq!(
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .apply_command(
                owner,
                &PieceCommand::Release(PieceId(0)),
                None,
                jigsall_core::LOCAL_PLAYER
            )
            .released,
        2
    );
    preview(&mut app, a);
    assert_eq!(
        masks(&app),
        (1, 1),
        "release restores component selectability"
    );

    // Disabled fixtures preserve the same component-wide selectability invariant.
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        for id in [PieceId(0), PieceId(1)] {
            store.states[id.0 as usize].flags &= !ENABLED;
            store.dirty_pieces.insert(id);
        }
    }
    preview(&mut app, a);
    assert_eq!(masks(&app), (0, 0));
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        for id in [PieceId(0), PieceId(1)] {
            store.states[id.0 as usize].flags |= ENABLED;
            store.dirty_pieces.insert(id);
        }
        store.apply_command(
            owner,
            &PieceCommand::Grab(PieceId(0)),
            Some(&def),
            jigsall_core::LOCAL_PLAYER,
        );
        store.apply_command(
            owner,
            &PieceCommand::Move {
                id: PieceId(0),
                position: def.correct_position(PieceId(0)),
            },
            Some(&def),
            jigsall_core::LOCAL_PLAYER,
        );
        assert_eq!(
            store
                .apply_command(
                    owner,
                    &PieceCommand::Release(PieceId(0)),
                    Some(&def),
                    jigsall_core::LOCAL_PLAYER
                )
                .placed,
            2
        );
        assert!(store
            .connectivity
            .iter_component(PieceId(0))
            .all(|id| store.states[id.0 as usize].flags & PLACED != 0));
    }
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation = Vec3::ZERO;
    preview(&mut app, a);
    assert_eq!(
        masks(&app),
        (0, 0),
        "visible placed components have no direct hits"
    );
    assert!(final_rectangle(&mut app, a).is_empty());
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation = Vec2::splat(10_000.0).extend(0.0);
    restore_components(&mut app, &def, &[(0, 1), (2, 6)]);
    update_gpu(&mut app);

    // A growing closure updates only formerly absorbed components.
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .snap_fixture_component(PieceId(0), &def);
    update_gpu(&mut app);
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert_eq!(gpu.root_upload_bytes, 24);
    assert_eq!(gpu.root_upload_calls, 1);
    assert_eq!(roots(&app), vec![0; 8]);
    preview(&mut app, a);
    assert_eq!(masks(&app), (1, 1));
    let pixels = rendered_pixels(&mut app, target);
    assert_blue(pixel(&pixels, 288, 128));
    for step in 0..4 {
        app.world_mut().resource_mut::<PieceDataStore>().drag.delta = Vec2::splat(step as f32);
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation
            .x += 1.0;
        update_gpu(&mut app);
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(
            (
                gpu.root_upload_bytes,
                gpu.root_upload_calls,
                gpu.preview_dispatches,
                gpu.selection_upload_bytes
            ),
            (0, 0, 0, 0)
        );
    }
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation
        .x = 10_000.0;
    app.world_mut().resource_mut::<PieceDataStore>().drag.delta = Vec2::ZERO;
    restore_components(&mut app, &def, &[(1, 2), (1, 3)]);
    assert_eq!(roots(&app), [0, 1, 1, 1, 4, 5, 6, 7]);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .snap_fixture_component(PieceId(0), &def);
    update_gpu(&mut app);
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert_eq!((gpu.root_upload_bytes, gpu.root_upload_calls), (20, 2));
    assert_eq!(
        roots(&app),
        vec![1; 8],
        "larger target wins; no stable-minimum key"
    );
    preview(&mut app, a);
    assert_eq!(
        masks(&app),
        (1, 2),
        "render follows representative, not direct ID"
    );
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_component_preview_crosses_mask_words_and_preserves_direct_high_bits() {
    let (mut app, camera, _) = gpu_app(384);
    let def = definition(UVec2::splat(8), 256, 42);
    app.world_mut().insert_resource(def.clone());
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation = Vec2::splat(10_000.0).extend(0.0);
    restore_components(&mut app, &def, &[(54, 55), (55, 63)]);
    let root = app
        .world()
        .resource::<PieceDataStore>()
        .connectivity
        .find_root(PieceId(63))
        .0;
    assert_eq!(root, 54);
    let words = |app: &App, direct: bool| {
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        let buffers = gpu.buffers.as_ref().unwrap();
        let buffer = if direct {
            &buffers.direct_hits
        } else {
            &buffers.preview
        };
        bytemuck::cast_slice::<u8, u32>(&read_buffer(app, buffer, 8)).to_vec()
    };
    let rect = Rect::new(302.0, 302.0, 306.0, 306.0);
    preview(&mut app, rect);
    assert_eq!(words(&app, true), [0, 1 << 31]);
    assert_eq!(words(&app, false), [0, 1 << (root - 32)]);
    assert!(app
        .sub_app(RenderApp)
        .world()
        .resource::<GpuRenderer>()
        .slots
        .is_empty());
    preview(&mut app, Rect::new(78.0, 78.0, 82.0, 82.0));
    assert_eq!(words(&app, true), [1, 0]);
    assert_eq!(words(&app, false), [1, 0]);
    let id = app
        .world_mut()
        .resource_mut::<PuzzleSelection>()
        .request(rect, SelectionMode::Rectangle);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mask = loop {
        update_gpu(&mut app);
        if let Some(result) = app
            .world_mut()
            .resource_mut::<PuzzleSelection>()
            .take_result(id)
        {
            assert!(result.error.is_none());
            let SelectionPayload::Rectangle(mask) = result.payload else {
                panic!()
            };
            break mask;
        }
        assert!(Instant::now() < deadline);
    };
    assert_eq!(mask.words().len() * 4, 8);
    assert_eq!(mask.iter().collect::<Vec<_>>(), [PieceId(63)]);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .commit_selection(mask, None, jigsall_core::LOCAL_PLAYER);
    assert_eq!(
        app.world()
            .resource::<PieceDataStore>()
            .selected_pieces
            .iter()
            .collect::<Vec<_>>(),
        [PieceId(54), PieceId(55), PieceId(63)]
    );
}
