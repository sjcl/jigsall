//! One procedural indirect draw; GPU visibility and picking share buffers and shape.
use crate::{
    components::MainCamera,
    resources::{PieceUpload, PuzzleImage},
    selection::{api::*, coordinates::*, RawResult},
};
use bevy::{
    core_pipeline::{core_2d::main_transparent_pass_2d, schedule::Core2d, Core2dSystems},
    prelude::*,
    render::{
        diagnostic::{DiagnosticsRecorder, RecordDiagnostics},
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
        sync_world::RenderEntity,
        texture::GpuImage,
        view::ViewTarget,
        Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
    },
};
macro_rules! set_viewport {
    ($pass:expr,$viewport:expr) => {{
        let r = $viewport;
        $pass.set_viewport(
            r.min.x as f32,
            r.min.y as f32,
            r.width() as f32,
            r.height() as f32,
            0.0,
            1.0,
        );
    }};
}
use crossbeam::channel::Sender;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};

#[derive(Resource, Default)]
pub struct SelectionOverlay(pub Option<Rect>);

#[derive(Resource, Clone, Default)]
pub struct RenderReady {
    enabled: bool,
    epoch: Arc<AtomicU64>,
    error: Arc<Mutex<Option<(u64, String)>>>,
}
impl RenderReady {
    pub fn is_ready(&self, epoch: u64) -> bool {
        !self.enabled || self.epoch.load(Ordering::Acquire) == epoch
    }
    pub fn error(&self, epoch: u64) -> Option<String> {
        self.error
            .lock()
            .unwrap()
            .as_ref()
            .filter(|(e, _)| *e == epoch)
            .map(|(_, error)| error.clone())
    }
}

pub(crate) fn install(app: &mut App, tx: Sender<RawResult>) {
    let enabled = app.get_sub_app(RenderApp).is_some();
    let ready = RenderReady {
        enabled,
        ..default()
    };
    app.insert_resource(ready.clone());
    if !enabled {
        return;
    }
    let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
    let shape = shaders.add(Shader::from_wgsl(
        include_str!("puzzle_shape.wgsl"),
        "puzzle_shape.wgsl",
    ));
    let draw = shaders.add(Shader::from_wgsl(
        include_str!("puzzle_render.wgsl"),
        "puzzle_render.wgsl",
    ));
    let visibility = shaders.add(Shader::from_wgsl(
        include_str!("visibility.wgsl"),
        "visibility.wgsl",
    ));
    let pick_visibility = shaders.add(Shader::from_wgsl(
        include_str!("pick_visibility.wgsl"),
        "pick_visibility.wgsl",
    ));
    let radix_sort = shaders.add(Shader::from_wgsl(
        include_str!("radix_sort.wgsl"),
        "radix_sort.wgsl",
    ));
    app.sub_app_mut(RenderApp)
        .insert_resource(ready)
        .insert_resource(GpuRenderer::new(
            tx,
            shape,
            draw,
            visibility,
            pick_visibility,
            radix_sort,
        ))
        .init_resource::<ExtractedPuzzle>()
        .add_systems(ExtractSchedule, extract_puzzle)
        .add_systems(
            Render,
            prepare_buffers.in_set(RenderSystems::PrepareResources),
        )
        .add_systems(
            Core2d,
            puzzle_node
                .after(main_transparent_pass_2d)
                .in_set(Core2dSystems::MainPass),
        )
        .add_systems(Render, map_results.in_set(RenderSystems::Cleanup));
}

#[derive(Clone, ShaderType, Default)]
pub struct PuzzleUniform {
    pub clip_from_world: Mat4,
    pub seed: UVec2,
    pub grid: UVec2,
    pub image_size: Vec2,
    pub size: Vec2,
    pub view_min: Vec2,
    pub view_max: Vec2,
    pub count: u32,
    pub capacity: u32,
    pub opaque: u32,
    pub reserved: u32,
    pub selection_min: Vec2,
    pub selection_max: Vec2,
    pub selection_enabled: UVec4,
    pub drag_delta: Vec2,
    pub drag_active: u32,
    pub preview_active: u32,
}
#[derive(Clone, ShaderType)]
struct SortUniform {
    shift: u32,
    count: u32,
    groups: u32,
    pad: u32,
}
#[derive(Resource, Default)]
struct ExtractedPuzzle {
    upload: PieceUpload,
    image: Option<AssetId<Image>>,
    config: PuzzleUniform,
    camera: Option<Entity>,
    request: Option<SelectionRequest>,
    region: Option<PixelRegion>,
    target: UVec2,
    viewport: URect,
}
#[allow(clippy::type_complexity)] // Keep the main-world camera's ECS access explicit.
fn extract_puzzle(
    mut out: ResMut<ExtractedPuzzle>,
    upload: Extract<Option<Res<PieceUpload>>>,
    image: Extract<Option<Res<PuzzleImage>>>,
    overlay: Extract<Option<Res<SelectionOverlay>>>,
    selection: Extract<Res<PuzzleSelection>>,
    cameras: Extract<Query<(&Camera, &GlobalTransform, &RenderEntity), With<MainCamera>>>,
) {
    out.camera = None;
    out.image = None;
    out.request = selection.latest;
    out.region = None;
    let (Some(upload), Some(image)) = (upload.as_ref(), image.as_ref()) else {
        out.upload = default();
        return;
    };
    out.upload = (*upload).clone();
    let Some(def) = &out.upload.definition else {
        return;
    };
    let (grid, seed, image_size, count) = (
        def.grid_size,
        def.seed,
        def.image_size.as_vec2(),
        def.piece_count() as u32,
    );
    let Ok((camera, transform, entity)) = cameras.single() else {
        return;
    };
    if !camera.is_active {
        return;
    }
    let (Some(target), Some(viewport), Some(scale)) = (
        camera.physical_target_size(),
        camera.physical_viewport_rect(),
        camera.target_scaling_factor(),
    ) else {
        return;
    };
    let clip = camera.clip_from_view() * transform.to_matrix().inverse();
    let inverse = clip.inverse();
    let a = inverse
        .project_point3(Vec3::new(-1.0, -1.0, 0.0))
        .truncate();
    let b = inverse.project_point3(Vec3::new(1.0, 1.0, 0.0)).truncate();
    out.config = PuzzleUniform {
        clip_from_world: clip,
        seed: UVec2::new(seed as u32, (seed >> 32) as u32),
        grid,
        image_size,
        size: image_size / grid.as_vec2(),
        view_min: a.min(b),
        view_max: a.max(b),
        count,
        capacity: count,
        opaque: u32::from(image.opaque),
        reserved: 0,
        drag_delta: out.upload.drag.delta,
        drag_active: u32::from(!out.upload.drag.members.is_empty()),
        preview_active: u32::from(selection.preview_active),
        ..default()
    };
    if let Some(rect) = overlay.as_ref().and_then(|o| o.0) {
        out.config.selection_min = rect.min;
        out.config.selection_max = rect.max;
        out.config.selection_enabled = UVec4::X;
    }
    out.camera = Some(entity.id());
    out.image = Some(image.handle.id());
    out.target = target;
    out.viewport = viewport;
    out.region = out
        .request
        .and_then(|r| pixel_region(r, scale, target, viewport));
}

struct StateBuffers {
    epoch: u64,
    revision: u64,
    states: Buffer,
    visible: Buffer,
    args: Buffer,
    sort: Option<RadixBuffers>,
    selectable: Buffer,
    dummy_selection: Buffer,
    drag_members: Buffer,
    current_drag: Arc<[u32]>,
    selected: Buffer,
    current_selected: Arc<[u32]>,
    preview: Buffer,
    capacity: u32,
    pick_visible: Buffer,
    pick_args: Buffer,
}
#[derive(Clone)]
struct RadixBuffers {
    scratch: Buffer,
    counts: Buffer,
    histogram: Buffer,
    dispatch: Buffer,
}
impl RadixBuffers {
    fn new(device: &RenderDevice, count: u32) -> Self {
        let groups = count.div_ceil(256);
        Self {
            scratch: buffer(
                device,
                "radix sort scratch IDs",
                u64::from(count) * 4,
                BufferUsages::STORAGE,
            ),
            counts: buffer(
                device,
                "ordered culling group counts",
                u64::from(groups + 1) * 4,
                BufferUsages::STORAGE,
            ),
            histogram: buffer(
                device,
                "radix bucket group offsets and totals",
                u64::from(groups + 1) * 256 * 4,
                BufferUsages::STORAGE,
            ),
            dispatch: buffer(
                device,
                "radix indirect dispatch",
                24,
                BufferUsages::STORAGE | BufferUsages::INDIRECT | BufferUsages::COPY_SRC,
            ),
        }
    }
}
struct Slot {
    bitset: Buffer,
    staging: Buffer,
    busy: Arc<AtomicBool>,
}
struct PendingMap {
    index: usize,
    request: SelectionRequest,
    size: u64,
}
struct PointTargets {
    id: Texture,
    id_view: TextureView,
    depth_view: TextureView,
}
struct ScreenTarget {
    size: UVec2,
    view: TextureView,
}
#[derive(Resource)]
struct GpuRenderer {
    _shape: Handle<Shader>,
    draw_shader: Handle<Shader>,
    compute_shader: Handle<Shader>,
    pick_compute_shader: Handle<Shader>,
    sort_shader: Handle<Shader>,
    buffers: Option<StateBuffers>,
    sender: Sender<RawResult>,
    last_submitted: u64,
    slots: Vec<Slot>,
    maps: Vec<PendingMap>,
    point: Option<PointTargets>,
    rectangle: Option<ScreenTarget>,
    depth: Option<ScreenTarget>,
    uniform: UniformBuffer<PuzzleUniform>,
    pick_uniform: UniformBuffer<PuzzleUniform>,
    sort_uniform: DynamicUniformBuffer<SortUniform>,
    draw_layout: BindGroupLayoutDescriptor,
    compute_layout: BindGroupLayoutDescriptor,
    texture_layout: BindGroupLayoutDescriptor,
    selection_layout: BindGroupLayoutDescriptor,
    sort_layout: BindGroupLayoutDescriptor,
    sort_dispatch_layout: BindGroupLayoutDescriptor,
    pick_compute_layout: BindGroupLayoutDescriptor,
    pick_cull: Option<CachedComputePipelineId>,
    main_pipelines: HashMap<(TextureFormat, bool), CachedRenderPipelineId>,
    box_pipelines: HashMap<TextureFormat, CachedRenderPipelineId>,
    point_pipeline: Option<CachedRenderPipelineId>,
    rectangle_pipeline: Option<CachedRenderPipelineId>,
    cull: Option<CachedComputePipelineId>,
    cull_ordered: Option<CachedComputePipelineId>,
    sort_prepare: Option<CachedComputePipelineId>,
    sort_compact: Option<CachedComputePipelineId>,
    sort_histogram: Option<CachedComputePipelineId>,
    sort_scan: Option<CachedComputePipelineId>,
    sort_scatter: Option<CachedComputePipelineId>,
    upload_bytes: u64,
    upload_calls: usize,
    drag_upload_bytes: u64,
    selection_upload_bytes: u64,
}
impl GpuRenderer {
    fn new(
        sender: Sender<RawResult>,
        shape: Handle<Shader>,
        draw: Handle<Shader>,
        compute: Handle<Shader>,
        pick_compute: Handle<Shader>,
        radix_sort: Handle<Shader>,
    ) -> Self {
        Self {
            _shape: shape,
            draw_shader: draw,
            compute_shader: compute,
            pick_compute_shader: pick_compute,
            sort_shader: radix_sort,
            buffers: None,
            sender,
            last_submitted: 0,
            slots: vec![],
            maps: vec![],
            point: None,
            rectangle: None,
            depth: None,
            uniform: default(),
            pick_uniform: default(),
            sort_uniform: default(),
            draw_layout: BindGroupLayoutDescriptor::new(
                "procedural puzzle",
                &BindGroupLayoutEntries::sequential(
                    ShaderStages::VERTEX_FRAGMENT,
                    (
                        uniform_buffer::<PuzzleUniform>(false),
                        storage_buffer_read_only_sized(false, None),
                        storage_buffer_read_only_sized(false, None),
                        storage_buffer_read_only_sized(false, None),
                        storage_buffer_read_only_sized(false, None),
                        storage_buffer_read_only_sized(false, None),
                    ),
                ),
            ),
            compute_layout: BindGroupLayoutDescriptor::new(
                "puzzle culling",
                &BindGroupLayoutEntries::sequential(
                    ShaderStages::COMPUTE,
                    (
                        uniform_buffer::<PuzzleUniform>(false),
                        storage_buffer_read_only_sized(false, None),
                        storage_buffer_sized(false, None),
                        storage_buffer_sized(false, None),
                        storage_buffer_sized(false, None),
                        storage_buffer_read_only_sized(false, None),
                        storage_buffer_sized(false, None),
                    ),
                ),
            ),
            texture_layout: BindGroupLayoutDescriptor::new(
                "puzzle image",
                &BindGroupLayoutEntries::sequential(
                    ShaderStages::FRAGMENT,
                    (
                        texture_2d(TextureSampleType::Float { filterable: true }),
                        sampler(SamplerBindingType::Filtering),
                    ),
                ),
            ),
            selection_layout: BindGroupLayoutDescriptor::new(
                "puzzle bitsets",
                &BindGroupLayoutEntries::sequential(
                    ShaderStages::FRAGMENT,
                    (
                        storage_buffer_sized(false, None),
                        storage_buffer_read_only_sized(false, None),
                    ),
                ),
            ),
            sort_layout: BindGroupLayoutDescriptor::new(
                "puzzle radix sort",
                &BindGroupLayoutEntries::sequential(
                    ShaderStages::COMPUTE,
                    (
                        uniform_buffer::<SortUniform>(true),
                        storage_buffer_read_only_sized(false, None),
                        storage_buffer_read_only_sized(false, None),
                        storage_buffer_sized(false, None),
                        storage_buffer_read_only_sized(false, None),
                        storage_buffer_sized(false, None),
                        storage_buffer_sized(false, None),
                    ),
                ),
            ),
            sort_dispatch_layout: BindGroupLayoutDescriptor::new(
                "radix dispatch preparation",
                &BindGroupLayoutEntries::single(
                    ShaderStages::COMPUTE,
                    storage_buffer_sized(false, None),
                ),
            ),
            pick_compute_layout: BindGroupLayoutDescriptor::new(
                "picking visibility",
                &BindGroupLayoutEntries::sequential(
                    ShaderStages::COMPUTE,
                    (
                        uniform_buffer::<PuzzleUniform>(false),
                        storage_buffer_read_only_sized(false, None),
                        storage_buffer_read_only_sized(false, None),
                        storage_buffer_read_only_sized(false, None),
                        storage_buffer_sized(false, None),
                        storage_buffer_sized(false, None),
                        storage_buffer_read_only_sized(false, None),
                    ),
                ),
            ),
            pick_cull: None,
            main_pipelines: default(),
            box_pipelines: default(),
            point_pipeline: None,
            rectangle_pipeline: None,
            cull: None,
            cull_ordered: None,
            sort_prepare: None,
            sort_compact: None,
            sort_histogram: None,
            sort_scan: None,
            sort_scatter: None,
            upload_bytes: 0,
            upload_calls: 0,
            drag_upload_bytes: 0,
            selection_upload_bytes: 0,
        }
    }
    fn sort_ready(&self, cache: &PipelineCache) -> bool {
        [
            self.cull_ordered,
            self.sort_prepare,
            self.sort_compact,
            self.sort_histogram,
            self.sort_scan,
            self.sort_scatter,
        ]
        .into_iter()
        .all(|id| id.and_then(|id| cache.get_compute_pipeline(id)).is_some())
    }
    fn queue_pipelines(&mut self, cache: &PipelineCache, format: TextureFormat, opaque: bool) {
        if self.cull.is_none() {
            self.pick_cull = Some(cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some("picking visibility".into()),
                layout: vec![self.pick_compute_layout.clone()],
                shader: self.pick_compute_shader.clone(),
                entry_point: Some("cull_pick".into()),
                ..default()
            }));
            for (entry, dest) in [
                ("cull", &mut self.cull),
                ("cull_ordered", &mut self.cull_ordered),
            ] {
                *dest = Some(cache.queue_compute_pipeline(ComputePipelineDescriptor {
                    label: Some(entry.into()),
                    layout: vec![self.compute_layout.clone()],
                    shader: self.compute_shader.clone(),
                    entry_point: Some(entry.into()),
                    ..default()
                }));
            }
            for (entry, dest) in [
                ("prepare_sort", &mut self.sort_prepare),
                ("compact_visible", &mut self.sort_compact),
                ("radix_histogram", &mut self.sort_histogram),
                ("radix_scan", &mut self.sort_scan),
                ("radix_scatter", &mut self.sort_scatter),
            ] {
                let mut layout = vec![self.sort_layout.clone()];
                if entry == "prepare_sort" {
                    layout.push(self.sort_dispatch_layout.clone());
                }
                *dest = Some(cache.queue_compute_pipeline(ComputePipelineDescriptor {
                    label: Some(entry.into()),
                    layout,
                    shader: self.sort_shader.clone(),
                    entry_point: Some(entry.into()),
                    ..default()
                }));
            }
        }
        let desc = |entry: &'static str,
                    format: TextureFormat,
                    blend: Option<BlendState>,
                    depth: bool,
                    write: bool| RenderPipelineDescriptor {
            label: Some(format!("procedural {entry}").into()),
            layout: vec![
                self.draw_layout.clone(),
                self.texture_layout.clone(),
                self.selection_layout.clone(),
            ],
            vertex: VertexState {
                shader: self.draw_shader.clone(),
                entry_point: Some(
                    if entry == "box_fragment" {
                        "box_vertex"
                    } else {
                        "vertex"
                    }
                    .into(),
                ),
                buffers: vec![],
                ..default()
            },
            fragment: Some(FragmentState {
                shader: self.draw_shader.clone(),
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState {
                    format,
                    blend,
                    write_mask: if entry == "rectangle_fragment" {
                        ColorWrites::empty()
                    } else {
                        ColorWrites::ALL
                    },
                })],
                ..default()
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleStrip,
                cull_mode: None,
                ..default()
            },
            depth_stencil: depth.then_some(DepthStencilState {
                format: TextureFormat::Depth32Float,
                depth_write_enabled: Some(write),
                depth_compare: Some(CompareFunction::GreaterEqual),
                stencil: default(),
                bias: default(),
            }),
            ..default()
        };
        self.main_pipelines
            .entry((format, opaque))
            .or_insert_with(|| {
                cache.queue_render_pipeline(desc(
                    "fragment",
                    format,
                    (!opaque).then_some(BlendState::ALPHA_BLENDING),
                    true,
                    opaque,
                ))
            });
        self.box_pipelines.entry(format).or_insert_with(|| {
            cache.queue_render_pipeline(desc(
                "box_fragment",
                format,
                Some(BlendState::ALPHA_BLENDING),
                false,
                false,
            ))
        });
        self.point_pipeline.get_or_insert_with(|| {
            cache.queue_render_pipeline(desc(
                "point_fragment",
                TextureFormat::R32Uint,
                None,
                true,
                true,
            ))
        });
        self.rectangle_pipeline.get_or_insert_with(|| {
            cache.queue_render_pipeline(desc(
                "rectangle_fragment",
                TextureFormat::R8Unorm,
                None,
                false,
                false,
            ))
        });
    }
}
fn buffer(device: &RenderDevice, label: &str, size: u64, usage: BufferUsages) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: size.max(4),
        usage,
        mapped_at_creation: false,
    })
}
fn prepare_buffers(
    mut gpu: ResMut<GpuRenderer>,
    frame: Res<ExtractedPuzzle>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    ready: Res<RenderReady>,
) {
    gpu.upload_bytes = 0;
    gpu.upload_calls = 0;
    gpu.drag_upload_bytes = 0;
    gpu.selection_upload_bytes = 0;
    if frame.upload.epoch == 0 {
        gpu.buffers = None;
        return;
    }
    if gpu
        .buffers
        .as_ref()
        .is_none_or(|b| b.epoch != frame.upload.epoch)
    {
        let Some(initial) = &frame.upload.initial else {
            return;
        };
        let count = initial.len() as u32;
        if count == 0 {
            gpu.buffers = None;
            return;
        }
        if u64::from(count) * 16 > device.limits().max_storage_buffer_binding_size {
            *ready.error.lock().unwrap() = Some((
                frame.upload.epoch,
                "GPU storage buffer limit is too small for this puzzle".into(),
            ));
            return;
        }
        let capacity = count;
        let states = buffer(
            &device,
            "dense piece states",
            u64::from(count) * 16,
            BufferUsages::STORAGE | BufferUsages::COPY_DST,
        );
        queue.write_buffer(&states, 0, bytemuck::cast_slice(initial));
        gpu.upload_bytes = u64::from(count) * 16;
        gpu.upload_calls = 1;
        gpu.buffers = Some(StateBuffers {
            epoch: frame.upload.epoch,
            revision: frame.upload.revision,
            states,
            visible: buffer(
                &device,
                "visible PieceIds",
                u64::from(capacity) * 4,
                BufferUsages::STORAGE | BufferUsages::COPY_SRC,
            ),
            args: buffer(
                &device,
                "puzzle indirect draw",
                16,
                BufferUsages::STORAGE
                    | BufferUsages::INDIRECT
                    | BufferUsages::COPY_DST
                    | BufferUsages::COPY_SRC,
            ),
            sort: None,
            selectable: buffer(
                &device,
                "selectable bitset",
                u64::from(count.div_ceil(32)) * 4,
                BufferUsages::STORAGE,
            ),
            dummy_selection: buffer(
                &device,
                "unused selection binding",
                4,
                BufferUsages::STORAGE,
            ),
            drag_members: buffer(
                &device,
                "drag membership bitset",
                u64::from(count.div_ceil(32)) * 4,
                BufferUsages::STORAGE | BufferUsages::COPY_DST,
            ),
            current_drag: Arc::default(),
            selected: buffer(
                &device,
                "committed selection bitset",
                u64::from(count.div_ceil(32)) * 4,
                BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            ),
            current_selected: Arc::default(),
            preview: buffer(
                &device,
                "GPU selection preview",
                u64::from(count.div_ceil(32)) * 4,
                BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            ),
            capacity,
            pick_visible: buffer(
                &device,
                "picking candidate PieceIds",
                u64::from(capacity) * 4,
                BufferUsages::STORAGE | BufferUsages::COPY_SRC,
            ),
            pick_args: buffer(
                &device,
                "picking indirect draw",
                16,
                BufferUsages::STORAGE
                    | BufferUsages::INDIRECT
                    | BufferUsages::COPY_DST
                    | BufferUsages::COPY_SRC,
            ),
        });
    }
    let mut bytes = 0;
    let mut calls = 0;
    let Some(buffers) = &mut gpu.buffers else {
        return;
    };
    if frame.config.opaque == 0 && buffers.sort.is_none() {
        buffers.sort = Some(RadixBuffers::new(&device, buffers.capacity));
    }
    if buffers.revision != frame.upload.revision {
        for range in frame.upload.ranges.iter() {
            queue.write_buffer(
                &buffers.states,
                u64::from(range.start) * 16,
                bytemuck::cast_slice(&range.states),
            );
            bytes += range.states.len() as u64 * 16;
            calls += 1;
        }
        buffers.revision = frame.upload.revision;
    }
    gpu.upload_bytes += bytes;
    gpu.upload_calls += calls;
    let buffers = gpu.buffers.as_mut().unwrap();
    if !Arc::ptr_eq(&buffers.current_selected, &frame.upload.selected) {
        if !frame.upload.selected.is_empty() {
            queue.write_buffer(
                &buffers.selected,
                0,
                bytemuck::cast_slice(&frame.upload.selected),
            );
        }
        buffers.current_selected = frame.upload.selected.clone();
        gpu.selection_upload_bytes = frame.upload.selected.len() as u64 * 4;
    }
    let buffers = gpu.buffers.as_mut().unwrap();
    if !Arc::ptr_eq(&buffers.current_drag, &frame.upload.drag.members) {
        if !frame.upload.drag.members.is_empty() {
            queue.write_buffer(
                &buffers.drag_members,
                0,
                bytemuck::cast_slice(&frame.upload.drag.members),
            );
        }
        buffers.current_drag = frame.upload.drag.members.clone();
        gpu.drag_upload_bytes = frame.upload.drag.members.len() as u64 * 4;
    }
}
fn texture(
    device: &RenderDevice,
    size: UVec2,
    format: TextureFormat,
    usage: TextureUsages,
) -> Texture {
    device.create_texture(&TextureDescriptor {
        label: Some("puzzle attachment"),
        size: Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}
fn screen_target(device: &RenderDevice, size: UVec2, format: TextureFormat) -> ScreenTarget {
    ScreenTarget {
        size,
        view: texture(device, size, format, TextureUsages::RENDER_ATTACHMENT)
            .create_view(&default()),
    }
}
fn point_targets(device: &RenderDevice) -> PointTargets {
    let id = texture(
        device,
        UVec2::ONE,
        TextureFormat::R32Uint,
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
    );
    let id_view = id.create_view(&default());
    let depth_view = screen_target(device, UVec2::ONE, TextureFormat::Depth32Float).view;
    PointTargets {
        id,
        id_view,
        depth_view,
    }
}
fn new_slot(device: &RenderDevice, bytes: u64) -> Slot {
    Slot {
        bitset: buffer(
            device,
            "rectangle selection",
            bytes,
            BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        ),
        staging: buffer(
            device,
            "selection readback",
            bytes,
            BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        ),
        busy: Arc::new(AtomicBool::new(false)),
    }
}

#[allow(clippy::too_many_arguments)]
fn puzzle_node(
    view: ViewQuery<&ViewTarget>,
    frame: Res<ExtractedPuzzle>,
    mut gpu: ResMut<GpuRenderer>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
    images: Res<RenderAssets<GpuImage>>,
    ready: Res<RenderReady>,
    mut context: RenderContext,
) {
    if frame.camera != Some(view.entity()) {
        return;
    }
    let target = view.into_inner();
    let opaque = frame.config.opaque != 0;
    gpu.queue_pipelines(&cache, target.main_texture_format(), opaque);
    for id in [
        gpu.main_pipelines[&(target.main_texture_format(), opaque)],
        gpu.point_pipeline.unwrap(),
        gpu.rectangle_pipeline.unwrap(),
    ] {
        if let CachedPipelineState::Err(error) = cache.get_render_pipeline_state(id) {
            *ready.error.lock().unwrap() = Some((frame.upload.epoch, error.to_string()));
            return;
        }
    }
    for id in [
        gpu.cull.unwrap(),
        gpu.cull_ordered.unwrap(),
        gpu.sort_prepare.unwrap(),
        gpu.sort_compact.unwrap(),
        gpu.sort_histogram.unwrap(),
        gpu.sort_scan.unwrap(),
        gpu.sort_scatter.unwrap(),
        gpu.pick_cull.unwrap(),
    ] {
        if let CachedPipelineState::Err(error) = cache.get_compute_pipeline_state(id) {
            *ready.error.lock().unwrap() = Some((frame.upload.epoch, error.to_string()));
            return;
        }
    }
    let Some(buffers) = gpu.buffers.as_ref() else {
        return;
    };
    let (Some(image), Some(main), Some(cull)) = (
        frame.image.and_then(|id| images.get(id)),
        cache.get_render_pipeline(gpu.main_pipelines[&(target.main_texture_format(), opaque)]),
        (if opaque { gpu.cull } else { gpu.cull_ordered })
            .and_then(|id| cache.get_compute_pipeline(id)),
    ) else {
        return;
    };
    if !opaque && !gpu.sort_ready(&cache) {
        return;
    }
    let states = buffers.states.clone();
    let visible = buffers.visible.clone();
    let args = buffers.args.clone();
    let selectable = buffers.selectable.clone();
    let capacity = buffers.capacity;
    let sort = buffers.sort.clone();
    let sort_counts = sort
        .as_ref()
        .map_or(&buffers.dummy_selection, |s| &s.counts)
        .clone();
    let dummy_selection = buffers.dummy_selection.clone();
    let drag_members = buffers.drag_members.clone();
    let preview = buffers.preview.clone();
    let selected = buffers.selected.clone();
    if gpu.depth.as_ref().is_none_or(|d| d.size != frame.target) {
        gpu.depth = Some(screen_target(
            &device,
            frame.target,
            TextureFormat::Depth32Float,
        ));
    }
    gpu.uniform.set(frame.config.clone());
    gpu.uniform.write_buffer(&device, &queue);
    let compute_group = device.create_bind_group(
        "puzzle compute",
        &cache.get_bind_group_layout(&gpu.compute_layout),
        &BindGroupEntries::sequential((
            gpu.uniform.binding().unwrap(),
            states.as_entire_buffer_binding(),
            visible.as_entire_buffer_binding(),
            args.as_entire_buffer_binding(),
            selectable.as_entire_buffer_binding(),
            drag_members.as_entire_buffer_binding(),
            sort_counts.as_entire_buffer_binding(),
        )),
    );
    let draw_group = device.create_bind_group(
        "puzzle draw",
        &cache.get_bind_group_layout(&gpu.draw_layout),
        &BindGroupEntries::sequential((
            gpu.uniform.binding().unwrap(),
            states.as_entire_buffer_binding(),
            visible.as_entire_buffer_binding(),
            drag_members.as_entire_buffer_binding(),
            preview.as_entire_buffer_binding(),
            selected.as_entire_buffer_binding(),
        )),
    );
    let image_group = device.create_bind_group(
        "puzzle texture",
        &cache.get_bind_group_layout(&gpu.texture_layout),
        &BindGroupEntries::sequential((&image.texture_view, &image.sampler)),
    );
    queue.write_buffer(&args, 0, bytemuck::cast_slice(&[4u32, 0, 0, 0]));
    let mut offsets = vec![];
    if !opaque {
        gpu.sort_uniform.clear();
        for shift in [0, 8, 16] {
            offsets.push(gpu.sort_uniform.push(&SortUniform {
                shift,
                count: capacity,
                groups: capacity.div_ceil(256),
                pad: 0,
            }));
        }
        gpu.sort_uniform.write_buffer(&device, &queue);
    }
    let sort_groups = (!opaque).then(|| {
        let sort = sort.as_ref().unwrap();
        [(&visible, &sort.scratch), (&sort.scratch, &visible)].map(|(input, output)| {
            device.create_bind_group(
                "puzzle radix sort",
                &cache.get_bind_group_layout(&gpu.sort_layout),
                &BindGroupEntries::sequential((
                    gpu.sort_uniform.binding().unwrap(),
                    states.as_entire_buffer_binding(),
                    input.as_entire_buffer_binding(),
                    output.as_entire_buffer_binding(),
                    args.as_entire_buffer_binding(),
                    sort_counts.as_entire_buffer_binding(),
                    sort.histogram.as_entire_buffer_binding(),
                )),
            )
        })
    });
    let diagnostics = context.diagnostic_recorder();
    let diagnostic_ref = diagnostics.as_deref();
    let encoder = context.command_encoder();
    let visibility_span = diagnostic_ref.time_span(encoder, "puzzle_visibility");
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
        pass.set_pipeline(cull);
        pass.set_bind_group(0, &compute_group, &[]);
        pass.dispatch_workgroups(frame.config.count.div_ceil(256), 1, 1);
    }
    visibility_span.end(encoder);
    if let Some(sort_groups) = sort_groups {
        let sort = sort.as_ref().unwrap();
        let sort_span = diagnostic_ref.time_span(encoder, "puzzle_sort");
        let dispatch_group = device.create_bind_group(
            "radix dispatch preparation",
            &cache.get_bind_group_layout(&gpu.sort_dispatch_layout),
            &BindGroupEntries::single(sort.dispatch.as_entire_buffer_binding()),
        );
        // The dispatch buffer is writable only in this pass; subsequent passes
        // use it as INDIRECT without binding it as writable storage.
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
            pass.set_pipeline(
                cache
                    .get_compute_pipeline(gpu.sort_prepare.unwrap())
                    .unwrap(),
            );
            pass.set_bind_group(0, &sort_groups[0], &[offsets[0]]);
            pass.set_bind_group(1, &dispatch_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
            pass.set_pipeline(
                cache
                    .get_compute_pipeline(gpu.sort_compact.unwrap())
                    .unwrap(),
            );
            pass.set_bind_group(0, &sort_groups[0], &[offsets[0]]);
            pass.dispatch_workgroups(capacity.div_ceil(256), 1, 1);
        }
        for (digit, offset) in offsets.into_iter().enumerate() {
            // Compaction starts in scratch; three stable passes finish in visible.
            let group = &sort_groups[1 - digit % 2];
            for (pipeline, dispatch_offset) in [
                (gpu.sort_histogram.unwrap(), 0),
                (gpu.sort_scan.unwrap(), 12),
                (gpu.sort_scatter.unwrap(), 0),
            ] {
                let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor::default());
                pass.set_pipeline(cache.get_compute_pipeline(pipeline).unwrap());
                pass.set_bind_group(0, group, &[offset]);
                pass.dispatch_workgroups_indirect(&sort.dispatch, dispatch_offset);
            }
        }
        sort_span.end(encoder);
    }
    // Populate the GPU preview before the main pass samples it this frame.
    draw_selection(
        &frame,
        &mut gpu,
        &device,
        &queue,
        &cache,
        &states,
        &visible,
        &args,
        &selectable,
        &image_group,
        encoder,
        diagnostics.as_deref(),
    );
    let draw_span = diagnostic_ref.time_span(encoder, "puzzle_draw");
    {
        let colors = [Some(target.get_color_attachment())];
        let depth = Some(RenderPassDepthStencilAttachment {
            view: &gpu.depth.as_ref().unwrap().view,
            depth_ops: Some(Operations {
                load: LoadOp::Clear(0.0),
                store: StoreOp::Discard,
            }),
            stencil_ops: None,
        });
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("procedural puzzle draw"),
            color_attachments: &colors,
            depth_stencil_attachment: depth,
            ..default()
        });
        pass.set_pipeline(main);
        pass.set_bind_group(0, &draw_group, &[]);
        pass.set_bind_group(1, &image_group, &[]);
        // Main fragment doesn't access group 2, but the explicit layout requires a binding.
        let dummy = device.create_bind_group(
            "unused selection",
            &cache.get_bind_group_layout(&gpu.selection_layout),
            &BindGroupEntries::sequential((
                dummy_selection.as_entire_buffer_binding(),
                selectable.as_entire_buffer_binding(),
            )),
        );
        pass.set_bind_group(2, &dummy, &[]);
        set_viewport!(pass, frame.viewport);
        pass.draw_indirect(&args, 0);
    }
    draw_span.end(encoder);
    if frame.config.selection_enabled.x != 0 {
        if let Some(pipeline) =
            cache.get_render_pipeline(gpu.box_pipelines[&target.main_texture_format()])
        {
            let colors = [Some(target.get_color_attachment())];
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("selection box"),
                color_attachments: &colors,
                ..default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &draw_group, &[]);
            pass.set_bind_group(1, &image_group, &[]);
            let dummy = device.create_bind_group(
                "unused box selection",
                &cache.get_bind_group_layout(&gpu.selection_layout),
                &BindGroupEntries::sequential((
                    dummy_selection.as_entire_buffer_binding(),
                    selectable.as_entire_buffer_binding(),
                )),
            );
            pass.set_bind_group(2, &dummy, &[]);
            set_viewport!(pass, frame.viewport);
            pass.draw(0..4, 0..1);
        }
    }
    ready.epoch.store(frame.upload.epoch, Ordering::Release);
}

#[allow(clippy::too_many_arguments)]
fn draw_selection(
    frame: &ExtractedPuzzle,
    gpu: &mut GpuRenderer,
    device: &RenderDevice,
    queue: &RenderQueue,
    cache: &PipelineCache,
    states: &Buffer,
    visible: &Buffer,
    args: &Buffer,
    selectable: &Buffer,
    image_group: &BindGroup,
    encoder: &mut CommandEncoder,
    diagnostics: Option<&DiagnosticsRecorder>,
) {
    let Some(request) = frame.request.filter(|r| r.request_id > gpu.last_submitted) else {
        return;
    };
    let Some(region) = frame.region else {
        if request.mode == SelectionMode::Rectangle {
            encoder.clear_buffer(&gpu.buffers.as_ref().unwrap().preview, 0, None);
        }
        if request.readback {
            let _ = gpu.sender.send(RawResult {
                request,
                bytes: vec![],
                error: None,
            });
        }
        gpu.last_submitted = request.request_id;
        return;
    };
    let point = request.mode == SelectionMode::Point;
    let Some(cull_pipeline) = gpu.pick_cull.and_then(|id| cache.get_compute_pipeline(id)) else {
        return;
    };
    let pipeline = if point {
        gpu.point_pipeline
    } else {
        gpu.rectangle_pipeline
    };
    let Some(pipeline) = pipeline.and_then(|id| cache.get_render_pipeline(id)) else {
        return;
    };
    let bytes = u64::from(frame.config.count.div_ceil(32)) * 4;
    let index = if !request.readback {
        None
    } else if let Some(i) = gpu
        .slots
        .iter()
        .position(|s| !s.busy.load(Ordering::Acquire))
    {
        Some(i)
    } else if gpu.slots.len() < 3 {
        gpu.slots
            .push(new_slot(device, bytes.next_power_of_two().max(4)));
        Some(gpu.slots.len() - 1)
    } else {
        return;
    };
    if let Some(index) = index {
        if gpu.slots[index].bitset.size() < bytes {
            gpu.slots[index] = new_slot(device, bytes.next_power_of_two());
        }
    }
    if point {
        gpu.point.get_or_insert_with(|| point_targets(device));
    } else if gpu
        .rectangle
        .as_ref()
        .is_none_or(|t| t.size != frame.target)
    {
        gpu.rectangle = Some(screen_target(device, frame.target, TextureFormat::R8Unorm));
    }
    let mut config = frame.config.clone();
    if point {
        config.clip_from_world =
            point_crop_projection(frame.viewport, region.min) * config.clip_from_world;
    }
    let inverse = frame.config.clip_from_world.inverse();
    let screen_to_world = |p: UVec2| {
        let uv = (p - frame.viewport.min).as_vec2() / frame.viewport.size().as_vec2();
        inverse
            .project_point3(Vec3::new(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0))
            .truncate()
    };
    let a = screen_to_world(region.min);
    let b = screen_to_world(region.min + region.size);
    config.view_min = a.min(b);
    config.view_max = a.max(b);
    gpu.pick_uniform.set(config);
    gpu.pick_uniform.write_buffer(device, queue);
    let buffers = gpu.buffers.as_ref().unwrap();
    let pick_ids = &buffers.pick_visible;
    let pick_args = &buffers.pick_args;
    queue.write_buffer(pick_args, 0, bytemuck::cast_slice(&[4u32, 0, 0, 0]));
    let cull_group = device.create_bind_group(
        "picking ROI culling",
        &cache.get_bind_group_layout(&gpu.pick_compute_layout),
        &BindGroupEntries::sequential((
            gpu.pick_uniform.binding().unwrap(),
            states.as_entire_buffer_binding(),
            visible.as_entire_buffer_binding(),
            args.as_entire_buffer_binding(),
            pick_ids.as_entire_buffer_binding(),
            pick_args.as_entire_buffer_binding(),
            buffers.drag_members.as_entire_buffer_binding(),
        )),
    );
    let cull_span = diagnostics.time_span(encoder, "puzzle_pick_visibility");
    {
        let mut pass = encoder.begin_compute_pass(&default());
        pass.set_pipeline(cull_pipeline);
        pass.set_bind_group(0, &cull_group, &[]);
        pass.dispatch_workgroups(frame.config.count.div_ceil(256), 1, 1);
    }
    cull_span.end(encoder);
    let group = device.create_bind_group(
        "procedural picking",
        &cache.get_bind_group_layout(&gpu.draw_layout),
        &BindGroupEntries::sequential((
            gpu.pick_uniform.binding().unwrap(),
            states.as_entire_buffer_binding(),
            pick_ids.as_entire_buffer_binding(),
            buffers.drag_members.as_entire_buffer_binding(),
            buffers.dummy_selection.as_entire_buffer_binding(),
            buffers.selected.as_entire_buffer_binding(),
        )),
    );
    let bitset = if point {
        &gpu.slots[index.unwrap()].bitset
    } else {
        &buffers.preview
    };
    let bits = device.create_bind_group(
        "selection masks",
        &cache.get_bind_group_layout(&gpu.selection_layout),
        &BindGroupEntries::sequential((
            bitset.as_entire_buffer_binding(),
            selectable.as_entire_buffer_binding(),
        )),
    );
    encoder.clear_buffer(bitset, 0, None);
    let color = if point {
        &gpu.point.as_ref().unwrap().id_view
    } else {
        &gpu.rectangle.as_ref().unwrap().view
    };
    let depth = point.then(|| RenderPassDepthStencilAttachment {
        view: &gpu.point.as_ref().unwrap().depth_view,
        depth_ops: Some(Operations {
            load: LoadOp::Clear(0.0),
            store: StoreOp::Discard,
        }),
        stencil_ops: None,
    });
    let span = diagnostics.time_span(
        encoder,
        if point {
            "puzzle_point"
        } else {
            "puzzle_rectangle"
        },
    );
    {
        let colors = [Some(RenderPassColorAttachment {
            view: color,
            depth_slice: None,
            resolve_target: None,
            ops: Operations {
                load: LoadOp::Clear(default()),
                store: StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("procedural selection"),
            color_attachments: &colors,
            depth_stencil_attachment: depth,
            ..default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.set_bind_group(1, image_group, &[]);
        pass.set_bind_group(2, &bits, &[]);
        if point {
            set_viewport!(pass, URect::new(0, 0, 1, 1));
            pass.set_scissor_rect(0, 0, 1, 1);
        } else {
            set_viewport!(pass, frame.viewport);
            pass.set_scissor_rect(region.min.x, region.min.y, region.size.x, region.size.y);
        }
        pass.draw_indirect(pick_args, 0);
    }
    span.end(encoder);
    gpu.last_submitted = request.request_id;
    let Some(index) = index else {
        return;
    };
    let slot = &gpu.slots[index];
    let size = if point {
        encoder.copy_texture_to_buffer(
            TexelCopyTextureInfo {
                texture: &gpu.point.as_ref().unwrap().id,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            TexelCopyBufferInfo {
                buffer: &slot.staging,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: None,
                    rows_per_image: None,
                },
            },
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        4
    } else {
        encoder.copy_buffer_to_buffer(bitset, 0, &slot.staging, 0, bytes);
        bytes
    };
    slot.busy.store(true, Ordering::Release);
    gpu.maps.push(PendingMap {
        index,
        request,
        size,
    });
}
fn map_results(mut gpu: ResMut<GpuRenderer>) {
    for map in std::mem::take(&mut gpu.maps) {
        let slot = &gpu.slots[map.index];
        let buffer = slot.staging.clone();
        let busy = slot.busy.clone();
        let sender = gpu.sender.clone();
        slot.staging
            .slice(0..map.size)
            .map_async(MapMode::Read, move |status| {
                let (bytes, error) = match status {
                    Ok(()) => {
                        let bytes = buffer.slice(0..map.size).get_mapped_range().to_vec();
                        buffer.unmap();
                        (bytes, None)
                    }
                    Err(e) => (vec![], Some(e.to_string())),
                };
                let _ = sender.send(RawResult {
                    request: map.request,
                    bytes,
                    error,
                });
                busy.store(false, Ordering::Release);
            });
    }
}

#[cfg(test)]
mod tests;
