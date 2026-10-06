use super::*;
use crate::resources::{pieces::HELD, remote_drag::RemoteDragPresentation};
use jigsall_core::PieceBitSet;

#[test]
fn remote_presentation_shader_paths_share_the_position_function() {
    for source in [
        include_str!("../puzzle_render.wgsl"),
        include_str!("../visibility.wgsl"),
        include_str!("../pick_visibility.wgsl"),
    ] {
        assert!(source.contains("#import jigsall::presentation::{presentation_pose"));
        assert_eq!(
            source.matches("presentation_pose(state.position,").count(),
            1
        );
        assert!(!source.contains("position+=config.drag_delta"));
    }
}

#[test]
#[ignore = "requires a GPU"]
fn gpu_remote_presentation_normal_far_culling_picking_and_scalar_uploads() {
    for size in [32, 2] {
        // 16px normal pieces and 1px far splats
        let (mut app, _, target) = gpu_app(128);
        app.world_mut()
            .insert_resource(definition(UVec2::splat(2), size, 42));
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            store.initialize(vec![Vec2::splat(1000.0); 4]);
            for state in store.states.iter_mut().take(2) {
                state.flags |= HELD;
            }
            for state in store.states.iter_mut().skip(2) {
                state.flags = 0;
            }
        }
        let epoch = app.world().resource::<PieceDataStore>().epoch;
        let slots = {
            let mut presentation = app.world_mut().resource_mut::<RemoteDragPresentation>();
            presentation.reset(epoch, 4);
            let mut a = PieceBitSet::new(4);
            a.insert(PieceId(0));
            let mut b = PieceBitSet::new(4);
            b.insert(PieceId(1));
            [
                presentation
                    .allocate(a, Vec2::new(-1032.0, -1000.0))
                    .unwrap(),
                presentation
                    .allocate(b, Vec2::new(-968.0, -1000.0))
                    .unwrap(),
            ]
        };
        wait_ready(&mut app);
        for _ in 0..4 {
            update_gpu(&mut app);
        }
        assert_eq!(
            app.sub_app(RenderApp)
                .world()
                .resource::<ExtractedPuzzle>()
                .config
                .far_zoom,
            u32::from(size == 2)
        );
        let pixels = rendered_pixels(&mut app, target.clone());
        for x in [32, 96] {
            assert_eq!(
                &pixels[(64 * 128 + x) * 4..(64 * 128 + x) * 4 + 3],
                &[255; 3]
            );
            for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
                assert!(
                    pick(
                        &mut app,
                        Rect::new(x as f32, 64.0, x as f32 + 1.0, 65.0),
                        mode
                    )
                    .is_empty(),
                    "remote-held must remain unselectable"
                );
            }
        }
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(
            u32::from_le_bytes(
                read_buffer(&app, &gpu.buffers.as_ref().unwrap().args, 16)[4..8]
                    .try_into()
                    .unwrap()
            ),
            2
        );
        assert_eq!(
            read_buffer_range(
                &app,
                &gpu.buffers.as_ref().unwrap().piece_metadata,
                PieceMetadataLayout { capacity: 4 }
                    .remote_range_offset(0, 4)
                    .unwrap(),
                16,
            ),
            bytemuck::cast_slice::<u32, u8>(&[slots[0], slots[1], 0, 0])
        );
        let canonical = app.world().resource::<PieceDataStore>().states.clone();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs_f64(1.0 / 60.0),
        ));
        app.world_mut()
            .resource_mut::<RemoteDragPresentation>()
            .set_target(slots[0], Vec2::new(-1032.0, -984.0));
        for _ in 0..3 {
            update_gpu(&mut app);
            let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
            assert_eq!(gpu.upload_bytes, 0);
            assert_eq!(gpu.drag_upload_bytes, 0);
            assert_eq!(gpu.root_upload_bytes, 0);
            assert_eq!(gpu.selection_upload_bytes, 0);
            assert_eq!(gpu.remote_mapping_upload_bytes, 0);
            assert_eq!(gpu.rotation_upload_bytes, 0);
            assert_eq!(gpu.remote_mapping_upload_calls, 0);
            assert_eq!(gpu.remote_delta_upload_bytes, 512);
            let presentation = app.world().resource::<RemoteDragPresentation>();
            let displayed = presentation.offset(PieceId(0));
            assert!(displayed.y > -1000.0 && displayed.y < -984.0);
            assert_eq!(presentation.target(PieceId(0)), Vec2::new(-1032.0, -984.0));
            let raw = read_buffer(&app, &gpu.buffers.as_ref().unwrap().remote_deltas, 512);
            assert_eq!(
                &raw[..8],
                bytemuck::cast_slice::<f32, u8>(&displayed.to_array())
            );
            assert_eq!(app.world().resource::<PieceDataStore>().states, canonical);
        }
        // Freeze only fixture frame time to read/pick the intermediate display.
        let displayed = app
            .world()
            .resource::<RemoteDragPresentation>()
            .offset(PieceId(0));
        let y = (64.0 - (1000.0 + displayed.y)).floor() as usize;
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::ZERO,
        ));
        let pixels = rendered_pixels(&mut app, target.clone());
        assert_eq!(
            &pixels[(y * 128 + 32) * 4..(y * 128 + 32) * 4 + 3],
            &[255; 3]
        );
        if size == 2 {
            assert_eq!(
                &pixels[(48 * 128 + 32) * 4..(48 * 128 + 32) * 4 + 3],
                &[0; 3]
            );
        }
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert!(pick(
                &mut app,
                Rect::new(32.0, y as f32, 33.0, y as f32 + 1.0),
                mode
            )
            .is_empty());
        }
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs_f64(1.0 / 60.0),
        ));
        for _ in 0..20 {
            update_gpu(&mut app);
        }
        assert_eq!(
            app.world()
                .resource::<RemoteDragPresentation>()
                .offset(PieceId(0)),
            Vec2::new(-1032.0, -984.0)
        );
        let pixels = rendered_pixels(&mut app, target.clone());
        assert_eq!(
            &pixels[(48 * 128 + 32) * 4..(48 * 128 + 32) * 4 + 3],
            &[255; 3]
        );
        update_gpu(&mut app);
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.remote_mapping_upload_bytes, 0);
        assert_eq!(gpu.remote_delta_upload_bytes, 0);
        app.world_mut()
            .resource_mut::<RemoteDragPresentation>()
            .release(slots[0]);
        let pixels = rendered_pixels(&mut app, target.clone());
        assert_eq!(
            &pixels[(48 * 128 + 32) * 4..(48 * 128 + 32) * 4 + 3],
            &[0; 3]
        );
        app.world_mut()
            .resource_mut::<RemoteDragPresentation>()
            .reset(epoch, 4);
        let pixels = rendered_pixels(&mut app, target);
        assert_eq!(
            &pixels[(64 * 128 + 96) * 4..(64 * 128 + 96) * 4 + 3],
            &[0; 3]
        );
    }
}
