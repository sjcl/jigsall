use super::*;
use crate::resources::rotation_visual::{
    update_rotation_clock, GpuRotationAnimation, RotationAnimation,
};
use jigsall_core::{
    protocol::{ComponentRef, PieceTarget},
    PieceCommand, LOCAL_PLAYER,
};

#[test]
fn continuous_presentation_is_shared_by_draw_and_both_visibility_paths() {
    for source in [
        include_str!("../puzzle_render.wgsl"),
        include_str!("../visibility.wgsl"),
        include_str!("../pick_visibility.wgsl"),
    ] {
        assert_eq!(
            source.matches("presentation_pose(state.position,").count(),
            1
        );
        assert!(source.contains("rotation_slot(component_root(id))"));
        assert!(source.contains("rotation_animations[animation_slot-1u]"));
        assert!(source.contains("presentation_splat_size("));
    }
    let draw = include_str!("../puzzle_render.wgsl");
    assert!(draw.contains("presentation_rotate(local,rotation)"));
    assert!(draw.contains("presentation_rotate(corners[vi]*splat_size*0.5,rotation)"));
    assert!(draw.contains("rotation=pose.rotation"));
    assert!(draw.contains("fn point_fragment"));
    assert!(draw.contains("fn rectangle_fragment"));
}

#[test]
fn quarter_turn_path_gates_rotation_metadata_and_continuous_math() {
    for source in [
        include_str!("../puzzle_render.wgsl"),
        include_str!("../visibility.wgsl"),
        include_str!("../pick_visibility.wgsl"),
    ] {
        // Metadata is loaded only inside the uniform gate, and pose/trig only
        // after a nonzero slot. Keep these performance guards explicit in WGSL.
        let compact: String = source.split_whitespace().collect();
        assert!(compact.contains(
            "ifconfig.rotation_active!=0u{animation_slot=rotation_slot(component_root(id));}"
        ));
        assert!(
            compact.contains(
                "ifanimation_slot!=0u{letanimation=rotation_animations[animation_slot-1u];"
            ) || compact.contains("}else{letanimation=rotation_animations[animation_slot-1u];")
        );
        let continuous = source
            .find("let animation=rotation_animations[animation_slot-1u];")
            .unwrap();
        let pose = source.find("let pose=presentation_pose(").unwrap();
        assert!(continuous < pose);
        assert!(source.contains("if animation_slot==0u {"));
        assert!(source.contains("quarter_splat_size(config.size,quarter,"));
        for (start, _) in source.match_indices("if animation_slot==0u {") {
            let body = &source[start + "if animation_slot==0u {".len()..];
            let mut depth = 1;
            let end = body
                .char_indices()
                .find_map(|(i, c)| {
                    match c {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    (depth == 0).then_some(i)
                })
                .unwrap();
            for continuous_only in [
                "rotation_elevation(",
                "rotation_progress(",
                "presentation_pose(",
                "rotation_animations[",
                "sin(",
                "cos(",
            ] {
                assert!(!body[..end].contains(continuous_only));
            }
        }
    }
    let shared = include_str!("../presentation.wgsl");
    let quarter_functions = shared.split("// Shared by normal/far draw").next().unwrap();
    for expensive in [
        "sin(",
        "cos(",
        "length(",
        "sqrt(",
        "rotation_progress(",
        "rotation_elevation(",
    ] {
        assert!(!quarter_functions.contains(expensive));
    }
}

#[test]
fn elevation_uses_one_progress_sample_without_new_bindings_or_varyings() {
    let shared = include_str!("../presentation.wgsl");
    let pose = shared.split("fn presentation_pose(").nth(1).unwrap();
    assert_eq!(pose.matches("rotation_progress(").count(), 1);
    assert_eq!(
        pose.matches("rotation_elevation(animation,progress)")
            .count(),
        1
    );
    assert!(shared.contains("duration:f32,start_elevation:f32,"));
    assert!(shared.contains("rotation:vec2<f32>,elevation:f32"));
    assert!(!shared.contains("@binding"));
    for (source, bindings) in [
        (include_str!("../puzzle_render.wgsl"), 13),
        (include_str!("../visibility.wgsl"), 10),
        (include_str!("../pick_visibility.wgsl"), 10),
    ] {
        assert_eq!(source.matches("@binding(").count(), bindings);
        // The scalar stays inside the continuous helper for future effects.
        assert!(!source.contains("elevation"));
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_rotation_elevation_matches_cpu_envelope_and_record_layout() {
    let (app, _, _) = gpu_app(128);
    let quarter = std::f32::consts::FRAC_PI_2;
    let mut inputs = Vec::<[f32; 12]>::new();
    let mut expected = Vec::new();
    for residual in [0.0, quarter / 9.0, quarter * 0.5, -quarter, quarter * 2.0] {
        for start_elevation in [0.0, 0.3, 0.8, 1.0] {
            for i in -1..=21 {
                let time = i as f32 / 20.0;
                let animation = GpuRotationAnimation {
                    residual,
                    duration: 1.0,
                    start_elevation,
                    ..default()
                };
                let mut input = [0.0; 12];
                input[..8].copy_from_slice(bytemuck::cast_slice(std::slice::from_ref(&animation)));
                input[8] = time;
                inputs.push(input);
                expected.push(
                    RotationAnimation {
                        pivot: Vec2::ZERO,
                        offset: Vec2::ZERO,
                        residual,
                        target_angle: 0.0,
                        start: 0.0,
                        duration: 1.0,
                        start_elevation,
                    }
                    .elevation(f64::from(time)),
                );
            }
        }
    }
    let mut source = include_str!("../presentation.wgsl")
        .lines()
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n");
    source.push_str(r#"
struct Sample { animation:RotationAnimation,time:vec4<f32> };
@group(0) @binding(0) var<storage,read> inputs:array<Sample>;
@group(0) @binding(1) var<storage,read_write> outputs:array<vec4<f32>>;
@compute @workgroup_size(64) fn test_elevation(@builtin(global_invocation_id) gid:vec3<u32>) {
    if gid.x>=arrayLength(&inputs) {return;}
    let sample=inputs[gid.x];
    let pose=presentation_pose(vec2(0.0),0u,false,vec2(0.0),0u,vec2(0.0),sample.animation,sample.time.x);
    outputs[gid.x]=vec4(pose.rotation,pose.elevation,0.0);
}
"#);
    let bytes = compute_output(
        &app,
        source,
        "test_elevation",
        bytemuck::cast_slice(&inputs),
        inputs.len() as u64 * 16,
        (inputs.len() as u32).div_ceil(64),
    );
    for (index, (actual, expected)) in bytemuck::cast_slice::<u8, [f32; 4]>(&bytes)
        .iter()
        .zip(expected)
        .enumerate()
    {
        let elevation = actual[2];
        assert!((0.0..=1.0).contains(&elevation));
        assert!(
            (elevation - expected).abs() < 1e-6,
            "sample {index}: {elevation} != {expected}"
        );
        if inputs[index][8] >= 1.0 {
            assert_eq!(elevation, 0.0);
        }
    }
}

#[derive(Resource, Default)]
struct VisualTime(f64);
fn freeze_clock(time: Res<VisualTime>, mut store: ResMut<PieceDataStore>) {
    store.rotation_visual.clock = time.0;
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_quarter_turn_unanimated_component_matches_idle_during_other_animation() {
    for far in [false, true] {
        let (mut app, camera, target) = gpu_app(128);
        app.init_resource::<VisualTime>()
            .add_systems(First, freeze_clock.after(update_rotation_clock));
        // Exercise different world-per-pixel scales along X and Y.
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(2.0, 1.0, 1.0);
        let mut def = definition(UVec2::new(2, 1), if far { 2 } else { 40 }, 42);
        def.image_size.y = if far { 8 } else { 10 };
        app.insert_resource(def.clone());
        for quarter in 0..4 {
            app.world_mut().resource_mut::<VisualTime>().0 = 0.0;
            {
                let mut store = app.world_mut().resource_mut::<PieceDataStore>();
                store.initialize(vec![Vec2::new(-32.0, -24.0), Vec2::new(32.0, 24.0)]);
                store.states[0].flags = jigsall_core::with_rotation(ENABLED, quarter);
            }
            wait_ready(&mut app);
            let before = rendered_pixels(&mut app, target.clone());
            let config = &app
                .sub_app(RenderApp)
                .world()
                .resource::<ExtractedPuzzle>()
                .config;
            assert_eq!(config.far_zoom, u32::from(far));
            assert_eq!(config.rotation_active, 0);
            assert_eq!(config.pixel_world_size, Vec2::new(2.0, 1.0));
            let center = Rect::new(48.0, 88.0, 49.0, 89.0);
            for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
                assert_eq!(pick(&mut app, center, mode), vec![PieceId(0)]);
            }
            {
                let mut store = app.world_mut().resource_mut::<PieceDataStore>();
                let cmd = PieceCommand::Rotate {
                    target: PieceTarget::Component(
                        ComponentRef::from_member(&store.connectivity, PieceId(1)).unwrap(),
                    ),
                    quarter_turns: 1,
                };
                let boundary = store.capture_rotation_command(&cmd, Some(&def));
                assert_eq!(
                    store
                        .apply_command(LOCAL_PLAYER, &cmd, Some(&def), LOCAL_PLAYER)
                        .rotated,
                    1
                );
                store.finish_rotation_boundary(boundary);
            }
            app.world_mut().resource_mut::<VisualTime>().0 = 0.060;
            let after = rendered_pixels(&mut app, target.clone());
            assert_eq!(
                app.sub_app(RenderApp)
                    .world()
                    .resource::<ExtractedPuzzle>()
                    .config
                    .rotation_active,
                1
            );
            // Bottom half contains only the unanimated component, including its
            // complete non-square shape/splat footprint, for all four quarters.
            assert_eq!(
                &before[64 * 128 * 4..],
                &after[64 * 128 * 4..],
                "far={far}, quarter={quarter}"
            );
            for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
                assert_eq!(pick(&mut app, center, mode), vec![PieceId(0)]);
            }
            assert_eq!(visible_ids(&app).len(), 2);
            let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
            assert_eq!(
                gpu.upload_bytes + gpu.root_upload_bytes + gpu.rotation_upload_bytes,
                0
            );
        }
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_continuous_rotation_normal_far_draw_point_rectangle_and_zero_piece_uploads() {
    for far in [false, true] {
        let (mut app, _, target) = gpu_app(256);
        app.init_resource::<VisualTime>()
            .add_systems(First, freeze_clock.after(update_rotation_clock));
        let mut def = definition(
            if far {
                UVec2::new(128, 1)
            } else {
                UVec2::new(2, 1)
            },
            if far { 128 } else { 80 },
            42,
        );
        def.image_size.y = if far { 1 } else { 20 };
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            store.initialize(
                (0..def.piece_count())
                    .map(|id| def.correct_position(PieceId(id as u32)))
                    .collect(),
            );
            if far {
                store.connectivity.union(PieceId(1), PieceId(2));
                store.connectivity.union(PieceId(0), PieceId(1));
                assert_eq!(store.connectivity.find_root(PieceId(0)), PieceId(1));
            }
            for id in 1..def.piece_count() {
                store.connectivity.union(PieceId(0), PieceId(id as u32));
            }
        }
        app.insert_resource(def.clone());
        wait_ready(&mut app);
        update_gpu(&mut app);
        let old = app.world().resource::<PieceDataStore>().states[0].position;
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            let cmd = PieceCommand::Rotate {
                target: PieceTarget::Component(
                    ComponentRef::from_member(&store.connectivity, PieceId(0)).unwrap(),
                ),
                quarter_turns: 1,
            };
            let boundary = store.capture_rotation_command(&cmd, Some(&def));
            assert_eq!(
                store
                    .apply_command(LOCAL_PLAYER, &cmd, Some(&def), LOCAL_PLAYER)
                    .rotated,
                def.piece_count()
            );
            store.finish_rotation_boundary(boundary);
        }
        update_gpu(&mut app);
        app.world_mut().resource_mut::<VisualTime>().0 = 0.060;
        update_gpu(&mut app);
        let halfway = Vec2::new(
            old.x * std::f32::consts::FRAC_1_SQRT_2,
            old.x * std::f32::consts::FRAC_1_SQRT_2,
        );
        let pixel = super::rotation_tests::screen_point(halfway, 256.0);
        let ids = pick(&mut app, pixel, SelectionMode::Point);
        assert!(!ids.is_empty(), "midpoint point pick, far={far}");
        assert!(!pick(&mut app, pixel, SelectionMode::Rectangle).is_empty());
        let pixels = rendered_pixels(&mut app, target.clone());
        let p = pixel.min.as_uvec2();
        let offset = ((p.y * 256 + p.x) * 4) as usize;
        assert!(
            pixels[offset..offset + 3].iter().any(|&c| c != 0),
            "main draw differs from pick"
        );
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.upload_bytes, 0);
        assert_eq!(gpu.root_upload_bytes, 0);
        assert_eq!(gpu.rotation_upload_bytes, 0);
        assert_eq!(gpu.remote_mapping_upload_bytes, 0);
        // Change only the former padding word at a fixed early progress. This
        // changes the envelope from 0.5 to 1, while every output pixel, visible
        // ID, and point/rectangle result must remain exactly the same.
        app.world_mut().resource_mut::<VisualTime>().0 = 0.030;
        let before = rendered_pixels(&mut app, target.clone());
        let visible = visible_ids(&app);
        let whole = Rect::new(0.0, 0.0, 256.0, 256.0);
        let rectangle = pick(&mut app, whole, SelectionMode::Rectangle);
        let drawn = before
            .chunks_exact(4)
            .position(|p| p[..3] == [255, 255, 255])
            .unwrap();
        let point = Rect::new(
            (drawn % 256) as f32,
            (drawn / 256) as f32,
            (drawn % 256 + 1) as f32,
            (drawn / 256 + 1) as f32,
        );
        let hit = pick(&mut app, point, SelectionMode::Point);
        assert!(!hit.is_empty());
        {
            let world = app.sub_app(RenderApp).world();
            let gpu = world.resource::<GpuRenderer>();
            world.resource::<RenderQueue>().write_buffer(
                &gpu.buffers.as_ref().unwrap().rotation_animations,
                std::mem::offset_of!(GpuRotationAnimation, start_elevation) as u64,
                bytemuck::bytes_of(&1.0_f32),
            );
        }
        assert_eq!(
            rendered_pixels(&mut app, target.clone()),
            before,
            "elevation affected draw, far={far}"
        );
        let mut after_visible = visible_ids(&app);
        let mut before_visible = visible;
        after_visible.sort_unstable();
        before_visible.sort_unstable();
        assert_eq!(after_visible, before_visible);
        assert_eq!(pick(&mut app, whole, SelectionMode::Rectangle), rectangle);
        assert_eq!(pick(&mut app, point, SelectionMode::Point), hit);
        app.world_mut().resource_mut::<VisualTime>().0 = 0.120;
        update_gpu(&mut app);
        assert!(
            !app.sub_app(RenderApp)
                .world()
                .resource::<ExtractedPuzzle>()
                .rotation
                .active
        );
        assert!(
            pick(&mut app, pixel, SelectionMode::Point).is_empty(),
            "pick snapped to final pose early or midpoint leaked"
        );
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_continuous_rotation_elongated_far_splat_rotates_its_footprint() {
    let (mut app, _, target) = gpu_app(256);
    app.init_resource::<VisualTime>()
        .add_systems(First, freeze_clock.after(update_rotation_clock));
    let mut def = definition(UVec2::ONE, 1, 42);
    def.image_size.y = 30;
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO]);
    app.insert_resource(def.clone());
    wait_ready(&mut app);
    update_gpu(&mut app);
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        let cmd = PieceCommand::Rotate {
            target: PieceTarget::Component(
                ComponentRef::from_member(&store.connectivity, PieceId(0)).unwrap(),
            ),
            quarter_turns: 1,
        };
        let boundary = store.capture_rotation_command(&cmd, Some(&def));
        assert_eq!(
            store
                .apply_command(LOCAL_PLAYER, &cmd, Some(&def), LOCAL_PLAYER)
                .rotated,
            1
        );
        store.finish_rotation_boundary(boundary);
    }
    app.world_mut().resource_mut::<VisualTime>().0 = 0.060;
    update_gpu(&mut app);
    let inside = super::rotation_tests::screen_point(Vec2::new(-5.0, 5.0), 256.0);
    let outside = super::rotation_tests::screen_point(Vec2::new(7.0, 0.0), 256.0);
    assert_eq!(
        pick(&mut app, inside, SelectionMode::Point),
        vec![PieceId(0)]
    );
    assert!(pick(&mut app, outside, SelectionMode::Point).is_empty());
    assert!(pick(&mut app, outside, SelectionMode::Rectangle).is_empty());
    let pixels = rendered_pixels(&mut app, target);
    for (rect, visible) in [(inside, true), (outside, false)] {
        let p = rect.min.as_uvec2();
        let offset = ((p.y * 256 + p.x) * 4) as usize;
        assert_eq!(pixels[offset..offset + 3].iter().any(|&c| c != 0), visible);
    }
}
