use super::*;
use crate::resources::pieces::{local_rotation::PresentationPose, HELD};
use puzzella_core::PieceBitSet;
use std::collections::HashMap;

#[test]
#[ignore = "requires a real GPU"]
fn gpu_local_rotation_overrides_share_draw_culling_picking_and_restore_sparse_ranges() {
    for size in [128, 4] {
        let (mut app, _, target) = gpu_app(128);
        let mut def = definition(UVec2::splat(2), size, 42);
        def.image_size.y = size / 2;
        app.world_mut().insert_resource(def.clone());
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            store.initialize(vec![Vec2::splat(1000.0); 4]);
            for state in store.states.iter_mut().skip(1) {
                state.flags = 0;
            }
        }
        wait_ready(&mut app);
        update_gpu(&mut app);
        let canonical = app.world().resource::<PieceDataStore>().states.clone();
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .set_local_rotation(HashMap::from([(
                PieceId(0),
                PresentationPose {
                    position: Vec2::ZERO,
                    rotation: 1,
                },
            )]));
        update_gpu(&mut app);
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.upload_bytes, 16);
        assert_eq!(gpu.upload_calls, 1);
        assert_eq!(gpu.root_upload_bytes, 0);
        assert_eq!(gpu.drag_upload_bytes, 0);
        let config = app
            .sub_app(RenderApp)
            .world()
            .resource::<ExtractedPuzzle>()
            .config
            .clone();
        assert_eq!(config.far_zoom, u32::from(size == 4));
        let pixels = rendered_pixels(&mut app, target.clone());
        assert_eq!(
            &pixels[(64 * 128 + 64) * 4..(64 * 128 + 64) * 4 + 3],
            &[255; 3]
        );
        let probes = if size == 128 {
            vec![
                Vec2::new(0.5, 25.5),
                Vec2::new(25.5, 0.5),
                Vec2::new(0.5, 0.5),
            ]
        } else {
            vec![Vec2::new(0.5, -0.5)]
        };
        for world in probes {
            let expected = if size == 4
                || piece_signed_distance(
                    puzzella_core::rotate_quarter(world, 3),
                    def.image_size.as_vec2() / def.grid_size.as_vec2(),
                    piece_profiles(def.seed, def.grid_size, UVec2::ZERO),
                ) < 0.0
            {
                vec![PieceId(0)]
            } else {
                vec![]
            };
            let screen = Vec2::new(64.0 + world.x, 64.0 - world.y).floor();
            for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
                assert_eq!(
                    pick(
                        &mut app,
                        Rect::from_corners(screen, screen + Vec2::ONE),
                        mode
                    ),
                    expected
                );
            }
        }
        assert_eq!(app.world().resource::<PieceDataStore>().states, canonical);
        update_gpu(&mut app);
        assert_eq!(
            app.sub_app(RenderApp)
                .world()
                .resource::<GpuRenderer>()
                .upload_bytes,
            0
        );
        // Latest authority flags/Z survive a pending override. Local HELD pieces
        // remain unselectable; pointer translation adds to the rotated base.
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            store.states[0].flags |= HELD;
            store.states[0].z_order = 123;
            store.dirty_pieces.insert(PieceId(0));
            let mut members = PieceBitSet::new(4);
            members.insert(PieceId(0));
            store.drag.members = members.words().clone();
            store.drag.delta = Vec2::new(16.0, 0.0);
        }
        update_gpu(&mut app);
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.upload_bytes, 16);
        assert_eq!(
            app.world().resource::<PieceUpload>().ranges[0].states[0].z_order,
            123
        );
        assert_eq!(
            puzzella_core::decode_rotation(
                app.world().resource::<PieceUpload>().ranges[0].states[0].flags
            ),
            1
        );
        for x in [16.0, 23.0] {
            app.world_mut()
                .resource_mut::<PieceDataStore>()
                .drag
                .delta
                .x = x;
            update_gpu(&mut app);
            let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
            assert_eq!(gpu.upload_bytes, 0);
            assert_eq!(gpu.drag_upload_bytes, 0);
            assert_eq!(gpu.root_upload_bytes, 0);
        }
        let pixels = rendered_pixels(&mut app, target.clone());
        assert_eq!(
            &pixels[(64 * 128 + 87) * 4..(64 * 128 + 87) * 4 + 3],
            &[255; 3]
        );
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert!(pick(&mut app, Rect::new(87.0, 64.0, 88.0, 65.0), mode).is_empty());
        }
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            store.clear_local_rotation();
            store.drag = default();
        }
        update_gpu(&mut app);
        assert_eq!(
            app.sub_app(RenderApp)
                .world()
                .resource::<GpuRenderer>()
                .upload_bytes,
            16
        );
        let pixels = rendered_pixels(&mut app, target);
        assert!(pixels.chunks_exact(4).all(|p| p[..3] == [0; 3]));
    }
}
