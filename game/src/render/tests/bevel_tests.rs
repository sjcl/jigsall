use super::shadow_tests::{
    assert_no_uploads, config, freeze_shadow_clock, render_frame, shadow_draws, turn, ShadowTime,
};
use super::side_tests::{preview_if_requested, set_visuals, side_draws, srgb_byte};
use super::*;
use crate::resources::rotation_visual::update_rotation_clock;

fn top_only(quality: PieceVisualQuality) -> ResolvedPieceVisuals {
    let mut visuals = quality.resolve();
    visuals.shadow_enabled = false;
    visuals.side_enabled = false;
    visuals
}

fn source(app: &mut App, rgba: [u8; 4]) {
    let handle = app.world().resource::<PuzzleImage>().handle.clone();
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .get_mut(&handle)
        .unwrap()
        .data = Some(rgba.to_vec());
    app.world_mut().resource_mut::<PuzzleImage>().opaque = rgba[3] == 255;
}

fn fixture(size: u32) -> (App, Entity, Handle<Image>) {
    let (mut app, camera, target) = gpu_app(128);
    app.insert_resource(ClearColor(Color::WHITE));
    source(&mut app, [128, 128, 128, 255]);
    app.insert_resource(definition(UVec2::ONE, size, 42));
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO]);
    wait_ready(&mut app);
    (app, camera, target)
}

fn pixel(pixels: &[u8], x: usize, y: usize) -> &[u8] {
    &pixels[(y * 128 + x) * 4..][..4]
}

#[test]
fn bevel_lod_far_gate_and_uniform_padding_do_not_expand_visual_bounds() {
    assert_eq!(PuzzleUniform::min_size().get(), 336);
    for quality in [
        PieceVisualQuality::Low,
        PieceVisualQuality::Medium,
        PieceVisualQuality::High,
    ] {
        for far in [false, true] {
            for size in [1.0, 12.0, 16.0, 17.99, 18.0, 27.99, 28.0, 64.0] {
                let mut config = PuzzleUniform {
                    far_zoom: u32::from(far),
                    piece_size_px: Vec2::new(size, 100.0),
                    ..default()
                };
                let visuals = top_only(quality);
                config.configure_visuals(visuals);
                assert_eq!(
                    config.bevel_enabled != 0,
                    visuals.bevel_enabled && !far && size >= visuals.bevel_min_piece_px
                );
                assert_eq!(config.visual_cull_extent, Vec2::ZERO);
            }
        }
    }
    let mut config = PuzzleUniform {
        piece_size_px: Vec2::splat(12.0),
        viewport_size: Vec2::splat(128.0),
        ..default()
    };
    config.configure_visuals(PieceVisualQuality::High.resolve());
    assert_eq!(
        (
            config.shadow_enabled,
            config.side_enabled,
            config.bevel_enabled
        ),
        (1, 0, 0)
    );
    config.piece_size_px = Vec2::splat(16.0);
    config.configure_visuals(PieceVisualQuality::High.resolve());
    assert_eq!(
        (
            config.shadow_enabled,
            config.side_enabled,
            config.bevel_enabled
        ),
        (1, 1, 0)
    );
    let mut visuals = top_only(PieceVisualQuality::High);
    visuals.bevel_min_piece_px = 0.0;
    config.far_zoom = 1;
    config.configure_visuals(visuals);
    assert_eq!(config.bevel_enabled, 0);
}

#[test]
fn bevel_is_fragment_only_without_new_bindings_or_elevation_work() {
    let shader = include_str!("../puzzle_render.wgsl");
    assert_eq!(shader.matches("@binding(").count(), 13);
    let vertex = shader
        .split("fn piece_vertex(")
        .nth(1)
        .unwrap()
        .split("fn distance(")
        .next()
        .unwrap();
    assert!(!vertex.contains("bevel"));
    let helper = shader
        .split("fn bevel_color(")
        .nth(1)
        .unwrap()
        .split("fn sample_visible(")
        .next()
        .unwrap();
    for forbidden in ["elevation", "rotation", "dpdx", "dpdy", "textureSample"] {
        assert!(!helper.contains(forbidden));
    }
    assert!(helper.contains("-config.pseudo_3d_direction"));
    assert!(helper.contains("color.a"));
    let fragment = shader
        .split("@fragment fn fragment")
        .nth(1)
        .unwrap()
        .split("fn check_selectable")
        .next()
        .unwrap();
    assert!(
        fragment.find("dpdx(bevel_boundary)").unwrap()
            < fragment.find("sample_visible(in,d)").unwrap()
    );
    assert!(
        fragment.find("bevel_color(").unwrap()
            < fragment.find("mix(color.rgb,line,coverage)").unwrap()
    );
    assert!(fragment
        .contains("select(outer_boundary_distance(edges,in.flags),0.0,(in.flags&480u)==480u)"));
    assert!(fragment.contains("if config.bevel_enabled!=0u {"));
    for name in [
        "shadow_depth_fragment",
        "shadow_fragment",
        "side_fragment",
        "point_fragment",
        "rectangle_fragment",
    ] {
        let body = shader
            .split(&format!("@fragment fn {name}"))
            .nth(1)
            .unwrap()
            .split("@fragment fn")
            .next()
            .unwrap();
        assert!(!body.contains("bevel_color"));
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_bevel_low_flat_lod_far_and_quality_switches_preserve_uploads_and_pipelines() {
    let (mut app, camera, target) = fixture(80);
    let flat = render_frame(&mut app, target.clone());
    for y in 0..128 {
        for x in 0..128 {
            assert_eq!(
                pixel(&flat, x, y),
                if (24..104).contains(&x) && (24..104).contains(&y) {
                    &[128, 128, 128, 255]
                } else {
                    &[255; 4]
                }
            );
        }
    }
    let pipeline = app
        .sub_app(RenderApp)
        .world()
        .resource::<GpuRenderer>()
        .main_pipelines
        .clone();
    for quality in [
        PieceVisualQuality::Low,
        PieceVisualQuality::Medium,
        PieceVisualQuality::High,
    ] {
        for size in [1.0, 12.0, 16.0, 17.0, 18.0, 27.0, 28.0, 40.0] {
            let scale = 80.0 / size;
            app.world_mut().get_mut::<Transform>(camera).unwrap().scale =
                Vec3::new(scale, scale, 1.0);
            set_visuals(&mut app, top_only(PieceVisualQuality::Low));
            let reference = render_frame(&mut app, target.clone());
            set_visuals(&mut app, top_only(quality));
            let actual = render_frame(&mut app, target.clone());
            let enabled = quality
                .resolve()
                .bevel_for_frame(config(&app).piece_size_px, config(&app).far_zoom != 0);
            assert_eq!(config(&app).bevel_enabled != 0, enabled);
            if !enabled {
                assert!(
                    actual == reference,
                    "flat regression {quality:?} size={size}"
                );
            }
            assert_eq!((shadow_draws(&app), side_draws(&app)), (0, 0));
            assert_no_uploads(&app);
        }
    }
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert_eq!(gpu.main_pipelines, pipeline);
    assert!(gpu.shadow_pipelines.is_empty() && gpu.side_pipelines.is_empty());
}

fn world_pixel(config: &PuzzleUniform, x: usize, y: usize) -> Vec2 {
    config
        .clip_from_world
        .inverse()
        .project_point3(Vec3::new(
            (x as f32 + 0.5) / 64.0 - 1.0,
            1.0 - (y as f32 + 0.5) / 64.0,
            0.0,
        ))
        .truncate()
}

fn display_angle(app: &App, time: f64) -> f32 {
    let store = app.world().resource::<PieceDataStore>();
    jigsall_core::decode_rotation(store.states[0].flags) as f32 * std::f32::consts::FRAC_PI_2
        + store
            .rotation_visual
            .animation(PieceId(0))
            .map_or(0.0, |a| a.angle(time))
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_bevel_screen_light_and_pixel_width_follow_zoom_camera_and_continuous_rotation() {
    let (mut app, camera, target) = fixture(80);
    let mut def = definition(UVec2::ONE, 80, 42);
    def.image_size.y = 48;
    app.insert_resource(def);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO]);
    app.init_resource::<ShadowTime>()
        .add_systems(First, freeze_shadow_clock.after(update_rotation_clock));
    turn(&mut app, 1);
    for time in [0.0, 0.060, 0.120] {
        app.world_mut().resource_mut::<ShadowTime>().0 = time;
        for (scale, camera_angle) in [(1.0, 0.0), (2.0, 0.0), (2.0, std::f32::consts::FRAC_PI_2)] {
            {
                let mut transform = app.world_mut().get_mut::<Transform>(camera).unwrap();
                transform.scale = Vec3::new(scale, scale, 1.0);
                transform.rotation = Quat::from_rotation_z(camera_angle);
            }
            set_visuals(&mut app, top_only(PieceVisualQuality::Low));
            let flat = render_frame(&mut app, target.clone());
            let visuals = top_only(PieceVisualQuality::High);
            set_visuals(&mut app, visuals);
            let actual = render_frame(&mut app, target.clone());
            let config = config(&app);
            assert_eq!(config.bevel_enabled, 1);
            let rotation = Mat2::from_angle(display_angle(&app, time));
            let inverse = config.clip_from_world.inverse();
            let world_dx = inverse
                .transform_vector3(Vec3::new(2.0 / 128.0, 0.0, 0.0))
                .truncate();
            let world_dy = inverse
                .transform_vector3(Vec3::new(0.0, -2.0 / 128.0, 0.0))
                .truncate();
            let mut bright = 0;
            let mut dark = 0;
            let mut interior = 0;
            for y in 0..128 {
                for x in 0..128 {
                    let base = pixel(&flat, x, y);
                    let lit = pixel(&actual, x, y);
                    if base == [255; 4] {
                        assert_eq!(lit, base);
                        continue;
                    }
                    let local = rotation.transpose() * world_pixel(&config, x, y);
                    let distances = Vec2::new(40.0, 24.0) - local.abs();
                    // Independent rectangle geometry; omit corners/medial ties,
                    // where a derivative of max(SDFs) mixes two edge normals.
                    if (distances.x - distances.y).abs() < 4.0 * scale {
                        continue;
                    }
                    let axis = if distances.x < distances.y { 0 } else { 1 };
                    let normal = rotation
                        * if axis == 0 {
                            Vec2::new(local.x.signum(), 0.0)
                        } else {
                            Vec2::new(0.0, local.y.signum())
                        };
                    let gradient = Vec2::new(normal.dot(world_dx), normal.dot(world_dy));
                    let inside_px = distances[axis] / gradient.length();
                    let nl = gradient.normalize().dot(-PSEUDO_3D_DIRECTION);
                    if inside_px > visuals.bevel_width_px + 0.01 {
                        assert_eq!(lit, base, "interior x={x} y={y} time={time} scale={scale}");
                        interior += 1;
                    } else if inside_px < visuals.bevel_width_px - 0.2 && nl.abs() > 0.25 {
                        if nl > 0.0 {
                            assert!(
                                lit[0] > base[0],
                                "screen highlight at {x},{y} time={time} camera={camera_angle}"
                            );
                            bright += 1;
                        } else {
                            assert!(
                                lit[0] < base[0],
                                "screen shadow at {x},{y} time={time} camera={camera_angle}"
                            );
                            dark += 1;
                        }
                    }
                    assert_eq!(lit[3], base[3]);
                }
            }
            assert!(bright > 20 && dark > 20 && interior > 100);
            assert_no_uploads(&app);
            if time == 0.060 && scale == 1.0 {
                set_visuals(&mut app, PieceVisualQuality::High.resolve());
                let presentation = render_frame(&mut app, target.clone());
                preview_if_requested("bevel-High-midpoint.png", &presentation);
            }
        }
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_bevel_connected_boundaries_and_enclosed_member_have_no_lighting_seams() {
    let (mut app, camera, target) = fixture(80);
    let offset = Vec2::splat(100.0);
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation = (offset + Vec2::new(0.25, 0.125)).extend(0.0);
    app.init_resource::<ShadowTime>()
        .add_systems(First, freeze_shadow_clock.after(update_rotation_clock));
    for grid in [UVec2::new(2, 1), UVec2::splat(2), UVec2::splat(3)] {
        let def = definition(grid, 80, 42);
        app.insert_resource(def.clone());
        app.world_mut().resource_mut::<ShadowTime>().0 = 0.0;
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            store.initialize(
                (0..def.piece_count())
                    .map(|id| def.correct_position(PieceId(id as u32)) + offset)
                    .collect(),
            );
            store.snap_fixture_component(PieceId(0), &def);
            assert_eq!(
                store.connectivity.component_size(PieceId(0)),
                def.piece_count()
            );
            if grid == UVec2::splat(3) {
                assert_eq!(store.states[4].flags & 480, 480);
            }
        }
        turn(&mut app, 1);
        for time in [0.0, 0.060, 0.120] {
            app.world_mut().resource_mut::<ShadowTime>().0 = time;
            for alpha in [255, 128] {
                source(&mut app, [128, 128, 128, alpha]);
                set_visuals(&mut app, top_only(PieceVisualQuality::Low));
                let flat = render_frame(&mut app, target.clone());
                set_visuals(&mut app, top_only(PieceVisualQuality::High));
                let actual = render_frame(&mut app, target.clone());
                let rotation = Mat2::from_angle(display_angle(&app, time));
                let config = config(&app);
                let mut interior = 0;
                let mut outer = 0;
                for y in 0..128 {
                    for x in 0..128 {
                        let base = pixel(&flat, x, y);
                        let lit = pixel(&actual, x, y);
                        if base == [255; 4] {
                            assert_eq!(lit, base);
                            continue;
                        }
                        let local = rotation.transpose() * (world_pixel(&config, x, y) - offset);
                        if local.abs().max_element() < 36.0 {
                            assert_eq!(
                            lit, base,
                            "connected bevel seam grid={grid:?} alpha={alpha} time={time} at {x},{y}"
                        );
                            interior += 1;
                        } else if lit != base {
                            outer += 1;
                        }
                    }
                }
                assert!(interior > 1000 && outer > 50);
                assert_no_uploads(&app);
                if grid == UVec2::splat(2) && time == 0.060 && alpha == 255 {
                    set_visuals(&mut app, PieceVisualQuality::High.resolve());
                    let presentation = render_frame(&mut app, target.clone());
                    preview_if_requested("bevel-High-connected-midpoint.png", &presentation);
                }
            }
        }
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_bevel_linear_color_preserves_source_alpha_and_both_pick_results() {
    let (mut app, _, target) = fixture(80);
    let visuals = top_only(PieceVisualQuality::High);
    let linear = ((128.0_f32 / 255.0 + 0.055) / 1.055).powf(2.4);
    let coverage = 1.0 - (3.0 * (0.5_f32 / 1.5).powi(2) - 2.0 * (0.5_f32 / 1.5).powi(3));
    for alpha in [0, 128, 255] {
        source(&mut app, [128, 128, 128, alpha]);
        set_visuals(&mut app, top_only(PieceVisualQuality::Low));
        let mut hits = Vec::new();
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            for rect in [
                Rect::new(64.0, 24.0, 65.0, 25.0),
                Rect::new(64.0, 64.0, 65.0, 65.0),
                Rect::new(0.0, 0.0, 1.0, 1.0),
            ] {
                hits.push(pick(&mut app, rect, mode));
            }
        }
        set_visuals(&mut app, visuals);
        let actual = render_frame(&mut app, target.clone());
        let a = f32::from(alpha) / 255.0;
        for (y, value) in [
            (
                24,
                linear
                    + visuals.bevel_highlight_strength
                        * coverage
                        * PSEUDO_3D_DIRECTION.y
                        * (1.0 - linear),
            ),
            (
                103,
                linear * (1.0 - visuals.bevel_shadow_strength * coverage * PSEUDO_3D_DIRECTION.y),
            ),
            (64, linear),
        ] {
            let expected = srgb_byte(value * a + 1.0 - a);
            let p = pixel(&actual, 64, y);
            for &channel in &p[..3] {
                assert!(
                    channel.abs_diff(expected) <= 1,
                    "linear blend alpha={alpha} y={y} actual={p:?} expected={expected}"
                );
            }
            assert_eq!(p[3], 255);
        }
        let mut index = 0;
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            for rect in [
                Rect::new(64.0, 24.0, 65.0, 25.0),
                Rect::new(64.0, 64.0, 65.0, 65.0),
                Rect::new(0.0, 0.0, 1.0, 1.0),
            ] {
                assert_eq!(pick(&mut app, rect, mode), hits[index]);
                index += 1;
            }
        }
        if alpha == 0 {
            assert!(actual.chunks_exact(4).all(|p| p == [255; 4]));
        }
        assert_no_uploads(&app);
    }
    // Alpha holes must not leave an independent bevel line.
    let mut image = Image::new(
        Extent3d {
            width: 2,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![128, 128, 128, 0, 128, 128, 128, 128],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::all(),
    );
    image.sampler = bevy::image::ImageSampler::nearest();
    let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().resource_mut::<PuzzleImage>().handle = handle;
    app.world_mut().resource_mut::<PuzzleImage>().opaque = false;
    let actual = render_frame(&mut app, target);
    assert_eq!(pixel(&actual, 40, 24), &[255; 4]);
    assert!(pixel(&actual, 88, 24)[0] < 255);
    for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
        assert!(pick(&mut app, Rect::new(40.0, 24.0, 41.0, 25.0), mode).is_empty());
        assert_eq!(
            pick(&mut app, Rect::new(88.0, 24.0, 89.0, 25.0), mode),
            vec![PieceId(0)]
        );
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_bevel_is_independent_of_rotation_elevation_at_the_same_pose() {
    use crate::resources::rotation_visual::GpuRotationAnimation;
    let (mut app, _, target) = fixture(80);
    app.init_resource::<ShadowTime>()
        .add_systems(First, freeze_shadow_clock.after(update_rotation_clock));
    set_visuals(&mut app, top_only(PieceVisualQuality::High));
    turn(&mut app, 1);
    app.world_mut().resource_mut::<ShadowTime>().0 = 0.030;
    let before = render_frame(&mut app, target.clone());
    assert_eq!(config(&app).rotation_active, 1);
    let visible = visible_ids(&app);
    for elevation in [0.0_f32, 0.5, 1.0] {
        let world = app.sub_app(RenderApp).world();
        let gpu = world.resource::<GpuRenderer>();
        world.resource::<RenderQueue>().write_buffer(
            &gpu.buffers.as_ref().unwrap().rotation_animations,
            std::mem::offset_of!(GpuRotationAnimation, start_elevation) as u64,
            bytemuck::bytes_of(&elevation),
        );
        assert!(
            render_frame(&mut app, target.clone()) == before,
            "elevation={elevation}"
        );
        assert_eq!(visible_ids(&app), visible);
        assert_no_uploads(&app);
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_bevel_selection_and_preview_keep_outline_colors_and_internal_suppression() {
    let (mut app, camera, target) = fixture(80);
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
        store.selected_pieces.extend((0..4).map(PieceId));
    }
    set_visuals(&mut app, top_only(PieceVisualQuality::Low));
    let selected = render_frame(&mut app, target.clone());
    set_visuals(&mut app, top_only(PieceVisualQuality::High));
    let lit = render_frame(&mut app, target.clone());
    assert_eq!(pixel(&lit, 64, 24), pixel(&selected, 64, 24));
    assert_eq!(pixel(&lit, 64, 24), &[255, srgb_byte(0.8), 0, 255]);
    for y in 48..80 {
        for x in 48..80 {
            assert_eq!(pixel(&lit, x, y), &[128, 128, 128, 255]);
        }
    }
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .selected_pieces = jigsall_core::PieceBitSet::new(4);
    super::component_preview_tests::preview(&mut app, Rect::new(64.0, 64.0, 65.0, 65.0));
    let lit = render_frame(&mut app, target.clone());
    set_visuals(&mut app, top_only(PieceVisualQuality::Low));
    let preview = render_frame(&mut app, target);
    assert_eq!(pixel(&lit, 64, 24), pixel(&preview, 64, 24));
    assert_eq!(
        pixel(&lit, 64, 24),
        &[srgb_byte(0.3), srgb_byte(0.6), 255, 255]
    );
    for y in 48..80 {
        for x in 48..80 {
            assert_eq!(pixel(&lit, x, y), &[128, 128, 128, 255]);
        }
    }
    assert_no_uploads(&app);
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_bevel_full_presentation_presets_can_save_native_previews() {
    let (mut app, _, target) = fixture(80);
    for quality in [PieceVisualQuality::Medium, PieceVisualQuality::High] {
        app.insert_resource(quality);
        let pixels = render_frame(&mut app, target.clone());
        assert_eq!(
            (
                shadow_draws(&app),
                side_draws(&app),
                config(&app).bevel_enabled
            ),
            (2, 1, 1)
        );
        preview_if_requested(&format!("bevel-{quality:?}-idle.png"), &pixels);
        assert_eq!(pixel(&pixels, 64, 64), &[128, 128, 128, 255]);
        assert_no_uploads(&app);
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_bevel_curved_loose_contour_follows_screen_light_during_rotation() {
    let (mut app, _, target) = fixture(80);
    let def = definition(UVec2::splat(2), 80, 42);
    let profiles = piece_profiles(def.seed, def.grid_size, UVec2::ZERO);
    app.insert_resource(def);
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.initialize(vec![Vec2::ZERO; 4]);
        for state in store.states.iter_mut().skip(1) {
            state.flags = 0;
        }
    }
    app.init_resource::<ShadowTime>()
        .add_systems(First, freeze_shadow_clock.after(update_rotation_clock));
    turn(&mut app, 1);
    let mut bright = 0;
    let mut dark = 0;
    for time in [0.0, 0.030, 0.060, 0.090, 0.120] {
        app.world_mut().resource_mut::<ShadowTime>().0 = time;
        set_visuals(&mut app, top_only(PieceVisualQuality::Low));
        let flat = render_frame(&mut app, target.clone());
        set_visuals(&mut app, top_only(PieceVisualQuality::High));
        let actual = render_frame(&mut app, target.clone());
        let config = config(&app);
        let rotation = Mat2::from_angle(display_angle(&app, time));
        let inverse = config.clip_from_world.inverse();
        let world_dx = inverse
            .transform_vector3(Vec3::new(2.0 / 128.0, 0.0, 0.0))
            .truncate();
        let world_dy = inverse
            .transform_vector3(Vec3::new(0.0, -2.0 / 128.0, 0.0))
            .truncate();
        for y in 1..127 {
            for x in 1..127 {
                let base = pixel(&flat, x, y);
                let lit = pixel(&actual, x, y);
                if base == [255; 4] {
                    assert_eq!(lit, base);
                    continue;
                }
                let world = world_pixel(&config, x, y);
                let local = rotation.transpose() * world;
                // Only the curved tab/blank region, away from rectangle corners
                // and nominal straight edges. The shape reference is CPU-only.
                if !((local.x.abs() - 20.0).abs() > 0.5 && local.y.abs() < 12.0
                    || (local.y.abs() - 20.0).abs() > 0.5 && local.x.abs() < 12.0)
                {
                    continue;
                }
                let distance = |world| {
                    piece_signed_distance(rotation.transpose() * world, Vec2::splat(40.0), profiles)
                };
                let d = distance(world);
                let gradient = Vec2::new(
                    distance(world + world_dx) - distance(world - world_dx),
                    distance(world + world_dy) - distance(world - world_dy),
                ) * 0.5;
                let length = gradient.length();
                if length < 0.1 {
                    continue;
                }
                let inside_px = -d / length;
                let nl = gradient.normalize().dot(-PSEUDO_3D_DIRECTION);
                if !(0.1..0.8).contains(&inside_px) || nl.abs() < 0.8 {
                    continue;
                }
                if nl > 0.0 {
                    assert!(
                        lit[0] > base[0],
                        "curved highlight at {x},{y}, time={time}, nl={nl}"
                    );
                    bright += 1;
                } else {
                    assert!(
                        lit[0] < base[0],
                        "curved shade at {x},{y}, time={time}, nl={nl}"
                    );
                    dark += 1;
                }
            }
        }
        assert_no_uploads(&app);
    }
    assert!(
        bright > 5 && dark > 5,
        "curved samples: bright={bright} dark={dark}"
    );
}
