use super::shadow_tests::{
    assert_no_uploads, config, freeze_shadow_clock, red_source, render_frame, shadow_draws, turn,
    wait_red_frame, ShadowTime,
};
use super::*;
use crate::resources::rotation_visual::update_rotation_clock;

#[test]
fn side_lod_is_independent_and_visual_bounds_include_side_without_shadow() {
    for quality in [
        PieceVisualQuality::Low,
        PieceVisualQuality::Medium,
        PieceVisualQuality::High,
    ] {
        let visuals = quality.resolve();
        for size in [0.25, 9.99, 10.0, 12.0, 13.99, 14.0, 20.0, 21.99, 22.0, 40.0] {
            let mut config = PuzzleUniform {
                clip_from_world: Mat4::IDENTITY,
                viewport_size: Vec2::splat(128.0),
                piece_size_px: Vec2::new(size, 100.0),
                ..default()
            };
            config.configure_visuals(visuals);
            assert_eq!(
                config.side_enabled != 0,
                visuals.side_enabled && size >= visuals.side_min_piece_px
            );
            assert_eq!(
                config.shadow_enabled != 0,
                visuals.shadow_enabled && size >= visuals.shadow_min_piece_px
            );
            assert_eq!(
                config.visual_cull_extent == Vec2::ZERO,
                config.side_enabled == 0 && config.shadow_enabled == 0
            );
        }
        let mut side_only = visuals;
        side_only.shadow_enabled = false;
        let mut config = PuzzleUniform {
            clip_from_world: Mat4::from_rotation_z(0.4),
            viewport_size: Vec2::splat(128.0),
            piece_size_px: Vec2::splat(40.0),
            ..default()
        };
        config.configure_visuals(side_only);
        let frame = visuals.for_frame(config.piece_size_px, false);
        let expected = config
            .clip_from_world
            .inverse()
            .transform_vector3((frame.side_offset_px() / 128.0 * Vec2::new(2.0, -2.0)).extend(0.0))
            .truncate()
            .abs();
        assert!(config.visual_cull_extent.abs_diff_eq(expected, 1e-6));
        assert_eq!(
            frame.side_offset_px(),
            PSEUDO_3D_DIRECTION * frame.side_thickness_px
        );
    }
    let mut config = PuzzleUniform {
        piece_size_px: Vec2::splat(12.0),
        viewport_size: Vec2::splat(128.0),
        ..default()
    };
    config.configure_visuals(PieceVisualQuality::High.resolve());
    assert_eq!((config.shadow_enabled, config.side_enabled), (1, 0));
    assert_eq!(PuzzleUniform::min_size().get() % 16, 0);
}

#[test]
fn side_entrypoint_keeps_static_thickness_fast_paths_and_existing_bindings() {
    let shader = include_str!("../puzzle_render.wesl");
    assert!(shader.contains("return piece_vertex(vi,instance,false,false);"));
    assert!(shader.contains("return piece_vertex(vi,instance,false,true);"));
    assert!(shader.contains("if config.rotation_active!=0u {"));
    assert!(shader.contains("if animation_slot==0u {world_offset=rotate_quarter(local,quarter);}"));
    let side = shader
        .split("if side {")
        .nth(1)
        .unwrap()
        .split('}')
        .next()
        .unwrap();
    assert!(side.contains(
        "screen_offset(out.position,config.pseudo_3d_direction*config.side_thickness_px)"
    ));
    for forbidden in [
        "elevation",
        "sin(",
        "cos(",
        "sqrt(",
        "length(",
        "component_root",
        "rotation_slot",
    ] {
        assert!(!side.contains(forbidden));
    }
    let fragment = shader
        .split("@fragment fn side_fragment")
        .nth(1)
        .unwrap()
        .split("@fragment fn fragment")
        .next()
        .unwrap();
    assert!(fragment.contains("silhouette_source(in)"));
    assert!(fragment.contains("config.side_color.a*source.a"));
    assert!(!fragment.contains("selected["));
    assert!(!fragment.contains("connected"));
    assert_eq!(shader.matches("@binding(").count(), 13);
    let varying = shader
        .split("struct VertexOutput {")
        .nth(1)
        .unwrap()
        .split("};")
        .next()
        .unwrap();
    assert!(!varying.contains("side"));
    let pick = include_str!("../pick_visibility.wesl")
        .split("fn cull_pick")
        .nth(1)
        .unwrap();
    assert!(!pick.contains("side"));
    assert!(!pick.contains("visual_cull_extent"));
}

pub(super) fn side_draws(app: &App) -> usize {
    app.sub_app(RenderApp)
        .world()
        .resource::<GpuRenderer>()
        .side_draws
}

fn fixture(size: u32, synchronous: bool) -> (App, Entity, Handle<Image>) {
    let (mut app, camera, target) = gpu_app_with_pipeline_compilation(128, synchronous);
    // Keep the existing side fixtures independent of top surface lighting.
    app.sub_app_mut(RenderApp).add_systems(
        ExtractSchedule,
        (|mut frame: ResMut<ExtractedPuzzle>| frame.config.bevel_enabled = 0)
            .after(extract_puzzle)
            .after(override_visuals),
    );
    app.insert_resource(ClearColor(Color::WHITE));
    red_source(&mut app, 255);
    app.insert_resource(definition(UVec2::ONE, size, 42));
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO]);
    wait_ready(&mut app);
    if !synchronous {
        wait_red_frame(&mut app, target.clone());
    }
    (app, camera, target)
}

#[derive(Resource)]
struct VisualOverride(ResolvedPieceVisuals);
fn override_visuals(mut frame: ResMut<ExtractedPuzzle>, visuals: Option<Res<VisualOverride>>) {
    if let Some(visuals) = visuals {
        frame.config.configure_visuals(visuals.0);
    }
}
pub(super) fn set_visuals(app: &mut App, visuals: ResolvedPieceVisuals) {
    if !app
        .sub_app(RenderApp)
        .world()
        .contains_resource::<VisualOverride>()
    {
        app.sub_app_mut(RenderApp)
            .add_systems(ExtractSchedule, override_visuals.after(extract_puzzle));
    }
    app.sub_app_mut(RenderApp)
        .insert_resource(VisualOverride(visuals));
}
pub(super) fn clear_visuals(app: &mut App) {
    app.sub_app_mut(RenderApp)
        .world_mut()
        .remove_resource::<VisualOverride>();
}

fn is_top(pixel: &[u8]) -> bool {
    pixel[..3] == [255, 0, 0]
}
fn is_side(pixel: &[u8]) -> bool {
    pixel[0] > 0 && pixel[0] < 150 && pixel[0] == pixel[1] && pixel[1] == pixel[2]
}
fn mask(pixels: &[u8], predicate: fn(&[u8]) -> bool) -> Vec<bool> {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| predicate(p))
        .collect()
}
pub(super) fn preview_if_requested(name: &str, pixels: &[u8]) {
    if let Some(directory) = std::env::var_os("JIGSALL_VISUAL_PREVIEW_DIR") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        image::save_buffer_with_format(
            directory.join(name),
            pixels,
            128,
            128,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .unwrap();
    }
}
fn shifted_top(app: &mut App, camera: Entity, target: &Handle<Image>, offset: Vec2) -> Vec<bool> {
    let config = config(app);
    let world_offset = config
        .clip_from_world
        .inverse()
        .transform_vector3((offset / config.viewport_size * Vec2::new(2.0, -2.0)).extend(0.0));
    let transform = *app.world().get::<Transform>(camera).unwrap();
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation -= world_offset;
    let pixels = render_frame(app, target.clone());
    *app.world_mut().get_mut::<Transform>(camera).unwrap() = transform;
    mask(&pixels, is_top)
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_side_static_quality_lod_ordering_flat_output_and_top_only_picking() {
    let (mut app, camera, target) = fixture(40, true);
    // Separate the smaller side/shadow minima on the hard pixel grid.
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation = Vec3::new(-0.45, 0.45, 0.0);
    let flat = render_frame(&mut app, target.clone());
    assert_eq!(side_draws(&app), 0);
    for quality in [PieceVisualQuality::Medium, PieceVisualQuality::High] {
        app.insert_resource(quality);
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(8.0, 8.0, 1.0);
        let lod = render_frame(&mut app, target.clone());
        assert_eq!((shadow_draws(&app), side_draws(&app)), (0, 0));
        assert_no_uploads(&app);
        app.insert_resource(PieceVisualQuality::Low);
        assert!(render_frame(&mut app, target.clone()) == lod);
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::ONE;
        assert!(render_frame(&mut app, target.clone()) == flat);
        app.insert_resource(quality);
        let pixels = render_frame(&mut app, target.clone());
        assert_eq!((shadow_draws(&app), side_draws(&app)), (2, 1));
        preview_if_requested(&format!("side-{quality:?}-idle.png"), &pixels);
        assert_eq!(config(&app).rotation_active, 0);
        let sides: Vec<_> = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, p)| is_side(*p))
            .map(|(i, _)| Vec2::new((i % 128) as f32, (i / 128) as f32))
            .collect();
        assert!(
            !sides.is_empty(),
            "static thickness missing for {quality:?}"
        );
        let p = sides[sides.len() / 2];
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert!(pick(&mut app, Rect::from_corners(p, p + Vec2::ONE), mode).is_empty());
            assert_eq!(
                pick(&mut app, Rect::new(64.0, 64.0, 65.0, 65.0), mode),
                vec![PieceId(0)]
            );
        }
        assert!(is_side(&pixels[(84 * 128 + 84) * 4..]));
        assert!(
            pixels[(85 * 128 + 85) * 4] > 200 && pixels[(85 * 128 + 85) * 4] < 250,
            "shadow must remain beyond side"
        );
        for (top, actual) in flat
            .as_chunks::<4>()
            .0
            .iter()
            .zip(pixels.as_chunks::<4>().0.iter())
        {
            if is_top(top) {
                assert_eq!(actual, top);
            }
        }
        assert_no_uploads(&app);
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_side_scaled_screen_thickness_tracks_continuous_pose_zoom_and_camera_rotation() {
    let (mut app, camera, target) = fixture(48, true);
    let mut def = definition(UVec2::ONE, 48, 42);
    def.image_size.y = 32;
    app.insert_resource(def);
    app.init_resource::<ShadowTime>()
        .add_systems(First, freeze_shadow_clock.after(update_rotation_clock));
    turn(&mut app, 1);
    let visuals = PieceVisualQuality::High.resolve();
    for time in [0.0, 0.060, 0.120] {
        app.world_mut().resource_mut::<ShadowTime>().0 = time;
        for (scale, angle) in [(1.0, 0.0), (2.0, std::f32::consts::FRAC_PI_2)] {
            {
                let mut transform = app.world_mut().get_mut::<Transform>(camera).unwrap();
                transform.scale = Vec3::new(scale, scale, 1.0);
                transform.rotation = Quat::from_rotation_z(angle);
            }
            app.insert_resource(PieceVisualQuality::Low);
            let flat = render_frame(&mut app, target.clone());
            let top = mask(&flat, is_top);
            let frame = visuals.for_frame(config(&app).piece_size_px, false);
            let side = shifted_top(&mut app, camera, &target, frame.side_offset_px());
            // Restore the projection before constructing the shadow reference.
            render_frame(&mut app, target.clone());
            let elevation = if time == 0.060 { 1.0 } else { 0.0 };
            let shadow = shifted_top(&mut app, camera, &target, frame.shadow_offset_px(elevation));
            app.insert_resource(PieceVisualQuality::High);
            let pixels = render_frame(&mut app, target.clone());
            if time == 0.060 && scale == 1.0 {
                preview_if_requested("side-High-midpoint.png", &pixels);
            }
            let actual_side = mask(&pixels, is_side);
            let mut side_count = 0;
            for (i, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
                assert_eq!(
                    actual_side[i],
                    side[i] && !top[i],
                    "side width time={time} scale={scale} pixel={i}"
                );
                if actual_side[i] {
                    side_count += 1;
                }
                if top[i] {
                    assert_eq!(pixel, &flat[i * 4..i * 4 + 4]);
                } else if shadow[i] && !side[i] {
                    assert!(pixel[0] > 200 && pixel[0] < 250);
                } else if !side[i] && !shadow[i] {
                    assert_eq!(pixel, &[255; 4]);
                }
            }
            assert!(side_count > 10);
            assert_eq!(config(&app).side_thickness_px, frame.side_thickness_px);
            assert_eq!(side_draws(&app), 1);
            assert_no_uploads(&app);
        }
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_side_connected_union_has_no_internal_seams_during_rotation() {
    let (mut app, camera, target) = fixture(80, true);
    let def = definition(UVec2::splat(2), 80, 42);
    let offset = Vec2::splat(100.0);
    app.insert_resource(def.clone());
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation = offset.extend(0.0);
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.initialize(
            (0..4)
                .map(|id| def.correct_position(PieceId(id)) + offset)
                .collect(),
        );
        store.snap_fixture_component(PieceId(0), &def);
        assert_eq!(store.connectivity.component_size(PieceId(0)), 4);
    }
    app.init_resource::<ShadowTime>()
        .add_systems(First, freeze_shadow_clock.after(update_rotation_clock));
    turn(&mut app, 1);
    for time in [0.0, 0.060, 0.120] {
        app.world_mut().resource_mut::<ShadowTime>().0 = time;
        app.insert_resource(PieceVisualQuality::Low);
        let flat = render_frame(&mut app, target.clone());
        let top = mask(&flat, is_top);
        let frame = PieceVisualQuality::High
            .resolve()
            .for_frame(config(&app).piece_size_px, false);
        let side = shifted_top(&mut app, camera, &target, frame.side_offset_px());
        app.insert_resource(PieceVisualQuality::High);
        let pixels = render_frame(&mut app, target.clone());
        if time == 0.060 {
            preview_if_requested("side-connected-midpoint.png", &pixels);
        }
        let mut outer = 0;
        for (i, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
            if top[i] {
                assert_eq!(
                    pixel,
                    &[255, 0, 0, 255],
                    "internal seam time={time} pixel={i}"
                );
            }
            if is_side(pixel) {
                assert!(side[i] && !top[i]);
                outer += 1;
            }
        }
        // This area contains both internal joins, including the four-piece junction.
        for y in 48..80 {
            for x in 48..80 {
                assert!(
                    is_top(&pixels[(y * 128 + x) * 4..]),
                    "connected junction at {x},{y}, time={time}"
                );
            }
        }
        assert!(outer > 50, "outer component thickness missing");
        assert_eq!(side_draws(&app), 1);
        assert_no_uploads(&app);
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_side_translucent_connected_union_has_no_extra_dark_band_during_rotation() {
    let (mut app, camera, target) = fixture(80, true);
    red_source(&mut app, 128);
    app.init_resource::<ShadowTime>()
        .add_systems(First, freeze_shadow_clock.after(update_rotation_clock));
    let offset = Vec2::splat(100.0);
    // Exercise exact pixel-centered joins as well as fractional raster phases.
    for pan in [Vec2::ZERO, Vec2::new(0.25, 0.125)] {
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation = (offset + pan).extend(0.0);

        for grid in [UVec2::new(2, 1), UVec2::splat(2)] {
            for shadow_enabled in [false, true] {
                let mut visuals = PieceVisualQuality::High.resolve();
                visuals.shadow_enabled = shadow_enabled;
                // Both geometric representations must use the component member's
                // dimensions; the single-piece union has a larger projected size.
                let frame = visuals.for_frame(Vec2::splat(40.0), false);
                visuals.side_thickness =
                    visuals::ProjectedDimension::fixed(frame.side_thickness_px);
                visuals.shadow_base_offset =
                    visuals::ProjectedDimension::fixed(frame.shadow_base_offset_px);
                visuals.shadow_lift_offset =
                    visuals::ProjectedDimension::fixed(frame.shadow_lift_offset_px);
                set_visuals(&mut app, visuals);
                let mut reference = Vec::new();
                for layout in [UVec2::ONE, grid] {
                    // The assembled union and the single rectangle have the same
                    // extent, source alpha and pivot, but only one has internal joins.
                    let mut def = definition(layout, 80, 42);
                    def.image_size.y = 40 * grid.y;
                    app.insert_resource(def.clone());
                    app.world_mut().resource_mut::<ShadowTime>().0 = 0.0;
                    {
                        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
                        store.initialize(
                            (0..def.piece_count())
                                .map(|id| def.correct_position(PieceId(id as u32)) + offset)
                                .collect(),
                        );
                        if layout != UVec2::ONE {
                            store.snap_fixture_component(PieceId(0), &def);
                        }
                        assert_eq!(
                            store.connectivity.component_size(PieceId(0)),
                            def.piece_count()
                        );
                    }
                    for (phase, time) in [0.0, 0.0, 0.060, 0.120].into_iter().enumerate() {
                        if phase == 1 {
                            turn(&mut app, 1);
                        }
                        app.world_mut().resource_mut::<ShadowTime>().0 = time;
                        let pixels = render_frame(&mut app, target.clone());
                        assert_eq!(
                            (shadow_draws(&app), side_draws(&app)),
                            (usize::from(shadow_enabled) * 2, 1)
                        );
                        assert_eq!(config(&app).rotation_active != 0, phase == 1 || phase == 2);
                        let center = &pixels[(64 * 128 + 64) * 4..][..4];
                        assert!(
                            center[0] > center[1]
                                && center[1] > 0
                                && center[1] < srgb_byte(1.0 - 128.0 / 255.0),
                            "translucent top must show side behind it: {center:?}"
                        );
                        assert_no_uploads(&app);
                        // Every sample lies inside the assembled rectangle. A
                        // connected seam must have one owner in both pick paths,
                        // including exact quarter/continuous raster ties.
                        for pixel in [
                            (63.0, 63.0),
                            (64.0, 64.0),
                            (63.0, 64.0),
                            (64.0, 63.0),
                            (71.0, 62.0),
                        ] {
                            let rect = Rect::new(pixel.0, pixel.1, pixel.0 + 1.0, pixel.1 + 1.0);
                            let point = pick(&mut app, rect, SelectionMode::Point);
                            assert_eq!(
                                point.len(), 1,
                                "point grid={grid:?} layout={layout:?} pan={pan:?} phase={phase} pixel={pixel:?}"
                            );
                            assert_eq!(
                                pick(&mut app, rect, SelectionMode::Rectangle), point,
                                "join ownership grid={grid:?} layout={layout:?} pan={pan:?} phase={phase} pixel={pixel:?}"
                            );
                        }
                        if layout == UVec2::ONE {
                            reference.push(pixels);
                            continue;
                        }
                        if phase == 2 {
                            preview_if_requested(
                                &format!(
                                    "side-connected-alpha128-{}x{}-shadow{shadow_enabled}.png",
                                    grid.x, grid.y
                                ),
                                &pixels,
                            );
                        }
                        // Check every pixel, including curved joins and the 2x2
                        // junction, against the single-piece brightness floor. Hard
                        // SDF interpolation can leave isolated brighter seam samples;
                        // this regression targets excess dark side accumulation.
                        for (i, (actual, expected)) in pixels
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .zip(reference[phase].as_chunks::<4>().0.iter())
                            .enumerate()
                        {
                            assert!(
                                actual.iter().zip(expected).all(|(&a, &b)| a >= b.saturating_sub(1)),
                                "extra dark band grid={grid:?} pan={pan:?} shadow={shadow_enabled} phase={phase} pixel=({}, {}): actual={actual:?} single={expected:?}",
                                i % 128,
                                i / 128
                            );
                        }
                    }
                }
            }
        }
    }
}

pub(super) fn srgb_byte(linear: f32) -> u8 {
    let srgb = if linear <= 0.0031308 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (srgb * 255.0).round() as u8
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_side_source_alpha_holes_and_translucent_blend_preserve_picking() {
    let (mut app, _, target) = fixture(40, true);
    let mut visuals = PieceVisualQuality::High.resolve();
    visuals.shadow_enabled = false;
    set_visuals(&mut app, visuals);
    for alpha in [255, 128, 0] {
        red_source(&mut app, alpha);
        let pixels = render_frame(&mut app, target.clone());
        let a = f32::from(alpha) / 255.0;
        let expected = srgb_byte(visuals.side_color.x * a + 1.0 - a);
        let side = &pixels[(84 * 128 + 64) * 4..][..4];
        assert!(
            (i32::from(side[0]) - i32::from(expected)).abs() <= 1,
            "alpha={alpha} side={side:?}"
        );
        assert_eq!(side[0], side[1]);
        assert_eq!(side[1], side[2]);
        assert_eq!((shadow_draws(&app), side_draws(&app)), (0, 1));
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert!(pick(&mut app, Rect::new(64.0, 84.0, 65.0, 85.0), mode).is_empty());
            assert_eq!(
                pick(&mut app, Rect::new(64.0, 64.0, 65.0, 65.0), mode),
                if alpha == 0 { vec![] } else { vec![PieceId(0)] }
            );
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: 2,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![255, 0, 0, 0, 255, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::all(),
    );
    image.sampler = bevy::image::ImageSampler::nearest();
    let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().resource_mut::<PuzzleImage>().handle = handle;
    app.world_mut().resource_mut::<PuzzleImage>().opaque = false;
    let pixels = render_frame(&mut app, target);
    assert_eq!(&pixels[(84 * 128 + 50) * 4..][..4], &[255; 4]);
    assert!(is_side(&pixels[(84 * 128 + 70) * 4..]));
    for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
        assert!(pick(&mut app, Rect::new(50.0, 64.0, 51.0, 65.0), mode).is_empty());
        assert_eq!(
            pick(&mut app, Rect::new(70.0, 64.0, 71.0, 65.0), mode),
            vec![PieceId(0)]
        );
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_side_far_splat_uses_center_alpha_and_excludes_shifted_pixels_from_picking() {
    let (mut app, _, target) = fixture(1, true);
    let mut visuals = PieceVisualQuality::High.resolve();
    visuals.shadow_enabled = false;
    visuals.side_min_piece_px = 0.25;
    visuals.side_thickness = visuals::ProjectedDimension::fixed(1.5);
    set_visuals(&mut app, visuals);
    for alpha in [255, 128, 0] {
        red_source(&mut app, alpha);
        let pixels = render_frame(&mut app, target.clone());
        assert_eq!(config(&app).far_zoom, 1);
        let a = f32::from(alpha) / 255.0;
        let expected = srgb_byte(visuals.side_color.x * a + 1.0 - a);
        assert!((i32::from(pixels[(65 * 128 + 65) * 4]) - i32::from(expected)).abs() <= 1);
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert!(pick(&mut app, Rect::new(65.0, 65.0, 66.0, 66.0), mode).is_empty());
            assert_eq!(
                pick(&mut app, Rect::new(64.0, 64.0, 65.0, 65.0), mode),
                if alpha == 0 { vec![] } else { vec![PieceId(0)] }
            );
        }
    }
    red_source(&mut app, 255);
    app.init_resource::<ShadowTime>()
        .add_systems(First, freeze_shadow_clock.after(update_rotation_clock));
    turn(&mut app, 1);
    app.world_mut().resource_mut::<ShadowTime>().0 = 0.060;
    let pixels = render_frame(&mut app, target);
    assert_eq!(config(&app).side_thickness_px, 1.5);
    assert!(is_side(&pixels[(65 * 128 + 65) * 4..]));
    for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
        assert!(pick(&mut app, Rect::new(65.0, 65.0, 66.0, 66.0), mode).is_empty());
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_side_visual_culling_keeps_side_only_viewport_edge_without_shadow() {
    let (mut app, camera, target) = fixture(10, true);
    app.init_resource::<ShadowTime>()
        .add_systems(First, freeze_shadow_clock.after(update_rotation_clock));
    let half = 10.0 * 0.72 * std::f32::consts::SQRT_2;
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation = Vec3::new(64.0 + half + 0.1, 0.25, 0.0);
    turn(&mut app, 1);
    app.world_mut().resource_mut::<ShadowTime>().0 = 0.060;
    let flat = render_frame(&mut app, target.clone());
    assert!(visible_ids(&app).is_empty());
    assert!(flat.as_chunks::<4>().0.iter().all(|p| *p == [255; 4]));
    let mut visuals = PieceVisualQuality::High.resolve();
    visuals.shadow_enabled = false;
    visuals.side_min_piece_px = 0.25;
    // Exceed the procedural quad padding to exercise the visual cull expansion.
    visuals.side_thickness = visuals::ProjectedDimension::fixed(10.0);
    set_visuals(&mut app, visuals);
    let pixels = render_frame(&mut app, target);
    assert_eq!(visible_ids(&app), vec![0]);
    assert_eq!((shadow_draws(&app), side_draws(&app)), (0, 1));
    assert!((0..128).any(|y| is_side(&pixels[(y * 128) * 4..])));
    for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
        assert!(pick(&mut app, Rect::new(0.0, 0.0, 2.0, 128.0), mode).is_empty());
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_side_overlap_uses_opaque_depth_and_existing_translucent_order() {
    let (mut app, _, target) = fixture(40, true);
    let mut visuals = PieceVisualQuality::High.resolve();
    visuals.shadow_enabled = false;
    set_visuals(&mut app, visuals);
    for count in [2, 64] {
        let mut def = definition(UVec2::new(count, 1), 40 * count, 42);
        def.image_size.y = 40;
        app.insert_resource(def);
        for alpha in [255, 128] {
            red_source(&mut app, alpha);
            {
                let mut store = app.world_mut().resource_mut::<PieceDataStore>();
                store.initialize(vec![Vec2::ZERO; count as usize]);
                for (id, state) in store.states.iter_mut().enumerate() {
                    state.z_order = crate::resources::pieces::MAX_Z - id as u32;
                }
            }
            let pixels = render_frame(&mut app, target.clone());
            let a = f32::from(alpha) / 255.0;
            let transmission = (1.0 - a).powi(count as i32);
            let expected = srgb_byte(visuals.side_color.x * (1.0 - transmission) + transmission);
            let pixel = &pixels[(84 * 128 + 64) * 4..][..4];
            assert!(
                (i32::from(pixel[0]) - i32::from(expected)).abs() <= 1,
                "count={count} alpha={alpha} pixel={pixel:?}"
            );
            let world = app.sub_app(RenderApp).world();
            let gpu = world.resource::<GpuRenderer>();
            let id = *gpu
                .side_pipelines
                .iter()
                .find(|((_, opaque), _)| *opaque == (alpha == 255))
                .unwrap()
                .1;
            let desc = world
                .resource::<PipelineCache>()
                .get_render_pipeline_descriptor(id);
            assert_eq!(
                desc.depth_stencil.as_ref().unwrap().depth_write_enabled,
                Some(alpha == 255)
            );
            assert_eq!(
                desc.fragment.as_ref().unwrap().targets[0]
                    .as_ref()
                    .unwrap()
                    .blend
                    .is_none(),
                alpha == 255
            );
            if alpha != 255 {
                assert!(gpu.buffers.as_ref().unwrap().sort.is_some());
                assert_eq!(visible_ids(&app), (0..count).rev().collect::<Vec<_>>());
            }
            assert_eq!(side_draws(&app), 1);
        }
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_side_lod_compilation_keeps_shadow_top_and_ready_epoch_visible() {
    for (initial_quality, initial_scale, quality, scale) in [
        (PieceVisualQuality::Low, 1.0, PieceVisualQuality::High, 1.0),
        (
            PieceVisualQuality::High,
            40.0 / 12.0,
            PieceVisualQuality::High,
            2.0,
        ),
        (
            PieceVisualQuality::Medium,
            2.0,
            PieceVisualQuality::High,
            2.0,
        ),
    ] {
        let (mut app, camera, target) = fixture(40, false);
        let mut shadow_only = quality.resolve();
        shadow_only.side_enabled = false;
        set_visuals(&mut app, shadow_only);
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(scale, scale, 1.0);
        let reference = render_frame(&mut app, target.clone());
        assert_eq!(shadow_draws(&app), 2);
        app.sub_app_mut(RenderApp)
            .world_mut()
            .remove_resource::<VisualOverride>();
        app.insert_resource(initial_quality);
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale =
            Vec3::new(initial_scale, initial_scale, 1.0);
        render_frame(&mut app, target.clone());
        assert!(app
            .sub_app(RenderApp)
            .world()
            .resource::<GpuRenderer>()
            .side_pipelines
            .is_empty());
        let epoch = app.world().resource::<PieceDataStore>().epoch;
        app.insert_resource(quality);
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(scale, scale, 1.0);
        update_gpu(&mut app);
        assert_eq!((shadow_draws(&app), side_draws(&app)), (2, 0));
        let world = app.sub_app(RenderApp).world();
        let cache = world.resource::<PipelineCache>();
        assert!(world
            .resource::<GpuRenderer>()
            .side_pipelines
            .values()
            .all(|id| cache.get_render_pipeline(*id).is_none()));
        let deadline = Instant::now() + Duration::from_secs(30);
        while side_draws(&app) == 0 {
            assert!(app.world().resource::<RenderReady>().is_ready(epoch));
            assert_eq!(shadow_draws(&app), 2);
            assert!(
                frame_pixels(&app, &target, 128) == reference,
                "side compilation interrupted shadow/top"
            );
            update_gpu(&mut app);
            assert!(
                Instant::now() < deadline,
                "side pipeline did not become ready"
            );
        }
        assert_eq!(side_draws(&app), 1);
        assert!(frame_pixels(&app, &target, 128)
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| is_side(p)));
        assert_no_uploads(&app);
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_side_independent_lod_never_queues_below_threshold() {
    let (mut app, camera, target) = fixture(40, true);
    for (quality, size) in [
        (PieceVisualQuality::High, 12.0),
        (PieceVisualQuality::Medium, 20.0),
    ] {
        app.insert_resource(quality);
        let scale = 40.0 / size;
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(scale, scale, 1.0);
        render_frame(&mut app, target.clone());
        assert_eq!(
            (config(&app).shadow_enabled, config(&app).side_enabled),
            (1, 0)
        );
        assert_eq!((shadow_draws(&app), side_draws(&app)), (2, 0));
        assert!(app
            .sub_app(RenderApp)
            .world()
            .resource::<GpuRenderer>()
            .side_pipelines
            .is_empty());
        assert_no_uploads(&app);
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_side_initial_high_epoch_waits_for_all_requested_optional_pipelines() {
    for previous_epoch in [false, true] {
        let (mut app, _, target) = gpu_app_with_pipeline_compilation(128, false);
        app.insert_resource(ClearColor(Color::WHITE));
        red_source(&mut app, 255);
        app.insert_resource(definition(UVec2::ONE, 40, 42));
        if previous_epoch {
            app.world_mut()
                .resource_mut::<PieceDataStore>()
                .initialize(vec![Vec2::ZERO]);
            wait_ready(&mut app);
        }
        app.insert_resource(PieceVisualQuality::High);
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .initialize(vec![Vec2::ZERO]);
        let epoch = app.world().resource::<PieceDataStore>().epoch;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            update_gpu(&mut app);
            assert!(!app.world().resource::<RenderReady>().is_ready(epoch));
            let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
            if !gpu.side_pipelines.is_empty() {
                assert!(
                    !gpu.shadow_pipelines.is_empty(),
                    "queue requested features together"
                );
                break;
            }
            assert!(Instant::now() < deadline);
        }
        assert_eq!((shadow_draws(&app), side_draws(&app)), (0, 0));
        wait_ready(&mut app);
        assert_eq!((shadow_draws(&app), side_draws(&app)), (2, 1));
        assert!(rendered_pixels(&mut app, target)
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| is_side(p)));
    }
}
