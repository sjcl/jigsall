use super::*;
use crate::resources::pieces::CONNECTED_EDGES;
use puzzella_core::{
    protocol::{ComponentRef, PieceTarget},
    rotate_quarter, with_rotation, PieceCommand, LOCAL_PLAYER,
};

fn rotate_body(app: &mut App, def: &PuzzleDefinition, id: PieceId) {
    let mut store = app.world_mut().resource_mut::<PieceDataStore>();
    let reference = ComponentRef::from_member(&store.connectivity, id).unwrap();
    assert!(
        store
            .apply_command(
                LOCAL_PLAYER,
                &PieceCommand::Rotate {
                    target: PieceTarget::Component(reference),
                    quarter_turns: 1
                },
                Some(def),
                puzzella_core::LOCAL_PLAYER
            )
            .rotated
            > 0
    );
}

fn screen_point(world: Vec2, resolution: f32) -> Rect {
    let pixel = Vec2::new(world.x + resolution * 0.5, resolution * 0.5 - world.y).floor();
    Rect::from_corners(pixel, pixel + Vec2::ONE)
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_hover_rotation_uses_frontmost_point_pick_and_rotates_the_connected_component() {
    use crate::{
        interaction::PieceInteraction,
        resources::{GameUiPointerCapture, InputState, LocalPlayerId, PerformanceMonitor},
        systems::{game_logic::apply_piece_commands, piece_interaction::handle_piece_input},
    };

    #[derive(Resource, Default)]
    struct TestKey(Option<KeyCode>);
    fn press_key(mut key: ResMut<TestKey>, mut keys: ResMut<ButtonInput<KeyCode>>) {
        if let Some(key) = key.0.take() {
            keys.press(key);
        }
    }
    fn rotate_at(app: &mut App, position: Vec2, key: KeyCode) {
        let mut input = app.world_mut().resource_mut::<InputState>();
        input.window_focused = true;
        input.mouse_position = Some(position);
        input.cursor_screen_position = Some(Vec2::new(128. + position.x, 128. - position.y));
        app.world_mut().resource_mut::<TestKey>().0 = Some(key);
        update_gpu(app);
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.world().resource::<PuzzleSelection>().latest.is_some() {
            update_gpu(app);
            assert!(Instant::now() < deadline, "hover rotation timed out");
        }
    }

    let (mut app, _, _) = gpu_app(256);
    let mut def = definition(UVec2::new(3, 1), 120, 42);
    def.image_size.y = 30;
    let translation = Vec2::new(-15., 20.);
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.initialize(
            (0..3)
                .map(|id| def.correct_position(PieceId(id)) + translation)
                .collect(),
        );
        store.connectivity.union(PieceId(0), PieceId(1));
        // Piece 2 overlaps member 1, so GPU depth must choose the singleton.
        store.states[2].position = translation;
        store.states[2].z_order = 100;
    }
    app.insert_resource(def)
        .insert_resource(LocalPlayerId(puzzella_core::PlayerId(42)))
        .insert_resource(crate::keybindings::KeyBindingsState::load(None))
        .init_resource::<PieceInteraction>()
        .init_resource::<InputState>()
        .init_resource::<GameUiPointerCapture>()
        .init_resource::<PerformanceMonitor>()
        .init_resource::<bevy_egui::EguiUserTextures>()
        .init_resource::<TestKey>()
        .add_message::<puzzella_core::ClientCommand>()
        .add_systems(
            Update,
            (press_key, handle_piece_input, apply_piece_commands).chain(),
        );
    wait_ready(&mut app);
    update_gpu(&mut app);
    let selected = app
        .world()
        .resource::<PieceDataStore>()
        .selected_pieces
        .words()
        .clone();
    let original = app.world().resource::<PieceDataStore>().states.to_vec();
    rotate_at(&mut app, translation, KeyCode::KeyQ);
    let store = app.world().resource::<PieceDataStore>();
    assert_eq!(&store.states[..2], &original[..2]);
    assert_eq!(puzzella_core::decode_rotation(store.states[2].flags), 1);
    assert_eq!(store.states[2].position, translation);
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert_eq!(gpu.upload_bytes, 16);
    assert_eq!(gpu.drag_upload_bytes, 0);
    assert_eq!(gpu.root_upload_bytes, 0);

    // After removing the covering piece, the same pixel addresses member 1,
    // whose component includes member 0 away from the cursor.
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.states[2].flags &= !ENABLED;
        store.dirty_pieces.insert(PieceId(2));
    }
    update_gpu(&mut app);
    rotate_at(&mut app, translation, KeyCode::KeyE);
    let store = app.world().resource::<PieceDataStore>();
    let pivot = (original[0].position + original[1].position) * 0.5;
    for (id, original) in original.iter().enumerate().take(2) {
        assert_eq!(puzzella_core::decode_rotation(store.states[id].flags), 3);
        assert_eq!(
            store.states[id].position,
            pivot + rotate_quarter(original.position - pivot, 3)
        );
    }
    assert!(store.selected_pieces.is_empty());
    assert!(Arc::ptr_eq(&selected, store.selected_pieces.words()));
    assert!(store.held_by.is_empty());
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert_eq!(gpu.upload_bytes, 32);
    assert_eq!(gpu.drag_upload_bytes, 0);
    assert_eq!(gpu.root_upload_bytes, 0);
    update_gpu(&mut app);
    assert!(app.world().resource::<PuzzleSelection>().latest.is_none());
    assert_eq!(
        app.sub_app(RenderApp)
            .world()
            .resource::<GpuRenderer>()
            .upload_bytes,
        0
    );
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_drag_rotation_rebase_uploads_only_state_and_keeps_membership_on_pointer_frames() {
    let (mut app, _, target) = gpu_app(256);
    let mut def = definition(UVec2::new(3, 2), 192, 42);
    def.image_size.y = 64;
    app.world_mut().insert_resource(def.clone());
    let mut members = puzzella_core::PieceBitSet::new(6);
    members.insert(PieceId(1));
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.initialize(vec![Vec2::new(-30.5, 20.5); 6]);
        for (id, state) in store.states.iter_mut().enumerate() {
            if id != 1 {
                state.flags = 0;
            }
        }
        store.drag.members = members.words().clone();
        assert_eq!(
            store
                .apply_command(
                    LOCAL_PLAYER,
                    &PieceCommand::GrabGroup {
                        members: members.clone()
                    },
                    Some(&def),
                    puzzella_core::LOCAL_PLAYER
                )
                .grabbed,
            1
        );
        store.drag.delta = Vec2::new(15., -10.);
    }
    wait_ready(&mut app);
    update_gpu(&mut app);
    let mask = app
        .world()
        .resource::<PieceDataStore>()
        .drag
        .members
        .clone();
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        assert!(
            store
                .apply_command(
                    LOCAL_PLAYER,
                    &PieceCommand::RotateDrag {
                        members: members.clone(),
                        delta: Vec2::new(15., -10.),
                        quarter_turns: 1,
                    },
                    Some(&def),
                    puzzella_core::LOCAL_PLAYER
                )
                .drag_rebased
        );
    }
    update_gpu(&mut app);
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert_eq!(gpu.upload_bytes, 16);
    assert_eq!(gpu.drag_upload_bytes, 0);
    assert_eq!(gpu.root_upload_bytes, 0);
    let store = app.world().resource::<PieceDataStore>();
    assert_eq!(store.states[1].position, Vec2::new(-15.5, 10.5));
    assert_eq!(store.drag.delta, Vec2::ZERO);
    assert!(Arc::ptr_eq(&mask, &store.drag.members));
    let pixels = rendered_pixels(&mut app, target.clone());
    let point = screen_point(Vec2::new(-15.5, 10.5), 256.);
    let offset = (point.min.y as usize * 256 + point.min.x as usize) * 4;
    assert_eq!(&pixels[offset..offset + 4], &[255, 255, 255, 255]);
    for delta in [Vec2::ZERO, Vec2::new(10., 0.)] {
        app.world_mut().resource_mut::<PieceDataStore>().drag.delta = delta;
        update_gpu(&mut app);
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.upload_bytes, 0);
        assert_eq!(gpu.drag_upload_bytes, 0);
        assert_eq!(gpu.root_upload_bytes, 0);
    }
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .apply_command(
            LOCAL_PLAYER,
            &PieceCommand::ReleaseGroup {
                members,
                delta: Vec2::new(10., 0.),
            },
            Some(&def),
            puzzella_core::LOCAL_PLAYER,
        );
    assert_eq!(
        pick(
            &mut app,
            screen_point(Vec2::new(-5.5, 10.5), 256.),
            SelectionMode::Point
        ),
        vec![PieceId(1)]
    );
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_quarter_turn_images_shapes_and_picking_agree() {
    let (mut app, _, target) = gpu_app(256);
    let mut def = definition(UVec2::new(3, 2), 192, 42);
    def.image_size.y = 64;
    app.world_mut().insert_resource(def.clone());
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.initialize(vec![Vec2::ZERO; 6]);
        for (id, state) in store.states.iter_mut().enumerate() {
            if id != 1 {
                state.flags = 0;
            }
        }
    }
    // Four colored quadrants in each canonical cell make UV rotation observable.
    let colors = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 255, 255],
    ];
    let data = (0..64)
        .flat_map(|y| {
            (0..192).flat_map(move |x| {
                colors[usize::from(x % 64 >= 32) + 2 * usize::from(y % 32 >= 16)]
            })
        })
        .collect();
    let image = Image::new(
        Extent3d {
            width: 192,
            height: 64,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::all(),
    );
    let image = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().resource_mut::<PuzzleImage>().handle = image;
    wait_ready(&mut app);
    let size = Vec2::new(64.0, 32.0);
    let profiles = piece_profiles(def.seed, def.grid_size, UVec2::new(1, 0));
    for rotation in 0..4 {
        let pixels = rendered_pixels(&mut app, target.clone());
        let mut inside = 0;
        for y in 64..192 {
            for x in 64..192 {
                let world = Vec2::new(x as f32 - 127.5, 127.5 - y as f32);
                let local = rotate_quarter(world, (4 - rotation) % 4);
                let d = piece_signed_distance(local, size, profiles);
                if d.abs() < 0.3 {
                    continue;
                }
                let pixel = (y * 256 + x) * 4;
                let drawn = pixels[pixel..pixel + 3] != [0, 0, 0];
                assert_eq!(drawn, d < 0.0, "shape r={rotation} at {world:?}, d={d}");
                inside += usize::from(drawn);
            }
        }
        assert!(inside > 1500);
        for (index, local) in [
            Vec2::new(-12.0, 8.0),
            Vec2::new(12.0, 8.0),
            Vec2::new(-12.0, -8.0),
            Vec2::new(12.0, -8.0),
        ]
        .into_iter()
        .enumerate()
        {
            let rect = screen_point(rotate_quarter(local, rotation), 256.0);
            let pixel = (rect.min.y as usize * 256 + rect.min.x as usize) * 4;
            assert_eq!(
                &pixels[pixel..pixel + 4],
                &colors[index],
                "UV rotation {rotation}"
            );
        }
        // Both raster picking paths must match the rotated SDF, including outside
        // the unrotated AABB and concave/tab regions, not only piece centers.
        for world in [
            Vec2::new(-20.5, 10.5),
            Vec2::new(20.5, -10.5),
            Vec2::new(0.5, 24.5),
            Vec2::new(30.5, 0.5),
            Vec2::new(0.5, -26.5),
            Vec2::new(38.5, 0.5),
            Vec2::new(0.5, 38.5),
        ] {
            let d =
                piece_signed_distance(rotate_quarter(world, (4 - rotation) % 4), size, profiles);
            if d.abs() < 0.3 {
                continue;
            }
            let expected = if d < 0.0 { vec![PieceId(1)] } else { vec![] };
            for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
                assert_eq!(
                    pick(&mut app, screen_point(world, 256.0), mode),
                    expected,
                    "pick {mode:?}, r={rotation}, world={world:?}"
                );
            }
        }
        rotate_body(&mut app, &def, PieceId(1));
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_rotated_connected_outline_keeps_canonical_internal_edges_hidden() {
    let (mut app, camera, target) = gpu_app(256);
    let def = definition(UVec2::splat(3), 192, 42);
    let translation = Vec2::splat(10_000.0);
    app.world_mut().insert_resource(def.clone());
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation = translation.extend(0.0);
    let ids = [0, 1, 3];
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.initialize(
            (0..9)
                .map(|id| def.correct_position(PieceId(id)) + translation)
                .collect(),
        );
        for id in 0..9 {
            if !ids.contains(&id) {
                store.states[id as usize].flags = 0;
            }
        }
        store.snap_fixture_component(PieceId(0), &def);
        store.selected_pieces.extend(ids.map(PieceId));
    }
    wait_ready(&mut app);
    let original_flags: Vec<_> = ids
        .iter()
        .map(|&id| {
            app.world().resource::<PieceDataStore>().states[id as usize].flags & CONNECTED_EDGES
        })
        .collect();
    for rotation in 0..4 {
        let pixels = rendered_pixels(&mut app, target.clone());
        let store = app.world().resource::<PieceDataStore>();
        let mut seams = 0;
        let mut outer = 0;
        for y in 0..256 {
            for x in 0..256 {
                let world = translation + Vec2::new(x as f32 - 127.5, 127.5 - y as f32);
                let drawn = ids.iter().rev().copied().find(|&id| {
                    let local = rotate_quarter(
                        world - store.states[id as usize].position,
                        (4 - rotation) % 4,
                    );
                    let profiles =
                        piece_profiles(def.seed, def.grid_size, UVec2::new(id % 3, id / 3));
                    piece_signed_distance(local, Vec2::splat(64.0), profiles) <= 0.0
                });
                let Some(id) = drawn else { continue };
                let local = rotate_quarter(
                    world - store.states[id as usize].position,
                    (4 - rotation) % 4,
                );
                let profiles = piece_profiles(def.seed, def.grid_size, UVec2::new(id % 3, id / 3));
                let edges =
                    super::outline_tests::edge_distances(local, Vec2::splat(64.0), profiles);
                let neighbors = def.neighbors(PieceId(id));
                let boundary = edges
                    .into_iter()
                    .zip([neighbors[2], neighbors[1], neighbors[3], neighbors[0]])
                    .filter_map(|(edge, neighbor)| {
                        (!neighbor.is_some_and(|n| ids.contains(&n.0))).then_some(edge)
                    })
                    .fold(-1e20_f32, f32::max);
                let pixel = (y * 256 + x) * 4;
                let full = edges.into_iter().fold(-1e20_f32, f32::max);
                if boundary.abs() > 8.0 {
                    assert_eq!(&pixels[pixel..pixel + 4], &[255, 255, 255, 255]);
                    seams += usize::from(full.abs() < 1.5);
                } else if boundary.abs() < 3.0 {
                    assert!(pixels[pixel + 2] < 40);
                    outer += 1;
                }
            }
        }
        assert!(
            seams > 20 && outer > 100,
            "r={rotation}, seams={seams}, outer={outer}"
        );
        assert_eq!(
            pick(
                &mut app,
                Rect::new(0.0, 0.0, 256.0, 256.0),
                SelectionMode::Rectangle
            ),
            ids.map(PieceId)
        );
        for (&id, &flags) in ids.iter().zip(&original_flags) {
            assert_eq!(
                app.world().resource::<PieceDataStore>().states[id as usize].flags
                    & CONNECTED_EDGES,
                flags
            );
        }
        rotate_body(&mut app, &def, PieceId(0));
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_rotated_nonsquare_visibility_and_far_splats_include_viewport_edges() {
    let (mut app, _, target) = gpu_app(128);
    let mut def = definition(UVec2::new(2, 1), 80, 42);
    def.image_size.y = 10;
    app.world_mut().insert_resource(def.clone());
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        // Odd turns have 40-world-unit Y extent: the bottom end is visible.
        store.initialize(vec![Vec2::new(0.5, 78.5), Vec2::splat(1000.0)]);
        store.states[1].flags = 0;
        store.states[0].flags = with_rotation(ENABLED, 1);
    }
    wait_ready(&mut app);
    for rotation in [1, 3] {
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            store.states[0].flags = with_rotation(ENABLED, rotation);
            store.dirty_pieces.insert(PieceId(0));
        }
        let pixels = rendered_pixels(&mut app, target.clone());
        assert_eq!(visible_ids(&app), vec![0]);
        let point = screen_point(Vec2::new(0.5, 60.5), 128.0);
        let pixel = (point.min.y as usize * 128 + point.min.x as usize) * 4;
        assert!(pixels[pixel] > 0);
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert_eq!(pick(&mut app, point, mode), vec![PieceId(0)]);
        }
    }
    // At far zoom an anisotropic 0.5 x 2.5 rotated cell becomes a vertical
    // three-pixel splat. Its center lies outside the top edge of the viewport.
    def.image_size = UVec2::new(5, 1);
    app.world_mut().insert_resource(def.clone());
    app.world_mut().resource_mut::<PieceUpload>().definition = Some(def);
    for rotation in 0..4 {
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            store.initialize(vec![Vec2::new(0.875, 64.625), Vec2::splat(1000.0)]);
            store.states[1].flags = 0;
            store.states[0].flags = with_rotation(ENABLED, rotation);
        }
        wait_ready(&mut app);
        let pixels = rendered_pixels(&mut app, target.clone());
        let drawn = pixels.chunks_exact(4).filter(|p| p[0] > 0).count();
        assert_eq!(drawn, if rotation % 2 == 1 { 1 } else { 0 });
        let point = Rect::new(64.0, 0.0, 65.0, 1.0);
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert_eq!(
                pick(&mut app, point, mode),
                if rotation % 2 == 1 {
                    vec![PieceId(0)]
                } else {
                    vec![]
                }
            );
        }
    }
}
