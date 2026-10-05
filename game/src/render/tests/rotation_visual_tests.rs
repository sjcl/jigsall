use super::*;
use crate::resources::rotation_visual::update_rotation_clock;
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
        assert!(source.contains("rotation_slots[component_roots[id]]"));
        assert!(source.contains("rotation_animations[rotation_slot-1u]"));
        assert!(source.contains("presentation_splat_size("));
    }
    let draw = include_str!("../puzzle_render.wgsl");
    assert!(draw.contains("presentation_rotate(local,rotation)"));
    assert!(draw.contains("presentation_rotate(corners[vi]*splat_size*0.5,rotation)"));
    assert!(draw.contains("let rotation=pose.rotation"));
    assert!(draw.contains("fn point_fragment"));
    assert!(draw.contains("fn rectangle_fragment"));
}

#[derive(Resource, Default)]
struct VisualTime(f64);
fn freeze_clock(time: Res<VisualTime>, mut store: ResMut<PieceDataStore>) {
    store.rotation_visual.clock = time.0;
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
