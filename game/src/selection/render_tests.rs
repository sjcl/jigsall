use super::super::*;
use super::*;
use crate::resources::{PieceRenderData, PieceShapeData, StoredPieceData};
use bevy::{
    asset::RenderAssetUsages,
    camera::{RenderTarget, Viewport},
    mesh::Indices,
};
use puzzella_core::PieceState;

#[test]
fn coordinates_normalize_all_directions_and_clip_before_scissor() {
    let viewport = URect::new(20, 10, 100, 90);
    for (a, b) in [
        (Vec2::new(15., 10.), Vec2::new(40., 30.)),
        (Vec2::new(40., 30.), Vec2::new(15., 10.)),
        (Vec2::new(15., 30.), Vec2::new(40., 10.)),
        (Vec2::new(40., 10.), Vec2::new(15., 30.)),
    ] {
        let request = SelectionRequest {
            request_id: 1,
            region: Rect { min: a, max: b },
            mode: SelectionMode::Rectangle,
        };
        assert_eq!(
            pixel_region(request, 2., UVec2::splat(128), viewport),
            Some(PixelRegion {
                min: UVec2::new(30, 20),
                size: UVec2::new(50, 40)
            })
        );
    }
    let make = |a, b, mode| SelectionRequest {
        request_id: 1,
        region: Rect { min: a, max: b },
        mode,
    };
    assert_eq!(
        pixel_region(
            make(
                Vec2::splat(-20.),
                Vec2::splat(80.),
                SelectionMode::Rectangle
            ),
            2.,
            UVec2::splat(128),
            viewport
        ),
        Some(PixelRegion {
            min: viewport.min,
            size: viewport.size()
        })
    );
    for (a, b) in [
        (Vec2::ZERO, Vec2::ZERO),
        (Vec2::ZERO, Vec2::Y),
        (Vec2::splat(-20.), Vec2::splat(-10.)),
        (Vec2::splat(f32::NAN), Vec2::ONE),
    ] {
        assert!(pixel_region(
            make(a, b, SelectionMode::Rectangle),
            2.,
            UVec2::splat(128),
            viewport
        )
        .is_none());
    }
    assert!(pixel_region(
        make(Vec2::splat(20.), Vec2::splat(40.), SelectionMode::Rectangle),
        2.,
        UVec2::ZERO,
        viewport
    )
    .is_none());
    assert!(pixel_region(
        make(Vec2::splat(-0.1), Vec2::ZERO, SelectionMode::Point),
        1.,
        UVec2::splat(128),
        URect::new(0, 0, 128, 128)
    )
    .is_none());
    assert_eq!(
        pixel_region(
            make(Vec2::splat(20.9), Vec2::ZERO, SelectionMode::Point),
            2.,
            UVec2::splat(128),
            viewport
        )
        .unwrap()
        .size,
        UVec2::ONE
    );
}
#[test]
fn bitset_sizes_and_zero_id_and_word_boundaries() {
    assert_eq!(bitset_bytes(Some(9999)), 1252);
    assert_eq!(bitset_bytes(Some(31)), 4);
    assert_eq!(bitset_bytes(Some(32)), 8);
    let mut bytes = vec![0; 1252];
    for id in [0u32, 31, 32, 9999] {
        let offset = (id / 32 * 4) as usize;
        let mut word = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        word |= 1 << (id % 32);
        bytes[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
    }
    assert_eq!(
        decode_ids(SelectionMode::Rectangle, &bytes),
        [0, 31, 32, 9999].map(PieceId)
    );
    assert_eq!(
        decode_ids(SelectionMode::Point, &1u32.to_le_bytes()),
        vec![PieceId(0)]
    );
    assert!(decode_ids(SelectionMode::Point, &0u32.to_le_bytes()).is_empty());
}
#[test]
fn stale_callbacks_do_not_overwrite_latest_or_cross_sessions() {
    let mut app = App::new();
    let (tx, rx) = unbounded();
    app.init_resource::<PuzzleSelection>()
        .init_resource::<PieceDataStore>()
        .insert_resource(ResultInbox(rx))
        .add_systems(Update, receive_results);
    let mut requests = app.world_mut().resource_mut::<PuzzleSelection>();
    let first = requests.request(Rect::default(), SelectionMode::Point);
    let old = requests.latest.unwrap();
    requests.cancel();
    let second = requests.request(Rect::default(), SelectionMode::Point);
    assert!(second > first);
    tx.send(RawResult {
        request: old,
        bytes: 1u32.to_le_bytes().to_vec(),
        error: None,
    })
    .unwrap();
    app.update();
    assert!(app
        .world()
        .resource::<PuzzleSelection>()
        .completed
        .is_none());
}

#[test]
fn selectable_mask_tracks_placement_ownership_and_removed_ids() {
    let mut app = App::new();
    app.init_resource::<PieceDataStore>()
        .init_resource::<Assets<Mesh>>();
    for id in [0, 31, 32, 9999] {
        spawn_fixture(
            &mut app,
            id,
            Rectangle::new(1., 1.).into(),
            Handle::default(),
            Vec3::ZERO,
        );
    }
    let mut store = app.world_mut().resource_mut::<PieceDataStore>();
    let mut bits = Vec::new();
    extract_selectable_bitset(&store, &mut bits);
    assert_eq!(bits.len(), 1252);
    assert_eq!(
        decode_ids(SelectionMode::Rectangle, &bits),
        [0, 31, 32, 9999].map(PieceId)
    );
    store.pieces.get_mut(&PieceId(31)).unwrap().state.placed = true;
    store.pieces.get_mut(&PieceId(32)).unwrap().state.held_by = Some(puzzella_core::PlayerId(1));
    store.pieces.get_mut(&PieceId(9999)).unwrap().state.held_by = Some(puzzella_core::LOCAL_PLAYER);
    extract_selectable_bitset(&store, &mut bits);
    assert_eq!(
        decode_ids(SelectionMode::Rectangle, &bits),
        vec![PieceId(0)]
    );
    store.pieces.get_mut(&PieceId(32)).unwrap().state.held_by = None;
    store.pieces.get_mut(&PieceId(9999)).unwrap().state.held_by = None;
    extract_selectable_bitset(&store, &mut bits);
    assert_eq!(
        decode_ids(SelectionMode::Rectangle, &bits),
        [0, 32, 9999].map(PieceId)
    );
    assert!(
        !store.is_selectable(PieceId(1)),
        "missing IDs are not selectable"
    );
    store.pieces.clear();
    extract_selectable_bitset(&store, &mut bits);
    assert_eq!(bits, vec![0; 4], "a smaller puzzle must clear old bits");
}

fn spawn_fixture(
    app: &mut App,
    id: u32,
    mesh: Mesh,
    material: Handle<ColorMaterial>,
    position: Vec3,
) -> Entity {
    let mut mesh = mesh;
    mesh.insert_attribute(ATTRIBUTE_PIECE_ID, vec![id; mesh.count_vertices()]);
    let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
    let piece = PuzzlePiece {
        id: PieceId(id),
        grid_position: UVec2::ZERO,
        correct_position: Vec2::ZERO,
        initial_position: position.truncate(),
    };
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .add_piece(StoredPieceData {
            definition: piece.clone(),
            state: PieceState::new(position.truncate()),
            render: PieceRenderData {
                bounds: Rect::default(),
                shape: PieceShapeData {
                    vertices: vec![],
                    indices: vec![],
                },
                mesh: mesh.clone(),
                stroke: default(),
                material: material.clone(),
            },
        });
    app.world_mut().spawn(PuzzlePieceId(PieceId(id)));
    app.world_mut()
        .spawn((
            Mesh2d(mesh),
            MeshMaterial2d(material),
            piece,
            Transform::from_translation(position),
        ))
        .id()
}
fn gpu_selection_result(app: &mut App, region: Rect, mode: SelectionMode) -> SelectionResult {
    // Let transform propagation, visibility and asset preparation catch up.
    for _ in 0..4 {
        app.update();
    }
    let id = app
        .world_mut()
        .resource_mut::<PuzzleSelection>()
        .request(region, mode);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        app.update();
        if let Some(result) = app
            .world_mut()
            .resource_mut::<PuzzleSelection>()
            .take_result(id)
        {
            return result;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "GPU request timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
fn gpu_result(app: &mut App, region: Rect, mode: SelectionMode) -> Vec<PieceId> {
    let result = gpu_selection_result(app, region, mode);
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.entities.len(), result.piece_ids.len());
    result.piece_ids
}
#[test]
#[ignore = "requires a real GPU: cargo test -p puzzella-game --locked gpu_raster_selection -- --ignored --nocapture"]
fn gpu_raster_selection() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>(),
    )
    .init_resource::<PieceDataStore>()
    .add_plugins(PuzzleSelectionPlugin);
    app.finish();
    app.cleanup();
    println!(
        "GPU adapter: {}",
        app.sub_app(RenderApp)
            .world()
            .resource::<bevy::render::renderer::RenderAdapterInfo>()
            .name
    );
    let target = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_target_texture(
            64,
            64,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
    let camera = app
        .world_mut()
        .spawn((
            Camera2d,
            MainCamera,
            RenderTarget::Image(target.clone().into()),
            Msaa::Off,
        ))
        .id();
    let material = app
        .world_mut()
        .resource_mut::<Assets<ColorMaterial>>()
        .add(ColorMaterial::default());
    let a = spawn_fixture(
        &mut app,
        0,
        Rectangle::new(32., 32.).into(),
        material.clone(),
        Vec3::ZERO,
    );
    let b = spawn_fixture(
        &mut app,
        9999,
        Rectangle::new(32., 32.).into(),
        material.clone(),
        Vec3::Z,
    );
    // One-pixel piece away from screen center distinguishes cropping from
    // shrinking the entire scene, half-pixel errors and inverted Y coordinates.
    spawn_fixture(
        &mut app,
        31,
        Rectangle::new(1., 1.).into(),
        material.clone(),
        Vec3::new(21.5, 13.5, 2.),
    );
    let point = Rect {
        min: Vec2::splat(32.5),
        max: Vec2::splat(32.5),
    };
    let rect = Rect::new(31., 31., 33., 33.);
    assert_eq!(
        gpu_result(&mut app, point, SelectionMode::Point),
        vec![PieceId(9999)]
    );
    let gpu = app.sub_app(RenderApp).world().resource::<GpuSelection>();
    assert!(
        gpu.rectangle_target.is_none(),
        "clicks must not allocate a full-screen picking target"
    );
    let points = gpu.point_targets.as_ref().unwrap();
    let one_pixel = Extent3d {
        width: 1,
        height: 1,
        depth_or_array_layers: 1,
    };
    assert_eq!(points.id.size(), one_pixel);
    assert_eq!(points.depth_view.texture().size(), one_pixel);
    let point_texture = points.id.id();
    let point_depth = points.depth_view.id();
    // Keep the front piece's rendered Z unchanged: selection must not rely on
    // placed/held pieces being behind movable pieces.
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .pieces
        .get_mut(&PieceId(9999))
        .unwrap()
        .state
        .placed = true;
    for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
        assert_eq!(gpu_result(&mut app, rect, mode), vec![PieceId(0)]);
    }
    for player in [puzzella_core::PlayerId(1), puzzella_core::LOCAL_PLAYER] {
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            let state = &mut store.pieces.get_mut(&PieceId(9999)).unwrap().state;
            state.placed = false;
            state.held_by = Some(player);
        }
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert_eq!(gpu_result(&mut app, rect, mode), vec![PieceId(0)]);
        }
    }
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .pieces
        .get_mut(&PieceId(9999))
        .unwrap()
        .state
        .held_by = None;
    assert_eq!(
        gpu_result(&mut app, point, SelectionMode::Point),
        vec![PieceId(9999)]
    );
    // The rendered high ID survives removal from the canonical store. A smaller
    // mask binding must reject it, even when the reused buffer has spare capacity.
    let removed = app
        .world_mut()
        .resource_mut::<PieceDataStore>()
        .pieces
        .remove(&PieceId(9999))
        .unwrap();
    for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
        assert_eq!(gpu_result(&mut app, rect, mode), vec![PieceId(0)]);
    }
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .pieces
        .insert(PieceId(9999), removed);
    assert_eq!(
        gpu_result(
            &mut app,
            Rect::new(53.99, 18.01, 54., 19.),
            SelectionMode::Point
        ),
        vec![PieceId(31)]
    );
    for (x, y) in [(52., 18.), (54., 18.), (53., 17.), (53., 19.)] {
        assert!(gpu_result(
            &mut app,
            Rect::new(x, y, x + 1., y + 1.),
            SelectionMode::Point
        )
        .is_empty());
    }
    assert_eq!(
        gpu_result(&mut app, rect, SelectionMode::Rectangle),
        vec![PieceId(0), PieceId(9999)]
    );
    assert_eq!(
        gpu_result(&mut app, point, SelectionMode::Point),
        vec![PieceId(9999)]
    );
    for (min, max) in [
        (rect.max, rect.min),
        (Vec2::new(31., 33.), Vec2::new(33., 31.)),
        (Vec2::new(33., 31.), Vec2::new(31., 33.)),
    ] {
        assert_eq!(
            gpu_result(&mut app, Rect { min, max }, SelectionMode::Rectangle),
            vec![PieceId(0), PieceId(9999)]
        );
    }
    app.world_mut().entity_mut(b).insert(Visibility::Hidden);
    assert_eq!(
        gpu_result(
            &mut app,
            Rect::new(47., 32., 48., 33.),
            SelectionMode::Rectangle
        ),
        vec![PieceId(0)]
    );
    assert_eq!(
        gpu_result(&mut app, point, SelectionMode::Point),
        vec![PieceId(0)]
    );
    let mut split_alpha = Image::new(
        Extent3d {
            width: 2,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![255, 255, 255, 255, 255, 255, 255, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    split_alpha.sampler = bevy::image::ImageSampler::nearest();
    let split_alpha = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(split_alpha);
    let split_material =
        app.world_mut()
            .resource_mut::<Assets<ColorMaterial>>()
            .add(ColorMaterial {
                texture: Some(split_alpha),
                ..default()
            });
    app.world_mut()
        .entity_mut(b)
        .insert((Visibility::Visible, MeshMaterial2d(split_material.clone())));
    assert_eq!(
        gpu_result(
            &mut app,
            Rect::new(24., 32., 25., 33.),
            SelectionMode::Point
        ),
        vec![PieceId(9999)]
    );
    assert_eq!(
        gpu_result(
            &mut app,
            Rect::new(40., 32., 41., 33.),
            SelectionMode::Point
        ),
        vec![PieceId(0)]
    );
    assert_eq!(
        gpu_result(
            &mut app,
            Rect::new(40., 32., 41., 33.),
            SelectionMode::Rectangle
        ),
        vec![PieceId(0)]
    );
    app.world_mut()
        .resource_mut::<Assets<ColorMaterial>>()
        .get_mut(&split_material)
        .unwrap()
        .uv_transform = bevy::math::Affine2::from_translation(Vec2::new(0.5, 0.));
    assert_eq!(
        gpu_result(
            &mut app,
            Rect::new(24., 32., 25., 33.),
            SelectionMode::Point
        ),
        vec![PieceId(0)]
    );
    app.world_mut().entity_mut(b).insert(Visibility::Hidden);
    // Concave/empty mesh regions cannot be filled by its bounding box.
    app.world_mut().entity_mut(a).insert(Visibility::Hidden);
    let mut triangle = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    triangle.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![[-16., -16., 0.], [16., -16., 0.], [-16., 16., 0.]],
    );
    triangle.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0., 0.]; 3]);
    triangle.insert_indices(Indices::U32(vec![0, 1, 2]));
    let c = spawn_fixture(&mut app, 32, triangle, material.clone(), Vec3::ZERO);
    assert!(gpu_result(
        &mut app,
        Rect::new(45., 17., 46., 18.),
        SelectionMode::Rectangle
    )
    .is_empty());
    assert_eq!(
        gpu_result(
            &mut app,
            Rect::new(18., 44., 19., 45.),
            SelectionMode::Rectangle
        ),
        vec![PieceId(32)]
    );
    app.world_mut().entity_mut(c).insert(Visibility::Hidden);
    app.world_mut().entity_mut(a).insert(Visibility::Visible);
    // Transparent front piece exposes the back piece for clicks.
    let image = Image::new_fill(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[255, 255, 255, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    let transparent = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    let mask = app
        .world_mut()
        .resource_mut::<Assets<ColorMaterial>>()
        .add(ColorMaterial {
            texture: Some(transparent),
            ..default()
        });
    app.world_mut()
        .entity_mut(b)
        .insert((Visibility::Visible, MeshMaterial2d(mask.clone())));
    assert_eq!(
        gpu_result(&mut app, rect, SelectionMode::Rectangle),
        vec![PieceId(0)]
    );
    assert_eq!(
        gpu_result(&mut app, point, SelectionMode::Point),
        vec![PieceId(0)]
    );
    app.world_mut()
        .resource_mut::<Assets<ColorMaterial>>()
        .get_mut(&mask)
        .unwrap()
        .alpha_mode = AlphaMode2d::Mask(0.5);
    assert_eq!(
        gpu_result(&mut app, point, SelectionMode::Point),
        vec![PieceId(0)]
    );
    // Same projection/transform after translation, zoom and viewport offset.
    app.world_mut().entity_mut(b).insert(Visibility::Hidden);
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation
        .x = 40.;
    assert!(gpu_result(&mut app, point, SelectionMode::Point).is_empty());
    assert_eq!(
        gpu_result(
            &mut app,
            Rect::new(0., 31., 1., 32.),
            SelectionMode::Rectangle
        ),
        vec![PieceId(0)]
    );
    app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(2., 2., 1.);
    assert_eq!(
        gpu_result(
            &mut app,
            Rect::new(12., 31., 13., 32.),
            SelectionMode::Point
        ),
        vec![PieceId(0)]
    );
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation
        .x = 0.;
    app.world_mut().get_mut::<Camera>(camera).unwrap().viewport = Some(Viewport {
        physical_position: UVec2::new(8, 16),
        physical_size: UVec2::splat(32),
        ..default()
    });
    assert_eq!(
        gpu_result(
            &mut app,
            Rect::new(24., 32., 25., 33.),
            SelectionMode::Point
        ),
        vec![PieceId(0)]
    );
    assert!(gpu_result(
        &mut app,
        Rect::new(-100., -100., -10., -10.),
        SelectionMode::Rectangle
    )
    .is_empty());
    // Picking uses the very same combined vertex/index buffers as normal batching.
    app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::ONE;
    app.world_mut().get_mut::<Camera>(camera).unwrap().viewport = None;
    app.world_mut().entity_mut(a).insert(Visibility::Hidden);
    let inputs: Vec<_> = [a, b]
        .iter()
        .enumerate()
        .map(|(rank, &entity)| {
            let handle = &app.world().get::<Mesh2d>(entity).unwrap().0;
            (
                app.world().resource::<Assets<Mesh>>().get(handle).unwrap(),
                Transform::from_xyz(0., 0., rank as f32),
            )
        })
        .collect();
    let combined = crate::systems::batching::combine_meshes(
        inputs,
        &crate::resources::PerformanceDebugLevel::Off,
    )
    .unwrap();
    let combined = app.world_mut().resource_mut::<Assets<Mesh>>().add(combined);
    app.world_mut().spawn((
        Mesh2d(combined),
        MeshMaterial2d(material.clone()),
        Transform::default(),
        BatchedMeshEntity {
            piece_count: 2,
            last_updated: std::time::Instant::now(),
        },
    ));
    assert_eq!(
        gpu_result(&mut app, rect, SelectionMode::Rectangle),
        vec![PieceId(0), PieceId(9999)]
    );
    assert_eq!(
        gpu_result(&mut app, point, SelectionMode::Point),
        vec![PieceId(9999)]
    );
    // Both IDs share a single draw. The fragment mask must reject only the
    // unselectable front ID, rather than excluding the entire batch.
    for held in [false, true] {
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            let state = &mut store.pieces.get_mut(&PieceId(9999)).unwrap().state;
            state.placed = !held;
            state.held_by = held.then_some(puzzella_core::PlayerId(1));
        }
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert_eq!(gpu_result(&mut app, rect, mode), vec![PieceId(0)]);
        }
    }
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .pieces
        .get_mut(&PieceId(0))
        .unwrap()
        .state
        .placed = true;
    for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
        assert!(gpu_result(&mut app, rect, mode).is_empty());
    }
    for id in [0, 9999] {
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .pieces
            .get_mut(&PieceId(id))
            .unwrap()
            .state = PieceState::new(Vec2::ZERO);
    }
    assert_eq!(
        gpu_result(&mut app, point, SelectionMode::Point),
        vec![PieceId(9999)]
    );
    // Idle frames reuse buffers and do not re-submit the same request.
    let render = app.sub_app(RenderApp);
    let gpu = render.world().resource::<GpuSelection>();
    assert!(gpu.slots.len() <= 3);
    assert!(gpu
        .slots
        .iter()
        .all(|s| s.bitset.size() >= 1252 && s.selectable.size() >= 1252));
    let buffers: Vec<_> = gpu
        .slots
        .iter()
        .map(|s| (s.bitset.id(), s.selectable.id(), s.staging.id()))
        .collect();
    let submitted = gpu.last_submitted;
    for _ in 0..8 {
        app.update();
    }
    let gpu = app.sub_app(RenderApp).world().resource::<GpuSelection>();
    assert_eq!(gpu.last_submitted, submitted);
    assert_eq!(
        gpu.slots
            .iter()
            .map(|s| (s.bitset.id(), s.selectable.id(), s.staging.id()))
            .collect::<Vec<_>>(),
        buffers
    );
    // Resizing the normal target to 4K must neither resize/recreate point
    // attachments nor allocate/resize the rectangle attachment on a click.
    let large_target =
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::new_target_texture(
                3840,
                2160,
                TextureFormat::Rgba8UnormSrgb,
                None,
            ));
    app.world_mut()
        .entity_mut(camera)
        .insert(RenderTarget::Image(large_target.into()));
    assert_eq!(
        gpu_result(
            &mut app,
            Rect::new(1941., 1066., 1942., 1067.),
            SelectionMode::Point
        ),
        vec![PieceId(31)]
    );
    assert!(gpu_result(
        &mut app,
        Rect::new(1940., 1066., 1941., 1067.),
        SelectionMode::Point
    )
    .is_empty());
    let gpu = app.sub_app(RenderApp).world().resource::<GpuSelection>();
    let points = gpu.point_targets.as_ref().unwrap();
    assert_eq!(points.id.size(), one_pixel);
    assert_eq!(points.depth_view.texture().size(), one_pixel);
    assert_eq!(points.id.id(), point_texture);
    assert_eq!(points.depth_view.id(), point_depth);
    assert_eq!(
        gpu.rectangle_target.as_ref().unwrap().size,
        UVec2::splat(64)
    );
    app.world_mut().get_mut::<Camera>(camera).unwrap().viewport = Some(Viewport {
        physical_position: UVec2::new(123, 77),
        physical_size: UVec2::new(3200, 1700),
        ..default()
    });
    assert_eq!(
        gpu_result(
            &mut app,
            Rect::new(1744., 913., 1745., 914.),
            SelectionMode::Point
        ),
        vec![PieceId(31)]
    );
    assert_eq!(
        gpu_result(
            &mut app,
            Rect::new(1723., 927., 1724., 928.),
            SelectionMode::Point
        ),
        vec![PieceId(9999)]
    );
    assert_eq!(
        gpu_result(
            &mut app,
            Rect::new(1723., 927., 1724., 928.),
            SelectionMode::Rectangle
        ),
        vec![PieceId(0), PieceId(9999)]
    );
    let gpu = app.sub_app(RenderApp).world().resource::<GpuSelection>();
    assert_eq!(
        gpu.rectangle_target.as_ref().unwrap().size,
        UVec2::new(3840, 2160)
    );
    assert_eq!(gpu.point_targets.as_ref().unwrap().id.id(), point_texture);
    // A permanently invalid mesh returns a terminal error, then a repaired mesh
    // can be picked by a new request without resetting the selection plugin.
    let invalid = spawn_fixture(
        &mut app,
        33,
        Rectangle::new(32., 32.).into(),
        material,
        Vec3::new(0., 0., 3.),
    );
    let invalid_mesh = app.world().get::<Mesh2d>(invalid).unwrap().0.clone();
    app.world_mut()
        .resource_mut::<Assets<Mesh>>()
        .get_mut(&invalid_mesh)
        .unwrap()
        .remove_attribute(ATTRIBUTE_PIECE_ID);
    let center = Rect::new(1723., 927., 1724., 928.);
    for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
        let result = gpu_selection_result(&mut app, center, mode);
        assert_eq!(
            result.error.as_deref(),
            Some("puzzle mesh missing ATTRIBUTE_PIECE_ID")
        );
        assert!(result.piece_ids.is_empty());
        assert_eq!(
            app.sub_app(RenderApp)
                .world()
                .resource::<GpuSelection>()
                .last_submitted,
            result.request_id
        );
        for _ in 0..8 {
            app.update();
        }
        assert!(app
            .world()
            .resource::<PuzzleSelection>()
            .completed
            .is_none());
    }
    {
        let mut meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
        let mut mesh = meshes.get_mut(&invalid_mesh).unwrap();
        let count = mesh.count_vertices();
        mesh.insert_attribute(ATTRIBUTE_PIECE_ID, vec![33u32; count]);
    }
    assert_eq!(
        gpu_result(&mut app, center, SelectionMode::Point),
        vec![PieceId(33)]
    );

    // Exercise the native generator's U16 mesh on the actual picking buffers:
    // the tab is selectable; the blank remains empty in point/rectangle modes.
    use puzzella_puzzle::shapes::{EdgeId, EdgeOrientation, EdgeProfile};
    let seed = (0..1000)
        .find(|&seed| {
            EdgeProfile::from_seed(
                seed,
                EdgeId {
                    orientation: EdgeOrientation::Vertical,
                    x: 1,
                    y: 0,
                },
            )
            .polarity
                > 0.0
                && EdgeProfile::from_seed(
                    seed,
                    EdgeId {
                        orientation: EdgeOrientation::Horizontal,
                        x: 0,
                        y: 1,
                    },
                )
                .polarity
                    > 0.0
        })
        .unwrap();
    let definition = puzzella_core::PuzzleDefinition {
        generator_version: puzzella_core::GENERATOR_VERSION,
        seed,
        grid_size: UVec2::splat(2),
        image_size: UVec2::splat(100),
        snap_distance: 10.0,
    };
    let data = puzzella_puzzle::TessellationWorker::default()
        .generate_piece(&definition, PieceId(0), Vec2::ZERO)
        .unwrap();
    let mesh = data.geometry.fill.into_mesh();
    assert!(matches!(mesh.indices(), Some(Indices::U16(_))));
    let target = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_target_texture(
            128,
            128,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
    app.world_mut()
        .entity_mut(camera)
        .insert((RenderTarget::Image(target.into()), Transform::default()));
    app.world_mut().get_mut::<Camera>(camera).unwrap().viewport = None;
    let material = app
        .world_mut()
        .resource_mut::<Assets<ColorMaterial>>()
        .add(ColorMaterial::default());
    spawn_fixture(&mut app, 65, mesh, material, Vec3::new(0.0, 0.0, 4.0));
    let right = EdgeProfile::from_seed(
        seed,
        EdgeId {
            orientation: EdgeOrientation::Vertical,
            x: 1,
            y: 0,
        },
    );
    let bottom = EdgeProfile::from_seed(
        seed,
        EdgeId {
            orientation: EdgeOrientation::Horizontal,
            x: 0,
            y: 1,
        },
    );
    let tab = Vec2::new(25.0 + right.depth * 50.0 * 0.7, 25.0 - right.center * 50.0);
    let blank = Vec2::new(
        bottom.center * 50.0 - 25.0,
        -25.0 + bottom.depth * 50.0 * 0.5,
    );
    for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
        let pixel = Vec2::new(64.0 + tab.x, 64.0 - tab.y);
        assert_eq!(
            gpu_result(
                &mut app,
                Rect {
                    min: pixel,
                    max: pixel + Vec2::ONE
                },
                mode
            ),
            vec![PieceId(65)]
        );
        let pixel = Vec2::new(64.0 + blank.x, 64.0 - blank.y);
        assert!(!gpu_result(
            &mut app,
            Rect {
                min: pixel,
                max: pixel + Vec2::ONE
            },
            mode
        )
        .contains(&PieceId(65)));
    }
}
