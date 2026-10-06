use super::shadow_tests::{assert_no_uploads, config, red_source, render_frame, shadow_draws};
use super::side_tests::side_draws;
use super::*;
use crate::settings::{DisplaySettingsAction, DisplaySettingsPlugin, DisplaySettingsState};

#[test]
#[ignore = "requires a real GPU"]
fn gpu_piece_quality_settings_apply_next_frame_without_reload_or_piece_uploads() {
    let (mut app, _, target) = gpu_app_with_setup(128, true, |app| {
        let mut state = DisplaySettingsState::load(None);
        state.current.piece_visual_quality = PieceVisualQuality::Low;
        app.insert_resource(state)
            .add_plugins(DisplaySettingsPlugin);
    });
    red_source(&mut app, 255);
    app.insert_resource(definition(UVec2::ONE, 40, 42));
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO]);
    wait_ready(&mut app);
    let flat = render_frame(&mut app, target.clone());
    let store = app.world().resource::<PieceDataStore>();
    let epoch = store.epoch;
    let canonical = bytemuck::cast_slice::<_, u8>(&store.states).to_vec();
    let ids = {
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        let b = gpu.buffers.as_ref().unwrap();
        [
            b.states.id(),
            b.piece_metadata.id(),
            b.visible.id(),
            b.args.id(),
            b.selected.id(),
            b.drag_members.id(),
        ]
    };
    for quality in [
        PieceVisualQuality::High,
        PieceVisualQuality::Medium,
        PieceVisualQuality::Low,
    ] {
        let mut draft = app
            .world()
            .resource::<DisplaySettingsState>()
            .current
            .clone();
        draft.piece_visual_quality = quality;
        app.world_mut()
            .write_message(DisplaySettingsAction::Apply(draft));
        // Inspect the very next extracted frame, before the image helper waits
        // for optional pipelines. Quality changes must not upload any piece data.
        update_gpu(&mut app);
        let uniform = config(&app);
        assert_eq!(*app.world().resource::<PieceVisualQuality>(), quality);
        let frame = quality.resolve().for_frame(uniform.piece_size_px, false);
        assert_eq!(uniform.shadow_enabled, u32::from(frame.shadow_enabled));
        assert_eq!(uniform.side_enabled, u32::from(frame.side_enabled));
        assert_eq!(uniform.bevel_enabled, u32::from(frame.bevel_enabled));
        assert_eq!(uniform.shadow_base_offset_px, frame.shadow_base_offset_px);
        assert_no_uploads(&app);
        let pixels = render_frame(&mut app, target.clone());
        assert_eq!(shadow_draws(&app), usize::from(frame.shadow_enabled) * 2);
        assert_eq!(side_draws(&app), usize::from(frame.side_enabled));
        if quality == PieceVisualQuality::Low {
            assert!(pixels == flat);
        } else {
            assert!(pixels != flat);
        }
        let store = app.world().resource::<PieceDataStore>();
        assert_eq!(store.epoch, epoch);
        assert_eq!(bytemuck::cast_slice::<_, u8>(&store.states), canonical);
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        let b = gpu.buffers.as_ref().unwrap();
        assert_eq!(
            [
                b.states.id(),
                b.piece_metadata.id(),
                b.visible.id(),
                b.args.id(),
                b.selected.id(),
                b.drag_members.id()
            ],
            ids
        );
        assert_no_uploads(&app);
        assert!(app
            .world()
            .resource::<DisplaySettingsState>()
            .confirmation_seconds()
            .is_none());
    }
}
