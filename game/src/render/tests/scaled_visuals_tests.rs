use super::drag_elevation_tests::{compare_reference, install_reference};
use super::shadow_tests::{
    assert_no_uploads, config, freeze_shadow_clock, red_source, render_frame, shadow_draws, turn,
    ShadowTime,
};
use super::side_tests::{clear_visuals, set_visuals, side_draws};
use super::*;
use crate::resources::rotation_visual::update_rotation_clock;
use jigsall_core::PieceBitSet;
use visuals::{FramePieceVisuals, ProjectedDimension};

fn dimensions(frame: FramePieceVisuals) -> [f32; 4] {
    [
        frame.side_thickness_px,
        frame.bevel_width_px,
        frame.shadow_base_offset_px,
        frame.shadow_lift_offset_px,
    ]
}
fn close(actual: [f32; 4], expected: [f32; 4]) {
    for (a, b) in actual.into_iter().zip(expected) {
        assert!(
            (a - b).abs() < 1e-5,
            "actual={actual:?} expected={expected:?}"
        );
    }
}

#[test]
fn scaled_visuals_high_and_medium_match_short_edge_clamps() {
    for (size, high, medium) in [
        (20.0, [1.0, 1.0, 2.0, 3.0], [0.0, 0.0, 1.5, 2.0]),
        (50.0, [1.25, 1.0, 2.0, 3.0], [0.8, 0.75, 1.5, 2.0]),
        (100.0, [2.5, 2.0, 4.0, 6.0], [1.6, 1.25, 3.0, 4.0]),
        (200.0, [4.0, 3.0, 6.0, 9.0], [2.5, 2.0, 4.5, 6.0]),
    ] {
        for (quality, expected) in [
            (PieceVisualQuality::High, high),
            (PieceVisualQuality::Medium, medium),
        ] {
            let preset = quality.resolve();
            for projected in [Vec2::new(size, 2000.0), Vec2::new(2000.0, size)] {
                let frame = preset.for_frame(projected, false);
                close(dimensions(frame), expected);
                if frame.bevel_enabled {
                    assert_eq!(
                        frame.bevel_highlight_strength,
                        preset.bevel_highlight_strength
                    );
                    assert_eq!(frame.bevel_shadow_strength, preset.bevel_shadow_strength);
                }
            }
        }
    }
}

#[test]
fn scaled_visuals_zoom_is_monotone_continuous_and_bounded_while_enabled() {
    for quality in [PieceVisualQuality::Medium, PieceVisualQuality::High] {
        let preset = quality.resolve();
        let rules = [
            preset.side_thickness,
            preset.bevel_width,
            preset.shadow_base_offset,
            preset.shadow_lift_offset,
        ];
        let mut previous = dimensions(preset.for_frame(Vec2::splat(30.0), false));
        for step in 301..=100_000 {
            let size = step as f32 * 0.1;
            let current = dimensions(preset.for_frame(Vec2::splat(size), false));
            for i in 0..4 {
                assert!(current[i] >= rules[i].min_px && current[i] <= rules[i].max_px);
                assert!(current[i] >= previous[i]);
                assert!(current[i] - previous[i] <= rules[i].scale * 0.101 + 1e-5);
            }
            previous = current;
        }
        close(previous, rules.map(|r| r.max_px));
    }
}

#[test]
fn scaled_visuals_lod_low_and_far_keep_optional_values_completely_off() {
    for quality in [PieceVisualQuality::Medium, PieceVisualQuality::High] {
        let preset = quality.resolve();
        for (threshold, feature) in [
            (preset.shadow_min_piece_px, 0),
            (preset.side_min_piece_px, 1),
            (preset.bevel_min_piece_px, 2),
        ] {
            for (size, enabled) in [(threshold - 0.01, false), (threshold, true)] {
                let frame = preset.for_frame(Vec2::new(200.0, size), false);
                assert_eq!(
                    [
                        frame.shadow_enabled,
                        frame.side_enabled,
                        frame.bevel_enabled
                    ][feature],
                    enabled
                );
                if !enabled {
                    match feature {
                        0 => assert_eq!(
                            (
                                frame.shadow_base_offset_px,
                                frame.shadow_lift_offset_px,
                                frame.shadow_opacity
                            ),
                            (0.0, 0.0, 0.0)
                        ),
                        1 => assert_eq!((frame.side_thickness_px, frame.side_opacity), (0.0, 0.0)),
                        _ => assert_eq!(
                            (
                                frame.bevel_width_px,
                                frame.bevel_highlight_strength,
                                frame.bevel_shadow_strength
                            ),
                            (0.0, 0.0, 0.0)
                        ),
                    }
                }
            }
        }
        let far = preset.for_frame(Vec2::splat(200.0), true);
        assert!(!far.bevel_enabled);
        assert_eq!(far.bevel_width_px, 0.0);
        let mut uniform = PuzzleUniform {
            piece_size_px: Vec2::splat(200.0),
            viewport_size: Vec2::splat(256.0),
            ..default()
        };
        uniform.configure_visuals(preset);
        assert!(uniform.visual_cull_extent != Vec2::ZERO);
        for (quality, size) in [(PieceVisualQuality::Low, 200.0), (quality, 0.25)] {
            uniform.piece_size_px = Vec2::splat(size);
            uniform.configure_visuals(quality.resolve());
            assert_eq!(
                (
                    uniform.shadow_enabled,
                    uniform.side_enabled,
                    uniform.bevel_enabled
                ),
                (0, 0, 0)
            );
            assert_eq!(
                (
                    uniform.shadow_base_offset_px,
                    uniform.shadow_lift_offset_px,
                    uniform.side_thickness_px,
                    uniform.bevel_width_px
                ),
                (0.0, 0.0, 0.0, 0.0)
            );
            assert_eq!(uniform.visual_cull_extent, Vec2::ZERO);
        }
    }
    assert_eq!(PuzzleUniform::min_size().get(), 336);
}

#[test]
fn scaled_visuals_elevation_only_multiplies_resolved_shadow_lift_and_bounds_cover_peak() {
    for size in [20.0, 50.0, 100.0, 200.0] {
        let preset = PieceVisualQuality::High.resolve();
        let frame = preset.for_frame(Vec2::splat(size), false);
        let mut uniform = PuzzleUniform {
            clip_from_world: Mat4::IDENTITY,
            viewport_size: Vec2::splat(256.0),
            piece_size_px: Vec2::splat(size),
            ..default()
        };
        for (rotation, drag) in [(0.0_f32, 0.0_f32), (0.5, 0.25), (0.25, 0.5), (1.0, 1.0)] {
            uniform.rotation_active = u32::from(rotation != 0.0);
            uniform.drag_elevation_active = u32::from(drag != 0.0);
            uniform.configure_visuals(preset);
            let elevation = rotation.max(drag);
            let expected = PSEUDO_3D_DIRECTION
                * (frame.shadow_base_offset_px + elevation * frame.shadow_lift_offset_px);
            assert_eq!(frame.shadow_offset_px(elevation), expected);
            assert_eq!(uniform.side_thickness_px, frame.side_thickness_px);
            assert_eq!(uniform.bevel_width_px, frame.bevel_width_px);
            let peak = (frame.shadow_base_offset_px + frame.shadow_lift_offset_px)
                .max(frame.side_thickness_px);
            assert!(uniform
                .visual_cull_extent
                .abs_diff_eq(PSEUDO_3D_DIRECTION * peak / 128.0, 1e-6));
        }
    }
}

fn fixture() -> (App, Entity, Handle<Image>) {
    let (mut app, camera, target) = gpu_app(256);
    app.insert_resource(ClearColor(Color::WHITE));
    red_source(&mut app, 255);
    let source = app.world().resource::<PuzzleImage>().handle.clone();
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .get_mut(&source)
        .unwrap()
        .data = Some(vec![128, 128, 128, 255]);
    app.insert_resource(definition(UVec2::ONE, 200, 42));
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO]);
    wait_ready(&mut app);
    (app, camera, target)
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_scaled_visuals_zoom_matches_fixed_pixel_reference_without_metadata_uploads() {
    let (mut app, camera, target) = fixture();
    let metadata_size = app
        .sub_app(RenderApp)
        .world()
        .resource::<GpuRenderer>()
        .buffers
        .as_ref()
        .unwrap()
        .piece_metadata
        .size();
    for (size, high, medium) in [
        (20.0, [1.0, 1.0, 2.0, 3.0], [0.0, 0.0, 1.5, 2.0]),
        (50.0, [1.25, 1.0, 2.0, 3.0], [0.8, 0.75, 1.5, 2.0]),
        (100.0, [2.5, 2.0, 4.0, 6.0], [1.6, 1.25, 3.0, 4.0]),
        (200.0, [4.0, 3.0, 6.0, 9.0], [2.5, 2.0, 4.5, 6.0]),
    ] {
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale =
            Vec3::new(200.0 / size, 200.0 / size, 1.0);
        clear_visuals(&mut app);
        app.insert_resource(PieceVisualQuality::Low);
        let flat = render_frame(&mut app, target.clone());
        let center = Rect::new(128.0, 128.0, 129.0, 129.0);
        let point = pick(&mut app, center, SelectionMode::Point);
        let rectangle = pick(&mut app, center, SelectionMode::Rectangle);
        for (quality, expected) in [
            (PieceVisualQuality::Low, [0.0; 4]),
            (PieceVisualQuality::Medium, medium),
            (PieceVisualQuality::High, high),
        ] {
            let preset = quality.resolve();
            clear_visuals(&mut app);
            app.insert_resource(quality);
            let actual = render_frame(&mut app, target.clone());
            let uniform = config(&app);
            close(
                [
                    uniform.side_thickness_px,
                    uniform.bevel_width_px,
                    uniform.shadow_base_offset_px,
                    uniform.shadow_lift_offset_px,
                ],
                expected,
            );
            assert_eq!(
                shadow_draws(&app),
                usize::from(uniform.shadow_enabled != 0) * 2
            );
            assert_eq!(side_draws(&app), usize::from(uniform.side_enabled != 0));
            assert_no_uploads(&app);
            assert_eq!(
                app.sub_app(RenderApp)
                    .world()
                    .resource::<GpuRenderer>()
                    .buffers
                    .as_ref()
                    .unwrap()
                    .piece_metadata
                    .size(),
                metadata_size
            );
            assert_eq!(pick(&mut app, center, SelectionMode::Point), point);
            assert_eq!(pick(&mut app, center, SelectionMode::Rectangle), rectangle);
            if quality == PieceVisualQuality::Low {
                assert!(actual == flat);
            }
            let mut fixed = preset;
            fixed.side_thickness = ProjectedDimension::fixed(expected[0]);
            fixed.bevel_width = ProjectedDimension::fixed(expected[1]);
            fixed.shadow_base_offset = ProjectedDimension::fixed(expected[2]);
            fixed.shadow_lift_offset = ProjectedDimension::fixed(expected[3]);
            set_visuals(&mut app, fixed);
            assert!(
                render_frame(&mut app, target.clone()) == actual,
                "pixel scale reference {quality:?} size={size}"
            );
            if let Some(directory) = std::env::var_os("JIGSALL_VISUAL_PREVIEW_DIR") {
                std::fs::create_dir_all(&directory).unwrap();
                image::save_buffer_with_format(
                    std::path::PathBuf::from(directory)
                        .join(format!("scaled-{quality:?}-{size:.0}px.png")),
                    &actual,
                    256,
                    256,
                    image::ColorType::Rgba8,
                    image::ImageFormat::Png,
                )
                .unwrap();
            }
        }
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_scaled_visuals_drag_rotation_max_moves_only_shadow_at_each_zoom() {
    for size in [20.0, 50.0, 100.0, 200.0] {
        let (mut app, camera, target) = fixture();
        install_reference(&mut app);
        app.init_resource::<ShadowTime>()
            .add_systems(First, freeze_shadow_clock.after(update_rotation_clock));
        app.insert_resource(PieceVisualQuality::High);
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale =
            Vec3::new(200.0 / size, 200.0 / size, 1.0);
        render_frame(&mut app, target.clone());
        turn(&mut app, 1);
        for now in [0.0, 0.03, 0.06, 0.09, 0.12, 0.15, 0.18] {
            app.world_mut().resource_mut::<ShadowTime>().0 = now;
            if now == 0.03 {
                let mut members = PieceBitSet::new(1);
                members.insert(PieceId(0));
                app.world_mut()
                    .resource_mut::<PieceDataStore>()
                    .drag
                    .members = members.words().clone();
            }
            let smooth = |t: f64| {
                let t = t.clamp(0.0, 1.0) as f32;
                t * t * (3.0 - 2.0 * t)
            };
            let drag = if now < 0.03 {
                0.0
            } else if now <= 0.09 {
                smooth((now - 0.03) / 0.08)
            } else {
                smooth(0.75) * (1.0 - smooth((now - 0.09) / 0.08))
            };
            let rotation = app
                .world()
                .resource::<PieceDataStore>()
                .rotation_visual
                .animation(PieceId(0))
                .map_or(0.0, |a| a.elevation(now));
            let actual = compare_reference(&mut app, &target, rotation.max(drag));
            // The reference render temporarily replaces base/lift. Restore the
            // actual frame before inspecting its resolved dimensions.
            assert!(render_frame(&mut app, target.clone()) == actual);
            let uniform = config(&app);
            let expected = PieceVisualQuality::High
                .resolve()
                .for_frame(Vec2::splat(size), false);
            close(
                [
                    uniform.side_thickness_px,
                    uniform.bevel_width_px,
                    uniform.shadow_base_offset_px,
                    uniform.shadow_lift_offset_px,
                ],
                dimensions(expected),
            );
            if now == 0.09 {
                app.world_mut()
                    .resource_mut::<PieceDataStore>()
                    .drag
                    .members = Arc::default();
                assert!(compare_reference(&mut app, &target, rotation.max(drag)) == actual);
            }
        }
        assert_eq!(config(&app).drag_elevation_active, 0);
    }
}
