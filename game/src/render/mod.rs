//! Procedural indirect drawing; GPU visibility, shadows and picking share shape.
use crate::{
    components::MainCamera,
    resources::{
        drag_elevation::{
            prepare_drag_elevation_upload, DragElevationPresentation, DragElevationUpload,
        },
        remote_drag::{prepare_remote_drag_upload, RemoteDragPresentation, RemoteDragUpload},
        rotation_visual::{
            prepare_rotation_visual_upload, update_rotation_clock, RotationVisualUpload,
        },
        PieceUpload, PuzzleImage,
    },
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
pub mod remote_cursor;
pub mod visuals;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use visuals::{PieceVisualQuality, ResolvedPieceVisuals, PSEUDO_3D_DIRECTION};

#[derive(Resource, Default)]
pub struct SelectionOverlay(pub Option<Rect>);

#[derive(Resource, Clone, Default)]
pub struct RenderReady {
    enabled: bool,
    epoch: Arc<AtomicU64>,
    error: Arc<Mutex<Option<(u64, String)>>>,
}
impl RenderReady {
    #[cfg(test)]
    pub(crate) fn waiting_for_test() -> Self {
        Self {
            enabled: true,
            ..default()
        }
    }
    #[cfg(test)]
    pub(crate) fn signal_for_test(&self, epoch: u64) {
        self.epoch.store(epoch, Ordering::Release);
    }
    pub fn is_ready(&self, epoch: u64) -> bool {
        !self.enabled || self.epoch.load(Ordering::Acquire) == epoch
    }
    fn fail(&self, epoch: u64, error: impl Into<String>) {
        self.epoch.store(0, Ordering::Release);
        *self.error.lock().unwrap() = Some((epoch, error.into()));
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
    remote_cursor::install(app);
    let enabled = app.get_sub_app(RenderApp).is_some();
    let ready = RenderReady {
        enabled,
        ..default()
    };
    app.insert_resource(ready.clone());
    app.init_resource::<RemoteDragPresentation>()
        .init_resource::<PieceVisualQuality>()
        .init_resource::<RemoteDragUpload>()
        .init_resource::<RotationVisualUpload>()
        .init_resource::<DragElevationPresentation>()
        .init_resource::<DragElevationUpload>()
        .add_systems(First, update_rotation_clock.after(bevy::time::TimeSystems))
        .add_systems(
            Last,
            (
                prepare_remote_drag_upload,
                prepare_rotation_visual_upload,
                prepare_drag_elevation_upload.after(prepare_remote_drag_upload),
            ),
        );
    if !enabled {
        return;
    }
    let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
    let shape = shaders.add(Shader::from_wgsl(
        include_str!("puzzle_shape.wgsl"),
        "puzzle_shape.wgsl",
    ));
    let presentation = shaders.add(Shader::from_wgsl(
        include_str!("presentation.wgsl"),
        "presentation.wgsl",
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
    let component_preview = shaders.add(Shader::from_wgsl(
        include_str!("component_preview.wgsl"),
        "component_preview.wgsl",
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
            component_preview,
        ))
        .insert_resource(PresentationShader(presentation))
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

// Physical pixels in the main camera viewport, also retained for cropped picks.
const FAR_ZOOM_THRESHOLD_PX: f32 = 1.5;
const FAR_SPLAT_MIN_PX: f32 = 1.0;

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
    pub viewport_size: Vec2,
    pub viewport_origin: Vec2,
    pub piece_size_px: Vec2,
    pub pixel_world_size: Vec2,
    // Applied after geometry generation in the main viewport's pixel grid.
    // Identity for main/rectangle rendering; the crop for far-zoom point picks.
    pub render_clip_scale: Vec2,
    pub render_clip_offset: Vec2,
    pub far_zoom: u32,
    pub splat_min_px: f32,
    pub splat_padding: UVec2,
    pub rotation_time: f32,
    pub rotation_active: u32,
    pub drag_elevation_time: f32,
    pub drag_elevation_active: u32,
    pub pseudo_3d_direction: Vec2,
    pub shadow_base_offset_px: f32,
    pub shadow_lift_offset_px: f32,
    pub shadow_opacity: f32,
    pub shadow_enabled: u32,
    pub shadow_padding: UVec2,
    pub visual_cull_extent: Vec2,
    pub bevel_width_px: f32,
    pub bevel_enabled: u32,
    pub side_color: Vec4,
    pub side_thickness_px: f32,
    pub side_enabled: u32,
    pub bevel_highlight_strength: f32,
    pub bevel_shadow_strength: f32,
}
impl PuzzleUniform {
    fn configure_visuals(&mut self, visuals: ResolvedPieceVisuals) {
        self.shadow_enabled = u32::from(visuals.shadow_for_frame(self.piece_size_px));
        self.pseudo_3d_direction = PSEUDO_3D_DIRECTION;
        self.shadow_base_offset_px = visuals.shadow_base_offset_px;
        self.shadow_lift_offset_px = visuals.shadow_lift_offset_px;
        self.shadow_opacity = visuals.shadow_opacity;
        self.side_enabled = u32::from(visuals.side_for_frame(self.piece_size_px));
        self.side_color = visuals.side_color.extend(visuals.side_opacity);
        self.side_thickness_px = visuals.side_thickness_px;
        self.bevel_enabled =
            u32::from(visuals.bevel_for_frame(self.piece_size_px, self.far_zoom != 0));
        self.bevel_width_px = visuals.bevel_width_px;
        self.bevel_highlight_strength = visuals.bevel_highlight_strength;
        self.bevel_shadow_strength = visuals.bevel_shadow_strength;
        self.visual_cull_extent = Vec2::ZERO;
        let shadow_offset = if self.shadow_enabled != 0 {
            visuals.shadow_base_offset_px + visuals.shadow_lift_offset_px
        } else {
            0.0
        };
        let side_offset = if self.side_enabled != 0 {
            visuals.side_thickness_px
        } else {
            0.0
        };
        let maximum_offset = shadow_offset.max(side_offset);
        if maximum_offset > 0.0 {
            let ndc =
                PSEUDO_3D_DIRECTION * maximum_offset / self.viewport_size * Vec2::new(2.0, -2.0);
            self.visual_cull_extent = self
                .clip_from_world
                .inverse()
                .transform_vector3(ndc.extend(0.0))
                .truncate()
                .abs();
        }
    }
    fn configure_screen_space(&mut self, viewport: URect) {
        self.viewport_size = viewport.size().as_vec2();
        self.viewport_origin = viewport.min.as_vec2();
        self.pixel_world_size = (self.view_max - self.view_min) / self.viewport_size;
        self.piece_size_px = self.size / self.pixel_world_size;
        self.far_zoom = u32::from(self.piece_size_px.min_element() < FAR_ZOOM_THRESHOLD_PX);
        self.splat_min_px = FAR_SPLAT_MIN_PX;
        self.render_clip_scale = Vec2::ONE;
        self.render_clip_offset = Vec2::ZERO;
    }
}
#[derive(Clone, ShaderType)]
struct SortUniform {
    shift: u32,
    count: u32,
    groups: u32,
    pad: u32,
}
#[derive(Resource)]
struct PresentationShader(#[allow(dead_code)] Handle<Shader>);
#[derive(Resource, Default)]
struct ExtractedPuzzle {
    upload: PieceUpload,
    remote: RemoteDragUpload,
    rotation: RotationVisualUpload,
    drag_elevation: DragElevationUpload,
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
    presentations: Extract<(
        Res<RemoteDragUpload>,
        Res<RotationVisualUpload>,
        Res<PieceVisualQuality>,
        Res<DragElevationUpload>,
    )>,
    image: Extract<Option<Res<PuzzleImage>>>,
    overlay: Extract<Option<Res<SelectionOverlay>>>,
    selection: Extract<Res<PuzzleSelection>>,
    cameras: Extract<Query<(&Camera, &GlobalTransform, &RenderEntity), With<MainCamera>>>,
) {
    out.remote = (*presentations.0).clone();
    out.rotation = (*presentations.1).clone();
    out.drag_elevation = (*presentations.3).clone();
    out.camera = None;
    out.image = None;
    out.request = selection.latest;
    out.region = None;
    // A joining baseline may arrive before its image worker completes. The
    // initial state/root snapshots live for one frame, so extract them even
    // without a texture; buffer preparation does not require drawing yet.
    out.upload = upload.as_deref().cloned().unwrap_or_default();
    let Some(image) = image.as_ref() else {
        return;
    };
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
    if viewport.size().min_element() == 0 {
        return;
    }
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
        rotation_time: out.rotation.time,
        rotation_active: u32::from(out.rotation.active && out.rotation.epoch == out.upload.epoch),
        drag_elevation_time: out.drag_elevation.time,
        drag_elevation_active: u32::from(
            out.drag_elevation.active && out.drag_elevation.epoch == out.upload.epoch,
        ),
        ..default()
    };
    out.config.configure_screen_space(viewport);
    out.config.configure_visuals(presentations.2.resolve());
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

/// Three always-present SoA regions. Drag lift adds a fourth region and a
/// compact record tail lazily, keeping the idle allocation and bindings intact.
#[derive(Clone, Copy)]
struct PieceMetadataLayout {
    capacity: u32,
}
impl PieceMetadataLayout {
    fn size(self) -> u64 {
        u64::from(self.capacity) * 3 * 4
    }
    fn range_offset(self, region: u64, start: u32, len: usize) -> Option<u64> {
        // Subtract before comparing so malformed ranges cannot overflow or
        // cross into the next region, even in release builds.
        if len > self.capacity.checked_sub(start)? as usize {
            return None;
        }
        Some((u64::from(self.capacity) * region + u64::from(start)) * 4)
    }
    fn root_range_offset(self, start: u32, len: usize) -> Option<u64> {
        self.range_offset(0, start, len)
    }
    fn remote_range_offset(self, start: u32, len: usize) -> Option<u64> {
        self.range_offset(1, start, len)
    }
    fn rotation_range_offset(self, root: u32, len: usize) -> Option<u64> {
        self.range_offset(2, root, len)
    }
    fn drag_elevation_range_offset(self, start: u32, len: usize) -> Option<u64> {
        self.range_offset(3, start, len)
    }
    fn drag_elevation_records_offset(self) -> u64 {
        u64::from(self.capacity) * 4 * 4
    }
    fn drag_elevation_size(self, records: usize) -> u64 {
        self.drag_elevation_records_offset() + records as u64 * 16
    }
}

struct StateBuffers {
    epoch: u64,
    revision: u64,
    root_revision: u64,
    piece_metadata: Buffer,
    drag_elevation_capacity: usize,
    drag_elevation_revision: u64,
    states: Buffer,
    visible: Buffer,
    args: Buffer,
    sort: Option<RadixBuffers>,
    selectable: Buffer,
    dummy_selection: Buffer,
    drag_members: Buffer,
    current_drag: Arc<[u32]>,
    remote_deltas: Buffer,
    remote_revision: u64,
    remote_delta_revision: u64,
    rotation_animations: Buffer,
    rotation_capacity: usize,
    rotation_revision: u64,
    selected: Buffer,
    current_selected: Arc<[u32]>,
    preview: Buffer,
    direct_hits: Buffer,
    capacity: u32,
    pick_visible: Buffer,
    pick_args: Buffer,
}
impl StateBuffers {
    fn root_binding(&self) -> BufferBinding<'_> {
        BufferBinding {
            buffer: &self.piece_metadata,
            offset: 0,
            size: BufferSize::new(u64::from(self.capacity) * 4),
        }
    }
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
    preview_shader: Handle<Shader>,
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
    preview_layout: BindGroupLayoutDescriptor,
    preview_collapse: Option<CachedComputePipelineId>,
    pick_cull: Option<CachedComputePipelineId>,
    main_pipelines: HashMap<(TextureFormat, bool), CachedRenderPipelineId>,
    shadow_pipelines: HashMap<TextureFormat, [CachedRenderPipelineId; 2]>,
    side_pipelines: HashMap<(TextureFormat, bool), CachedRenderPipelineId>,
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
    drag_elevation_upload_bytes: u64,
    remote_mapping_upload_bytes: u64,
    remote_mapping_upload_calls: usize,
    remote_delta_upload_bytes: u64,
    rotation_upload_bytes: u64,
    selection_upload_bytes: u64,
    root_upload_bytes: u64,
    root_upload_calls: usize,
    preview_dispatches: usize,
    // Per-frame indirect draws (depth + color), not per-piece counters.
    shadow_draws: usize,
    side_draws: usize,
}
impl GpuRenderer {
    fn new(
        sender: Sender<RawResult>,
        shape: Handle<Shader>,
        draw: Handle<Shader>,
        compute: Handle<Shader>,
        pick_compute: Handle<Shader>,
        radix_sort: Handle<Shader>,
        component_preview: Handle<Shader>,
    ) -> Self {
        Self {
            _shape: shape,
            draw_shader: draw,
            compute_shader: compute,
            pick_compute_shader: pick_compute,
            sort_shader: radix_sort,
            preview_shader: component_preview,
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
                    ShaderStages::VERTEX,
                    (
                        uniform_buffer::<PuzzleUniform>(false)
                            .visibility(ShaderStages::VERTEX_FRAGMENT),
                        storage_buffer_read_only_sized(false, None), // states
                        storage_buffer_read_only_sized(false, None), // visible
                        storage_buffer_read_only_sized(false, None), // drag_members
                        storage_buffer_read_only_sized(false, None), // preview
                        storage_buffer_read_only_sized(false, None)
                            .visibility(ShaderStages::FRAGMENT), // selected
                        storage_buffer_read_only_sized(false, None), // piece_metadata
                        uniform_buffer_sized(false, None),           // remote_deltas
                        storage_buffer_read_only_sized(false, None), // rotation_animations
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
                        storage_buffer_read_only_sized(false, None),
                        uniform_buffer_sized(false, None),
                        storage_buffer_read_only_sized(false, None),
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
                        storage_buffer_read_only_sized(false, None),
                        uniform_buffer_sized(false, None),
                        storage_buffer_read_only_sized(false, None),
                    ),
                ),
            ),
            pick_cull: None,
            preview_layout: BindGroupLayoutDescriptor::new(
                "component preview",
                &BindGroupLayoutEntries::sequential(
                    ShaderStages::COMPUTE,
                    (
                        storage_buffer_read_only_sized(false, None),
                        storage_buffer_read_only_sized(false, None),
                        storage_buffer_sized(false, None),
                    ),
                ),
            ),
            preview_collapse: None,
            main_pipelines: default(),
            shadow_pipelines: default(),
            side_pipelines: default(),
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
            drag_elevation_upload_bytes: 0,
            remote_mapping_upload_bytes: 0,
            remote_mapping_upload_calls: 0,
            remote_delta_upload_bytes: 0,
            rotation_upload_bytes: 0,
            selection_upload_bytes: 0,
            root_upload_bytes: 0,
            root_upload_calls: 0,
            preview_dispatches: 0,
            shadow_draws: 0,
            side_draws: 0,
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
            self.preview_collapse = Some(cache.queue_compute_pipeline(ComputePipelineDescriptor {
                label: Some("collapse_components".into()),
                layout: vec![self.preview_layout.clone()],
                shader: self.preview_shader.clone(),
                entry_point: Some("collapse_components".into()),
                ..default()
            }));
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

    fn queue_shadow_pipelines(&mut self, cache: &PipelineCache, format: TextureFormat) {
        self.shadow_pipelines.entry(format).or_insert_with(|| {
            ["shadow_depth_fragment", "shadow_fragment"].map(|entry| {
                cache.queue_render_pipeline(RenderPipelineDescriptor {
                    label: Some(format!("procedural {entry}").into()),
                    layout: vec![
                        self.draw_layout.clone(),
                        self.texture_layout.clone(),
                        self.selection_layout.clone(),
                    ],
                    vertex: VertexState {
                        shader: self.draw_shader.clone(),
                        entry_point: Some("shadow_vertex".into()),
                        buffers: vec![],
                        ..default()
                    },
                    fragment: Some(FragmentState {
                        shader: self.draw_shader.clone(),
                        entry_point: Some(entry.into()),
                        targets: vec![Some(ColorTargetState {
                            format,
                            blend: (entry == "shadow_fragment")
                                .then_some(BlendState::ALPHA_BLENDING),
                            write_mask: if entry == "shadow_fragment" {
                                ColorWrites::ALL
                            } else {
                                ColorWrites::empty()
                            },
                        })],
                        ..default()
                    }),
                    primitive: PrimitiveState {
                        topology: PrimitiveTopology::TriangleStrip,
                        cull_mode: None,
                        ..default()
                    },
                    depth_stencil: Some(DepthStencilState {
                        format: TextureFormat::Depth32Float,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(if entry == "shadow_fragment" {
                            CompareFunction::Greater
                        } else {
                            CompareFunction::GreaterEqual
                        }),
                        stencil: default(),
                        bias: default(),
                    }),
                    ..default()
                })
            })
        });
    }

    fn queue_side_pipeline(&mut self, cache: &PipelineCache, format: TextureFormat, opaque: bool) {
        self.side_pipelines
            .entry((format, opaque))
            .or_insert_with(|| {
                cache.queue_render_pipeline(RenderPipelineDescriptor {
                    label: Some("procedural side".into()),
                    layout: vec![
                        self.draw_layout.clone(),
                        self.texture_layout.clone(),
                        self.selection_layout.clone(),
                    ],
                    vertex: VertexState {
                        shader: self.draw_shader.clone(),
                        entry_point: Some("side_vertex".into()),
                        buffers: vec![],
                        ..default()
                    },
                    fragment: Some(FragmentState {
                        shader: self.draw_shader.clone(),
                        entry_point: Some("side_fragment".into()),
                        targets: vec![Some(ColorTargetState {
                            format,
                            blend: (!opaque).then_some(BlendState::ALPHA_BLENDING),
                            write_mask: ColorWrites::ALL,
                        })],
                        ..default()
                    }),
                    primitive: PrimitiveState {
                        topology: PrimitiveTopology::TriangleStrip,
                        cull_mode: None,
                        ..default()
                    },
                    depth_stencil: Some(DepthStencilState {
                        format: TextureFormat::Depth32Float,
                        depth_write_enabled: Some(opaque),
                        depth_compare: Some(CompareFunction::GreaterEqual),
                        stencil: default(),
                        bias: default(),
                    }),
                    ..default()
                })
            });
    }
}

// Optional visuals wait before initial display, but compile independently once
// this epoch is visible. A compilation failure retains the renderer error policy.
fn optional_render_pipelines<'a, const N: usize>(
    cache: &'a PipelineCache,
    ids: [CachedRenderPipelineId; N],
    ready: &RenderReady,
    epoch: u64,
) -> Result<Option<[&'a RenderPipeline; N]>, ()> {
    for id in ids {
        if let CachedPipelineState::Err(error) = cache.get_render_pipeline_state(id) {
            ready.fail(epoch, error.to_string());
            return Err(());
        }
    }
    let pipelines = ids.map(|id| cache.get_render_pipeline(id));
    if pipelines.iter().all(Option::is_some) {
        Ok(Some(pipelines.map(Option::unwrap)))
    } else if ready.is_ready(epoch) {
        Ok(None)
    } else {
        Err(())
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
    gpu.drag_elevation_upload_bytes = 0;
    gpu.remote_mapping_upload_bytes = 0;
    gpu.remote_mapping_upload_calls = 0;
    gpu.remote_delta_upload_bytes = 0;
    gpu.rotation_upload_bytes = 0;
    gpu.selection_upload_bytes = 0;
    gpu.root_upload_bytes = 0;
    gpu.root_upload_calls = 0;
    gpu.preview_dispatches = 0;
    gpu.shadow_draws = 0;
    gpu.side_draws = 0;
    if frame.upload.epoch == 0 {
        gpu.buffers = None;
        return;
    }
    // Latch renderer failures for this puzzle. A fresh epoch can initialize again.
    if ready.error(frame.upload.epoch).is_some() {
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
            ready.fail(
                frame.upload.epoch,
                "GPU storage buffer limit is too small for this puzzle",
            );
            return;
        }
        let capacity = count;
        let Some(initial_roots) = &frame.upload.initial_roots else {
            return;
        };
        let metadata = PieceMetadataLayout { capacity };
        let Some(offset) = metadata.root_range_offset(0, initial_roots.len()) else {
            ready.fail(
                frame.upload.epoch,
                "initial component roots exceed piece capacity",
            );
            return;
        };
        let piece_metadata = buffer(
            &device,
            "piece metadata: component roots, remote slots, rotation slots",
            metadata.size(),
            BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        );
        // wgpu zero-initializes the remote/rotation regions; only roots need data.
        queue.write_buffer(&piece_metadata, offset, bytemuck::cast_slice(initial_roots));
        gpu.root_upload_bytes = u64::from(count) * 4;
        gpu.root_upload_calls = 1;
        let states = buffer(
            &device,
            "dense piece states",
            u64::from(count) * 16,
            BufferUsages::STORAGE | BufferUsages::COPY_DST,
        );
        queue.write_buffer(&states, 0, bytemuck::cast_slice(initial));
        gpu.upload_bytes = u64::from(count) * 16;
        gpu.upload_calls = 1;
        for range in frame.upload.ranges.iter() {
            queue.write_buffer(
                &states,
                u64::from(range.start) * 16,
                bytemuck::cast_slice(&range.states),
            );
            gpu.upload_bytes += range.states.len() as u64 * 16;
            gpu.upload_calls += 1;
        }
        gpu.buffers = Some(StateBuffers {
            epoch: frame.upload.epoch,
            revision: frame.upload.revision,
            root_revision: frame.upload.root_revision,
            piece_metadata,
            drag_elevation_capacity: 0,
            drag_elevation_revision: u64::MAX,
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
            remote_deltas: buffer(
                &device,
                "remote drag deltas",
                512,
                BufferUsages::UNIFORM
                    | BufferUsages::COPY_DST
                    | if cfg!(test) {
                        BufferUsages::COPY_SRC
                    } else {
                        BufferUsages::empty()
                    },
            ),
            remote_revision: u64::MAX,
            remote_delta_revision: u64::MAX,
            rotation_animations: buffer(
                &device,
                "rotation presentation records",
                32,
                BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            ),
            rotation_capacity: 1,
            rotation_revision: u64::MAX,
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
            direct_hits: buffer(
                &device,
                "direct rectangle hit mask",
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
    let mut root_bytes = 0;
    let mut root_calls = 0;
    let Some(buffers) = &mut gpu.buffers else {
        return;
    };
    let metadata = PieceMetadataLayout {
        capacity: buffers.capacity,
    };
    if frame.image.is_some() && frame.config.opaque == 0 && buffers.sort.is_none() {
        buffers.sort = Some(RadixBuffers::new(&device, buffers.capacity));
    }
    if buffers.root_revision != frame.upload.root_revision {
        for range in frame.upload.root_ranges.iter() {
            let Some(offset) = metadata.root_range_offset(range.start, range.roots.len()) else {
                ready.fail(buffers.epoch, "component root range exceeds piece capacity");
                return;
            };
            queue.write_buffer(
                &buffers.piece_metadata,
                offset,
                bytemuck::cast_slice(&range.roots),
            );
            root_bytes += range.roots.len() as u64 * 4;
            root_calls += 1;
        }
        buffers.root_revision = frame.upload.root_revision;
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
    gpu.root_upload_bytes += root_bytes;
    gpu.root_upload_calls += root_calls;
    let mut remote_bytes = 0;
    let mut remote_calls = 0;
    let mut delta_bytes = 0;
    let buffers = gpu.buffers.as_mut().unwrap();
    if frame.remote.epoch == buffers.epoch {
        if buffers.remote_revision != frame.remote.revision {
            if let Some(initial) = &frame.remote.initial {
                let Some(offset) = metadata.remote_range_offset(0, initial.len()) else {
                    ready.fail(buffers.epoch, "initial remote slots exceed piece capacity");
                    return;
                };
                queue.write_buffer(
                    &buffers.piece_metadata,
                    offset,
                    bytemuck::cast_slice(initial),
                );
                remote_bytes = initial.len() as u64 * 4;
                remote_calls = 1;
            } else {
                for range in frame.remote.ranges.iter() {
                    let Some(offset) = metadata.remote_range_offset(range.start, range.slots.len())
                    else {
                        ready.fail(buffers.epoch, "remote slot range exceeds piece capacity");
                        return;
                    };
                    queue.write_buffer(
                        &buffers.piece_metadata,
                        offset,
                        bytemuck::cast_slice(&range.slots),
                    );
                    remote_bytes += range.slots.len() as u64 * 4;
                    remote_calls += 1;
                }
            }
            buffers.remote_revision = frame.remote.revision;
        }
        if buffers.remote_delta_revision != frame.remote.delta_revision {
            queue.write_buffer(
                &buffers.remote_deltas,
                0,
                bytemuck::cast_slice(&frame.remote.deltas),
            );
            buffers.remote_delta_revision = frame.remote.delta_revision;
            delta_bytes = 512;
        }
    }
    let mut rotation_bytes = 0;
    if frame.rotation.epoch == buffers.epoch && buffers.rotation_revision != frame.rotation.revision
    {
        let count = frame.rotation.records.len();
        if count > buffers.rotation_capacity {
            buffers.rotation_capacity = count.next_power_of_two();
            buffers.rotation_animations = buffer(
                &device,
                "rotation presentation records",
                buffers.rotation_capacity as u64 * 32,
                BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            );
        }
        if count != 0 {
            queue.write_buffer(
                &buffers.rotation_animations,
                0,
                bytemuck::cast_slice(&frame.rotation.records),
            );
            rotation_bytes += count as u64 * 32;
        }
        for range in frame.rotation.ranges.iter() {
            let Some(offset) = metadata.rotation_range_offset(range.start, range.slots.len())
            else {
                ready.fail(buffers.epoch, "rotation slot range exceeds piece capacity");
                return;
            };
            queue.write_buffer(
                &buffers.piece_metadata,
                offset,
                bytemuck::cast_slice(&range.slots),
            );
            rotation_bytes += range.slots.len() as u64 * 4;
        }
        buffers.rotation_revision = frame.rotation.revision;
    }
    let mut lift_bytes = 0;
    if frame.drag_elevation.epoch == buffers.epoch
        && buffers.drag_elevation_revision != frame.drag_elevation.revision
    {
        let count = frame.drag_elevation.records.len();
        if count > buffers.drag_elevation_capacity {
            let capacity = count.next_power_of_two();
            let size = metadata.drag_elevation_size(capacity);
            if size > device.limits().max_storage_buffer_binding_size {
                ready.fail(
                    buffers.epoch,
                    "GPU storage buffer limit is too small for drag presentation",
                );
                return;
            }
            let grown = buffer(
                &device,
                "piece metadata with drag elevation",
                size,
                BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            );
            let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
                label: Some("grow presentation metadata"),
            });
            encoder.copy_buffer_to_buffer(
                &buffers.piece_metadata,
                0,
                &grown,
                0,
                buffers.piece_metadata.size(),
            );
            // Submit the preservation copy before queuing changed mappings and
            // records. No dense CPU upload is needed when the record pool grows.
            queue.submit([encoder.finish()]);
            buffers.piece_metadata = grown;
            buffers.drag_elevation_capacity = capacity;
        }
        if count != 0 {
            queue.write_buffer(
                &buffers.piece_metadata,
                metadata.drag_elevation_records_offset(),
                bytemuck::cast_slice(&frame.drag_elevation.records),
            );
            lift_bytes += count as u64 * 16;
        }
        for range in frame.drag_elevation.ranges.iter() {
            let Some(offset) = metadata.drag_elevation_range_offset(range.start, range.slots.len())
            else {
                ready.fail(buffers.epoch, "drag elevation slots exceed piece capacity");
                return;
            };
            queue.write_buffer(
                &buffers.piece_metadata,
                offset,
                bytemuck::cast_slice(&range.slots),
            );
            lift_bytes += range.slots.len() as u64 * 4;
        }
        buffers.drag_elevation_revision = frame.drag_elevation.revision;
    }
    gpu.rotation_upload_bytes = rotation_bytes;
    gpu.drag_elevation_upload_bytes = lift_bytes;
    gpu.remote_mapping_upload_bytes = remote_bytes;
    gpu.remote_mapping_upload_calls = remote_calls;
    gpu.remote_delta_upload_bytes = delta_bytes;
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
    if frame.camera != Some(view.entity()) || ready.error(frame.upload.epoch).is_some() {
        return;
    }
    let target = view.into_inner();
    let opaque = frame.config.opaque != 0;
    gpu.queue_pipelines(&cache, target.main_texture_format(), opaque);
    // Queue all requested features before waiting on any one of them.
    if frame.config.side_enabled != 0 {
        gpu.queue_side_pipeline(&cache, target.main_texture_format(), opaque);
    }
    let shadow = if frame.config.shadow_enabled != 0 {
        gpu.queue_shadow_pipelines(&cache, target.main_texture_format());
        let ids = gpu.shadow_pipelines[&target.main_texture_format()];
        match optional_render_pipelines(&cache, ids, &ready, frame.upload.epoch) {
            Ok(pipelines) => pipelines,
            Err(()) => return,
        }
    } else {
        None
    };
    let side = if frame.config.side_enabled != 0 {
        let id = gpu.side_pipelines[&(target.main_texture_format(), opaque)];
        match optional_render_pipelines(&cache, [id], &ready, frame.upload.epoch) {
            Ok(pipeline) => pipeline,
            Err(()) => return,
        }
    } else {
        None
    };
    for id in [
        gpu.main_pipelines[&(target.main_texture_format(), opaque)],
        gpu.point_pipeline.unwrap(),
        gpu.rectangle_pipeline.unwrap(),
    ] {
        if let CachedPipelineState::Err(error) = cache.get_render_pipeline_state(id) {
            ready.fail(frame.upload.epoch, error.to_string());
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
        gpu.preview_collapse.unwrap(),
    ] {
        if let CachedPipelineState::Err(error) = cache.get_compute_pipeline_state(id) {
            ready.fail(frame.upload.epoch, error.to_string());
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
    if frame.remote.epoch != buffers.epoch
        || buffers.remote_revision != frame.remote.revision
        || buffers.remote_delta_revision != frame.remote.delta_revision
    {
        return;
    }
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
    let piece_metadata = buffers.piece_metadata.clone();
    let remote_deltas = buffers.remote_deltas.clone();
    let preview = buffers.preview.clone();
    let selected = buffers.selected.clone();
    let rotation_animations = buffers.rotation_animations.clone();
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
            piece_metadata.as_entire_buffer_binding(),
            remote_deltas.as_entire_buffer_binding(),
            rotation_animations.as_entire_buffer_binding(),
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
            piece_metadata.as_entire_buffer_binding(),
            remote_deltas.as_entire_buffer_binding(),
            rotation_animations.as_entire_buffer_binding(),
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
    if let Some(pipelines) = shadow {
        let span = diagnostic_ref.time_span(encoder, "puzzle_shadow");
        let dummy = device.create_bind_group(
            "unused shadow selection",
            &cache.get_bind_group_layout(&gpu.selection_layout),
            &BindGroupEntries::sequential((
                dummy_selection.as_entire_buffer_binding(),
                selectable.as_entire_buffer_binding(),
            )),
        );
        // Resolve the front silhouette before blending, independently of opaque
        // append order or translucent back-to-front sorting. Reuse main depth.
        for (index, pipeline) in pipelines.into_iter().enumerate() {
            let colors = [Some(target.get_color_attachment())];
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some(if index == 0 {
                    "puzzle shadow depth"
                } else {
                    "puzzle shadow color"
                }),
                color_attachments: &colors,
                depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                    view: &gpu.depth.as_ref().unwrap().view,
                    depth_ops: Some(Operations {
                        load: if index == 0 {
                            LoadOp::Clear(0.0)
                        } else {
                            LoadOp::Load
                        },
                        store: if index == 0 {
                            StoreOp::Store
                        } else {
                            StoreOp::Discard
                        },
                    }),
                    stencil_ops: None,
                }),
                ..default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &draw_group, &[]);
            pass.set_bind_group(1, &image_group, &[]);
            pass.set_bind_group(2, &dummy, &[]);
            set_viewport!(pass, frame.viewport);
            pass.draw_indirect(&args, 0);
        }
        gpu.shadow_draws = 2;
        span.end(encoder);
    }
    if let Some([pipeline]) = side {
        let span = diagnostic_ref.time_span(encoder, "puzzle_side");
        let dummy = device.create_bind_group(
            "unused side selection",
            &cache.get_bind_group_layout(&gpu.selection_layout),
            &BindGroupEntries::sequential((
                dummy_selection.as_entire_buffer_binding(),
                selectable.as_entire_buffer_binding(),
            )),
        );
        let colors = [Some(target.get_color_attachment())];
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("puzzle side"),
                color_attachments: &colors,
                depth_stencil_attachment: Some(RenderPassDepthStencilAttachment {
                    view: &gpu.depth.as_ref().unwrap().view,
                    depth_ops: Some(Operations {
                        load: LoadOp::Clear(0.0),
                        store: StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &draw_group, &[]);
            pass.set_bind_group(1, &image_group, &[]);
            pass.set_bind_group(2, &dummy, &[]);
            set_viewport!(pass, frame.viewport);
            pass.draw_indirect(&args, 0);
        }
        gpu.side_draws = 1;
        span.end(encoder);
    }
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
            let buffers = gpu.buffers.as_ref().unwrap();
            encoder.clear_buffer(&buffers.direct_hits, 0, None);
            encoder.clear_buffer(&buffers.preview, 0, None);
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
    let collapse = !point && frame.config.preview_active != 0;
    if collapse
        && gpu
            .preview_collapse
            .and_then(|id| cache.get_compute_pipeline(id))
            .is_none()
    {
        return;
    }
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
    // Picking writes direct PieceId hits; only the main draw needs preview roots.
    config.preview_active = 0;
    if point {
        let crop = point_crop_projection(frame.viewport, region.min);
        if config.far_zoom != 0 {
            config.render_clip_scale = Vec2::new(crop.x_axis.x, crop.y_axis.y);
            config.render_clip_offset = crop.w_axis.truncate().truncate();
        } else {
            config.clip_from_world = crop * config.clip_from_world;
        }
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
            buffers.piece_metadata.as_entire_buffer_binding(),
            buffers.remote_deltas.as_entire_buffer_binding(),
            buffers.rotation_animations.as_entire_buffer_binding(),
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
            buffers.piece_metadata.as_entire_buffer_binding(),
            buffers.remote_deltas.as_entire_buffer_binding(),
            buffers.rotation_animations.as_entire_buffer_binding(),
        )),
    );
    let bitset = if point {
        &gpu.slots[index.unwrap()].bitset
    } else {
        &buffers.direct_hits
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
    if !point {
        encoder.clear_buffer(&buffers.preview, 0, None);
    }
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
    if collapse {
        let group = device.create_bind_group(
            "component preview",
            &cache.get_bind_group_layout(&gpu.preview_layout),
            &BindGroupEntries::sequential((
                buffers.direct_hits.as_entire_buffer_binding(),
                buffers.root_binding(),
                buffers.preview.as_entire_buffer_binding(),
            )),
        );
        let span = diagnostics.time_span(encoder, "puzzle_component_preview");
        {
            let mut pass = encoder.begin_compute_pass(&default());
            pass.set_pipeline(
                cache
                    .get_compute_pipeline(gpu.preview_collapse.unwrap())
                    .unwrap(),
            );
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(frame.config.count.div_ceil(32).div_ceil(256), 1, 1);
        }
        span.end(encoder);
        gpu.preview_dispatches += 1;
    }
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
