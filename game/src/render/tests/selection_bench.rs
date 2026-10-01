use super::*;
use crate::{resources::pieces::DragTransform, selection::SelectionPayload};
use bevy::ecs::system::RunSystemOnce;
use puzzella_core::{PieceCommand, LOCAL_PLAYER};

#[test]
#[ignore = "real GPU million-selection benchmark; writes target/million-selection-gpu.csv"]
fn gpu_million_selection_benchmark() {
    if cfg!(debug_assertions) {
        panic!("run with --release");
    }
    let resolution = 2048;
    let (mut app, camera, _) = gpu_app(resolution);
    let def = definition(UVec2::splat(1000), 4096, 42);
    app.world_mut().insert_resource(def.clone());
    app.world_mut().resource_mut::<PieceDataStore>().initialize(
        (0..1_000_000)
            .map(|id| def.piece(id, Vec2::ZERO).correct_position)
            .collect(),
    );
    app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(2.02, 2.02, 1.0);
    wait_ready(&mut app);
    for _ in 0..8 {
        update_gpu(&mut app);
    }
    let info = app
        .sub_app(RenderApp)
        .world()
        .resource::<bevy::render::renderer::RenderAdapterInfo>();
    let adapter = info.name.to_string().replace(',', ";");
    let backend = format!("{:?}", info.backend);
    let driver = format!("{} {}", info.driver, info.driver_info).replace(',', ";");
    let region = Rect::new(0.0, 0.0, resolution as f32, resolution as f32);
    app.world_mut()
        .resource_mut::<PuzzleSelection>()
        .request_preview(region);
    for _ in 0..4 {
        update_gpu(&mut app);
    }
    assert!(app
        .world()
        .resource::<PuzzleSelection>()
        .completed
        .is_none());
    assert!(app
        .sub_app(RenderApp)
        .world()
        .resource::<GpuRenderer>()
        .slots
        .is_empty());
    assert_eq!(
        app.sub_app(RenderApp)
            .world()
            .resource::<GpuRenderer>()
            .upload_bytes,
        0
    );
    app.world_mut().resource_mut::<PuzzleSelection>().cancel();
    let mut csv=String::from("adapter,backend,driver,pieces,resolution,run,rectangle_gpu_ms,readback_bytes,receive_us,selection_commit_us,highlight_prepare_us,total_cpu_finalization_us,selection_upload_bytes,state_upload_bytes,grab_authority_us,grab_frame_us,grab_state_bytes,grab_membership_bytes,release_authority_us,release_frame_us,release_state_bytes,commands_per_transition\n");
    for run in 0..5 {
        // Clear committed selection without touching any state.
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .selected_pieces
            .clear();
        update_gpu(&mut app);
        let request = app
            .world_mut()
            .resource_mut::<PuzzleSelection>()
            .request(region, SelectionMode::Rectangle);
        let deadline = Instant::now() + Duration::from_secs(20);
        let result = loop {
            update_gpu(&mut app);
            if let Some(result) = app
                .world_mut()
                .resource_mut::<PuzzleSelection>()
                .take_result(request)
            {
                break result;
            }
            assert!(Instant::now() < deadline, "million rectangle timed out");
        };
        assert!(result.error.is_none(), "{:?}", result.error);
        let SelectionPayload::Rectangle(mask) = result.payload else {
            panic!()
        };
        assert_eq!(
            mask.count(),
            1_000_000,
            "every piece must have a rectangle hit"
        );
        let receive = app.world().resource::<PuzzleSelection>().receive_cpu_ns as f64 / 1000.0;
        let rect_gpu = gpu_ms(&app, "puzzle_rectangle") + gpu_ms(&app, "puzzle_pick_visibility");
        app.world_mut().resource_mut::<PuzzleSelection>().cancel();
        let start = Instant::now();
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .commit_selection(mask, None);
        let commit = start.elapsed().as_secs_f64() * 1e6;
        let start = Instant::now();
        app.world_mut()
            .run_system_once(prepare_piece_upload)
            .unwrap();
        let highlight = start.elapsed().as_secs_f64() * 1e6;
        update_gpu(&mut app);
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.upload_bytes, 0);
        assert_eq!(gpu.selection_upload_bytes, 125_000);
        assert!(
            read_buffer(&app, &gpu.buffers.as_ref().unwrap().selected, 125_000)
                .chunks_exact(4)
                .all(|word| u32::from_le_bytes(word.try_into().unwrap()) == u32::MAX)
        );
        let (members, grab_cpu) = {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            let members = store.selected_pieces.clone();
            store.drag = DragTransform {
                members: members.words().clone(),
                delta: Vec2::ZERO,
            };
            let grab = PieceCommand::GrabGroup {
                members: members.clone(),
            };
            let start = Instant::now();
            let outcome = store.apply_command(LOCAL_PLAYER, &grab, Some(&def));
            let grab_cpu = start.elapsed().as_secs_f64() * 1e6;
            assert_eq!(outcome.grabbed, 1_000_000);
            (members, grab_cpu)
        };
        let start = Instant::now();
        update_gpu(&mut app);
        let grab_frame = start.elapsed().as_secs_f64() * 1e6;
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        let grab_bytes = gpu.upload_bytes;
        let drag_bytes = gpu.drag_upload_bytes;
        assert_eq!(grab_bytes, 16_000_000);
        assert_eq!(drag_bytes, 125_000);
        for step in 1..=5 {
            app.world_mut().resource_mut::<PieceDataStore>().drag.delta =
                Vec2::new(step as f32 * 2.0, 20.0);
            update_gpu(&mut app);
            let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
            assert_eq!(
                (
                    gpu.upload_bytes,
                    gpu.drag_upload_bytes,
                    gpu.selection_upload_bytes
                ),
                (0, 0, 0)
            );
        }
        let release_cpu = {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            let release = PieceCommand::ReleaseGroup {
                members,
                delta: store.drag.delta,
            };
            store.drag = default();
            let start = Instant::now();
            let outcome = store.apply_command(LOCAL_PLAYER, &release, Some(&def));
            let release_cpu = start.elapsed().as_secs_f64() * 1e6;
            assert_eq!((outcome.released, outcome.placed), (1_000_000, 0));
            assert!(store.held_by.is_empty());
            release_cpu
        };
        let start = Instant::now();
        update_gpu(&mut app);
        let release_frame = start.elapsed().as_secs_f64() * 1e6;
        let release_bytes = app
            .sub_app(RenderApp)
            .world()
            .resource::<GpuRenderer>()
            .upload_bytes;
        assert_eq!(release_bytes, 16_000_000);
        // Restore positions for another full-screen selection. Explicit benchmark setup only.
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            for (id, state) in store.states.iter_mut().enumerate() {
                state.position = def.piece(id as u32, Vec2::ZERO).correct_position;
            }
            store.dirty_pieces.fill();
        }
        update_gpu(&mut app);
        let total = receive + commit + highlight;
        let row=format!("{adapter},{backend},{driver},1000000,{resolution},{run},{rect_gpu:.6},125000,{receive:.3},{commit:.3},{highlight:.3},{total:.3},125000,0,{grab_cpu:.3},{grab_frame:.3},{grab_bytes},{drag_bytes},{release_cpu:.3},{release_frame:.3},{release_bytes},1\n");
        print!("{row}");
        csv.push_str(&row);
    }
    std::fs::create_dir_all("../target").unwrap();
    std::fs::write("../target/million-selection-gpu.csv", csv).unwrap();
}
