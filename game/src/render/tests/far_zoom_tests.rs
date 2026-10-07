use super::*;

#[test]
fn far_zoom_threshold_uses_the_short_physical_side_in_both_directions() {
    let viewport = URect::new(137, 59, 329, 187);
    let mut config = PuzzleUniform {
        view_min: Vec2::new(-192.0, -128.0),
        view_max: Vec2::new(192.0, 128.0),
        count: 1_000_000,
        ..default()
    };
    for short in [0.25, 1.499, 1.5, 1.501, 3.0, 1.5, 1.499, 0.25] {
        for size in [Vec2::new(short, 8.0), Vec2::new(8.0, short)] {
            config.size = size * 2.0;
            config.configure_screen_space(viewport);
            assert_eq!(config.piece_size_px, size);
            assert_eq!(config.pixel_world_size, Vec2::splat(2.0));
            assert_eq!(config.far_zoom, u32::from(short < 1.5));
            assert_eq!(config.splat_min_px, 1.0);
            assert_eq!(config.viewport_size, Vec2::new(192.0, 128.0));
            assert_eq!(config.viewport_origin, Vec2::new(137.0, 59.0));
            assert_eq!(config.render_clip_scale, Vec2::ONE);
            assert_eq!(config.render_clip_offset, Vec2::ZERO);
        }
    }
    // Doubling physical resolution (e.g. DPI) changes the mode, not world size.
    config.size = Vec2::splat(2.0);
    config.configure_screen_space(viewport);
    assert_eq!(config.far_zoom, 1);
    config.configure_screen_space(URect::from_corners(
        viewport.min,
        viewport.min + viewport.size() * 2,
    ));
    assert_eq!(config.far_zoom, 0);
    assert_eq!(PuzzleUniform::min_size().get() % 16, 0);
}

fn main_config(app: &App) -> PuzzleUniform {
    app.sub_app(RenderApp)
        .world()
        .resource::<ExtractedPuzzle>()
        .config
        .clone()
}

fn world_at_pixel(config: &PuzzleUniform, pixel: Vec2) -> Vec2 {
    let uv = (pixel - config.viewport_origin) / config.viewport_size;
    config
        .clip_from_world
        .inverse()
        .project_point3(Vec3::new(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0))
        .truncate()
}

fn projected_pixel(config: &PuzzleUniform, position: Vec2) -> Vec2 {
    let ndc = config.clip_from_world.project_point3(position.extend(0.0));
    config.viewport_origin
        + (Vec2::new(ndc.x, -ndc.y) * 0.5 + Vec2::splat(0.5)) * config.viewport_size
}

fn pixel_rect(pixel: UVec2) -> Rect {
    Rect::from_corners(pixel.as_vec2(), pixel.as_vec2() + Vec2::ONE)
}

fn drawn_mask(pixels: &[u8]) -> Vec<bool> {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[..3] != [0, 0, 0])
        .collect()
}

fn assert_mask_matches(actual: &[bool], expected: &[bool]) {
    assert_eq!(actual.len(), expected.len());
    let mismatches: Vec<_> = actual
        .iter()
        .zip(expected)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .take(12)
        .map(|(index, _)| index)
        .collect();
    assert!(
        mismatches.is_empty(),
        "coverage differs at pixels {mismatches:?}"
    );
}

fn single_splat_pixels(
    config: &PuzzleUniform,
    positions: &[Vec2],
    pixels: &[u8],
    resolution: usize,
) -> Vec<UVec2> {
    let mask = drawn_mask(pixels);
    assert_eq!(
        mask.iter().filter(|&&p| p).count(),
        positions.len(),
        "each visible subpixel piece must retain a raster sample"
    );
    positions
        .iter()
        .enumerate()
        .map(|(id, &position)| {
            let center = projected_pixel(config, position);
            let expected = center.floor().as_ivec2();
            let mut hits = vec![];
            for y in -1..=1 {
                for x in -1..=1 {
                    let pixel = expected + IVec2::new(x, y);
                    if pixel.cmpge(IVec2::ZERO).all()
                        && pixel.cmplt(IVec2::splat(resolution as i32)).all()
                        && mask[pixel.y as usize * resolution + pixel.x as usize]
                    {
                        hits.push(pixel.as_uvec2());
                    }
                }
            }
            assert_eq!(
                hits.len(),
                1,
                "piece {id} at {center} must cover exactly one pixel"
            );
            let pixel = hits[0];
            // CPU/GPU projection rounding may choose either side of an integer
            // boundary. Check coverage and pick the actual rendered sample there.
            for axis in 0..2 {
                if pixel[axis] as i32 != expected[axis] {
                    assert!(
                        (center[axis] - center[axis].round()).abs() < 1e-4,
                        "piece {id} snapped away from its pixel: {center}, {pixel}"
                    );
                }
            }
            pixel
        })
        .collect()
}

fn splat_mask(config: &PuzzleUniform, positions: &[Vec2], resolution: usize) -> Vec<bool> {
    let mut mask = vec![false; resolution * resolution];
    for &position in positions {
        let pixel = projected_pixel(config, position).floor().as_ivec2();
        if pixel.cmpge(IVec2::ZERO).all() && pixel.cmplt(IVec2::splat(resolution as i32)).all() {
            mask[pixel.y as usize * resolution + pixel.x as usize] = true;
        }
    }
    mask
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_far_zoom_subpixel_phases_pan_and_picking_share_coverage() {
    const RESOLUTION: usize = 128;
    let (mut app, camera, target) = gpu_app(RESOLUTION as u32);
    // Exercise an offset, non-square viewport as well as a full viewport.
    for viewport in [
        None,
        Some(bevy::camera::Viewport {
            physical_position: UVec2::new(8, 16),
            physical_size: UVec2::new(96, 80),
            ..default()
        }),
    ] {
        app.world_mut().get_mut::<Camera>(camera).unwrap().viewport = viewport;
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation = Vec3::ZERO;
        app.world_mut()
            .insert_resource(definition(UVec2::splat(8), 2, 42));
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .initialize(vec![Vec2::ZERO; 64]);
        wait_ready(&mut app);
        update_gpu(&mut app);
        let config = main_config(&app);
        assert!((config.pixel_world_size - Vec2::ONE).abs().max_element() < 1e-5);
        assert!(
            (config.piece_size_px - Vec2::splat(0.25))
                .abs()
                .max_element()
                < 1e-5
        );
        let positions: Vec<_> = (0..64)
            .map(|id| {
                let x = id % 8;
                let y = id / 8;
                world_at_pixel(
                    &config,
                    Vec2::new(30.0 + x as f32 * 6.125, 30.0 + y as f32 * 6.125),
                )
            })
            .collect();
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            for (id, &position) in positions.iter().enumerate() {
                store.states[id].position = position;
                store.dirty_pieces.insert(PieceId(id as u32));
            }
        }
        for phase in 0..8 {
            let pan = phase as f32 * 0.125;
            app.world_mut()
                .get_mut::<Transform>(camera)
                .unwrap()
                .translation = Vec3::new(pan, -pan, 0.0);
            update_gpu(&mut app);
            update_gpu(&mut app);
            let config = main_config(&app);
            assert_eq!(config.far_zoom, 1);
            let pixels = rendered_pixels(&mut app, target.clone());
            let splats = single_splat_pixels(&config, &positions, &pixels, RESOLUTION);
            let mut visible = visible_ids(&app);
            visible.sort_unstable();
            assert_eq!(visible, (0..64).collect::<Vec<_>>());
            // At phase zero check all 64 X/Y phase combinations individually.
            // Subsequent pans check a different member in each lattice row.
            for id in (0..64).filter(|id| phase == 0 || id % 8 == phase) {
                let pixel = splats[id];
                assert_eq!(
                    pick(&mut app, pixel_rect(pixel), SelectionMode::Point),
                    vec![PieceId(id as u32)],
                    "point phase {phase}, ID {id}"
                );
                if phase == 0 {
                    assert_eq!(
                        pick(&mut app, pixel_rect(pixel), SelectionMode::Rectangle),
                        vec![PieceId(id as u32)]
                    );
                }
                // A neighboring empty pixel must stay unpickable with a 1x1 target.
                assert!(
                    pick(&mut app, pixel_rect(pixel + UVec2::X), SelectionMode::Point).is_empty()
                );
            }
            assert_eq!(
                pick(
                    &mut app,
                    Rect::new(0.0, 0.0, 128.0, 128.0),
                    SelectionMode::Rectangle
                ),
                (0..64).map(PieceId).collect::<Vec<_>>()
            );
            let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
            assert_eq!(gpu.upload_bytes, 0);
            assert_eq!(gpu.root_upload_bytes, 0);
            assert_eq!(gpu.drag_upload_bytes, 0);
            assert_eq!(gpu.point.as_ref().unwrap().id.size().width, 1);
        }
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_far_zoom_threshold_preserves_exact_sdf_coverage() {
    let (mut app, camera, target) = gpu_app(128);
    let def = definition(UVec2::splat(2), 6, 42);
    app.world_mut().insert_resource(def.clone());
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::new(0.25, -0.125); 4]);
    for state in app
        .world_mut()
        .resource_mut::<PieceDataStore>()
        .states
        .iter_mut()
        .skip(1)
    {
        state.flags = 0;
    }
    for short in [1.499, 1.5, 1.501, 3.0, 1.501, 1.5, 1.499] {
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale =
            Vec3::new(3.0 / short, 3.0 / short, 1.0);
        wait_ready(&mut app);
        update_gpu(&mut app);
        update_gpu(&mut app);
        let config = main_config(&app);
        assert_eq!(
            config.far_zoom,
            u32::from(short < 1.5),
            "short side {short}, projected {:?}",
            config.piece_size_px
        );
        let pixels = rendered_pixels(&mut app, target.clone());
        let position = app.world().resource::<PieceDataStore>().states[0].position;
        let expected = if short < 1.5 {
            splat_mask(&config, &[position], 128)
        } else {
            let profiles = piece_profiles(def.seed, def.grid_size, UVec2::ZERO);
            (0..128 * 128)
                .map(|pixel| {
                    let sample = Vec2::new((pixel % 128) as f32 + 0.5, (pixel / 128) as f32 + 0.5);
                    piece_signed_distance(
                        world_at_pixel(&config, sample) - position,
                        config.size,
                        profiles,
                    ) <= 0.0
                })
                .collect()
        };
        assert!(expected.iter().any(|&p| p));
        assert_mask_matches(&drawn_mask(&pixels), &expected);
        for (index, &drawn) in expected.iter().enumerate().filter(|(i, drawn)| {
            **drawn || (62..67).contains(&(i % 128)) && (62..67).contains(&(i / 128))
        }) {
            let rect = pixel_rect(UVec2::new((index % 128) as u32, (index / 128) as u32));
            for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
                assert_eq!(
                    pick(&mut app, rect, mode),
                    if drawn { vec![PieceId(0)] } else { vec![] },
                    "threshold {short}, pixel {index}, mode {mode:?}"
                );
            }
        }
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_far_zoom_alpha_tints_and_viewport_edges() {
    let (mut app, _, target) = gpu_app(128);
    app.world_mut()
        .insert_resource(definition(UVec2::splat(2), 1, 42));
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO; 4]);
    let image = Image::new(
        Extent3d {
            width: 2,
            height: 2,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![
            255, 255, 255, 255, 255, 255, 255, 0, 255, 255, 255, 128, 255, 255, 255, 255,
        ],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::all(),
    );
    let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().resource_mut::<PuzzleImage>().handle = handle;
    app.world_mut().resource_mut::<PuzzleImage>().opaque = false;
    wait_ready(&mut app);
    update_gpu(&mut app);
    let config = main_config(&app);
    // Pixel centers are deliberately far from the original tiny geometry.
    let centers = [
        Vec2::new(0.875, 0.875),
        Vec2::new(127.875, 0.875),
        Vec2::new(0.875, 127.875),
        Vec2::splat(127.875),
    ];
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        for (id, &center) in centers.iter().enumerate() {
            store.states[id].position = world_at_pixel(&config, center);
            store.states[id].flags |= crate::resources::pieces::CONNECTED_EDGES;
            store.dirty_pieces.insert(PieceId(id as u32));
        }
    }
    update_gpu(&mut app);
    let normal = rendered_pixels(&mut app, target.clone());
    assert_eq!(visible_ids(&app).len(), 4);
    for (id, center) in centers.iter().enumerate() {
        let pixel = center.floor().as_uvec2();
        let offset = (pixel.y * 128 + pixel.x) as usize * 4;
        assert_eq!(normal[offset] > 0, id != 1);
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert_eq!(
                pick(&mut app, pixel_rect(pixel), mode),
                if id == 1 {
                    vec![]
                } else {
                    vec![PieceId(id as u32)]
                }
            );
        }
    }
    assert_eq!(
        pick(
            &mut app,
            Rect::new(0.0, 0.0, 128.0, 128.0),
            SelectionMode::Rectangle
        ),
        vec![PieceId(0), PieceId(2), PieceId(3)]
    );
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .selected_pieces
        .insert(PieceId(3));
    app.world_mut()
        .resource_mut::<PuzzleSelection>()
        .request_preview(Rect::new(0.0, 0.0, 128.0, 128.0));
    update_gpu(&mut app);
    let tinted = rendered_pixels(&mut app, target);
    assert_eq!(drawn_mask(&normal), drawn_mask(&tinted));
    for id in [0, 2, 3] {
        let pixel = centers[id].floor().as_uvec2();
        let offset = (pixel.y * 128 + pixel.x) as usize * 4;
        let rgb = &tinted[offset..offset + 3];
        if id == 3 {
            assert!(
                rgb[0] >= rgb[1] && rgb[1] > rgb[2],
                "selected yellow wins preview: {rgb:?}"
            );
        } else {
            assert!(rgb[2] > rgb[1] && rgb[1] > rgb[0], "preview blue: {rgb:?}");
        }
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_far_zoom_anisotropic_splats_culling_and_drag() {
    use crate::resources::pieces::{DragTransform, HELD};
    let (mut app, _, target) = gpu_app(128);
    let mut def = definition(UVec2::splat(2), 5, 42);
    def.image_size.y = 1;
    app.world_mut().insert_resource(def);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO; 4]);
    wait_ready(&mut app);
    update_gpu(&mut app);
    let config = main_config(&app);
    let mut positions: Vec<_> = [
        Vec2::new(-0.125, 64.875),
        Vec2::new(128.125, 32.875),
        Vec2::new(40.875, 96.875),
        Vec2::new(80.875, 16.875),
    ]
    .into_iter()
    .map(|p| world_at_pixel(&config, p))
    .collect();
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        for (id, &position) in positions.iter().enumerate() {
            store.states[id].position = position;
            store.dirty_pieces.insert(PieceId(id as u32));
        }
    }
    // A 2.5 x 0.5 piece must keep its long axis and expand only the short axis.
    let expected_pixels = |config: &PuzzleUniform, positions: &[Vec2]| {
        positions
            .iter()
            .enumerate()
            .flat_map(|(id, &position)| {
                let center = projected_pixel(config, position).floor().as_ivec2();
                (-1..=1).filter_map(move |x| {
                    let pixel = center + IVec2::new(x, 0);
                    (pixel.cmpge(IVec2::ZERO).all() && pixel.cmplt(IVec2::splat(128)).all())
                        .then_some((pixel.as_uvec2(), PieceId(id as u32)))
                })
            })
            .collect::<Vec<_>>()
    };
    update_gpu(&mut app);
    let pixels = rendered_pixels(&mut app, target.clone());
    let expected = expected_pixels(&main_config(&app), &positions);
    assert_eq!(expected.len(), 8);
    assert_eq!(
        drawn_mask(&pixels).iter().filter(|&&p| p).count(),
        expected.len()
    );
    assert_eq!(
        visible_ids(&app).len(),
        4,
        "centers outside the viewport still have visible splats"
    );
    for (pixel, id) in expected {
        assert!(pixels[(pixel.y * 128 + pixel.x) as usize * 4] > 0);
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert_eq!(pick(&mut app, pixel_rect(pixel), mode), vec![id]);
        }
    }
    let base = positions[2];
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.states[2].flags = ENABLED | HELD;
        store.dirty_pieces.insert(PieceId(2));
        store.drag = DragTransform {
            members: Arc::from([1u32 << 2]),
            delta: Vec2::ZERO,
        };
    }
    for phase in 0..8 {
        let delta = Vec2::new(10.3125 + phase as f32 * 0.125, -0.3125);
        app.world_mut().resource_mut::<PieceDataStore>().drag.delta = delta;
        positions[2] = base + delta;
        update_gpu(&mut app);
        let pixels = rendered_pixels(&mut app, target.clone());
        let config = main_config(&app);
        let expected = expected_pixels(&config, &positions);
        assert_eq!(
            drawn_mask(&pixels).iter().filter(|&&p| p).count(),
            expected.len()
        );
        for (pixel, id) in expected {
            assert!(pixels[(pixel.y * 128 + pixel.x) as usize * 4] > 0);
            assert_eq!(
                pick(&mut app, pixel_rect(pixel), SelectionMode::Point),
                if id == PieceId(2) { vec![] } else { vec![id] }
            );
        }
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.upload_bytes, 0);
        assert_eq!(gpu.drag_upload_bytes, 0);
        assert_eq!(
            app.world().resource::<PieceDataStore>().states[2].position,
            base
        );
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_far_zoom_placed_depth_is_independent_of_visible_order() {
    use crate::resources::pieces::PLACED;
    const COUNT: u32 = 1_000_000;
    let ids = [0, 1, COUNT - 2, COUNT - 1];
    let colors = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 0, 255],
    ];
    let orders = [
        [0, 1, 2, 3],
        [3, 2, 1, 0],
        [1, 0, 3, 2],
        [2, 3, 0, 1],
        [1, 3, 2, 0],
        [3, 1, 0, 2],
    ];
    let (mut app, camera, target) = gpu_app(128);

    // Exercise the production opaque draw with deliberate permutations of the
    // culling output. Natural atomic append order may appear stable on one GPU.
    #[derive(Resource, Default)]
    struct ForcedOrder(u32);
    fn extract_order(mut frame: ResMut<ExtractedPuzzle>, order: Extract<Res<ForcedOrder>>) {
        frame.config.reserved = order.0;
    }
    let original = include_str!("../visibility.wgsl");
    let append = "let dst=atomicAdd(&args.instance_count,1u);visible[dst]=id;";
    assert!(original.contains(append));
    let forced = original.replace(
        append,
        "
        atomicAdd(&args.instance_count,1u);
        let candidate=select(id,id-(config.count-4u),id>=config.count-2u);
        let dst=(config.reserved>>(candidate*2u))&3u;
        visible[dst]=id;",
    );
    let shader = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(Shader::from_wgsl(forced, "placed_test_visibility.wgsl"));
    app.insert_resource(ForcedOrder::default());
    app.sub_app_mut(RenderApp)
        .world_mut()
        .resource_mut::<GpuRenderer>()
        .compute_shader = shader;
    app.sub_app_mut(RenderApp)
        .add_systems(ExtractSchedule, extract_order.after(extract_puzzle));

    let def = definition(UVec2::splat(1000), 1000, 42);
    app.world_mut().insert_resource(def.clone());
    let mut rgba = [0, 0, 0, 255].repeat(COUNT as usize);
    for (&id, color) in ids.iter().zip(colors) {
        rgba[id as usize * 4..id as usize * 4 + 4].copy_from_slice(&color);
    }
    let image = Image::new(
        Extent3d {
            width: 1000,
            height: 1000,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::all(),
    );
    let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().resource_mut::<PuzzleImage>().handle = handle;
    assert!(app.world().resource::<PuzzleImage>().opaque);
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.initialize(vec![Vec2::ZERO; COUNT as usize]);
        for state in store.states.iter_mut() {
            state.flags = 0;
        }
        for id in ids {
            store.states[id as usize].position = def.correct_position(PieceId(id));
            store.states[id as usize].flags = ENABLED | PLACED;
            store.states[id as usize].z_order = 0;
        }
    }
    {
        let mut transform = app.world_mut().get_mut::<Transform>(camera).unwrap();
        transform.scale = Vec3::new(2000.0, 2000.0, 1.0);
        transform.translation = Vec3::new(-1000.0, 1000.0, 0.0);
    }
    wait_ready(&mut app);
    update_gpu(&mut app);
    let config = main_config(&app);
    assert_eq!(config.count, COUNT);
    assert_eq!(config.far_zoom, 1);
    let pixel = UVec2::splat(64);
    for id in ids {
        assert_eq!(
            projected_pixel(&config, def.correct_position(PieceId(id)))
                .floor()
                .as_uvec2(),
            pixel
        );
    }
    let offset = (pixel.y * 128 + pixel.x) as usize * 4;
    // Check repeated frames and the smallest loose rank against the largest
    // placed IDs. Their far depths must remain strictly below loose rank 2.
    for loose in [false, true] {
        if loose {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            store.states[0].flags = ENABLED;
            store.dirty_pieces.insert(PieceId(0));
        }
        let expected = colors[if loose { 0 } else { 3 }];
        for _ in 0..2 {
            for order in orders {
                app.world_mut().resource_mut::<ForcedOrder>().0 = order
                    .iter()
                    .enumerate()
                    .fold(0, |bits, (slot, &candidate)| {
                        bits | (slot as u32) << (candidate * 2)
                    });
                update_gpu(&mut app);
                let pixels = rendered_pixels(&mut app, target.clone());
                assert_eq!(
                    visible_ids(&app),
                    order.map(|candidate| ids[candidate as usize]),
                    "forced culling order"
                );
                assert_eq!(drawn_mask(&pixels).iter().filter(|&&p| p).count(), 1);
                for (&actual, expected) in pixels[offset..offset + 4].iter().zip(expected) {
                    assert!((i32::from(actual) - i32::from(expected)).abs() <= 2,
                        "placed splat color depends on visible order {order:?}, loose {loose}: {:?}", &pixels[offset..offset + 4]);
                }
            }
        }
        assert_eq!(
            pick(&mut app, pixel_rect(pixel), SelectionMode::Point),
            if loose { vec![PieceId(0)] } else { vec![] }
        );
        assert_eq!(
            pick(&mut app, pixel_rect(pixel), SelectionMode::Rectangle),
            if loose { vec![PieceId(0)] } else { vec![] }
        );
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_far_zoom_million_initial_lattice_has_no_missing_coverage() {
    const COUNT: u32 = 1_000_000;
    const RESOLUTION: usize = 512;
    let (mut app, camera, target) = gpu_app(RESOLUTION as u32);
    let def = definition(UVec2::splat(1000), 1000, 42);
    let states = crate::resources::DensePieceStates::generate(&def);
    let positions: Vec<_> = states.iter().map(|state| state.position).collect();
    let extent = positions.iter().fold(Vec2::ZERO, |a, p| a.max(p.abs()));
    let scale = extent.max_element() * 2.0 / RESOLUTION as f32 * 1.02;
    app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(scale, scale, 1.0);
    app.world_mut()
        .insert_resource(definition(UVec2::splat(1000), 1000, 42));
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize_dense(states);
    wait_ready(&mut app);
    for phase in 0..8 {
        let pan = phase as f32 * 0.125 * scale;
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation = Vec3::new(pan, -pan, 0.0);
        update_gpu(&mut app);
        update_gpu(&mut app);
        let config = main_config(&app);
        assert_eq!(config.far_zoom, 1);
        assert!(config.piece_size_px.max_element() < 1.0);
        let pixels = rendered_pixels(&mut app, target.clone());
        assert_mask_matches(
            &drawn_mask(&pixels),
            &splat_mask(&config, &positions, RESOLUTION),
        );
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        let buffers = gpu.buffers.as_ref().unwrap();
        let args = read_buffer(&app, &buffers.args, 16);
        assert_eq!(bytemuck::cast_slice::<u8, u32>(&args), &[4, COUNT, 0, 0]);
        assert_eq!(buffers.states.size(), u64::from(COUNT) * 16);
        assert_eq!(gpu.upload_bytes, 0);
        assert_eq!(gpu.root_upload_bytes, 0);
        assert_eq!(gpu.selection_upload_bytes, 0);
        assert_eq!(gpu.drag_upload_bytes, 0);
        assert_eq!(gpu.remote_mapping_upload_bytes, 0);
        assert_eq!(gpu.remote_delta_upload_bytes, 0);
        assert_eq!(gpu.rotation_upload_bytes, 0);
        assert_eq!(
            gpu.buffers.as_ref().unwrap().piece_metadata.size(),
            12_000_000
        );
        assert_eq!(gpu.buffers.as_ref().unwrap().remote_deltas.size(), 512);
    }
    assert_eq!(
        pick(
            &mut app,
            Rect::new(0.0, 0.0, 512.0, 512.0),
            SelectionMode::Rectangle
        )
        .len(),
        COUNT as usize
    );
    let config = main_config(&app);
    for id in [0, COUNT / 4, COUNT / 2, COUNT - 1] {
        let pixel = projected_pixel(&config, positions[id as usize])
            .floor()
            .as_uvec2();
        let hits = pick(&mut app, pixel_rect(pixel), SelectionMode::Point);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            projected_pixel(&config, positions[hits[0].0 as usize])
                .floor()
                .as_uvec2(),
            pixel
        );
    }
    assert_eq!(
        app.world_mut().query::<&Mesh2d>().iter(app.world()).count(),
        0
    );
}
