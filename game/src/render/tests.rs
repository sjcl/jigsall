use super::*;
mod component_preview_tests;
mod far_zoom_tests;
mod local_rotation_tests;
mod outline_tests;
mod remote_drag_tests;
mod rotation_tests;
mod selection_bench;
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
use puzzella_puzzle::fingerprint::{sample_profile, worst_case_profiles, EdgeFingerprint};
use puzzella_puzzle::procedural::*;
use std::{
    borrow::Cow,
    sync::Mutex,
    time::{Duration, Instant},
};

fn gpu_app(resolution: u32) -> (App, Entity, Handle<Image>) {
    crate::test_logging::init();
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .set(RenderPlugin {
                // Pixel readback must wait for Bevy's output pipelines as well
                // as the puzzle pipelines; avoid startup compile timing races.
                synchronous_pipeline_compilation: true,
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
    .init_resource::<crate::resources::LocalPlayerId>()
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
    bevy::log::info!(adapter = %info.name, backend = ?info.backend, "GPU test adapter");
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
        logical_size: UVec2::splat(128),
        texture_size: UVec2::ONE,
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
            return result.payload.ids();
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
    let bytes = compute_output(
        app,
        source,
        "test_profile",
        bytemuck::cast_slice(&inputs),
        expected.len() as u64 * 8,
        inputs.len() as u32,
    );
    assert_eq!(bytemuck::cast_slice::<u8, [u32; 2]>(&bytes), expected);
}
fn shape_parity(app: &App) {
    let edge = EdgeId {
        orientation: EdgeOrientation::Horizontal,
        x: 0,
        y: 1,
    };
    let mut inputs = vec![];
    let mut expected = vec![];
    let mut profiles = worst_case_profiles();
    for style in 1..=6 {
        profiles.push(
            (0..10000)
                .map(|seed| raw_profile(seed, edge))
                .find(|raw| raw[0] & 7 == style)
                .unwrap(),
        );
    }
    for raw in profiles {
        for polarity in [0, 8] {
            let raw = [(raw[0] & !8) | polarity, raw[1]];
            let p = decode_profile(raw);
            for (length, short) in [(100.0, 100.0), (200.0, 50.0), (64.0, 64.0)] {
                let root_half =
                    (p.neck_width + (p.width - p.neck_width) * ROOT_WIDTH_FACTOR) * length * 0.5;
                let height = p.depth * short * ROOT_HEIGHT_FACTOR;
                for t in [
                    -1.0, -0.01, 0.0, 0.0001, 0.01, 0.05, 0.1, 0.25, 0.5, 0.75, 0.95, 1.0, 1.05,
                    1.5, 2.0, 3.0, 4.0,
                ] {
                    for offset in [
                        -1.1, -1.0, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0, 1.1,
                    ] {
                        let q = Vec2::new(p.center * length + offset * root_half, t * height);
                        inputs.push([
                            raw[0],
                            raw[1],
                            length.to_bits(),
                            short.to_bits(),
                            q.x.to_bits(),
                            q.y.to_bits(),
                            0,
                            0,
                        ]);
                        expected.push((
                            [
                                sd_tab(q, p, length, short),
                                edge_distance(q, raw, length, short),
                            ],
                            short,
                        ));
                    }
                }
            }
        }
    }
    let source = include_str!("puzzle_shape.wgsl")
        .lines()
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n")
        + "
struct ShapeInput {raw:vec2<u32>,len:u32,short:u32,x:u32,y:u32,pad:vec2<u32>};
@group(0) @binding(0) var<storage,read> inputs:array<ShapeInput>;
@group(0) @binding(1) var<storage,read_write> outputs:array<vec2<f32>>;
@compute @workgroup_size(64) fn test_shape(@builtin(global_invocation_id) id:vec3<u32>) {
if id.x>=arrayLength(&inputs) {return;}
let p=inputs[id.x];let q=vec2(bitcast<f32>(p.x),bitcast<f32>(p.y));
let len=bitcast<f32>(p.len);let short=bitcast<f32>(p.short);
outputs[id.x]=vec2(sd_tab(q,edge_profile(p.raw),len,short),edge_distance(q,p.raw,len,short));}";
    let bytes = compute_output(
        app,
        source,
        "test_shape",
        bytemuck::cast_slice(&inputs),
        expected.len() as u64 * 8,
        (inputs.len() as u32).div_ceil(64),
    );
    for (index, (actual, (expected, short))) in bytemuck::cast_slice::<u8, [f32; 2]>(&bytes)
        .iter()
        .zip(expected)
        .enumerate()
    {
        for (actual, expected) in actual.iter().zip(expected) {
            assert!(
                actual.is_finite() && (actual - expected).abs() <= short * 1e-5,
                "GPU shape parity {index}: {actual} vs {expected}"
            );
            if expected.abs() > short * 1e-5 {
                assert_eq!(actual.is_sign_negative(), expected.is_sign_negative());
            }
        }
    }
}
fn decode_parity(app: &App) {
    let mut inputs = worst_case_profiles();
    for style in 1..=6 {
        for axis in 0..6 {
            for sample in 0..256 {
                let mut samples = [128; 6];
                samples[axis] = sample;
                let mut raw = sample_profile(style, samples);
                raw[0] |= (sample & 1) << 3;
                inputs.push(raw);
            }
        }
    }
    let source=include_str!("puzzle_shape.wgsl").lines().skip(1).collect::<Vec<_>>().join("\n")+"
struct DecodeOutput {a:vec4<u32>,b:vec4<u32>,c:vec4<f32>,d:vec4<f32>};
@group(0) @binding(0) var<storage,read> inputs:array<vec2<u32>>;
@group(0) @binding(1) var<storage,read_write> outputs:array<DecodeOutput>;
@compute @workgroup_size(64) fn test_decode(@builtin(global_invocation_id) id:vec3<u32>) {
if id.x>=arrayLength(&inputs) {return;} let r=inputs[id.x];let p=edge_profile(r);
outputs[id.x]=DecodeOutput(
vec4((r.x&7u)-1u,u32(class_sample(r.x,4u,7u).x),u32(class_sample(r.x,12u,4u).x),u32(class_sample(r.x,20u,4u).x)),
vec4(u32(class_sample(r.y,0u,4u).x),u32(class_sample(r.y,8u,4u).x),u32(class_sample(r.y,16u,7u).x),0u),
vec4(p.polarity,p.center,p.width,p.depth),vec4(p.neck,p.head,p.asymmetry,0.0));}";
    let bytes = compute_output(
        app,
        source,
        "test_decode",
        bytemuck::cast_slice(&inputs),
        inputs.len() as u64 * 64,
        (inputs.len() as u32).div_ceil(64),
    );
    for (&raw, actual) in inputs
        .iter()
        .zip(bytemuck::cast_slice::<u8, [u32; 16]>(&bytes))
    {
        let classes = EdgeFingerprint::from_raw(raw).classes().map(u32::from);
        assert_eq!(&actual[..7], classes.as_slice());
        let p = decode_profile(raw);
        let expected = [
            p.polarity,
            p.center,
            p.width,
            p.depth,
            p.neck_width,
            p.head_width,
            p.asymmetry,
        ];
        for (&bits, want) in actual[8..15].iter().zip(expected) {
            let value = f32::from_bits(bits);
            assert!(
                value.is_finite() && (value - want).abs() < 1e-6,
                "decode {raw:?}: {value} vs {want}"
            );
        }
    }
}
fn compute_output(
    app: &App,
    source: String,
    entry: &str,
    input_bytes: &[u8],
    output_bytes: u64,
    workgroups: u32,
) -> Vec<u8> {
    let world = app.sub_app(RenderApp).world();
    let device = world.resource::<RenderDevice>();
    let queue = world.resource::<RenderQueue>();
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
        entry_point: Some(entry),
        compilation_options: default(),
        cache: None,
    });
    let input = device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("hash cases"),
        contents: input_bytes,
        usage: BufferUsages::STORAGE,
    });
    let output = buffer(
        device,
        "raw profiles",
        output_bytes,
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
        pass.dispatch_workgroups(workgroups, 1, 1);
    }
    queue.submit([encoder.finish()]);
    read_buffer(app, &output, output_bytes)
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
fn gpu_drag_transform_and_preview_without_readback() {
    use crate::resources::pieces::{DragTransform, HELD};
    let (mut app, _, target) = gpu_app(128);
    app.world_mut()
        .insert_resource(definition(UVec2::splat(2), 128, 42));
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.initialize(vec![Vec2::ZERO; 4]);
        for state in store.states.iter_mut().skip(1) {
            state.flags = 0;
        }
    }
    wait_ready(&mut app);
    // Preview draws directly from its GPU bitset, with no staging allocation/map.
    let request_id = app
        .world_mut()
        .resource_mut::<PuzzleSelection>()
        .request_preview(Rect::new(0.0, 0.0, 128.0, 128.0));
    // The rectangle pipeline compiles asynchronously, after main readiness.
    let deadline = Instant::now() + Duration::from_secs(20);
    while app
        .sub_app(RenderApp)
        .world()
        .resource::<GpuRenderer>()
        .last_submitted
        != request_id
    {
        update_gpu(&mut app);
        assert!(Instant::now() < deadline, "preview submission timed out");
    }
    let pixels = rendered_pixels(&mut app, target.clone());
    let edge = (64 * 128 + 32) * 4;
    for (&actual, expected) in pixels[edge..edge + 3].iter().zip([149, 203, 255]) {
        assert!(
            (i32::from(actual) - expected).abs() <= 2,
            "preview pixel: {:?}",
            &pixels[edge..edge + 4]
        );
    }
    {
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.last_submitted, request_id);
        assert!(gpu.slots.is_empty());
        assert!(gpu.maps.is_empty());
        assert_eq!(gpu.upload_bytes, 0);
        assert_eq!(
            read_buffer(&app, &gpu.buffers.as_ref().unwrap().preview, 4),
            1u32.to_le_bytes()
        );
    }
    assert!(app
        .world()
        .resource::<PuzzleSelection>()
        .completed
        .is_none());
    assert_eq!(
        app.world().resource::<PieceDataStore>().states[0].flags,
        ENABLED
    );
    // Cancellation hides the old bitset, and an empty/outside preview clears it.
    app.world_mut().resource_mut::<PuzzleSelection>().cancel();
    let pixels = rendered_pixels(&mut app, target.clone());
    assert_eq!(&pixels[edge..edge + 4], &[255, 255, 255, 255]);
    app.world_mut()
        .resource_mut::<PuzzleSelection>()
        .request_preview(Rect::new(200.0, 200.0, 210.0, 210.0));
    update_gpu(&mut app);
    let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
    assert_eq!(
        read_buffer(&app, &gpu.buffers.as_ref().unwrap().preview, 4),
        0u32.to_le_bytes()
    );
    assert_eq!(
        pick(
            &mut app,
            Rect::new(0.0, 0.0, 128.0, 128.0),
            SelectionMode::Rectangle
        ),
        vec![PieceId(0)]
    );
    app.world_mut().resource_mut::<PuzzleSelection>().cancel();
    // A CPU position outside the viewport is translated into view by GPU culling.
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.states[0].position = Vec2::new(128.0, 0.0);
        store.states[0].flags = ENABLED | HELD;
        store.dirty_pieces.insert(PieceId(0));
        store.states[1].position = Vec2::new(128.0, 0.0);
        store.states[1].flags = ENABLED | HELD;
        store.dirty_pieces.insert(PieceId(1));
        store.drag = DragTransform {
            members: Arc::from([1u32]),
            delta: Vec2::new(-128.0, 0.0),
        };
    }
    update_gpu(&mut app);
    assert_eq!(
        app.sub_app(RenderApp)
            .world()
            .resource::<GpuRenderer>()
            .drag_upload_bytes,
        4
    );
    let pixels = rendered_pixels(&mut app, target.clone());
    let center = (64 * 128 + 64) * 4;
    assert_eq!(&pixels[center..center + 4], &[255, 255, 255, 255]);
    assert_eq!(visible_ids(&app), vec![0]);
    // The sorted alpha-blending path uses the same transform and membership.
    app.world_mut().resource_mut::<PuzzleImage>().opaque = false;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        update_gpu(&mut app);
        let world = app.sub_app(RenderApp).world();
        let gpu = world.resource::<GpuRenderer>();
        let cache = world.resource::<PipelineCache>();
        if gpu
            .main_pipelines
            .iter()
            .any(|((_, opaque), id)| !opaque && cache.get_render_pipeline(*id).is_some())
            && gpu.sort_ready(cache)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "transparent pipeline preparation timed out"
        );
    }
    let pixels = rendered_pixels(&mut app, target.clone());
    assert_eq!(&pixels[center..center + 4], &[255, 255, 255, 255]);
    assert_eq!(visible_ids(&app), vec![0]);
    assert_eq!(
        app.world().resource::<PieceDataStore>().states[0].position,
        Vec2::new(128.0, 0.0)
    );
    assert!(
        pick(
            &mut app,
            Rect::new(63.0, 63.0, 65.0, 65.0),
            SelectionMode::Point
        )
        .is_empty(),
        "held pieces stay unselectable"
    );
    // Pointer frames upload neither piece states nor the immutable membership.
    app.world_mut().resource_mut::<PieceDataStore>().drag.delta = Vec2::new(128.0, 0.0);
    update_gpu(&mut app);
    assert!(visible_ids(&app).is_empty());
    {
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.upload_bytes, 0);
        assert_eq!(gpu.drag_upload_bytes, 0);
    }
    // Release uploads the final CPU position and removes the temporary transform.
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        store.drag = default();
        store.states[0].position = Vec2::ZERO;
        store.states[0].flags = ENABLED;
        store.dirty_pieces.insert(PieceId(0));
    }
    assert_eq!(
        pick(
            &mut app,
            Rect::new(63.0, 63.0, 65.0, 65.0),
            SelectionMode::Point
        ),
        vec![PieceId(0)]
    );
    let pixels = rendered_pixels(&mut app, target);
    assert_eq!(&pixels[center..center + 4], &[255, 255, 255, 255]);
    // Session replacement must not reuse membership or preview from the old epoch.
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::splat(1000.0); 4]);
    update_gpu(&mut app);
    assert!(visible_ids(&app).is_empty());
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_raster_selection() {
    let (mut app, camera, target) = gpu_app(128);
    hash_parity(&app);
    decode_parity(&app);
    shape_parity(&app);
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
    let pixels = rendered_pixels(&mut app, target.clone());
    let profiles = piece_profiles(def.seed, def.grid_size, UVec2::ZERO);
    let mut samples = vec![];
    for y in 15..113 {
        for x in 15..113 {
            let local = Vec2::new(x as f32 - 63.5, 63.5 - y as f32);
            let inside = piece_signed_distance(local, Vec2::splat(64.0), profiles) <= 0.0;
            let drawn = pixels[(y * 128 + x) * 4] > 0;
            assert_eq!(drawn, inside, "render coverage {x},{y} {local}");
            if inside {
                assert_eq!(
                    &pixels[(y * 128 + x) * 4..(y * 128 + x) * 4 + 4],
                    &[255, 255, 255, 255],
                    "normal piece must preserve image color at {x},{y}"
                );
            }
            let d = piece_signed_distance(local, Vec2::splat(64.0), profiles);
            if d.abs() < 0.8 && (x + y) % 13 == 0 {
                samples.push((x, y, inside));
            }
        }
    }
    samples.extend([(64, 64, true), (15, 15, false), (90, 64, true)]);
    // Highlight only selected/preview silhouettes, including selected priority.
    for (flags, expected) in [
        (crate::resources::pieces::SELECTED, [255, 231, 0]),
        (crate::resources::pieces::PREVIEW, [149, 203, 255]),
        (
            crate::resources::pieces::SELECTED | crate::resources::pieces::PREVIEW,
            [255, 231, 0],
        ),
        (0, [255, 255, 255]),
    ] {
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            if flags & crate::resources::pieces::SELECTED != 0 {
                store.selected_pieces.insert(PieceId(0));
            } else {
                store.selected_pieces.clear();
            }
        }
        app.world_mut()
            .resource_mut::<PuzzleSelection>()
            .preview_active = flags & crate::resources::pieces::PREVIEW != 0;
        let render = app.sub_app(RenderApp).world();
        let gpu = render.resource::<GpuRenderer>();
        render.resource::<RenderQueue>().write_buffer(
            &gpu.buffers.as_ref().unwrap().preview,
            0,
            bytemuck::bytes_of(&1u32),
        );
        let pixels = rendered_pixels(&mut app, target.clone());
        let edge = (64 * 128 + 32) * 4;
        for (actual, expected) in pixels[edge..edge + 3].iter().zip(expected) {
            assert!(
                (i32::from(*actual) - expected).abs() <= 2,
                "highlight {flags}: {:?}",
                &pixels[edge..edge + 4]
            );
        }
        let center = (64 * 128 + 64) * 4;
        assert_eq!(&pixels[center..center + 4], &[255, 255, 255, 255]);
    }
    app.world_mut().resource_mut::<PuzzleSelection>().cancel();
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
        store.set_state(PieceId(0), s, puzzella_core::LOCAL_PLAYER);
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
            store.set_state(PieceId(1), state, puzzella_core::LOCAL_PLAYER);
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
fn gpu_resized_texture_uses_logical_coordinates_and_shared_alpha_picking() {
    use crate::resources::{ImageDecodeLimits, PuzzleImageLimits};
    use bevy::ecs::system::RunSystemOnce;
    let (mut app, _, target) = gpu_app(128);
    app.world_mut()
        .run_system_once(crate::systems::image_loading::setup_image_load_system)
        .unwrap();
    let limits = *app.world().resource::<PuzzleImageLimits>();
    assert!(limits.device_max_dimension > 0);
    assert!(limits.gpu_memory_bytes.is_none_or(|bytes| bytes > 0));
    println!(
        "Active GPU: {:?}; image limits: {limits:?}",
        app.world()
            .resource::<bevy::render::renderer::RenderAdapter>()
            .get_info()
    );
    let source = image::RgbaImage::from_fn(128, 128, |x, y| {
        if (32..96).contains(&x) && (32..96).contains(&y) {
            image::Rgba([0, 0, 0, 0])
        } else if x < 64 {
            image::Rgba([255, 0, 0, 255])
        } else {
            image::Rgba([0, 0, 255, 255])
        }
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(source)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let definition = definition(UVec2::ONE, 128, 42);
    app.world_mut().insert_resource(definition.clone());
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::ZERO]);
    for cap in [32, 16] {
        let decoded = crate::asset_reader::decode_image_bytes(
            bytes.get_ref(),
            ImageDecodeLimits {
                max_texture_dimension: cap,
            },
        )
        .unwrap();
        assert_eq!(decoded.logical_size, definition.image_size);
        let texture_size = decoded.image.size();
        // Keep the transparent sample beyond Lanczos3's support at both caps.
        let center = (texture_size.y / 2 * texture_size.x + texture_size.x / 2) as usize;
        assert_eq!(decoded.image.data.as_ref().unwrap()[center * 4 + 3], 0);
        let opaque = crate::resources::images::image_is_opaque(&decoded.image);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(decoded.image);
        app.world_mut().insert_resource(PuzzleImage {
            handle,
            logical_size: decoded.logical_size,
            texture_size,
            opaque,
        });
        wait_ready(&mut app);
        for _ in 0..4 {
            update_gpu(&mut app);
        }
        let pixels = rendered_pixels(&mut app, target.clone());
        for (x, expected) in [
            (8, [255, 0, 0, 255]),
            (120, [0, 0, 255, 255]),
            (64, [0, 0, 0, 255]),
        ] {
            let pixel = (64 * 128 + x) * 4;
            // Resampling and sRGB conversion may round an RGB channel by one.
            for channel in 0..3 {
                assert!(pixels[pixel + channel].abs_diff(expected[channel]) <= 1);
            }
            assert_eq!(pixels[pixel + 3], expected[3]);
            let rect = Rect::new(x as f32, 64.0, x as f32 + 1.0, 65.0);
            for mode in [SelectionMode::Point, SelectionMode::Rectangle] {
                assert_eq!(
                    pick(&mut app, rect, mode),
                    if x == 64 { vec![] } else { vec![PieceId(0)] },
                    "cap={cap}, x={x}, mode={mode:?}"
                );
            }
        }
        assert_eq!(
            app.world().resource::<PuzzleImage>().logical_size,
            UVec2::splat(128)
        );
        assert_eq!(
            app.world().resource::<PuzzleImage>().texture_size,
            UVec2::splat(cap)
        );
    }
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
    // A transparent one-piece puzzle also works with a partial radix workgroup.
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

#[test]
#[ignore = "requires a real GPU"]
fn gpu_radix_sort_visible_counts_and_ties() {
    use crate::resources::pieces::{MAX_Z, PLACED};
    const COUNT: u32 = 1_000_000;
    let (mut app, _, _) = gpu_app(128);
    let def = definition(UVec2::splat(1000), 128, 42);
    app.world_mut().insert_resource(def.clone());
    app.world_mut().resource_mut::<PuzzleImage>().opaque = false;
    app.world_mut()
        .resource_mut::<PieceDataStore>()
        .initialize(vec![Vec2::splat(10000.0); COUNT as usize]);
    let mut previous = vec![];
    for visible_count in [0, 1, 255, 256, 257, 1000, 65_537, COUNT, 1000, 0] {
        let mut expected: Vec<u32> = (0..visible_count)
            .map(|i| (u64::from(i) * 8191 % u64::from(COUNT)) as u32)
            .collect();
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            for &id in &previous {
                store.states[id as usize].position = Vec2::splat(10000.0);
                store.dirty_pieces.insert(PieceId(id));
            }
            for &id in &expected {
                let state = &mut store.states[id as usize];
                state.position = def.piece(id, Vec2::ZERO).correct_position;
                state.z_order = match id % 11 {
                    0 => 0,
                    1 => 254,
                    2 => 255,
                    3 => 65534,
                    4 => 65535,
                    5 => MAX_Z,
                    _ => (mix32(id) & 0x00ff_ffff).min(MAX_Z),
                };
                if id % 7 == 0 {
                    state.flags |= PLACED;
                }
                store.dirty_pieces.insert(PieceId(id));
            }
            expected.sort_unstable_by_key(|&id| {
                let state = store.states[id as usize];
                (
                    if state.flags & PLACED != 0 {
                        0
                    } else {
                        state.z_order + 1
                    },
                    id,
                )
            });
        }
        wait_ready(&mut app);
        update_gpu(&mut app);
        update_gpu(&mut app);
        let actual = visible_ids(&app);
        assert_eq!(
            actual.len(),
            expected.len(),
            "visible count {visible_count}"
        );
        assert_eq!(
            actual
                .iter()
                .zip(&expected)
                .enumerate()
                .find(|(_, (a, b))| a != b),
            None,
            "first differing ID at visible count {visible_count}",
        );
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        let dispatch = read_buffer(
            &app,
            &gpu.buffers
                .as_ref()
                .unwrap()
                .sort
                .as_ref()
                .unwrap()
                .dispatch,
            24,
        );
        assert_eq!(
            bytemuck::cast_slice::<u8, u32>(&dispatch),
            &[
                visible_count.div_ceil(256),
                1,
                1,
                if visible_count == 0 { 0 } else { 256 },
                1,
                1
            ],
            "radix workgroups must depend on visible count, not million-piece capacity",
        );
        previous = expected;
    }
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
        logical_size: UVec2::splat(side),
        texture_size: UVec2::splat(side),
        opaque: true,
    });
    let mut csv=String::from("pieces,view,visible,placement_ms,state_ms,initial_upload_prep_ms,dirty_prep_us,frame_ms,cull_gpu_ms,draw_gpu_ms,point_gpu_ms,rectangle_gpu_ms,cpu_state_bytes,gpu_state_bytes,visible_bytes,selectable_bytes,cpu_image_bytes,gpu_image_bytes,pick_visible_bytes,sort_gpu_ms,selection_and_staging_bytes,meshes,piece_entities,draw_calls,sort_workgroups,sort_dispatches,sort_scratch_bytes\n");
    for grid in [
        UVec2::new(40, 25),
        UVec2::splat(100),
        UVec2::new(400, 250),
        UVec2::splat(1000),
    ] {
        // Start each new session opaque, before buffer preparation can allocate
        // optional radix scratch from the preceding transparent session's flag.
        if !app.world().resource::<PuzzleImage>().opaque {
            let handle = app.world().resource::<PuzzleImage>().handle.clone();
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            let mut image = images.get_mut(&handle).unwrap();
            for pixel in image.data.as_mut().unwrap().chunks_exact_mut(4) {
                pixel[3] = 255;
            }
        }
        app.world_mut().resource_mut::<PuzzleImage>().opaque = true;
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
            .init_resource::<crate::resources::LocalPlayerId>()
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
            store.set_state(PieceId(0), s, puzzella_core::LOCAL_PLAYER);
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
            (
                "near_translucent",
                (1000.0 * size.x * size.y).sqrt() / 1024.0,
            ),
            (
                "medium_translucent",
                (10000.0 * size.x * size.y).sqrt() / 1024.0,
            ),
            ("entire_translucent", side as f32 / 1024.0 * 1.01),
        ] {
            let translucent = name.ends_with("_translucent");
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
            if translucent {
                assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
            }
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
            let sort_bytes = b.sort.as_ref().map_or(0, |s| {
                s.scratch.size() + s.counts.size() + s.histogram.size() + s.dispatch.size()
            });
            let (sort_workgroups, sort_dispatches) = if translucent {
                let dispatch = read_buffer(&app, &b.sort.as_ref().unwrap().dispatch, 24);
                let workgroups = u32::from_le_bytes(dispatch[..4].try_into().unwrap());
                assert_eq!(workgroups, (ids.len() as u32).div_ceil(256));
                (workgroups, 11)
            } else {
                assert!(b.sort.is_none(), "opaque puzzles need no radix scratch");
                (0, 0)
            };
            let row=format!("{count},{name},{},{placement:.4},{state_ms:.4},{prep:.4},{dirty:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{},{},{},{},{image_bytes},{image_bytes},{pick_bytes},{sort_ms:.4},{selection_bytes},{meshes},0,1,{sort_workgroups},{sort_dispatches},{sort_bytes}\n",ids.len(),frame/30.0,cull/30.0,draw/30.0,point/5.0,rectangle/5.0,app.world().resource::<PieceDataStore>().states.capacity()*16,b.states.size(),b.visible.size(),b.selectable.size());
            csv.push_str(&row);
        }
    }
    std::fs::create_dir_all("../target").unwrap();
    std::fs::write("../target/procedural-benchmark.csv", csv).unwrap();
}
