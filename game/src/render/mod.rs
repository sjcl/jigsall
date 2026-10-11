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
mod buffers;
mod draw;
mod extraction;
mod picking;
mod pipelines;
mod uniforms;
use buffers::*;
use draw::puzzle_node;
use extraction::*;
use picking::*;
use pipelines::optional_render_pipelines;
pub use uniforms::PuzzleUniform;
use uniforms::SortUniform;

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
    let shape = shaders.add(Shader::from_wesl(
        include_str!("puzzle_shape.wesl"),
        "embedded://jigsall_game/render/puzzle_shape.wesl",
    ));
    let presentation = shaders.add(Shader::from_wesl(
        include_str!("presentation.wesl"),
        "embedded://jigsall_game/render/presentation.wesl",
    ));
    let draw = shaders.add(Shader::from_wesl(
        include_str!("puzzle_render.wesl"),
        "embedded://jigsall_game/render/puzzle_render.wesl",
    ));
    let visibility = shaders.add(Shader::from_wesl(
        include_str!("visibility.wesl"),
        "embedded://jigsall_game/render/visibility.wesl",
    ));
    let pick_visibility = shaders.add(Shader::from_wesl(
        include_str!("pick_visibility.wesl"),
        "embedded://jigsall_game/render/pick_visibility.wesl",
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
            prepare_buffers
                .in_set(RenderSystems::PrepareResources)
                .before(RenderSystems::Render),
        )
        .add_systems(
            Core2d,
            puzzle_node
                .after(main_transparent_pass_2d)
                .in_set(Core2dSystems::MainPass),
        )
        .add_systems(
            Render,
            // Readback mapping must start after the frame has been submitted.
            map_results
                .in_set(RenderSystems::Cleanup)
                .after(RenderSystems::Render),
        );
}

#[derive(Resource)]
struct PresentationShader(#[allow(dead_code)] Handle<Shader>);
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
#[cfg(test)]
mod tests;
