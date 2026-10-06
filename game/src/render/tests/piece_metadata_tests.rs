use super::*;
use crate::resources::{
    pieces::ComponentRootRange,
    remote_drag::RemoteSlotRange,
    rotation_visual::{GpuRotationAnimation, RotationSlotRange},
};
use bevy::ecs::system::RunSystemOnce;

#[test]
fn piece_metadata_region_offsets_and_size_use_u64_capacity() {
    let layout = PieceMetadataLayout { capacity: 5 };
    assert_eq!(layout.size(), 60);
    assert_eq!(layout.root_offset(0), 0);
    assert_eq!(layout.root_offset(4), 16);
    assert_eq!(layout.remote_slot_offset(0), 20);
    assert_eq!(layout.remote_slot_offset(4), 36);
    assert_eq!(layout.rotation_slot_offset(0), 40);
    assert_eq!(layout.rotation_slot_offset(4), 56);
    assert_eq!(
        PieceMetadataLayout {
            capacity: 1_000_000
        }
        .size(),
        12_000_000
    );
    let large = PieceMetadataLayout { capacity: u32::MAX };
    assert_eq!(large.size(), 51_539_607_540);
    assert_eq!(large.remote_slot_offset(u32::MAX - 1), 34_359_738_356);
    assert_eq!(large.rotation_slot_offset(u32::MAX - 1), large.size() - 4);
}

fn storage_count(layout: &BindGroupLayoutDescriptor, stage: ShaderStages) -> usize {
    layout
        .entries
        .iter()
        .filter(|entry| {
            entry.visibility.contains(stage)
                && matches!(
                    entry.ty,
                    BindingType::Buffer {
                        ty: BufferBindingType::Storage { .. },
                        ..
                    }
                )
        })
        .count()
}

#[test]
fn piece_metadata_shader_and_layout_bindings_fit_eight_storage_buffers_per_stage() {
    let (sender, _) = crossbeam::channel::unbounded();
    let gpu = GpuRenderer::new(
        sender,
        default(),
        default(),
        default(),
        default(),
        default(),
        default(),
    );
    for (source, layout, metadata_binding, storage_bindings) in [
        (
            include_str!("../puzzle_render.wgsl"),
            &gpu.draw_layout,
            6,
            7,
        ),
        (
            include_str!("../visibility.wgsl"),
            &gpu.compute_layout,
            7,
            8,
        ),
        (
            include_str!("../pick_visibility.wgsl"),
            &gpu.pick_compute_layout,
            7,
            8,
        ),
    ] {
        assert_eq!(
            source
                .matches("var<storage,read> piece_metadata:array<u32>;")
                .count(),
            1
        );
        for old in ["component_roots", "remote_slots", "rotation_slots"] {
            assert!(!source.contains(old), "old individual binding: {old}");
        }
        assert!(source.contains("piece_metadata[config.capacity+id]"));
        assert!(source.contains("piece_metadata[2u*config.capacity+root]"));
        assert!(source.contains("rotation_slot(component_root(id))"));
        // Compare every group-0 source declaration with the actual Rust layout.
        let declarations: Vec<_> = source
            .lines()
            .filter(|line| line.starts_with("@group(0)"))
            .collect();
        assert_eq!(declarations.len(), layout.entries.len());
        assert_eq!(
            declarations
                .iter()
                .filter(|line| line.contains("var<storage,"))
                .count(),
            storage_bindings
        );
        for (binding, (line, entry)) in declarations.iter().zip(&layout.entries).enumerate() {
            assert!(line.starts_with(&format!("@group(0) @binding({binding})")));
            assert_eq!(entry.binding, binding as u32);
            let expected = if line.contains("var<storage,read_write>") {
                BufferBindingType::Storage { read_only: false }
            } else if line.contains("var<storage,read>") {
                BufferBindingType::Storage { read_only: true }
            } else {
                BufferBindingType::Uniform
            };
            assert!(matches!(entry.ty, BindingType::Buffer { ty, .. } if ty == expected));
        }
        assert!(declarations[metadata_binding].contains("piece_metadata"));
    }
    // The previous vertex layout had 9, and both compute layouts had 10.
    assert_eq!(storage_count(&gpu.draw_layout, ShaderStages::VERTEX), 7);
    assert_eq!(storage_count(&gpu.compute_layout, ShaderStages::COMPUTE), 8);
    assert_eq!(
        storage_count(&gpu.pick_compute_layout, ShaderStages::COMPUTE),
        8
    );
    assert_eq!(storage_count(&gpu.draw_layout, ShaderStages::FRAGMENT), 6);
    assert_eq!(
        storage_count(&gpu.draw_layout, ShaderStages::FRAGMENT)
            + storage_count(&gpu.selection_layout, ShaderStages::FRAGMENT),
        8
    );
    assert_eq!(storage_count(&gpu.preview_layout, ShaderStages::COMPUTE), 3);
    let preview = include_str!("../component_preview.wgsl");
    assert!(preview.contains("var<storage,read> piece_metadata:array<u32>;"));
    assert!(!preview.contains("component_roots"));
    assert!(preview.contains("arrayLength(&piece_metadata)/3u"));
}

fn metadata_words(app: &App) -> Vec<u32> {
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    let buffers = gpu.buffers.as_ref().unwrap();
    bytemuck::cast_slice::<u8, u32>(&read_buffer(
        app,
        &buffers.piece_metadata,
        PieceMetadataLayout {
            capacity: buffers.capacity,
        }
        .size(),
    ))
    .to_vec()
}

fn prepare_frame(app: &mut App, edit: impl FnOnce(&mut ExtractedPuzzle)) {
    let world = app.sub_app_mut(RenderApp).world_mut();
    edit(&mut world.resource_mut::<ExtractedPuzzle>());
    world.run_system_once(prepare_buffers).unwrap();
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_piece_metadata_initialization_sparse_regions_revisions_and_new_epoch() {
    let (mut app, _, _) = gpu_app(128);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO; 5]);
    app.insert_resource(definition(UVec2::new(5, 1), 100, 42));
    wait_ready(&mut app);
    update_gpu(&mut app);
    assert_eq!(
        app.sub_app(RenderApp)
            .world()
            .resource::<RenderDevice>()
            .limits()
            .max_storage_buffers_per_shader_stage,
        8,
    );
    assert_eq!(
        metadata_words(&app),
        [0, 1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );

    // Exercise real renderer range writes with distinct nonzero sentinel values.
    prepare_frame(&mut app, |frame| {
        frame.rotation.revision += 1;
        frame.rotation.records = vec![GpuRotationAnimation::default(); 2].into();
        frame.rotation.ranges = vec![RotationSlotRange {
            start: 1,
            slots: vec![2, 1],
        }]
        .into();
    });
    assert_eq!(
        metadata_words(&app),
        [0, 1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 2, 1, 0, 0]
    );
    {
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.rotation_upload_bytes, 2 * 32 + 2 * 4);
        assert_eq!(gpu.root_upload_bytes, 0);
        assert_eq!(gpu.remote_mapping_upload_bytes, 0);
    }

    prepare_frame(&mut app, |frame| {
        frame.remote.revision += 1;
        frame.remote.initial = None;
        frame.remote.ranges = vec![RemoteSlotRange {
            start: 2,
            slots: vec![7, 8],
        }]
        .into();
    });
    assert_eq!(
        metadata_words(&app),
        [0, 1, 2, 3, 4, 0, 0, 7, 8, 0, 0, 2, 1, 0, 0]
    );
    {
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.remote_mapping_upload_bytes, 8);
        assert_eq!(gpu.remote_mapping_upload_calls, 1);
        assert_eq!(gpu.root_upload_bytes, 0);
        assert_eq!(gpu.rotation_upload_bytes, 0);
    }

    // DSU history: root 1 differs from the stable minimum member 0.
    let mut connectivity = jigsall_core::PieceConnectivity::new(5);
    connectivity.union(PieceId(1), PieceId(2));
    connectivity.union(PieceId(0), PieceId(1));
    let roots: Vec<_> = (0..3)
        .map(|id| connectivity.find_root(PieceId(id)).0)
        .collect();
    assert_eq!(roots, [1, 1, 1]);
    prepare_frame(&mut app, |frame| {
        frame.upload.root_revision += 1;
        frame.upload.root_ranges = vec![ComponentRootRange { start: 0, roots }].into();
    });
    assert_eq!(
        metadata_words(&app),
        [1, 1, 1, 3, 4, 0, 0, 7, 8, 0, 0, 2, 1, 0, 0]
    );
    {
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.root_upload_bytes, 12);
        assert_eq!(gpu.root_upload_calls, 1);
        assert_eq!(gpu.remote_mapping_upload_bytes, 0);
        assert_eq!(gpu.rotation_upload_bytes, 0);
    }

    // Unchanged revisions must ignore retained payloads while only time advances.
    prepare_frame(&mut app, |frame| {
        frame.rotation.time += 0.01;
    });
    {
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.upload_bytes, 0);
        assert_eq!(gpu.root_upload_bytes, 0);
        assert_eq!(gpu.remote_mapping_upload_bytes, 0);
        assert_eq!(gpu.rotation_upload_bytes, 0);
    }
    // Clear only one animation mapping; remote/root regions must remain intact.
    prepare_frame(&mut app, |frame| {
        frame.rotation.revision += 1;
        frame.rotation.records = Arc::default();
        frame.rotation.ranges = vec![RotationSlotRange {
            start: 1,
            slots: vec![0],
        }]
        .into();
    });
    assert_eq!(
        metadata_words(&app),
        [1, 1, 1, 3, 4, 0, 0, 7, 8, 0, 0, 0, 1, 0, 0]
    );

    // New capacity/epoch must not retain either mapping region or old offsets.
    prepare_frame(&mut app, |frame| {
        let epoch = frame.upload.epoch + 1;
        frame.upload = PieceUpload {
            epoch,
            initial: Some(
                (0..3)
                    .map(|id| crate::resources::GpuPieceState::new(Vec2::ZERO, PieceId(id)))
                    .collect::<Vec<_>>()
                    .into(),
            ),
            initial_roots: Some(vec![0, 1, 2].into()),
            ..default()
        };
        frame.remote = RemoteDragUpload { epoch, ..default() };
        frame.rotation = RotationVisualUpload { epoch, ..default() };
    });
    assert_eq!(metadata_words(&app), [0, 1, 2, 0, 0, 0, 0, 0, 0]);
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert_eq!(gpu.root_upload_bytes, 12);
    assert_eq!(gpu.remote_mapping_upload_bytes, 0);
    assert_eq!(gpu.rotation_upload_bytes, 0);
}
