use super::*;
use crate::{
    render::remote_cursor::*,
    resources::{remote_cursor::RemoteCursorPresentation, GameSubState},
};
use ab_glyph::{Font, FontRef, ScaleFont};
use jigsall_core::PlayerId;
use std::collections::BTreeMap;

fn japanese_atlas(app: &mut App, scale: f32) -> Arc<RemoteCursorLabelAtlas> {
    // Tests read the same asset from disk; the application embeds it only once.
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ui/fonts/MPLUS1p-Regular.ttf"
    ))
    .unwrap();
    let font = FontRef::try_from_slice(&bytes).unwrap();
    let scaled = font.as_scaled(12.0 * scale);
    let width = (16.0 * scale) as u32;
    let height = (16.0 * scale) as u32;
    let glyph = scaled.glyph_id('山').with_scale_and_position(
        scaled.scale(),
        ab_glyph::point(2.0 * scale, scaled.ascent()),
    );
    let outline = font.outline_glyph(glyph).unwrap();
    let bounds = outline.px_bounds();
    let mut bitmap = vec![0u8; (width * height) as usize];
    outline.draw(|x, y, alpha| {
        let x = x + bounds.min.x as u32;
        let y = y + bounds.min.y as u32;
        if x < width && y < height {
            bitmap[(y * width + x) as usize] = (alpha * 255.0) as u8;
        }
    });
    assert!(bitmap.iter().any(|&v| v > 0));
    let image = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new(
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            bitmap,
            TextureFormat::R8Unorm,
            RenderAssetUsages::RENDER_WORLD,
        ));
    Arc::new(RemoteCursorLabelAtlas {
        revision: 1,
        image,
        labels: BTreeMap::from([(
            PlayerId(1),
            CursorLabel {
                uv: Vec4::new(0.0, 0.0, 1.0, 1.0),
                logical_size: Vec2::splat(16.0),
            },
        )]),
    })
}

fn setup_cursors(app: &mut App, scale: f32) {
    app.world_mut()
        .insert_resource(State::new(GameSubState::Playing));
    app.world_mut().init_resource::<RemoteCursorPresentation>();
    let atlas = japanese_atlas(app, scale);
    app.world_mut().insert_resource(RemoteCursorLabels {
        members: Arc::from([PlayerId(0), PlayerId(1), PlayerId(2)]),
        atlas: Some(atlas),
        scale_factor: scale,
    });
    let mut presentation = app.world_mut().resource_mut::<RemoteCursorPresentation>();
    presentation.set_target(PlayerId(0), Some(Vec2::new(-80.0, 60.0))); // own
    presentation.set_target(PlayerId(1), Some(Vec2::ZERO));
    presentation.set_target(PlayerId(2), Some(Vec2::new(140.0, 0.0))); // label would protrude from left edge later
}

fn no_piece_uploads(app: &App) {
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert_eq!(
        (
            gpu.upload_bytes,
            gpu.root_upload_bytes,
            gpu.selection_upload_bytes,
            gpu.drag_upload_bytes,
            gpu.remote_mapping_upload_bytes,
            gpu.remote_delta_upload_bytes
        ),
        (0, 0, 0, 0, 0, 0)
    );
}

fn projected(frame: &ExtractedPuzzle, world: Vec2) -> Vec2 {
    let p = frame
        .config
        .clip_from_world
        .project_point3(world.extend(0.0))
        .truncate();
    frame.config.viewport_origin
        + (p * Vec2::new(0.5, -0.5) + Vec2::splat(0.5)) * frame.config.viewport_size
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_remote_cursor_same_frame_pan_zoom_marker_label_hidpi_and_offscreen() {
    for scale in [1.0, 2.0] {
        let resolution = (256.0 * scale) as u32;
        let (mut app, camera, target) = gpu_app(resolution);
        app.world_mut()
            .insert_resource(definition(UVec2::ONE, 64, 42));
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .initialize(vec![Vec2::ZERO]);
        setup_cursors(&mut app, scale);
        wait_ready(&mut app);
        for _ in 0..5 {
            update_gpu(&mut app);
        }
        let atlas_id = app
            .world()
            .resource::<RemoteCursorLabels>()
            .atlas
            .as_ref()
            .unwrap()
            .image
            .id();
        let atlas_texture = app
            .sub_app(RenderApp)
            .world()
            .resource::<RenderAssets<GpuImage>>()
            .get(atlas_id)
            .unwrap()
            .texture
            .id();
        for (pan, zoom) in [
            (Vec2::ZERO, 1.0),
            (Vec2::new(22.0, -13.0), 0.5),
            (Vec2::new(-19.0, 16.0), 2.0),
        ] {
            app.world_mut().get_mut::<Camera>(camera).unwrap().viewport =
                (zoom == 2.0).then_some(bevy::camera::Viewport {
                    physical_position: UVec2::new((32.0 * scale) as u32, (24.0 * scale) as u32),
                    physical_size: UVec2::new((192.0 * scale) as u32, (160.0 * scale) as u32),
                    ..default()
                });
            app.world_mut()
                .get_mut::<Transform>(camera)
                .unwrap()
                .translation = pan.extend(0.0);
            {
                let mut projection = app.world_mut().get_mut::<Projection>(camera).unwrap();
                if let Projection::Orthographic(p) = &mut *projection {
                    p.scale = zoom;
                } else {
                    panic!();
                }
            }
            // Exact frame N camera update; direct readback cannot advance to N+1.
            update_gpu(&mut app);
            let frame = app.sub_app(RenderApp).world().resource::<ExtractedPuzzle>();
            let cursor_gpu = app.sub_app(RenderApp).world().resource::<CursorRenderer>();
            assert_eq!(
                cursor_gpu.view.get().clip_from_world,
                frame.config.clip_from_world
            );
            assert_eq!(
                cursor_gpu.view.get().viewport_size,
                frame.config.viewport_size
            );
            assert_eq!(cursor_gpu.instance_upload_bytes, 0);
            assert_eq!(
                app.sub_app(RenderApp)
                    .world()
                    .resource::<RenderAssets<GpuImage>>()
                    .get(atlas_id)
                    .unwrap()
                    .texture
                    .id(),
                atlas_texture
            );
            assert_eq!(cursor_gpu.previous.len(), 2); // own player filtered
            let center = projected(frame, Vec2::ZERO);
            let pixels = frame_pixels(&app, &target, resolution);
            let pixel = |offset: Vec2| {
                let p = (center + offset * scale).floor().as_uvec2();
                let i = ((p.y * resolution + p.x) * 4) as usize;
                &pixels[i..i + 3]
            };
            assert_eq!(
                pixel(Vec2::new(-3.0, 0.0)),
                &[255; 3],
                "piece must move on frame N"
            );
            assert_eq!(
                pixel(Vec2::new(5.0, 8.0)),
                &[101, 190, 255],
                "marker must move on frame N"
            );
            // Label coverage and background are on top of the white piece.
            assert!(
                pixel(Vec2::new(14.0, 8.0)).iter().all(|&v| v < 200),
                "GPU label background"
            );
            let mut glyph_pixels = 0;
            for y in 9..25 {
                for x in 17..33 {
                    if pixel(Vec2::new(x as f32, y as f32))
                        .iter()
                        .all(|&v| v > 180)
                    {
                        glyph_pixels += 1;
                    }
                }
            }
            assert!(
                glyph_pixels > 3,
                "Japanese GPU label must have white coverage"
            );
            // A region of fixed logical dimensions must contain the whole marker
            // at each zoom and DPI, with comparable logical pixel area.
            let mut marker_pixels = 0;
            for y in 0..(18.0 * scale) as u32 {
                for x in 0..(17.0 * scale) as u32 {
                    let p = (center + Vec2::new(x as f32, y as f32)).floor().as_uvec2();
                    let i = ((p.y * resolution + p.x) * 4) as usize;
                    if pixels[i..i + 3] == [101, 190, 255] {
                        marker_pixels += 1;
                    }
                }
            }
            let logical_area = marker_pixels as f32 / (scale * scale);
            assert!(
                (65.0..130.0).contains(&logical_area),
                "screen-constant marker, area {logical_area}"
            );
            no_piece_uploads(&app);
            assert_eq!(
                app.world()
                    .resource::<RemoteCursorLabels>()
                    .atlas
                    .as_ref()
                    .unwrap()
                    .image
                    .id(),
                atlas_id
            );
        }
        app.world_mut()
            .insert_resource(State::new(GameSubState::Paused));
        update_gpu(&mut app);
        assert!(app
            .sub_app(RenderApp)
            .world()
            .resource::<CursorRenderer>()
            .previous
            .is_empty());
        let paused = frame_pixels(&app, &target, resolution);
        assert!(!paused
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[..3] == [101, 190, 255]));
        app.world_mut()
            .insert_resource(State::new(GameSubState::Playing));
        // Left-offscreen center: the right/down label would otherwise enter view.
        let frame = app.sub_app(RenderApp).world().resource::<ExtractedPuzzle>();
        let inverse = frame.config.clip_from_world.inverse();
        let offscreen = inverse
            .project_point3(Vec3::new(-1.01, 0.0, 0.0))
            .truncate();
        let offscreen_center = projected(frame, offscreen);
        let visible_start = frame.viewport.min;
        app.world_mut()
            .resource_mut::<RemoteCursorPresentation>()
            .set_target(PlayerId(1), Some(offscreen));
        app.world_mut()
            .resource_mut::<RemoteCursorPresentation>()
            .advance(1.0);
        app.world_mut()
            .resource_mut::<RemoteCursorPresentation>()
            .remove(PlayerId(2));
        update_gpu(&mut app);
        let pixels = frame_pixels(&app, &target, resolution);
        // Scan where the whole right/down label would enter this viewport,
        // including its glyphs. The viewport may have an offset and non-square size.
        let end = (offscreen_center + Vec2::new(38.0, 30.0) * scale)
            .ceil()
            .as_uvec2()
            .min(UVec2::splat(resolution));
        for y in (offscreen_center.y.floor() as u32).max(visible_start.y)..end.y {
            for x in visible_start.x..end.x {
                let i = ((y * resolution + x) * 4) as usize;
                assert_eq!(
                    &pixels[i..i + 3],
                    &[0; 3],
                    "offscreen center hides marker and label"
                );
            }
        }
        app.world_mut()
            .insert_resource(State::new(GameSubState::Paused));
        update_gpu(&mut app);
        assert!(app
            .sub_app(RenderApp)
            .world()
            .resource::<CursorRenderer>()
            .previous
            .is_empty());
    }
}

#[test]
#[ignore = "requires a real GPU and a million-piece fixture"]
fn gpu_remote_cursor_million_pieces_smoothing_settle_camera_upload_isolation() {
    let (mut app, camera, target) = gpu_app(128);
    app.world_mut()
        .insert_resource(definition(UVec2::splat(1000), 4096, 42));
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::splat(10000.0); 1_000_000]);
    setup_cursors(&mut app, 1.0);
    app.world_mut().resource_mut::<RemoteCursorLabels>().members = (1..=64).map(PlayerId).collect();
    for id in 1..=64 {
        app.world_mut()
            .resource_mut::<RemoteCursorPresentation>()
            .set_target(PlayerId(id), Some(Vec2::ZERO));
    }
    wait_ready(&mut app);
    for _ in 0..5 {
        update_gpu(&mut app);
    }
    let revision = app.world().resource::<PieceUpload>().revision;
    let atlas = app
        .world()
        .resource::<RemoteCursorLabels>()
        .atlas
        .as_ref()
        .unwrap()
        .clone();
    let atlas_texture = app
        .sub_app(RenderApp)
        .world()
        .resource::<RenderAssets<GpuImage>>()
        .get(atlas.image.id())
        .unwrap()
        .texture
        .id();
    for id in 1..=64 {
        app.world_mut()
            .resource_mut::<RemoteCursorPresentation>()
            .set_target(PlayerId(id), Some(Vec2::splat(10.0)));
    }
    for _ in 0..6 {
        app.world_mut()
            .resource_mut::<RemoteCursorPresentation>()
            .advance(1.0 / 120.0);
        crate::resources::pieces::without_piece_state_access(|| update_gpu(&mut app));
        assert_eq!(
            app.sub_app(RenderApp)
                .world()
                .resource::<CursorRenderer>()
                .instance_upload_bytes,
            64 * 48
        );
        no_piece_uploads(&app);
        assert_eq!(app.world().resource::<PieceUpload>().revision, revision);
        assert!(Arc::ptr_eq(
            &atlas,
            app.world()
                .resource::<RemoteCursorLabels>()
                .atlas
                .as_ref()
                .unwrap()
        ));
        assert_eq!(
            app.sub_app(RenderApp)
                .world()
                .resource::<RenderAssets<GpuImage>>()
                .get(atlas.image.id())
                .unwrap()
                .texture
                .id(),
            atlas_texture
        );
    }
    app.world_mut()
        .resource_mut::<RemoteCursorPresentation>()
        .advance(1.0);
    update_gpu(&mut app);
    for frame in 0..8 {
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation
            .x = frame as f32;
        crate::resources::pieces::without_piece_state_access(|| update_gpu(&mut app));
        assert_eq!(
            app.sub_app(RenderApp)
                .world()
                .resource::<CursorRenderer>()
                .instance_upload_bytes,
            0
        );
        no_piece_uploads(&app);
        assert_eq!(app.world().resource::<PieceUpload>().revision, revision);
    }
    // A fresh atlas that has not reached RenderAssets yet must only skip labels.
    let missing = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .reserve_handle();
    app.world_mut().resource_mut::<RemoteCursorLabels>().atlas =
        Some(Arc::new(RemoteCursorLabelAtlas {
            revision: 2,
            image: missing,
            labels: BTreeMap::new(),
        }));
    update_gpu(&mut app);
    assert_eq!(
        app.sub_app(RenderApp)
            .world()
            .resource::<CursorRenderer>()
            .previous
            .len(),
        64
    );
    assert!(app
        .world()
        .resource::<RenderReady>()
        .is_ready(app.world().resource::<PieceDataStore>().epoch));
    no_piece_uploads(&app);
    let pixels = frame_pixels(&app, &target, 128);
    assert!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[..3] == [255, 116, 113]),
        "markers remain visible while the replacement label image is unavailable"
    );
}
