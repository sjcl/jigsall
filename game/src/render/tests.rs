use super::*;
use crate::{
    resources::{
        pieces::{prepare_piece_upload, ENABLED},
        PieceDataStore,
    },
    selection::PuzzleSelectionPlugin,
};
use bevy::{
    asset::RenderAssetUsages,
    camera::RenderTarget,
    render::{
        diagnostic::RenderDiagnosticsPlugin,
        gpu_readback::{Readback, ReadbackComplete},
        settings::{RenderCreation, WgpuSettings},
        RenderPlugin,
    },
};
use puzzella_core::{PieceId, PuzzleDefinition, GENERATOR_VERSION};
use puzzella_puzzle::procedural::*;
use std::{
    borrow::Cow,
    sync::Mutex,
    time::{Duration, Instant},
};

fn gpu_app(resolution: u32) -> (App, Entity, Handle<Image>) {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(Box::new(WgpuSettings {
                    features: WgpuFeatures::TIMESTAMP_QUERY
                        | WgpuFeatures::TIMESTAMP_QUERY_INSIDE_ENCODERS,
                    ..default()
                })),
                ..default()
            })
            .disable::<bevy::log::LogPlugin>()
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>(),
    )
    .add_plugins(RenderDiagnosticsPlugin)
    .init_resource::<PieceDataStore>()
    .init_resource::<PieceUpload>()
    .add_plugins(PuzzleSelectionPlugin)
    .add_systems(Last, prepare_piece_upload)
    .insert_resource(ClearColor(Color::BLACK));
    app.finish();
    app.cleanup();
    let info = app
        .sub_app(RenderApp)
        .world()
        .resource::<bevy::render::renderer::RenderAdapterInfo>();
    println!("GPU adapter: {} ({:?})", info.name, info.backend);
    let mut target =
        Image::new_target_texture(resolution, resolution, TextureFormat::Rgba8UnormSrgb, None);
    target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(target);
    let camera = app
        .world_mut()
        .spawn((
            Camera2d,
            MainCamera,
            RenderTarget::Image(target.clone().into()),
            Msaa::Off,
        ))
        .id();
    let image = Image::new_fill(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[255, 255, 255, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::all(),
    );
    let image = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().insert_resource(PuzzleImage {
        handle: image,
        size: Vec2::splat(128.0),
        opaque: true,
    });
    (app, camera, target)
}
fn definition(grid: UVec2, size: u32, seed: u64) -> PuzzleDefinition {
    PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed,
        grid_size: grid,
        image_size: UVec2::splat(size),
        snap_distance: 5.0,
    }
}
fn update_gpu(app: &mut App) {
    app.update();
    assert!(
        app.world()
            .resource::<Messages<bevy::app::AppExit>>()
            .is_empty(),
        "GPU validation failed"
    );
    app.sub_app(RenderApp)
        .world()
        .resource::<RenderDevice>()
        .poll(PollType::Wait {
            timeout: Some(Duration::from_secs(20)),
            submission_index: None,
        })
        .unwrap();
}
fn wait_ready(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !app
        .world()
        .resource::<RenderReady>()
        .is_ready(app.world().resource::<PieceDataStore>().epoch)
    {
        update_gpu(app);
        assert!(Instant::now() < deadline, "GPU initialization timed out");
    }
}
fn pick(app: &mut App, rect: Rect, mode: SelectionMode) -> Vec<PieceId> {
    let id = app
        .world_mut()
        .resource_mut::<PuzzleSelection>()
        .request(rect, mode);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        update_gpu(app);
        if let Some(result) = app
            .world_mut()
            .resource_mut::<PuzzleSelection>()
            .take_result(id)
        {
            assert!(result.error.is_none(), "{:?}", result.error);
            return result.piece_ids;
        }
        assert!(Instant::now() < deadline, "GPU picking timed out");
    }
}
fn read_buffer(app: &App, source: &Buffer, size: u64) -> Vec<u8> {
    let world = app.sub_app(RenderApp).world();
    let device = world.resource::<RenderDevice>();
    let queue = world.resource::<RenderQueue>();
    let staging = buffer(
        device,
        "test readback",
        size,
        BufferUsages::COPY_DST | BufferUsages::MAP_READ,
    );
    let mut encoder = device.create_command_encoder(&default());
    encoder.copy_buffer_to_buffer(source, 0, &staging, 0, size);
    queue.submit([encoder.finish()]);
    let (tx, rx) = crossbeam::channel::bounded(1);
    staging.slice(..).map_async(MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    device
        .poll(PollType::Wait {
            timeout: Some(Duration::from_secs(20)),
            submission_index: None,
        })
        .unwrap();
    rx.recv().unwrap().unwrap();
    let bytes = staging.slice(..).get_mapped_range().to_vec();
    staging.unmap();
    bytes
}
fn hash_parity(app: &App) {
    let world = app.sub_app(RenderApp).world();
    let device = world.resource::<RenderDevice>();
    let queue = world.resource::<RenderQueue>();
    let mut inputs = vec![];
    let mut expected = vec![];
    for seed in [0, 1, 42, u64::MAX, 1 << 32, 0x123456789abcdef0] {
        for orientation in [EdgeOrientation::Horizontal, EdgeOrientation::Vertical] {
            for (x, y) in [(0, 0), (1, 2), (999, 1000)] {
                inputs.push([
                    seed as u32,
                    (seed >> 32) as u32,
                    u32::from(orientation == EdgeOrientation::Vertical),
                    x,
                    y,
                ]);
                expected.push(raw_profile(seed, EdgeId { orientation, x, y }));
            }
        }
    }
    let source = include_str!("puzzle_shape.wgsl")
        .lines()
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n")
        + "
struct Input {low:u32,high:u32,orientation:u32,x:u32,y:u32};
@group(0) @binding(0) var<storage,read> inputs:array<Input>;
@group(0) @binding(1) var<storage,read_write> outputs:array<vec2<u32>>;
@compute @workgroup_size(1) fn test_profile(@builtin(global_invocation_id) id:vec3<u32>) {
let p=inputs[id.x];outputs[id.x]=raw_profile(vec2(p.low,p.high),p.orientation,p.x,p.y);}";
    let shader = device
        .wgpu_device()
        .create_shader_module(ShaderModuleDescriptor {
            label: Some("hash parity"),
            source: ShaderSource::Wgsl(Cow::Owned(source)),
        });
    let pipeline = device.create_compute_pipeline(&RawComputePipelineDescriptor {
        label: Some("hash parity"),
        layout: None,
        module: &shader,
        entry_point: Some("test_profile"),
        compilation_options: default(),
        cache: None,
    });
    let input = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("hash cases"),
        contents: bytemuck::cast_slice(&inputs),
        usage: BufferUsages::STORAGE,
    });
    let output = buffer(
        device,
        "raw profiles",
        expected.len() as u64 * 8,
        BufferUsages::STORAGE | BufferUsages::COPY_SRC,
    );
    let group = device
        .wgpu_device()
        .create_bind_group(&BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::Buffer(input.as_entire_buffer_binding()),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Buffer(output.as_entire_buffer_binding()),
                },
            ],
        });
    let mut encoder = device.create_command_encoder(&default());
    {
        let mut pass = encoder.begin_compute_pass(&default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(inputs.len() as u32, 1, 1);
    }
    queue.submit([encoder.finish()]);
    let bytes = read_buffer(app, &output, expected.len() as u64 * 8);
    assert_eq!(bytemuck::cast_slice::<u8, [u32; 2]>(&bytes), expected);
}
fn rendered_pixels(app: &mut App, target: Handle<Image>) -> Vec<u8> {
    let pixels = Arc::new(Mutex::new(None));
    let out = pixels.clone();
    let entity = app
        .world_mut()
        .spawn(Readback::texture(target))
        .observe(move |event: On<ReadbackComplete>| {
            *out.lock().unwrap() = Some(event.data.clone());
        })
        .id();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        update_gpu(app);
        if let Some(bytes) = pixels.lock().unwrap().take() {
            app.world_mut().entity_mut(entity).despawn();
            return bytes;
        }
        assert!(Instant::now() < deadline);
    }
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_raster_selection() {
    let (mut app, camera, target) = gpu_app(128);
    hash_parity(&app);
    let def = definition(UVec2::splat(2), 128, 42);
    app.world_mut().insert_resource(def.clone());
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO; 4]);
    for state in app
        .world_mut()
        .resource_mut::<PieceDataStore>()
        .states
        .iter_mut()
        .skip(1)
    {
        state.flags = 0;
    }
    wait_ready(&mut app);
    for _ in 0..4 {
        update_gpu(&mut app);
    }
    let pixels = rendered_pixels(&mut app, target);
    let profiles = piece_profiles(def.seed, def.grid_size, UVec2::ZERO);
    let mut samples = vec![];
    for y in 15..113 {
        for x in 15..113 {
            let local = Vec2::new(x as f32 - 63.5, 63.5 - y as f32);
            let inside = piece_signed_distance(local, Vec2::splat(64.0), profiles) <= 0.0;
            let drawn = pixels[(y * 128 + x) * 4] > 0;
            assert_eq!(drawn, inside, "render coverage {x},{y} {local}");
            let d = piece_signed_distance(local, Vec2::splat(64.0), profiles);
            if d.abs() < 0.8 && (x + y) % 13 == 0 {
                samples.push((x, y, inside));
            }
        }
    }
    samples.extend([(64, 64, true), (15, 15, false), (90, 64, true)]);
    for (x, y, inside) in samples {
        let rect = Rect::new(
            x as f32 + 0.1,
            y as f32 + 0.1,
            x as f32 + 0.9,
            y as f32 + 0.9,
        );
        for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
            assert_eq!(
                pick(&mut app, rect, mode),
                if inside { vec![PieceId(0)] } else { vec![] },
                "pick {x},{y}"
            );
        }
    }
    assert_eq!(
        app.world_mut().query::<&Mesh2d>().iter(app.world()).count(),
        0
    );
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert_eq!(gpu.point.as_ref().unwrap().id.size().width, 1);
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        let mut s = store.state(PieceId(0)).unwrap();
        s.position.x += 1.0;
        store.set_state(PieceId(0), s);
    }
    update_gpu(&mut app);
    {
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.upload_bytes, 16);
        assert_eq!(gpu.upload_calls, 1);
    }
    update_gpu(&mut app);
    assert_eq!(
        app.sub_app(RenderApp)
            .world()
            .resource::<GpuRenderer>()
            .upload_bytes,
        0
    );
    // Two overlapping instances: reverse-Z picking chooses the largest rank.
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.states[1].flags = ENABLED;
        store.dirty_pieces.insert(PieceId(1));
        store.bring_piece_to_front(PieceId(1));
    }
    let rect = Rect::new(63.0, 63.0, 65.0, 65.0);
    assert_eq!(pick(&mut app, rect, SelectionMode::Point), vec![PieceId(1)]);
    assert_eq!(
        pick(&mut app, rect, SelectionMode::Rectangle),
        vec![PieceId(0), PieceId(1)]
    );
    for held in [false, true] {
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            let mut state = store.state(PieceId(1)).unwrap();
            state.placed = !held;
            state.held_by = held.then_some(puzzella_core::LOCAL_PLAYER);
            store.set_state(PieceId(1), state);
        }
        assert_eq!(pick(&mut app, rect, SelectionMode::Point), vec![PieceId(0)]);
    }
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.states[1].flags = 0;
        store.dirty_pieces.insert(PieceId(1));
    }
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation
        .x = 20.0;
    assert_eq!(
        pick(
            &mut app,
            Rect::new(44.0, 64.0, 45.0, 65.0),
            SelectionMode::Point
        ),
        vec![PieceId(0)]
    );
    app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(2.0, 2.0, 1.0);
    assert_eq!(
        pick(
            &mut app,
            Rect::new(54.0, 64.0, 55.0, 65.0),
            SelectionMode::Point
        ),
        vec![PieceId(0)]
    );
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation = Vec3::ZERO;
    app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::ONE;
    app.world_mut().get_mut::<Camera>(camera).unwrap().viewport = Some(bevy::camera::Viewport {
        physical_position: UVec2::new(8, 16),
        physical_size: UVec2::splat(32),
        ..default()
    });
    assert_eq!(
        pick(
            &mut app,
            Rect::new(24.0, 32.0, 25.0, 33.0),
            SelectionMode::Point
        ),
        vec![PieceId(0)]
    );
    assert!(pick(
        &mut app,
        Rect::new(1.0, 1.0, 2.0, 2.0),
        SelectionMode::Point
    )
    .is_empty());
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_transparency_and_visibility() {
    let (mut app, camera, target) = gpu_app(128);
    let mut def = definition(UVec2::new(2, 1), 128, 43);
    def.image_size.y = 64;
    app.world_mut().insert_resource(def);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO; 2]);
    let mut data = vec![];
    for _ in 0..64 {
        for x in 0..128 {
            data.extend_from_slice(if x < 64 {
                &[255, 0, 0, 128]
            } else {
                &[0, 0, 255, 128]
            });
        }
    }
    let image = Image::new(
        Extent3d {
            width: 128,
            height: 64,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::all(),
    );
    assert!(!crate::resources::images::image_is_opaque(&image));
    let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().resource_mut::<PuzzleImage>().handle = handle.clone();
    app.world_mut().resource_mut::<PuzzleImage>().opaque = false;
    wait_ready(&mut app);
    for _ in 0..4 {
        update_gpu(&mut app);
    }
    let center = (64 * 128 + 64) * 4;
    let pixels = rendered_pixels(&mut app, target.clone());
    assert!((pixels[center] as i32 - 137).abs() < 3);
    assert!((pixels[center + 2] as i32 - 188).abs() < 3);
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .bring_piece_to_front(PieceId(0));
    for _ in 0..4 {
        update_gpu(&mut app);
    }
    let pixels = rendered_pixels(&mut app, target.clone());
    assert!((pixels[center] as i32 - 188).abs() < 3);
    assert!((pixels[center + 2] as i32 - 137).abs() < 3);
    let rect = Rect::new(64.0, 64.0, 65.0, 65.0);
    assert_eq!(pick(&mut app, rect, SelectionMode::Point), vec![PieceId(0)]);
    {
        let mut images = app.world_mut().resource_mut::<Assets<Image>>();
        let mut image = images.get_mut(&handle).unwrap();
        for (i, p) in image.data.as_mut().unwrap().chunks_exact_mut(4).enumerate() {
            if i % 128 < 64 {
                p[3] = 0;
            }
        }
    }
    for _ in 0..4 {
        update_gpu(&mut app);
    }
    for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
        assert_eq!(pick(&mut app, rect, mode), vec![PieceId(1)]);
    }
    // Conservative visibility includes a tab extending into the viewport.
    let seed = (0..100)
        .find(|&seed| {
            raw_profile(
                seed,
                EdgeId {
                    orientation: EdgeOrientation::Vertical,
                    x: 1,
                    y: 0,
                },
            )[0] & 8
                != 0
        })
        .unwrap();
    {
        let mut images = app.world_mut().resource_mut::<Assets<Image>>();
        let mut image = images.get_mut(&handle).unwrap();
        for p in image.data.as_mut().unwrap().chunks_exact_mut(4) {
            p[3] = 128;
        }
    }
    app.world_mut().resource_mut::<PuzzleDefinition>().seed = seed;
    app.world_mut()
        .resource_mut::<PieceUpload>()
        .definition
        .as_mut()
        .unwrap()
        .seed = seed;
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.states[0].position = Vec2::splat(10000.0);
        store.dirty_pieces.insert(PieceId(0));
        store.states[1].position = Vec2::new(100.0, 0.0);
        store.dirty_pieces.insert(PieceId(1));
    }
    for _ in 0..4 {
        update_gpu(&mut app);
    }
    assert_eq!(visible_ids(&app), vec![1]);
    assert_eq!(
        pick(
            &mut app,
            Rect::new(124.0, 64.0, 125.0, 65.0),
            SelectionMode::Point
        ),
        vec![PieceId(1)]
    );
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation
        .x = -1000.0;
    for _ in 0..4 {
        update_gpu(&mut app);
    }
    assert!(visible_ids(&app).is_empty());
    // A transparent one-piece puzzle also works without any sort stages.
    app.world_mut()
        .insert_resource(definition(UVec2::ONE, 64, 42));
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO]);
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation = Vec3::ZERO;
    wait_ready(&mut app);
    for _ in 0..4 {
        update_gpu(&mut app);
    }
    assert_eq!(visible_ids(&app), vec![0]);
}

fn gpu_ms(app: &App, name: &str) -> f64 {
    app.world()
        .resource::<bevy::diagnostic::DiagnosticsStore>()
        .iter()
        .find(|d| d.path().as_str().contains(name) && d.path().as_str().ends_with("elapsed_gpu"))
        .and_then(|d| d.value())
        .expect("GPU timestamp measurement missing")
}
fn visible_ids(app: &App) -> Vec<u32> {
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    let buffers = gpu.buffers.as_ref().unwrap();
    let args = read_buffer(app, &buffers.args, 16);
    let count = u32::from_le_bytes(args[4..8].try_into().unwrap());
    if count == 0 {
        return vec![];
    }
    let bytes = read_buffer(app, &buffers.visible, u64::from(count) * 4);
    bytemuck::cast_slice(&bytes).to_vec()
}
#[test]
#[ignore = "release benchmark on a real GPU"]
fn procedural_gpu_benchmark() {
    use bevy::ecs::system::RunSystemOnce;
    use puzzella_puzzle::placement::generate_placement_grid;
    let (mut app, camera, _) = gpu_app(1024);
    let side = 4096u32;
    let mut rgba = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            rgba.extend_from_slice(&[
                (x % 256) as u8,
                (y % 256) as u8,
                ((x / 16 + y / 16) % 2 * 160 + 60) as u8,
                255,
            ]);
        }
    }
    let image = Image::new(
        Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::all(),
    );
    let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().insert_resource(PuzzleImage {
        handle,
        size: Vec2::splat(side as f32),
        opaque: true,
    });
    let mut csv=String::from("pieces,view,visible,placement_ms,state_ms,initial_upload_prep_ms,dirty_prep_us,frame_ms,cull_gpu_ms,draw_gpu_ms,point_gpu_ms,rectangle_gpu_ms,cpu_state_bytes,gpu_state_bytes,visible_bytes,selectable_bytes,cpu_image_bytes,gpu_image_bytes,pick_visible_bytes,sort_gpu_ms,selection_and_staging_bytes,meshes,piece_entities,draw_calls\n");
    for grid in [
        UVec2::new(40, 25),
        UVec2::splat(100),
        UVec2::new(400, 250),
        UVec2::splat(1000),
    ] {
        let mut def = definition(grid, 100, 42);
        def.image_size = UVec2::splat(side);
        let size = def.image_size.as_vec2() / grid.as_vec2();
        let count = def.piece_count();
        let start = Instant::now();
        let positions = generate_placement_grid(
            grid.x as usize,
            grid.y as usize,
            size.x,
            size.y,
            def.image_size.x as f32,
            def.image_size.y as f32,
            def.seed,
        );
        let placement = start.elapsed().as_secs_f64() * 1000.0;
        let start = Instant::now();
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .initialize(positions);
        let state_ms = start.elapsed().as_secs_f64() * 1000.0;
        // Assemble the same authoritative states for a dense, fully visible worst case.
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            for (i, state) in store.states.iter_mut().enumerate() {
                state.position = def.piece(i as u32, Vec2::ZERO).correct_position;
            }
        }
        app.world_mut().insert_resource(def.clone());
        let start = Instant::now();
        let initial: Arc<[crate::resources::GpuPieceState]> = app
            .world()
            .resource::<PieceDataStore>()
            .states
            .clone()
            .into();
        let prep = start.elapsed().as_secs_f64() * 1000.0;
        drop(initial);
        wait_ready(&mut app);
        update_gpu(&mut app);
        // Time actual coalescing of a single dirty piece in a separate main-world fixture.
        let mut cpu_app = App::new();
        cpu_app
            .init_resource::<PieceUpload>()
            .init_resource::<PieceDataStore>()
            .add_systems(Update, prepare_piece_upload);
        cpu_app
            .world_mut()
            .resource_mut::<PieceDataStore>()
            .initialize(vec![Vec2::ZERO; count]);
        cpu_app.update();
        cpu_app.update();
        {
            let mut store = cpu_app.world_mut().resource_mut::<PieceDataStore>();
            let mut s = store.state(PieceId(0)).unwrap();
            s.position.x += 1.0;
            store.set_state(PieceId(0), s);
        }
        let start = Instant::now();
        cpu_app
            .world_mut()
            .run_system_once(prepare_piece_upload)
            .unwrap();
        let dirty = start.elapsed().as_secs_f64() * 1e6;
        assert!(cpu_app.world().resource::<PieceUpload>().initial.is_none());
        assert_eq!(
            cpu_app.world().resource::<PieceUpload>().ranges[0]
                .states
                .len(),
            1
        );
        drop(cpu_app);
        for (name, scale) in [
            ("near", (1000.0 * size.x * size.y).sqrt() / 1024.0),
            ("medium", (10000.0 * size.x * size.y).sqrt() / 1024.0),
            ("entire", side as f32 / 1024.0 * 1.01),
            ("entire_translucent", side as f32 / 1024.0 * 1.01),
        ] {
            let translucent = name == "entire_translucent";
            if app.world().resource::<PuzzleImage>().opaque == translucent {
                let handle = app.world().resource::<PuzzleImage>().handle.clone();
                let mut images = app.world_mut().resource_mut::<Assets<Image>>();
                let mut image = images.get_mut(&handle).unwrap();
                for pixel in image.data.as_mut().unwrap().chunks_exact_mut(4) {
                    pixel[3] = if translucent { 128 } else { 255 };
                }
            }
            app.world_mut().resource_mut::<PuzzleImage>().opaque = !translucent;
            app.world_mut().get_mut::<Transform>(camera).unwrap().scale =
                Vec3::new(scale, scale, 1.0);
            for _ in 0..8 {
                update_gpu(&mut app);
            }
            if translucent {
                let deadline = Instant::now() + Duration::from_secs(30);
                while !app
                    .world()
                    .resource::<bevy::diagnostic::DiagnosticsStore>()
                    .iter()
                    .any(|d| {
                        d.path().as_str().contains("puzzle_sort")
                            && d.path().as_str().ends_with("elapsed_gpu")
                    })
                {
                    update_gpu(&mut app);
                    assert!(
                        Instant::now() < deadline,
                        "transparent pipeline did not become ready"
                    );
                }
            }
            let mut frame = 0.0;
            let mut cull = 0.0;
            let mut draw = 0.0;
            let mut sort = 0.0;
            for _ in 0..30 {
                let start = Instant::now();
                update_gpu(&mut app);
                frame += start.elapsed().as_secs_f64() * 1000.0;
                cull += gpu_ms(&app, "puzzle_visibility");
                draw += gpu_ms(&app, "puzzle_draw");
                if translucent {
                    sort += gpu_ms(&app, "puzzle_sort");
                }
            }
            let ids = visible_ids(&app);
            assert!(ids.len() <= count);
            if name.starts_with("entire") {
                assert_eq!(ids.len(), count);
            }
            let config = &app
                .sub_app(RenderApp)
                .world()
                .resource::<ExtractedPuzzle>()
                .config;
            for &id in &ids {
                let p = app.world().resource::<PieceDataStore>().states[id as usize].position;
                let half = config.size * 0.5 + config.size.min_element() * 0.22;
                assert!(
                    (p + half).cmpge(config.view_min).all()
                        && (p - half).cmple(config.view_max).all()
                );
            }
            let mut point = 0.0;
            let mut rectangle = 0.0;
            for _ in 0..5 {
                pick(
                    &mut app,
                    Rect::new(512.0, 512.0, 513.0, 513.0),
                    SelectionMode::Point,
                );
                update_gpu(&mut app);
                point += gpu_ms(&app, "puzzle_point") + gpu_ms(&app, "puzzle_pick_visibility");
                if name == "entire" {
                    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
                    let bytes = read_buffer(&app, &gpu.buffers.as_ref().unwrap().pick_args, 16);
                    let candidates = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
                    assert!(candidates <= 16, "click must not submit all visible pieces");
                }
                pick(
                    &mut app,
                    Rect::new(0.0, 0.0, 1024.0, 1024.0),
                    SelectionMode::Rectangle,
                );
                update_gpu(&mut app);
                rectangle +=
                    gpu_ms(&app, "puzzle_rectangle") + gpu_ms(&app, "puzzle_pick_visibility");
            }
            app.world_mut().resource_mut::<PuzzleSelection>().cancel();
            let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
            let b = gpu.buffers.as_ref().unwrap();
            let selection_bytes: u64 = gpu
                .slots
                .iter()
                .map(|s| s.bitset.size() + s.staging.size())
                .sum();
            let meshes = app.world().resource::<Assets<Mesh>>().len();
            assert!(meshes <= 1);
            let image_bytes = u64::from(side) * u64::from(side) * 4;
            let sort_ms = sort / 30.0;
            let pick_bytes = b.pick_visible.size();
            let row=format!("{count},{name},{},{placement:.4},{state_ms:.4},{prep:.4},{dirty:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{},{},{},{},{image_bytes},{image_bytes},{pick_bytes},{sort_ms:.4},{selection_bytes},{meshes},0,1\n",ids.len(),frame/30.0,cull/30.0,draw/30.0,point/5.0,rectangle/5.0,app.world().resource::<PieceDataStore>().states.capacity()*16,b.states.size(),b.visible.size(),b.selectable.size());
            print!("{row}");
            csv.push_str(&row);
        }
    }
    std::fs::create_dir_all("../target").unwrap();
    std::fs::write("../target/procedural-benchmark.csv", csv).unwrap();
}
