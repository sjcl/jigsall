//! Bounded session presentation, independent of piece storage and network runtime.
use super::*;
use crate::resources::{
    remote_cursor::RemoteCursorPresentation, AppState, GameSubState, LocalGameplayBlocked,
    LocalPlayerId,
};
use jigsall_core::PlayerId;
use std::collections::BTreeMap;

pub const MAX_REMOTE_CURSORS: usize = 64;
pub const MAX_LABEL_ATLAS_DIMENSION: u32 = 4096;
pub const MAX_LABEL_ATLAS_BYTES: usize = 4096 * 4096;

#[derive(Clone, Debug)]
pub struct CursorLabel {
    pub uv: Vec4,
    pub logical_size: Vec2,
}

/// Texture and metadata are published together with a fresh image identity.
#[derive(Clone, Debug)]
pub struct RemoteCursorLabelAtlas {
    pub revision: u64,
    pub image: Handle<Image>,
    pub labels: BTreeMap<PlayerId, CursorLabel>,
}

/// The UI resolves names and rasterizes; the renderer only consumes coverage.
#[derive(Resource, Clone)]
pub struct RemoteCursorLabels {
    /// Sorted PlayerIds, including members whose label could not be rasterized.
    pub members: Arc<[PlayerId]>,
    pub atlas: Option<Arc<RemoteCursorLabelAtlas>>,
    pub scale_factor: f32,
}
impl Default for RemoteCursorLabels {
    fn default() -> Self {
        Self {
            members: Arc::default(),
            atlas: None,
            scale_factor: 1.0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuRemoteCursor {
    pub world_position: [f32; 2],
    pub logical_size: [f32; 2],
    pub color: [f32; 4],
    pub label_uv: [f32; 4],
}

/// Shared sRGB identity color for cursor rendering and player-list UI.
pub fn player_color_srgb(player: PlayerId) -> [u8; 3] {
    const PALETTE: [[u8; 3]; 8] = [
        [255, 116, 113],
        [101, 190, 255],
        [115, 222, 157],
        [241, 199, 92],
        [189, 152, 255],
        [255, 151, 212],
        [100, 220, 224],
        [255, 173, 110],
    ];
    PALETTE[(player.0 % 8) as usize]
}

fn player_color(player: PlayerId) -> [f32; 4] {
    let rgb = player_color_srgb(player);
    // Palette colors are sRGB; the main view target receives linear values.
    let c = Color::srgb_u8(rgb[0], rgb[1], rgb[2]).to_linear();
    [c.red, c.green, c.blue, 1.0]
}

#[derive(Clone, ShaderType, Default)]
pub(super) struct CursorView {
    pub(super) clip_from_world: Mat4,
    pub(super) viewport_size: Vec2,
    viewport_origin: Vec2,
    pub(super) scale_factor: f32,
    padding: Vec3,
}

#[derive(Resource, Default)]
struct ExtractedCursors {
    instances: Vec<GpuRemoteCursor>,
    atlas: Option<Arc<RemoteCursorLabelAtlas>>,
    scale_factor: f32,
}

#[allow(clippy::too_many_arguments)] // Keep extraction inputs explicitly bounded.
fn extract_cursors(
    mut out: ResMut<ExtractedCursors>,
    presentation: Extract<Option<Res<RemoteCursorPresentation>>>,
    labels: Extract<Res<RemoteCursorLabels>>,
    local: Extract<Option<Res<LocalPlayerId>>>,
    state: Extract<Option<Res<State<GameSubState>>>>,
    next: Extract<Option<Res<NextState<GameSubState>>>>,
    app_next: Extract<Option<Res<NextState<AppState>>>>,
    blocked: Extract<Option<Res<LocalGameplayBlocked>>>,
) {
    out.instances.clear();
    out.atlas = labels.atlas.clone();
    out.scale_factor = labels.scale_factor;
    if state
        .as_ref()
        .is_none_or(|s| *s.get() != GameSubState::Playing)
        || blocked.as_ref().is_some_and(|b| b.0)
    {
        return;
    }
    if next.as_ref().is_some_and(|n| matches!(**n, NextState::Pending(s) | NextState::PendingIfNeq(s) if s != GameSubState::Playing))
        || app_next.as_ref().is_some_and(|n| matches!(**n, NextState::Pending(s) | NextState::PendingIfNeq(s) if s != AppState::InGame)) {
        return;
    }
    let (Some(presentation), Some(local)) = (presentation.as_ref(), local.as_ref()) else {
        return;
    };
    for (player, cursor) in presentation.cursors() {
        if player == local.0
            || labels.members.binary_search(&player).is_err()
            || !cursor.displayed_world_position.is_finite()
        {
            continue;
        }
        let label = labels.atlas.as_ref().and_then(|a| a.labels.get(&player));
        out.instances.push(GpuRemoteCursor {
            world_position: cursor.displayed_world_position.to_array(),
            logical_size: label.map_or([0.0; 2], |l| l.logical_size.to_array()),
            color: player_color(player),
            label_uv: label.map_or([0.0; 4], |l| l.uv.to_array()),
        });
        if out.instances.len() == MAX_REMOTE_CURSORS {
            break;
        }
    }
}

#[derive(Resource)]
pub(super) struct CursorRenderer {
    shader: Handle<Shader>,
    pub(super) view: UniformBuffer<CursorView>,
    instances: Option<Buffer>,
    pub(super) previous: Vec<GpuRemoteCursor>,
    layout: BindGroupLayoutDescriptor,
    atlas_layout: BindGroupLayoutDescriptor,
    pipelines: HashMap<TextureFormat, [CachedRenderPipelineId; 2]>,
    pub(super) instance_upload_bytes: usize,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<RemoteCursorLabels>();
    if app.get_sub_app(RenderApp).is_none() {
        return;
    }
    let shader = app
        .world_mut()
        .resource_mut::<Assets<Shader>>()
        .add(Shader::from_wgsl(
            include_str!("remote_cursor.wgsl"),
            "remote_cursor.wgsl",
        ));
    app.sub_app_mut(RenderApp)
        .init_resource::<ExtractedCursors>()
        .insert_resource(CursorRenderer {
            shader,
            view: default(),
            instances: None,
            previous: vec![],
            layout: BindGroupLayoutDescriptor::new(
                "remote cursor instances",
                &BindGroupLayoutEntries::sequential(
                    ShaderStages::VERTEX_FRAGMENT,
                    (
                        uniform_buffer::<CursorView>(false),
                        storage_buffer_read_only_sized(false, None),
                    ),
                ),
            ),
            atlas_layout: BindGroupLayoutDescriptor::new(
                "remote cursor coverage",
                &BindGroupLayoutEntries::sequential(
                    ShaderStages::FRAGMENT,
                    (
                        texture_2d(TextureSampleType::Float { filterable: true }),
                        sampler(SamplerBindingType::Filtering),
                    ),
                ),
            ),
            pipelines: HashMap::new(),
            instance_upload_bytes: 0,
        })
        .add_systems(ExtractSchedule, extract_cursors)
        .add_systems(
            Render,
            prepare_cursors.in_set(RenderSystems::PrepareResources),
        )
        .add_systems(
            Core2d,
            cursor_node
                .after(puzzle_node)
                .in_set(Core2dSystems::MainPass),
        );
}

fn prepare_cursors(
    frame: Res<ExtractedCursors>,
    puzzle: Res<ExtractedPuzzle>,
    mut gpu: ResMut<CursorRenderer>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    gpu.instance_upload_bytes = 0;
    if frame.instances != gpu.previous {
        if !frame.instances.is_empty() {
            let instances = gpu.instances.get_or_insert_with(|| {
                buffer(
                    &device,
                    "remote cursor bounded instances",
                    (MAX_REMOTE_CURSORS * size_of::<GpuRemoteCursor>()) as u64,
                    BufferUsages::STORAGE | BufferUsages::COPY_DST,
                )
            });
            let bytes = bytemuck::cast_slice(&frame.instances);
            queue.write_buffer(instances, 0, bytes);
            gpu.instance_upload_bytes = bytes.len();
        }
        gpu.previous.clone_from(&frame.instances);
    }
    if frame.instances.is_empty() || puzzle.camera.is_none() {
        return;
    }
    // Copy the already extracted puzzle camera, never project in a UI schedule.
    let view = CursorView {
        clip_from_world: puzzle.config.clip_from_world,
        viewport_size: puzzle.config.viewport_size,
        viewport_origin: puzzle.config.viewport_origin,
        scale_factor: frame.scale_factor,
        ..default()
    };
    if gpu.view.get().clip_from_world != view.clip_from_world
        || gpu.view.get().viewport_size != view.viewport_size
        || gpu.view.get().viewport_origin != view.viewport_origin
        || gpu.view.get().scale_factor != view.scale_factor
        || gpu.view.buffer().is_none()
    {
        gpu.view.set(view);
        gpu.view.write_buffer(&device, &queue);
    }
}

#[allow(clippy::too_many_arguments)] // Dedicated render pass resources.
fn cursor_node(
    view: ViewQuery<&ViewTarget>,
    puzzle: Res<ExtractedPuzzle>,
    frame: Res<ExtractedCursors>,
    mut gpu: ResMut<CursorRenderer>,
    device: Res<RenderDevice>,
    cache: Res<PipelineCache>,
    images: Res<RenderAssets<GpuImage>>,
    mut context: RenderContext,
) {
    if frame.instances.is_empty() || puzzle.camera != Some(view.entity()) {
        return;
    }
    let target = view.into_inner();
    let format = target.main_texture_format();
    if !gpu.pipelines.contains_key(&format) {
        let pipelines = [
            ("marker_vertex", "marker_fragment"),
            ("label_vertex", "label_fragment"),
        ]
        .map(|(vertex, fragment)| {
            cache.queue_render_pipeline(RenderPipelineDescriptor {
                label: Some(format!("remote cursor {fragment}").into()),
                layout: if vertex == "marker_vertex" {
                    vec![gpu.layout.clone()]
                } else {
                    vec![gpu.layout.clone(), gpu.atlas_layout.clone()]
                },
                vertex: VertexState {
                    shader: gpu.shader.clone(),
                    entry_point: Some(vertex.into()),
                    ..default()
                },
                fragment: Some(FragmentState {
                    shader: gpu.shader.clone(),
                    entry_point: Some(fragment.into()),
                    targets: vec![Some(ColorTargetState {
                        format,
                        blend: Some(BlendState::ALPHA_BLENDING),
                        write_mask: ColorWrites::ALL,
                    })],
                    ..default()
                }),
                primitive: PrimitiveState {
                    topology: PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..default()
                },
                depth_stencil: None,
                ..default()
            })
        });
        gpu.pipelines.insert(format, pipelines);
    }
    let Some(instances) = gpu.instances.as_ref() else {
        return;
    };
    let group = device.create_bind_group(
        "remote cursors",
        &cache.get_bind_group_layout(&gpu.layout),
        &BindGroupEntries::sequential((
            gpu.view.binding().unwrap(),
            instances.as_entire_buffer_binding(),
        )),
    );
    let atlas = frame.atlas.as_ref().and_then(|a| images.get(a.image.id()));
    let atlas_group = atlas.map(|image| {
        device.create_bind_group(
            "remote cursor label atlas",
            &cache.get_bind_group_layout(&gpu.atlas_layout),
            &BindGroupEntries::sequential((&image.texture_view, &image.sampler)),
        )
    });
    let diagnostics = context.diagnostic_recorder();
    let diagnostic_ref = diagnostics.as_deref();
    let encoder = context.command_encoder();
    for (index, vertices) in [(0, 3), (1, 6)] {
        if index == 1 && atlas_group.is_none() {
            continue;
        }
        let Some(pipeline) = cache.get_render_pipeline(gpu.pipelines[&format][index]) else {
            continue;
        };
        let span = diagnostic_ref.time_span(
            encoder,
            if index == 0 {
                "remote_cursor_marker"
            } else {
                "remote_cursor_label"
            },
        );
        {
            let colors = [Some(target.get_color_attachment())];
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("remote cursor presentation"),
                color_attachments: &colors,
                ..default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[]);
            if index == 1 {
                pass.set_bind_group(1, atlas_group.as_ref().unwrap(), &[]);
            }
            set_viewport!(pass, puzzle.viewport);
            pass.set_scissor_rect(
                puzzle.viewport.min.x,
                puzzle.viewport.min.y,
                puzzle.viewport.width(),
                puzzle.viewport.height(),
            );
            pass.draw(0..vertices, 0..frame.instances.len() as u32);
        }
        span.end(encoder);
    }
}
