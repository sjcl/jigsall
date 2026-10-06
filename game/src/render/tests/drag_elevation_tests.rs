use super::shadow_tests::{
    assert_no_uploads, config, freeze_shadow_clock, red_source, render_frame, shadow_draws, turn,
    ShadowTime,
};
use super::side_tests::{preview_if_requested, set_visuals, side_draws};
use super::*;
use crate::resources::{
    drag_elevation::DragElevationUpload, remote_drag::RemoteDragPresentation,
    rotation_visual::update_rotation_clock,
};
use jigsall_core::PieceBitSet;

#[derive(Resource, Default)]
struct LiftReference(Option<f32>);
fn reference_offset(reference: Res<LiftReference>, mut frame: ResMut<ExtractedPuzzle>) {
    if let Some(elevation) = reference.0 {
        frame.config.drag_elevation_active = 0;
        frame.config.shadow_base_offset_px += elevation * frame.config.shadow_lift_offset_px;
        frame.config.shadow_lift_offset_px = 0.0;
    }
}
fn fixture(connected: bool) -> (App, Entity, Handle<Image>) {
    let (mut app, camera, target) = gpu_app(128);
    app.init_resource::<ShadowTime>()
        .add_systems(First, freeze_shadow_clock.after(update_rotation_clock))
        .insert_resource(ClearColor(Color::WHITE));
    app.sub_app_mut(RenderApp)
        .init_resource::<LiftReference>()
        .add_systems(ExtractSchedule, reference_offset.after(extract_puzzle));
    let mut def = definition(UVec2::new(2, 1), 80, 42);
    def.image_size.y = 40;
    app.insert_resource(def.clone());
    let offset = Vec2::splat(100.0);
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation = offset.extend(0.0);
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.initialize(if connected {
            (0..2)
                .map(|id| def.correct_position(PieceId(id)) + offset)
                .collect()
        } else {
            vec![offset - Vec2::X * 32.0, offset + Vec2::X * 32.0]
        });
        if connected {
            store.snap_fixture_component(PieceId(0), &def);
            assert_eq!(store.connectivity.component_size(PieceId(0)), 2);
        }
    }
    red_source(&mut app, 255);
    wait_ready(&mut app);
    (app, camera, target)
}
fn local_members(app: &mut App, ids: &[u32]) {
    let mut mask = PieceBitSet::new(2);
    mask.extend(ids.iter().copied().map(PieceId));
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .drag
        .members = if ids.is_empty() {
        Arc::default()
    } else {
        mask.words().clone()
    };
}
fn time(app: &mut App, now: f64) {
    app.world_mut().resource_mut::<ShadowTime>().0 = now;
}
fn smooth(t: f64) -> f32 {
    let t = t.clamp(0.0, 1.0) as f32;
    t * t * (3.0 - 2.0 * t)
}
fn compare_reference(app: &mut App, target: &Handle<Image>, elevation: f32) -> Vec<u8> {
    app.sub_app_mut(RenderApp)
        .world_mut()
        .resource_mut::<LiftReference>()
        .0 = None;
    let actual = render_frame(app, target.clone());
    app.sub_app_mut(RenderApp)
        .world_mut()
        .resource_mut::<LiftReference>()
        .0 = Some(elevation);
    let reference = render_frame(app, target.clone());
    assert!(
        actual == reference,
        "shadow separation elevation={elevation}"
    );
    app.sub_app_mut(RenderApp)
        .world_mut()
        .resource_mut::<LiftReference>()
        .0 = None;
    assert_no_uploads(app);
    actual
}

#[test]
fn drag_elevation_shader_is_shadow_only_and_reuses_uniform_and_storage_layouts() {
    assert_eq!(PuzzleUniform::min_size().get(), 336);
    let source = include_str!("../puzzle_render.wgsl");
    let compact: String = source.split_whitespace().collect();
    assert!(compact.contains(
        "ifshadow&&config.drag_elevation_active!=0u{elevation=max(elevation,drag_elevation(id));}"
    ));
    for name in ["../visibility.wgsl", "../pick_visibility.wgsl"] {
        let source = if name.contains("pick_") {
            include_str!("../pick_visibility.wgsl")
        } else {
            include_str!("../visibility.wgsl")
        };
        assert_eq!(source.matches("drag_elevation").count(), 2); // uniform layout only
    }
    let layout = PieceMetadataLayout {
        capacity: 1_000_000,
    };
    assert_eq!(layout.size(), 12_000_000);
    assert_eq!(
        layout.drag_elevation_range_offset(999_999, 1),
        Some(15_999_996)
    );
    assert_eq!(layout.drag_elevation_range_offset(999_999, 2), None);
    assert_eq!(layout.drag_elevation_records_offset(), 16_000_000);
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_drag_elevation_component_grab_release_smoothly_moves_only_shadow() {
    let (mut app, _, target) = fixture(true);
    app.insert_resource(PieceVisualQuality::High);
    let canonical = app.world().resource::<PieceDataStore>().states.clone();
    let base = compare_reference(&mut app, &target, 0.0);
    preview_if_requested("drag-base.png", &base);
    assert_eq!(config(&app).drag_elevation_active, 0);
    local_members(&mut app, &[0, 1]);
    update_gpu(&mut app);
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert_eq!(gpu.upload_bytes, 0);
    assert_eq!(gpu.drag_upload_bytes, 4);
    assert_eq!(gpu.drag_elevation_upload_bytes, 24); // one shared record + two slots
    let buffers = gpu.buffers.as_ref().unwrap();
    assert_eq!(
        read_buffer_range(&app, &buffers.piece_metadata, 24, 8),
        bytemuck::cast_slice::<u32, u8>(&[1, 1])
    );
    for now in [0.0, 0.02, 0.04, 0.06, 0.08, 0.5] {
        time(&mut app, now);
        let pixels = compare_reference(&mut app, &target, smooth(now / 0.08));
        if now == 0.0 {
            assert!(pixels == base);
        }
        assert_eq!((shadow_draws(&app), side_draws(&app)), (2, 1));
    }
    let held = compare_reference(&mut app, &target, 1.0);
    preview_if_requested("drag-held.png", &held);
    local_members(&mut app, &[]);
    assert!(compare_reference(&mut app, &target, 1.0) == held);
    for now in [0.52, 0.54, 0.56, 0.581] {
        time(&mut app, now);
        let pixels = compare_reference(&mut app, &target, 1.0 - smooth((now - 0.5) / 0.08));
        if now == 0.54 {
            preview_if_requested("drag-release-midpoint.png", &pixels);
        }
        if now == 0.581 {
            assert!(pixels == base);
        }
    }
    assert_eq!(config(&app).drag_elevation_active, 0);
    assert_eq!(app.world().resource::<PieceDataStore>().states, canonical);
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_drag_elevation_side_bevel_alpha_depth_and_picking_are_unchanged() {
    let (mut app, _, target) = fixture(true);
    let mut visuals = PieceVisualQuality::High.resolve();
    visuals.shadow_enabled = false;
    set_visuals(&mut app, visuals);
    for (index, alpha) in [255, 128, 0].into_iter().enumerate() {
        let start = index as f64 * 4.0;
        red_source(&mut app, alpha);
        local_members(&mut app, &[]);
        time(&mut app, start + 1.0);
        render_frame(&mut app, target.clone()); // finish the previous fade
        time(&mut app, start + 2.0);
        let before = render_frame(&mut app, target.clone());
        let mut expected = Vec::new();
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            for rect in [
                Rect::new(44.0, 64.0, 45.0, 65.0),
                Rect::new(84.0, 64.0, 85.0, 65.0),
                Rect::new(24.0, 44.0, 104.0, 84.0),
            ] {
                expected.push(pick(&mut app, rect, mode));
            }
        }
        local_members(&mut app, &[0, 1]);
        render_frame(&mut app, target.clone());
        for elapsed in [0.04, 0.08, 0.5] {
            time(&mut app, start + 2.0 + elapsed);
            assert!(
                render_frame(&mut app, target.clone()) == before,
                "alpha={alpha}, elapsed={elapsed}"
            );
            assert_eq!(
                (config(&app).bevel_enabled, config(&app).side_enabled),
                (1, 1)
            );
            assert_eq!(shadow_draws(&app), 0);
            let mut index = 0;
            for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
                for rect in [
                    Rect::new(44.0, 64.0, 45.0, 65.0),
                    Rect::new(84.0, 64.0, 85.0, 65.0),
                    Rect::new(24.0, 44.0, 104.0, 84.0),
                ] {
                    assert_eq!(pick(&mut app, rect, mode), expected[index]);
                    index += 1;
                }
            }
            assert_no_uploads(&app);
        }
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_drag_elevation_rotation_uses_max_through_grab_and_release() {
    let (mut app, _, target) = fixture(true);
    app.insert_resource(PieceVisualQuality::High);
    turn(&mut app, 1);
    let mut release_from = 0.0;
    for now in [0.0, 0.03, 0.04, 0.06, 0.08, 0.09, 0.10, 0.12, 0.13, 0.171] {
        time(&mut app, now);
        if now == 0.03 {
            local_members(&mut app, &[0, 1]);
        }
        let drag = if now < 0.03 {
            0.0
        } else if now <= 0.09 {
            smooth((now - 0.03) / 0.08)
        } else {
            release_from * (1.0 - smooth((now - 0.09) / 0.08))
        };
        let rotation = app
            .world()
            .resource::<PieceDataStore>()
            .rotation_visual
            .animation(PieceId(0))
            .map_or(0.0, |animation| animation.elevation(now));
        let actual = compare_reference(&mut app, &target, rotation.max(drag));
        if now == 0.09 {
            release_from = drag;
            local_members(&mut app, &[]);
            assert!(compare_reference(&mut app, &target, rotation.max(drag)) == actual);
        }
    }
    assert_eq!(config(&app).drag_elevation_active, 0);
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_drag_elevation_remote_slot_reuse_preserves_metadata_and_fades_on_disconnect() {
    let (mut app, _, target) = fixture(false);
    app.insert_resource(PieceVisualQuality::High);
    let mut mask = PieceBitSet::new(2);
    mask.insert(PieceId(0));
    let slot = app
        .world_mut()
        .resource_mut::<RemoteDragPresentation>()
        .allocate(mask, Vec2::ZERO)
        .unwrap();
    render_frame(&mut app, target.clone());
    time(&mut app, 0.1);
    render_frame(&mut app, target.clone());
    {
        let mut remote = app.world_mut().resource_mut::<RemoteDragPresentation>();
        remote.release(slot);
        let mut mask = PieceBitSet::new(2);
        mask.insert(PieceId(1));
        assert_eq!(remote.allocate(mask, Vec2::ZERO), Some(slot));
    }
    render_frame(&mut app, target.clone());
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    let buffer = &gpu.buffers.as_ref().unwrap().piece_metadata;
    assert_eq!(
        read_buffer_range(&app, buffer, 0, 16),
        bytemuck::cast_slice::<u32, u8>(&[0, 1, 0, slot])
    );
    assert_eq!(
        read_buffer_range(&app, buffer, 24, 8),
        bytemuck::cast_slice::<u32, u8>(&[1, 2])
    );
    time(&mut app, 0.14);
    compare_reference(&mut app, &target, 0.5);
    time(&mut app, 0.2);
    let actual = render_frame(&mut app, target.clone());
    for (side, elevation) in [(0, 0.0), (1, 1.0)] {
        app.sub_app_mut(RenderApp)
            .world_mut()
            .resource_mut::<LiftReference>()
            .0 = Some(elevation);
        let expected = render_frame(&mut app, target.clone());
        for y in 0..128 {
            // The uniform reference also lifts the other piece. Keep its
            // protruding tab/shadow outside this piece's comparison region.
            for x in if side == 0 { 0..64 } else { 72..128 } {
                let offset = (y * 128 + x) * 4;
                assert_eq!(&actual[offset..offset + 4], &expected[offset..offset + 4]);
            }
        }
    }
    app.sub_app_mut(RenderApp)
        .world_mut()
        .resource_mut::<LiftReference>()
        .0 = None;
    let epoch = app.world().resource::<PieceDataStore>().epoch;
    app.world_mut()
        .resource_mut::<RemoteDragPresentation>()
        .reset(epoch, 2);
    render_frame(&mut app, target.clone());
    assert!(app.world().resource::<DragElevationUpload>().active);
    time(&mut app, 0.281);
    compare_reference(&mut app, &target, 0.0);
    assert!(!app.world().resource::<DragElevationUpload>().active);
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_drag_elevation_metadata_growth_keeps_preview_padding_out_of_root_region() {
    let (mut app, _, target) = fixture(true);
    local_members(&mut app, &[0, 1]);
    render_frame(&mut app, target);
    super::component_preview_tests::preview(&mut app, Rect::new(44.0, 64.0, 45.0, 65.0));
    let world = app.sub_app(RenderApp).world();
    let gpu = world.resource::<GpuRenderer>();
    let buffers = gpu.buffers.as_ref().unwrap();
    assert!(buffers.piece_metadata.size() > PieceMetadataLayout { capacity: 2 }.size());
    let device = world.resource::<RenderDevice>();
    let queue = world.resource::<RenderQueue>();
    let cache = world.resource::<PipelineCache>();
    let group = device.create_bind_group(
        "preview after metadata growth",
        &cache.get_bind_group_layout(&gpu.preview_layout),
        &BindGroupEntries::sequential((
            buffers.direct_hits.as_entire_buffer_binding(),
            buffers.root_binding(),
            buffers.preview.as_entire_buffer_binding(),
        )),
    );
    for (hits, expected) in [(4u32, 0u32), (1 << 31, 0), (5, 1), (2, 1)] {
        queue.write_buffer(&buffers.direct_hits, 0, bytemuck::bytes_of(&hits));
        queue.write_buffer(&buffers.preview, 0, bytemuck::bytes_of(&0u32));
        let mut encoder = device.create_command_encoder(&default());
        {
            let mut pass = encoder.begin_compute_pass(&default());
            pass.set_pipeline(
                cache
                    .get_compute_pipeline(gpu.preview_collapse.unwrap())
                    .unwrap(),
            );
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        queue.submit([encoder.finish()]);
        assert_eq!(
            read_buffer(&app, &buffers.preview, 4),
            bytemuck::bytes_of(&expected)
        );
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_drag_elevation_low_and_overview_keep_draw_upload_and_idle_fast_paths() {
    let (mut app, camera, target) = fixture(true);
    for (index, (quality, scale)) in [
        (PieceVisualQuality::Low, 1.0),
        (PieceVisualQuality::High, 16.0),
    ]
    .into_iter()
    .enumerate()
    {
        let start = index as f64 * 4.0;
        app.insert_resource(quality);
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(scale, scale, 1.0);
        local_members(&mut app, &[]);
        time(&mut app, start + 1.0);
        render_frame(&mut app, target.clone());
        time(&mut app, start + 2.0);
        let flat = render_frame(&mut app, target.clone());
        local_members(&mut app, &[0, 1]);
        render_frame(&mut app, target.clone());
        time(&mut app, start + 2.08);
        assert!(render_frame(&mut app, target.clone()) == flat);
        assert_eq!(config(&app).drag_elevation_active, 1);
        assert_eq!((shadow_draws(&app), side_draws(&app)), (0, 0));
        assert_no_uploads(&app);
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert!(gpu.shadow_pipelines.is_empty() && gpu.side_pipelines.is_empty());
    }
}
