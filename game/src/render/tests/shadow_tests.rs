use super::*;
use crate::render::visuals::ProjectedDimension;
use crate::resources::rotation_visual::update_rotation_clock;
use jigsall_core::{
    protocol::{ComponentRef, PieceTarget},
    PieceCommand, LOCAL_PLAYER,
};

#[test]
fn shadow_quality_and_lod_resolve_once_per_frame() {
    for quality in [
        PieceVisualQuality::Low,
        PieceVisualQuality::Medium,
        PieceVisualQuality::High,
    ] {
        let visuals = quality.resolve();
        for size in [0.25, 9.99, 10.0, 13.99, 14.0, 40.0] {
            let mut config = PuzzleUniform {
                clip_from_world: Mat4::IDENTITY,
                viewport_size: Vec2::new(128.0, 256.0),
                piece_size_px: Vec2::new(size, 100.0),
                ..default()
            };
            config.configure_visuals(visuals);
            assert_eq!(
                config.shadow_enabled != 0,
                quality != PieceVisualQuality::Low && size >= visuals.shadow_min_piece_px
            );
            assert_eq!(
                config.visual_cull_extent == Vec2::ZERO,
                config.shadow_enabled == 0
            );
        }
        let frame = visuals.for_frame(Vec2::splat(40.0), false);
        assert_eq!(
            frame.shadow_offset_px(0.0),
            PSEUDO_3D_DIRECTION * frame.shadow_base_offset_px
        );
        assert_eq!(
            frame.shadow_offset_px(1.0),
            PSEUDO_3D_DIRECTION * (frame.shadow_base_offset_px + frame.shadow_lift_offset_px)
        );
    }
    assert_eq!(PuzzleUniform::min_size().get() % 16, 0);
}

#[test]
fn shadow_shares_silhouette_without_changing_top_or_pick_entrypoints() {
    let shader = include_str!("../puzzle_render.wgsl");
    assert!(shader.contains("return piece_vertex(vi,instance,false,false);"));
    assert!(shader.contains("return piece_vertex(vi,instance,true,false);"));
    assert!(shader.contains("if shadow {elevation=pose.elevation;}"));
    let output = shader
        .split("struct VertexOutput {")
        .nth(1)
        .unwrap()
        .split("};")
        .next()
        .unwrap();
    assert!(!output.contains("elevation"));
    assert!(!output.contains("shadow"));
    assert!(shader.contains("return sample_visible(in,distance(in));"));
    assert!(shader.contains("return sample_splat(in);"));
    let pick = include_str!("../pick_visibility.wgsl")
        .split("fn cull_pick")
        .nth(1)
        .unwrap();
    assert!(!pick.contains("shadow"));
}

pub(super) fn config(app: &App) -> PuzzleUniform {
    app.sub_app(RenderApp)
        .world()
        .resource::<ExtractedPuzzle>()
        .config
        .clone()
}

pub(super) fn shadow_draws(app: &App) -> usize {
    app.sub_app(RenderApp)
        .world()
        .resource::<GpuRenderer>()
        .shadow_draws
}

pub(super) fn render_frame(app: &mut App, target: Handle<Image>) -> Vec<u8> {
    // Quality switches can lazily queue a new pipeline after RenderReady was
    // already signalled for this epoch. Wait for all requested optional draws.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        update_gpu(app);
        let config = config(app);
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        if (config.shadow_enabled == 0 || gpu.shadow_draws == 2)
            && (config.side_enabled == 0 || gpu.side_draws == 1)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "shadow pipeline did not become ready"
        );
    }
    rendered_pixels(app, target)
}

// With asynchronous compilation, RenderReady can precede Bevy's final output
// pipeline. Warm the red fixture before measuring a lazy-feature transition.
pub(super) fn wait_red_frame(app: &mut App, target: Handle<Image>) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let pixels = render_frame(app, target.clone());
        if pixels[(64 * 128 + 64) * 4..][..4] == [255, 0, 0, 255] {
            return pixels;
        }
        assert!(
            Instant::now() < deadline,
            "red fixture output did not become ready"
        );
    }
}

pub(super) fn red_source(app: &mut App, alpha: u8) {
    let handle = app.world().resource::<PuzzleImage>().handle.clone();
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .get_mut(&handle)
        .unwrap()
        .data = Some(vec![255, 0, 0, alpha]);
    app.world_mut().resource_mut::<PuzzleImage>().opaque = alpha == 255;
}

fn fixture(size: u32) -> (App, Entity, Handle<Image>) {
    let (mut app, camera, target) = gpu_app(128);
    shadow_only(&mut app);
    app.insert_resource(ClearColor(Color::WHITE));
    red_source(&mut app, 255);
    app.insert_resource(definition(UVec2::ONE, size, 42));
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO]);
    wait_ready(&mut app);
    (app, camera, target)
}

fn gray_shadow(pixel: &[u8]) -> bool {
    pixel[0] > 0 && pixel[0] < 250 && pixel[0] == pixel[1] && pixel[1] == pixel[2]
}

pub(super) fn shadow_pixels(pixels: &[u8]) -> Vec<UVec2> {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, p)| gray_shadow(*p))
        .map(|(index, _)| UVec2::new((index % 128) as u32, (index / 128) as u32))
        .collect()
}

pub(super) fn assert_no_uploads(app: &App) {
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert_eq!(
        gpu.upload_bytes
            + gpu.root_upload_bytes
            + gpu.rotation_upload_bytes
            + gpu.remote_mapping_upload_bytes
            + gpu.remote_delta_upload_bytes
            + gpu.selection_upload_bytes
            + gpu.drag_upload_bytes
            + gpu.drag_elevation_upload_bytes,
        0
    );
}

// Preserve these fixtures as shadow-only references.
fn shadow_only(app: &mut App) {
    app.sub_app_mut(RenderApp).add_systems(
        ExtractSchedule,
        (|mut frame: ResMut<ExtractedPuzzle>| {
            frame.config.side_enabled = 0;
            frame.config.bevel_enabled = 0;
        })
        .after(extract_puzzle),
    );
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_shadow_lazy_compilation_keeps_ready_epoch_visible() {
    for (initial_quality, initial_scale, quality, scale) in [
        (PieceVisualQuality::Low, 1.0, PieceVisualQuality::High, 1.0),
        (
            PieceVisualQuality::Low,
            1.0,
            PieceVisualQuality::Medium,
            1.0,
        ),
        (
            PieceVisualQuality::High,
            8.0,
            PieceVisualQuality::High,
            40.0 / 11.0,
        ),
        (
            PieceVisualQuality::Medium,
            40.0 / 11.0,
            PieceVisualQuality::High,
            40.0 / 11.0,
        ),
    ] {
        let (mut app, camera, target) = gpu_app_with_pipeline_compilation(128, false);
        shadow_only(&mut app);
        app.insert_resource(ClearColor(Color::WHITE));
        red_source(&mut app, 255);
        app.insert_resource(definition(UVec2::ONE, 40, 42));
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .initialize(vec![Vec2::ZERO]);
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(scale, scale, 1.0);
        wait_ready(&mut app);
        // Capture the flat reference at the zoom used by the transition.
        let flat = wait_red_frame(&mut app, target.clone());
        assert_eq!(&flat[(64 * 128 + 64) * 4..][..4], &[255, 0, 0, 255]);
        app.insert_resource(initial_quality);
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale =
            Vec3::new(initial_scale, initial_scale, 1.0);
        render_frame(&mut app, target.clone());
        assert!(app
            .sub_app(RenderApp)
            .world()
            .resource::<GpuRenderer>()
            .shadow_pipelines
            .is_empty());
        let epoch = app.world().resource::<PieceDataStore>().epoch;

        app.insert_resource(quality);
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(scale, scale, 1.0);
        update_gpu(&mut app);
        assert_ne!(config(&app).shadow_enabled, 0);
        // puzzle_node queues these after PipelineCache has processed this frame.
        // Read this exact frame, without advancing past the compilation gap.
        assert_eq!(shadow_draws(&app), 0);
        let world = app.sub_app(RenderApp).world();
        let gpu = world.resource::<GpuRenderer>();
        let cache = world.resource::<PipelineCache>();
        assert!(gpu
            .shadow_pipelines
            .values()
            .flatten()
            .all(|id| cache.get_render_pipeline(*id).is_none()));
        assert!(
            frame_pixels(&app, &target, 128) == flat,
            "first lazy shadow frame lost top pieces"
        );

        let deadline = Instant::now() + Duration::from_secs(30);
        while shadow_draws(&app) == 0 {
            assert!(app.world().resource::<RenderReady>().is_ready(epoch));
            assert!(app.world().resource::<RenderReady>().error(epoch).is_none());
            assert!(
                frame_pixels(&app, &target, 128) == flat,
                "top pieces disappeared while compiling shadows"
            );
            update_gpu(&mut app);
            assert!(
                Instant::now() < deadline,
                "shadow pipeline did not become ready"
            );
        }
        assert_eq!(shadow_draws(&app), 2);
        let pixels = frame_pixels(&app, &target, 128);
        assert!(!shadow_pixels(&pixels).is_empty());
        assert_eq!(&pixels[(64 * 128 + 64) * 4..][..4], &[255, 0, 0, 255]);
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_shadow_unready_epoch_waits_for_requested_pipelines() {
    for previous_epoch_ready in [false, true] {
        let (mut app, _, target) = gpu_app_with_pipeline_compilation(128, false);
        shadow_only(&mut app);
        app.insert_resource(ClearColor(Color::WHITE));
        red_source(&mut app, 255);
        app.insert_resource(definition(UVec2::ONE, 40, 42));
        if previous_epoch_ready {
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
        assert!(!app.world().resource::<RenderReady>().is_ready(epoch));
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            update_gpu(&mut app);
            assert!(!app.world().resource::<RenderReady>().is_ready(epoch));
            assert!(app.world().resource::<RenderReady>().error(epoch).is_none());
            if !app
                .sub_app(RenderApp)
                .world()
                .resource::<GpuRenderer>()
                .shadow_pipelines
                .is_empty()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "shadow pipelines were not queued"
            );
        }
        assert_eq!(shadow_draws(&app), 0);
        wait_ready(&mut app);
        assert_eq!(shadow_draws(&app), 2);
        // RenderReady covers the puzzle pipelines, not Bevy's independently
        // compiled final output pipeline. Wait for actual pixels before reading.
        assert!(!shadow_pixels(&wait_red_frame(&mut app, target)).is_empty());
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_shadow_static_low_flat_pixels_lod_and_top_only_picking() {
    let (mut app, camera, target) = fixture(40);
    let flat = render_frame(&mut app, target.clone());
    assert_eq!(shadow_draws(&app), 0);
    // Exact flat reference: a single all-border rectangular piece, no profiles.
    for (index, pixel) in flat.as_chunks::<4>().0.iter().enumerate() {
        let x = index % 128;
        let y = index / 128;
        let expected = if (44..84).contains(&x) && (44..84).contains(&y) {
            [255, 0, 0, 255]
        } else {
            [255; 4]
        };
        assert_eq!(*pixel, expected, "flat pixel {x},{y}");
    }
    for quality in [PieceVisualQuality::Medium, PieceVisualQuality::High] {
        app.insert_resource(quality);
        let pixels = render_frame(&mut app, target.clone());
        assert_eq!(shadow_draws(&app), 2);
        assert_eq!(config(&app).rotation_active, 0);
        let shadows = shadow_pixels(&pixels);
        assert!(!shadows.is_empty(), "idle base shadow missing");
        for pixel in [shadows[0], *shadows.last().unwrap()] {
            let rect = Rect::from_corners(pixel.as_vec2(), pixel.as_vec2() + Vec2::ONE);
            for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
                assert!(pick(&mut app, rect, mode).is_empty());
            }
        }
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert_eq!(
                pick(&mut app, Rect::new(64.0, 64.0, 65.0, 65.0), mode),
                vec![PieceId(0)]
            );
        }
        assert_no_uploads(&app);
        // Zoom out below both presets: pixels equal the same Low frame.
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(8.0, 8.0, 1.0);
        let lod_pixels = render_frame(&mut app, target.clone());
        assert_eq!(shadow_draws(&app), 0);
        app.insert_resource(PieceVisualQuality::Low);
        assert_eq!(render_frame(&mut app, target.clone()), lod_pixels);
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::ONE;
        assert_eq!(render_frame(&mut app, target.clone()), flat);
    }
}

#[derive(Resource, Default)]
pub(super) struct ShadowTime(pub(super) f64);
pub(super) fn freeze_shadow_clock(time: Res<ShadowTime>, mut store: ResMut<PieceDataStore>) {
    store.rotation_visual.clock = time.0;
}
pub(super) fn turn(app: &mut App, quarter_turns: i8) {
    let def = app.world().resource::<PuzzleDefinition>().clone();
    let time = app.world().get_resource::<ShadowTime>().map(|time| time.0);
    let mut store = app.world_mut().resource_mut::<PieceDataStore>();
    if let Some(time) = time {
        store.rotation_visual.clock = time;
    }
    let cmd = PieceCommand::Rotate {
        target: PieceTarget::Component(
            ComponentRef::from_member(&store.connectivity, PieceId(0)).unwrap(),
        ),
        quarter_turns,
    };
    let boundary = store.capture_rotation_command(&cmd, Some(&def));
    let members = store.connectivity.component_size(PieceId(0));
    assert_eq!(
        store
            .apply_command(LOCAL_PLAYER, &cmd, Some(&def), LOCAL_PLAYER)
            .rotated,
        members
    );
    store.finish_rotation_boundary(boundary);
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_shadow_rotation_separation_and_screen_direction_survive_zoom_and_rotation() {
    let (mut app, camera, target) = fixture(40);
    app.init_resource::<ShadowTime>()
        .add_systems(First, freeze_shadow_clock.after(update_rotation_clock));
    turn(&mut app, 1);
    for time in [0.0, 0.060, 0.120] {
        app.world_mut().resource_mut::<ShadowTime>().0 = time;
        for scale in [1.0, 2.0] {
            {
                let mut transform = app.world_mut().get_mut::<Transform>(camera).unwrap();
                transform.scale = Vec3::new(scale, scale, 1.0);
                transform.rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
            }
            app.insert_resource(PieceVisualQuality::Low);
            let flat = render_frame(&mut app, target.clone());
            let top: Vec<_> = flat
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
                .filter(|(_, p)| p[..3] == [255, 0, 0])
                .map(|(i, _)| UVec2::new((i % 128) as u32, (i / 128) as u32))
                .collect();
            app.insert_resource(PieceVisualQuality::High);
            let pixels = render_frame(&mut app, target.clone());
            let shadows = shadow_pixels(&pixels);
            assert!(!shadows.is_empty());
            let frame = config(&app);
            let expected = (PSEUDO_3D_DIRECTION.x
                * (frame.shadow_base_offset_px
                    + if time == 0.060 {
                        frame.shadow_lift_offset_px
                    } else {
                        0.0
                    }))
            .round() as i32;
            for axis in 0..2 {
                let top_max = top.iter().map(|p| p[axis]).max().unwrap();
                let shadow_max = shadows.iter().map(|p| p[axis]).max().unwrap();
                assert!(
                    (shadow_max as i32 - top_max as i32 - expected).abs() <= 1,
                    "time={time}, scale={scale}, axis={axis}"
                );
            }
            // High only adds shadow pixels; every top fragment remains exact.
            for p in top {
                let offset = (p.y * 128 + p.x) as usize * 4;
                assert_eq!(&pixels[offset..offset + 4], &flat[offset..offset + 4]);
            }
            assert_no_uploads(&app);
        }
    }
}

// Exercise the implemented splat shadow with a test-only smaller LOD threshold.
// Production presets deliberately skip every far overview.
fn enable_far_shadow(mut frame: ResMut<ExtractedPuzzle>) {
    let mut visuals = PieceVisualQuality::High.resolve();
    visuals.shadow_min_piece_px = 0.25;
    // Keep the isolated far-splat pixel fixture's geometry unchanged.
    visuals.shadow_base_offset = visuals::ProjectedDimension::fixed(3.0);
    visuals.shadow_lift_offset = visuals::ProjectedDimension::fixed(4.5);
    frame.config.configure_visuals(visuals);
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_shadow_far_splat_and_alpha_share_point_rectangle_semantics() {
    let (mut app, _, target) = fixture(1);
    app.sub_app_mut(RenderApp)
        .add_systems(ExtractSchedule, enable_far_shadow.after(extract_puzzle));
    for alpha in [255, 128, 0] {
        red_source(&mut app, alpha);
        let pixels = render_frame(&mut app, target.clone());
        assert_eq!(config(&app).far_zoom, 1);
        assert_eq!(shadow_draws(&app), 2);
        let shadows = shadow_pixels(&pixels);
        if alpha == 0 {
            assert!(shadows.is_empty());
            assert!(pixels.as_chunks::<4>().0.iter().all(|p| p[..3] == [255; 3]));
        } else {
            assert_eq!(shadows, vec![UVec2::new(66, 66)]);
        }
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert!(pick(&mut app, Rect::new(66.0, 66.0, 67.0, 67.0), mode).is_empty());
            assert_eq!(
                pick(&mut app, Rect::new(64.0, 64.0, 65.0, 65.0), mode),
                if alpha == 0 { vec![] } else { vec![PieceId(0)] }
            );
        }
    }
    // The same splat vertex helper also consumes lift only for a nonzero slot.
    red_source(&mut app, 255);
    app.init_resource::<ShadowTime>()
        .add_systems(First, freeze_shadow_clock.after(update_rotation_clock));
    turn(&mut app, 1);
    app.world_mut().resource_mut::<ShadowTime>().0 = 0.060;
    let midpoint = render_frame(&mut app, target.clone());
    let shadows = shadow_pixels(&midpoint);
    assert!(!shadows.is_empty());
    assert!(shadows.iter().all(|p| p.x >= 67 && p.y >= 67));
    for pixel in shadows {
        let rect = Rect::from_corners(pixel.as_vec2(), pixel.as_vec2() + Vec2::ONE);
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert!(pick(&mut app, rect, mode).is_empty());
        }
    }
    app.world_mut().resource_mut::<ShadowTime>().0 = 0.120;
    assert_eq!(
        shadow_pixels(&render_frame(&mut app, target)),
        vec![UVec2::new(66, 66)]
    );
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_shadow_normal_alpha_hole_does_not_cast_or_pick() {
    let (mut app, _, target) = fixture(40);
    app.insert_resource(PieceVisualQuality::High);
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
    for x in [50, 60] {
        let index = (84 * 128 + x) * 4;
        assert_eq!(&pixels[index..index + 3], &[255; 3]);
    }
    assert!(gray_shadow(
        &pixels[(84 * 128 + 70) * 4..(84 * 128 + 70) * 4 + 4]
    ));
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
fn gpu_shadow_culling_keeps_shadow_only_viewport_edge() {
    let (mut app, camera, target) = fixture(10);
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
    assert!(flat.as_chunks::<4>().0.iter().all(|p| p[..3] == [255; 3]));
    app.insert_resource(PieceVisualQuality::High);
    // Keep this clipping fixture's separation beyond the conservative quad
    // padding. Production size rules are covered by scaled_visuals_tests.
    let mut visuals = PieceVisualQuality::High.resolve();
    visuals.shadow_base_offset = ProjectedDimension::fixed(3.0);
    visuals.shadow_lift_offset = ProjectedDimension::fixed(4.5);
    super::side_tests::set_visuals(&mut app, visuals);
    let pixels = render_frame(&mut app, target);
    assert_eq!(visible_ids(&app), vec![0]);
    assert!(shadow_pixels(&pixels).iter().any(|p| p.x == 0));
    for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
        assert!(pick(&mut app, Rect::new(0.0, 0.0, 2.0, 128.0), mode).is_empty());
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_shadow_overlap_blends_once_for_opaque_translucent_and_equal_ranks() {
    let (mut app, _, target) = fixture(10);
    app.insert_resource(PieceVisualQuality::High);
    let mut def = definition(UVec2::new(64, 1), 640, 42);
    def.image_size.y = 10;
    app.insert_resource(def);
    for alpha in [255, 128] {
        red_source(&mut app, alpha);
        for ranks in ["ascending", "tied", "maximum"] {
            {
                let mut store = app.world_mut().resource_mut::<PieceDataStore>();
                store.initialize(vec![Vec2::ZERO; 64]);
                for (id, state) in store.states.iter_mut().enumerate() {
                    match ranks {
                        "tied" => state.z_order = 0,
                        "maximum" => state.z_order = crate::resources::pieces::MAX_Z - id as u32,
                        _ => {}
                    }
                }
            }
            let pixels = render_frame(&mut app, target.clone());
            let shadows: Vec<_> = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| gray_shadow(p.as_slice()))
                .collect();
            assert!(!shadows.is_empty());
            let expected = if alpha == 255 { 225 } else { 240 };
            for pixel in shadows {
                assert!(
                    (pixel[0] as i32 - expected).abs() <= 2,
                    "accumulation: alpha={alpha}, ranks={ranks}, pixel={pixel:?}"
                );
            }
            assert_eq!(shadow_draws(&app), 2);
        }
    }
}

#[test]
#[ignore = "release benchmark on a real GPU"]
fn gpu_shadow_million_overview_low_high_skip_draw_benchmark() {
    let (mut app, camera, target) = gpu_app(512);
    let def = definition(UVec2::splat(1000), 1000, 42);
    let states = crate::resources::DensePieceStates::generate(&def);
    let extent = states
        .iter()
        .fold(Vec2::ZERO, |a, p| a.max(p.position.abs()));
    let scale = extent.max_element() * 2.0 / 512.0 * 1.02;
    app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(scale, scale, 1.0);
    app.insert_resource(def);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize_dense(states);
    wait_ready(&mut app);
    let reference = render_frame(&mut app, target.clone());
    for quality in [
        PieceVisualQuality::Low,
        PieceVisualQuality::Medium,
        PieceVisualQuality::High,
    ] {
        app.insert_resource(quality);
        for _ in 0..8 {
            update_gpu(&mut app);
        }
        assert_eq!(render_frame(&mut app, target.clone()), reference);
        assert_eq!(config(&app).far_zoom, 1);
        assert_eq!(config(&app).shadow_enabled, 0);
        assert_eq!(config(&app).side_enabled, 0);
        assert_eq!(config(&app).bevel_enabled, 0);
        let mut cull = 0.0;
        let mut draw = 0.0;
        for _ in 0..12 {
            update_gpu(&mut app);
            assert_eq!(shadow_draws(&app), 0);
            assert_eq!(
                app.sub_app(RenderApp)
                    .world()
                    .resource::<GpuRenderer>()
                    .side_draws,
                0
            );
            assert_no_uploads(&app);
            cull += gpu_ms(&app, "puzzle_visibility");
            draw += gpu_ms(&app, "puzzle_draw");
        }
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert!(gpu.shadow_pipelines.is_empty());
        assert!(gpu.side_pipelines.is_empty());
        assert_eq!(gpu.buffers.as_ref().unwrap().drag_elevation_capacity, 0);
        assert_eq!(config(&app).drag_elevation_active, 0);
        assert_eq!(visible_ids(&app).len(), 1_000_000);
        bevy::log::info!(
            ?quality,
            visibility_ms = cull / 12.0,
            draw_ms = draw / 12.0,
            shadow_draws = shadow_draws(&app),
            side_draws = gpu.side_draws,
            bevel_enabled = config(&app).bevel_enabled,
            "million shadow LOD benchmark"
        );
    }
    // A million-member drag still advances only one envelope. Boundary mapping
    // upload is allowed; ordinary held and post-release frames must upload zero.
    app.insert_resource(ShadowTime(1.0))
        .add_systems(First, freeze_shadow_clock.after(update_rotation_clock));
    let mut members = jigsall_core::PieceBitSet::new(1_000_000);
    members.fill();
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .drag
        .members = members.words().clone();
    update_gpu(&mut app);
    app.world_mut().resource_mut::<ShadowTime>().0 = 1.1;
    for _ in 0..8 {
        update_gpu(&mut app);
    }
    assert_eq!(render_frame(&mut app, target.clone()), reference);
    assert_eq!(config(&app).drag_elevation_active, 1);
    let mut cull = 0.0;
    let mut draw = 0.0;
    for _ in 0..12 {
        update_gpu(&mut app);
        assert_no_uploads(&app);
        assert_eq!(shadow_draws(&app), 0);
        assert_eq!(
            app.sub_app(RenderApp)
                .world()
                .resource::<GpuRenderer>()
                .side_draws,
            0
        );
        cull += gpu_ms(&app, "puzzle_visibility");
        draw += gpu_ms(&app, "puzzle_draw");
    }
    bevy::log::info!(
        visibility_ms = cull / 12.0,
        draw_ms = draw / 12.0,
        "million drag elevation LOD benchmark"
    );
    app.world_mut().resource_mut::<PieceDataStore>().drag = default();
    app.world_mut().resource_mut::<ShadowTime>().0 = 2.0;
    update_gpu(&mut app);
    app.world_mut().resource_mut::<ShadowTime>().0 = 2.1;
    for _ in 0..8 {
        update_gpu(&mut app);
    }
    assert_eq!(config(&app).drag_elevation_active, 0);
    assert_eq!(render_frame(&mut app, target), reference);
    assert_no_uploads(&app);
}
