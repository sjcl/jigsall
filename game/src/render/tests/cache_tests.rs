use super::*;

fn groups(app: &App) -> [Option<BindGroupId>; 11] {
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    let mut ids = [None; 11];
    ids[..7].copy_from_slice(&gpu.buffers.as_ref().unwrap().groups.ids());
    ids[7] = gpu.image_group.id();
    ids[8] = gpu.slots.first().and_then(|slot| slot.selection_group.id());
    ids[9..].copy_from_slice(&gpu.buffers.as_ref().unwrap().groups.radix_ids());
    ids
}
fn exercise_selection(app: &mut App) {
    let rect = Rect::new(64.0, 64.0, 65.0, 65.0);
    assert!(!pick(app, rect, SelectionMode::Point).is_empty());
    assert!(!pick(app, rect, SelectionMode::Rectangle).is_empty());
    app.world_mut().resource_mut::<PuzzleSelection>().cancel();
}
fn reset_puzzle(app: &mut App, grid: UVec2) {
    app.world_mut().insert_resource(definition(grid, 128, 42));
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO; (grid.x * grid.y) as usize]);
    wait_ready(app);
    exercise_selection(app);
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_bind_group_reuse_and_invalidation() {
    let (mut app, camera, target) = gpu_app(128);
    reset_puzzle(&mut app, UVec2::new(2, 1));
    let initial = groups(&app);
    assert!(initial[..6].iter().all(Option::is_some));
    assert!(initial[6].is_none()); // Opaque frames do not allocate a sort group.
    assert!(initial[7..9].iter().all(Option::is_some));
    assert!(initial[9..].iter().all(Option::is_none));

    app.world_mut()
        .insert_resource(SelectionOverlay(Some(Rect::new(-5.0, -5.0, 5.0, 5.0))));
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation
        .x = 1.0;
    let mut state = app
        .world()
        .resource::<PieceDataStore>()
        .state(PieceId(0))
        .unwrap();
    state.position.x = 1.0;
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .set_state(PieceId(0), state);
    for _ in 0..4 {
        update_gpu(&mut app);
    }
    exercise_selection(&mut app);
    assert_eq!(
        groups(&app),
        initial,
        "content uploads must reuse all groups"
    );
    app.world_mut().resource_mut::<SelectionOverlay>().0 = None;

    // A sampler replacement on the same GPU image must also refresh the binding.
    let handle = app.world().resource::<PuzzleImage>().handle.clone();
    {
        let world = app.sub_app_mut(RenderApp).world_mut();
        let sampler = world.resource::<RenderDevice>().create_sampler(&default());
        world
            .resource_mut::<RenderAssets<GpuImage>>()
            .get_mut(&handle)
            .unwrap()
            .sampler = sampler;
    }
    update_gpu(&mut app);
    let sampler_changed = groups(&app);
    assert_ne!(sampler_changed[7], initial[7]);
    assert_eq!(&sampler_changed[..7], &initial[..7]);

    // Updating an existing asset replaces its GPU view without changing its AssetId.
    {
        let mut images = app.world_mut().resource_mut::<Assets<Image>>();
        let mut image = images.get_mut(&handle).unwrap();
        image.data.as_mut().unwrap()[..4].copy_from_slice(&[255, 0, 0, 255]);
    }
    for _ in 0..4 {
        update_gpu(&mut app);
    }
    let pixels = rendered_pixels(&mut app, target);
    let center = (64 * 128 + 64) * 4;
    assert_eq!(&pixels[center..center + 4], &[255, 0, 0, 255]);
    let view_changed = groups(&app);
    assert_ne!(view_changed[7], sampler_changed[7]);
    assert_eq!(&view_changed[..7], &sampler_changed[..7]);

    // Force backing-buffer replacements independently of the puzzle epoch.
    {
        let mut gpu = app
            .sub_app_mut(RenderApp)
            .world_mut()
            .resource_mut::<GpuRenderer>();
        gpu.uniform = default();
        gpu.pick_uniform = default();
    }
    exercise_selection(&mut app);
    let uniforms_changed = groups(&app);
    for i in [0, 1, 4, 5] {
        assert_ne!(uniforms_changed[i], view_changed[i]);
    }
    for i in [2, 3, 7, 8] {
        assert_eq!(uniforms_changed[i], view_changed[i]);
    }

    app.world_mut().resource_mut::<PuzzleImage>().opaque = false;
    let deadline = Instant::now() + Duration::from_secs(20);
    while groups(&app)[6].is_none() {
        update_gpu(&mut app);
        assert!(Instant::now() < deadline, "transparent pipeline timed out");
    }
    let transparent = groups(&app);
    assert_ne!(
        transparent[0], uniforms_changed[0],
        "compute must bind new cull counts"
    );
    assert!(transparent[9..].iter().all(Option::is_some));
    for _ in 0..4 {
        update_gpu(&mut app);
    }
    exercise_selection(&mut app);
    assert_eq!(groups(&app), transparent);
    let old_sort_buffer = app
        .sub_app(RenderApp)
        .world()
        .resource::<GpuRenderer>()
        .sort_uniform
        .buffer()
        .unwrap()
        .id();

    // A larger epoch replaces radix buffers and grows the readback slot.
    // The dynamic sort uniform retains its three fixed radix passes.
    reset_puzzle(&mut app, UVec2::new(16, 8));
    let larger = groups(&app);
    for i in (0..7).chain([8, 9, 10]) {
        assert_ne!(larger[i], transparent[i]);
        assert!(larger[i].is_some());
    }
    assert_eq!(larger[7], transparent[7]);
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert_eq!(gpu.sort_uniform.buffer().unwrap().id(), old_sort_buffer);
    let slot_buffer = gpu.slots[0].bitset.id();

    // Smaller epochs retain slot capacity but must replace its selectable binding.
    reset_puzzle(&mut app, UVec2::new(2, 1));
    let smaller = groups(&app);
    assert_ne!(smaller[8], larger[8]);
    assert_eq!(
        app.sub_app(RenderApp)
            .world()
            .resource::<GpuRenderer>()
            .slots[0]
            .bitset
            .id(),
        slot_buffer
    );
    for _ in 0..4 {
        update_gpu(&mut app);
    }
    exercise_selection(&mut app);
    assert_eq!(groups(&app), smaller);

    app.world_mut().insert_resource(PieceDataStore::default());
    update_gpu(&mut app);
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert!(gpu.buffers.is_none());
    assert!(gpu.image_group.id().is_none());
    assert!(gpu
        .slots
        .iter()
        .all(|slot| slot.selection_group.id().is_none()));
}
