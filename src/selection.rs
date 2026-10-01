//! GPU selection of the *rendered* Mesh2d buffers, including combined batches.
//! Bevy 0.19 render graph nodes are systems in the RenderGraph schedule.
use crate::{components::*, gameplay::PieceId, resources::PieceDataStore};
use bevy::{
    camera::visibility::VisibleEntities,
    mesh::{MeshVertexAttribute, VertexBufferLayout},
    prelude::*,
    render::{
        mesh::{allocator::MeshAllocator, RenderMesh, RenderMeshBufferInfo},
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderGraph, RenderGraphSystems, RenderQueue},
        texture::GpuImage,
        Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
    },
    sprite_render::AlphaMode2d,
};
use crossbeam::channel::{unbounded, Receiver, Sender};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

pub const ATTRIBUTE_PIECE_ID: MeshVertexAttribute =
    MeshVertexAttribute::new("PuzzlePieceId", 0x5055_5a5a, VertexFormat::Uint32);

/// Stable identity entity: unaffected by batch extraction/return.
#[derive(Component, Clone, Copy)]
pub struct PuzzlePieceId(pub PieceId);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMode {
    Point,
    Rectangle,
}

#[derive(Clone, Copy, Debug)]
pub struct SelectionRequest {
    pub request_id: u64,
    /// Absolute logical coordinates in the camera render target, top-left origin.
    pub region: Rect,
    pub mode: SelectionMode,
}

#[derive(Clone, Debug)]
pub struct SelectionResult {
    pub request_id: u64,
    pub mode: SelectionMode,
    pub piece_ids: Vec<PieceId>,
    pub entities: Vec<Entity>,
    pub error: Option<String>,
}

#[derive(Resource, Default)]
pub struct PuzzleSelection {
    next_id: u64,
    pub latest: Option<SelectionRequest>,
    pub completed: Option<SelectionResult>,
    pub debug: bool,
}
impl PuzzleSelection {
    pub fn request(&mut self, region: Rect, mode: SelectionMode) -> u64 {
        self.next_id = self.next_id.checked_add(1).expect("selection ID exhausted");
        let request_id = self.next_id;
        self.latest = Some(SelectionRequest {
            request_id,
            region,
            mode,
        });
        self.completed = None;
        request_id
    }
    pub fn cancel(&mut self) {
        // Never reset next_id between sessions: late callbacks cannot alias a new request.
        self.latest = None;
        self.completed = None;
    }
    pub fn take_result(&mut self, id: u64) -> Option<SelectionResult> {
        if self.completed.as_ref().is_some_and(|r| r.request_id == id) {
            self.completed.take()
        } else {
            None
        }
    }
}

pub struct PuzzleSelectionPlugin;
impl Plugin for PuzzleSelectionPlugin {
    fn build(&self, app: &mut App) {
        let (tx, rx) = unbounded();
        app.init_resource::<PuzzleSelection>()
            .insert_resource(ResultInbox(rx))
            .add_systems(PreUpdate, receive_results);
        if app.get_sub_app(RenderApp).is_none() {
            return;
        }
        let shader = app
            .world_mut()
            .resource_mut::<Assets<Shader>>()
            .add(Shader::from_wgsl(
                include_str!("selection.wgsl"),
                "selection.wgsl",
            ));
        app.sub_app_mut(RenderApp)
            .insert_resource(GpuSelection::new(tx, shader))
            .init_resource::<ExtractedSelection>()
            .add_systems(ExtractSchedule, extract_selection)
            .add_systems(
                RenderGraph,
                selection_node.in_set(RenderGraphSystems::Render),
            )
            // Queue submission precedes mapping; poll is Bevy's regular nonblocking poll.
            .add_systems(Render, map_results.in_set(RenderSystems::Cleanup));
    }
}

#[derive(Resource)]
struct ResultInbox(Receiver<RawResult>);
struct RawResult {
    request: SelectionRequest,
    bytes: Vec<u8>,
    error: Option<String>,
}

fn decode_ids(mode: SelectionMode, bytes: &[u8]) -> Vec<PieceId> {
    if mode == SelectionMode::Point {
        return bytes
            .get(..4)
            .and_then(|b| u32::from_le_bytes(b.try_into().unwrap()).checked_sub(1))
            .map(PieceId)
            .into_iter()
            .collect();
    }
    let mut ids = Vec::new();
    for (word_index, bytes) in bytes.chunks_exact(4).enumerate() {
        let mut word = u32::from_le_bytes(bytes.try_into().unwrap());
        while word != 0 {
            ids.push(PieceId(word_index as u32 * 32 + word.trailing_zeros()));
            word &= word - 1;
        }
    }
    ids
}
fn receive_results(
    inbox: Res<ResultInbox>,
    mut selection: ResMut<PuzzleSelection>,
    ids: Query<(Entity, &PuzzlePieceId)>,
    store: Res<PieceDataStore>,
) {
    for raw in inbox.0.try_iter() {
        if !selection
            .latest
            .is_some_and(|r| r.request_id == raw.request.request_id)
        {
            continue;
        }
        let piece_ids: Vec<_> = decode_ids(raw.request.mode, &raw.bytes)
            .into_iter()
            .filter(|id| store.pieces.contains_key(id))
            .collect();
        let mapping: HashMap<_, _> = ids.iter().map(|(entity, id)| (id.0, entity)).collect();
        let entities = piece_ids
            .iter()
            .filter_map(|id| mapping.get(id).copied())
            .collect();
        let result = SelectionResult {
            request_id: raw.request.request_id,
            mode: raw.request.mode,
            piece_ids,
            entities,
            error: raw.error,
        };
        if selection.debug {
            info!(request_id = result.request_id, ids = ?result.piece_ids, entities = ?result.entities, region = ?selection.latest.map(|r| r.region), "GPU selection result");
        }
        selection.completed = Some(result);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRegion {
    pub min: UVec2,
    pub size: UVec2,
}

/// Floor/ceil covers every touched pixel. Clip before integer conversion, then
/// intersect the physical viewport. Degenerate rectangles have no fragments.
pub fn pixel_region(
    request: SelectionRequest,
    scale: f32,
    target: UVec2,
    viewport: URect,
) -> Option<PixelRegion> {
    if !scale.is_finite()
        || scale <= 0.0
        || target.min_element() == 0
        || !request.region.min.is_finite()
        || !request.region.max.is_finite()
    {
        return None;
    }
    let bounds_min = viewport.min.min(target);
    let bounds_max = viewport.max.min(target);
    if bounds_min.cmpge(bounds_max).any() {
        return None;
    }
    let a = request.region.min * scale;
    let b = request.region.max * scale;
    let (min, max) = if request.mode == SelectionMode::Point {
        let min = a.floor();
        (min, min + Vec2::ONE)
    } else {
        if a.x == b.x || a.y == b.y {
            return None;
        }
        (a.min(b).floor(), a.max(b).ceil())
    };
    let min = min.max(bounds_min.as_vec2());
    let max = max.min(bounds_max.as_vec2());
    if min.cmpge(max).any() {
        return None;
    }
    let min = min.as_uvec2();
    Some(PixelRegion {
        min,
        size: max.as_uvec2() - min,
    })
}

struct PickDraw {
    mesh: AssetId<Mesh>,
    transform: Mat4,
    material: ColorMaterial,
    z: f32,
}
#[derive(Resource, Default)]
struct ExtractedSelection {
    request: Option<SelectionRequest>,
    region: Option<PixelRegion>,
    viewport: URect,
    target: UVec2,
    clip_from_world: Mat4,
    bitset_bytes: u64,
    draws: Vec<PickDraw>,
}

#[allow(clippy::type_complexity)]
fn extract_selection(
    mut extracted: ResMut<ExtractedSelection>,
    selection: Extract<Res<PuzzleSelection>>,
    store: Extract<Res<PieceDataStore>>,
    cameras: Extract<Query<(&Camera, &GlobalTransform, &VisibleEntities), With<MainCamera>>>,
    pieces: Extract<
        Query<
            (&Mesh2d, &GlobalTransform, &MeshMaterial2d<ColorMaterial>),
            Or<(With<PuzzlePiece>, With<BatchedMeshEntity>)>,
        >,
    >,
    materials: Extract<Res<Assets<ColorMaterial>>>,
) {
    extracted.request = selection.latest;
    extracted.region = None;
    extracted.draws.clear();
    if extracted.request.is_none() {
        return;
    }
    let Ok((camera, transform, visible)) = cameras.single() else {
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
    extracted.region = pixel_region(extracted.request.unwrap(), scale, target, viewport);
    extracted.target = target;
    extracted.viewport = viewport;
    extracted.clip_from_world = camera.clip_from_view() * transform.to_matrix().inverse();
    extracted.bitset_bytes = bitset_bytes(store.pieces.keys().map(|id| id.0).max());
    for &entity in visible.get(std::any::TypeId::of::<Mesh2d>()) {
        if let Ok((mesh, transform, material)) = pieces.get(entity) {
            if let Some(material) = materials.get(&material.0) {
                extracted.draws.push(PickDraw {
                    mesh: mesh.0.id(),
                    transform: transform.to_matrix(),
                    material: material.clone(),
                    z: transform.translation().z,
                });
            }
        }
    }
    extracted.draws.sort_by(|a, b| a.z.total_cmp(&b.z));
}

fn bitset_bytes(max_id: Option<u32>) -> u64 {
    max_id.map_or(4, |id| (u64::from(id) / 32 + 1) * 4)
}

#[derive(Clone, ShaderType)]
struct PickUniform {
    clip_from_model: Mat4,
    uv_from_mesh: Mat4,
    alpha: f32,
    cutoff: f32,
    alpha_mode: u32,
    unused: u32,
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
struct Targets {
    size: UVec2,
    id: Texture,
    id_view: TextureView,
    rect_view: TextureView,
    depth_view: TextureView,
}

#[derive(Resource)]
struct GpuSelection {
    sender: Sender<RawResult>,
    shader: Handle<Shader>,
    last_submitted: u64,
    slots: Vec<Slot>,
    maps: Vec<PendingMap>,
    targets: Option<Targets>,
    uniforms: DynamicUniformBuffer<PickUniform>,
    pipelines: HashMap<(VertexBufferLayout, bool), CachedRenderPipelineId>,
    texture_groups: HashMap<AssetId<Image>, (TextureViewId, BindGroup)>,
    uniform_layout: BindGroupLayoutDescriptor,
    texture_layout: BindGroupLayoutDescriptor,
}
impl GpuSelection {
    fn new(sender: Sender<RawResult>, shader: Handle<Shader>) -> Self {
        Self {
            sender,
            shader,
            last_submitted: 0,
            slots: Vec::new(),
            maps: Vec::new(),
            targets: None,
            uniforms: DynamicUniformBuffer::default(),
            pipelines: HashMap::new(),
            texture_groups: HashMap::new(),
            uniform_layout: BindGroupLayoutDescriptor::new(
                "puzzle picking",
                &BindGroupLayoutEntries::sequential(
                    ShaderStages::VERTEX_FRAGMENT,
                    (
                        uniform_buffer::<PickUniform>(true),
                        storage_buffer_sized(false, None),
                    ),
                ),
            ),
            texture_layout: BindGroupLayoutDescriptor::new(
                "puzzle picking texture",
                &BindGroupLayoutEntries::sequential(
                    ShaderStages::FRAGMENT,
                    (
                        texture_2d(TextureSampleType::Float { filterable: true }),
                        sampler(SamplerBindingType::Filtering),
                    ),
                ),
            ),
        }
    }
    fn pipeline(
        &mut self,
        mesh: &RenderMesh,
        point: bool,
        cache: &PipelineCache,
    ) -> CachedRenderPipelineId {
        let layout = mesh
            .layout
            .0
            .get_layout(&[
                Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
                Mesh::ATTRIBUTE_UV_0.at_shader_location(1),
                ATTRIBUTE_PIECE_ID.at_shader_location(2),
            ])
            .expect("puzzle mesh must carry picking attributes");
        *self
            .pipelines
            .entry((layout.clone(), point))
            .or_insert_with(|| {
                cache.queue_render_pipeline(RenderPipelineDescriptor {
                    label: Some("puzzle selection".into()),
                    layout: vec![self.uniform_layout.clone(), self.texture_layout.clone()],
                    vertex: VertexState {
                        shader: self.shader.clone(),
                        entry_point: Some("vertex".into()),
                        buffers: vec![layout],
                        ..default()
                    },
                    fragment: Some(FragmentState {
                        shader: self.shader.clone(),
                        entry_point: Some(
                            if point {
                                "point_fragment"
                            } else {
                                "rectangle_fragment"
                            }
                            .into(),
                        ),
                        targets: vec![Some(ColorTargetState {
                            format: if point {
                                TextureFormat::R32Uint
                            } else {
                                TextureFormat::R8Unorm
                            },
                            blend: None,
                            write_mask: if point {
                                ColorWrites::ALL
                            } else {
                                ColorWrites::empty()
                            },
                        })],
                        ..default()
                    }),
                    primitive: PrimitiveState {
                        cull_mode: None,
                        ..default()
                    },
                    depth_stencil: point.then_some(DepthStencilState {
                        format: TextureFormat::Depth32Float,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(CompareFunction::GreaterEqual),
                        stencil: default(),
                        bias: default(),
                    }),
                    ..default()
                })
            })
    }
}

fn new_slot(device: &RenderDevice, bytes: u64) -> Slot {
    Slot {
        bitset: device.create_buffer(&BufferDescriptor {
            label: Some("selection bitset"),
            size: bytes,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        staging: device.create_buffer(&BufferDescriptor {
            label: Some("selection readback"),
            size: bytes.max(4),
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        busy: Arc::new(AtomicBool::new(false)),
    }
}
fn targets(device: &RenderDevice, size: UVec2) -> Targets {
    let make = |format, usage| {
        device.create_texture(&TextureDescriptor {
            label: Some("puzzle selection target"),
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
    };
    let id = make(
        TextureFormat::R32Uint,
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
    );
    let id_view = id.create_view(&default());
    Targets {
        size,
        id,
        id_view,
        rect_view: make(TextureFormat::R8Unorm, TextureUsages::RENDER_ATTACHMENT)
            .create_view(&default()),
        depth_view: make(
            TextureFormat::Depth32Float,
            TextureUsages::RENDER_ATTACHMENT,
        )
        .create_view(&default()),
    }
}

/// Clear -> scissored rasterization -> tiny copy are recorded on one encoder.
#[allow(clippy::too_many_arguments)]
fn selection_node(
    extracted: Res<ExtractedSelection>,
    mut gpu: ResMut<GpuSelection>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    cache: Res<PipelineCache>,
    meshes: Res<RenderAssets<RenderMesh>>,
    allocator: Res<MeshAllocator>,
    images: Res<RenderAssets<GpuImage>>,
    mut context: RenderContext,
) {
    let Some(request) = extracted.request else {
        return;
    };
    if request.request_id <= gpu.last_submitted {
        return;
    }
    let Some(region) = extracted.region else {
        let _ = gpu.sender.send(RawResult {
            request,
            bytes: vec![],
            error: None,
        });
        gpu.last_submitted = request.request_id;
        return;
    };
    gpu.texture_groups.retain(|id, _| images.get(*id).is_some());
    let point = request.mode == SelectionMode::Point;
    // Retry instead of returning a partial hit set while meshes/textures/pipelines load.
    let mut draws = Vec::new();
    for draw in &extracted.draws {
        let Some(mesh) = meshes.get(draw.mesh) else {
            return;
        };
        if !mesh.layout.0.contains(ATTRIBUTE_PIECE_ID) {
            return;
        }
        let pipeline = gpu.pipeline(mesh, point, &cache);
        if cache.get_render_pipeline(pipeline).is_none() {
            if let CachedPipelineState::Err(error) = cache.get_render_pipeline_state(pipeline) {
                let _ = gpu.sender.send(RawResult {
                    request,
                    bytes: vec![],
                    error: Some(error.to_string()),
                });
                gpu.last_submitted = request.request_id;
            }
            return;
        }
        let image_id = draw
            .material
            .texture
            .as_ref()
            .map(|h| h.id())
            .unwrap_or_default();
        let Some(image) = images.get(image_id) else {
            return;
        };
        let view_id = image.texture_view.id();
        if !gpu
            .texture_groups
            .get(&image_id)
            .is_some_and(|(id, _)| *id == view_id)
        {
            let group = device.create_bind_group(
                "puzzle texture",
                &cache.get_bind_group_layout(&gpu.texture_layout),
                &BindGroupEntries::sequential((&image.texture_view, &image.sampler)),
            );
            gpu.texture_groups.insert(image_id, (view_id, group));
        }
        if allocator.mesh_vertex_slice(&draw.mesh).is_none() {
            return;
        }
        if matches!(mesh.buffer_info, RenderMeshBufferInfo::Indexed { .. })
            && allocator.mesh_index_slice(&draw.mesh).is_none()
        {
            return;
        }
        draws.push((draw, mesh, pipeline, image_id));
    }
    let bytes = extracted.bitset_bytes.max(4);
    if bytes > device.limits().max_storage_buffer_binding_size {
        let _ = gpu.sender.send(RawResult {
            request,
            bytes: vec![],
            error: Some("PieceId exceeds GPU bitset limits".into()),
        });
        gpu.last_submitted = request.request_id;
        return;
    }
    let slot_index = if let Some(index) = gpu
        .slots
        .iter()
        .position(|s| !s.busy.load(Ordering::Acquire))
    {
        index
    } else if gpu.slots.len() < 3 {
        gpu.slots.push(new_slot(&device, bytes.next_power_of_two()));
        gpu.slots.len() - 1
    } else {
        return;
    }; // Bound in-flight work; latest pending request will be retried.
    if gpu.slots[slot_index].bitset.size() < bytes {
        gpu.slots[slot_index] = new_slot(&device, bytes.next_power_of_two());
    }
    if !gpu
        .targets
        .as_ref()
        .is_some_and(|t| t.size == extracted.target)
    {
        gpu.targets = Some(targets(&device, extracted.target));
    }
    gpu.uniforms.clear();
    let offsets: Vec<_> = draws
        .iter()
        .map(|(draw, _, _, _)| {
            let (mode, cutoff) = match draw.material.alpha_mode {
                AlphaMode2d::Opaque => (0, 0.0),
                AlphaMode2d::Mask(c) => (1, c),
                AlphaMode2d::Blend => (2, 0.0),
            };
            let affine = draw.material.uv_transform;
            let uv = Mat4::from_cols(
                affine.matrix2.x_axis.extend(0.0).extend(0.0),
                affine.matrix2.y_axis.extend(0.0).extend(0.0),
                Vec4::Z,
                affine.translation.extend(0.0).extend(1.0),
            );
            gpu.uniforms.push(&PickUniform {
                clip_from_model: extracted.clip_from_world * draw.transform,
                uv_from_mesh: uv,
                alpha: draw.material.color.alpha(),
                cutoff,
                alpha_mode: mode,
                unused: 0,
            })
        })
        .collect();
    // An empty visible scene still clears and returns the empty result.
    if draws.is_empty() {
        gpu.uniforms.push(&PickUniform {
            clip_from_model: Mat4::IDENTITY,
            uv_from_mesh: Mat4::IDENTITY,
            alpha: 1.0,
            cutoff: 0.0,
            alpha_mode: 0,
            unused: 0,
        });
    }
    gpu.uniforms.write_buffer(&device, &queue);
    let slot = &gpu.slots[slot_index];
    let group = device.create_bind_group(
        "puzzle picking",
        &cache.get_bind_group_layout(&gpu.uniform_layout),
        &BindGroupEntries::sequential((
            gpu.uniforms.binding().unwrap(),
            slot.bitset.as_entire_buffer_binding(),
        )),
    );
    let targets = gpu.targets.as_ref().unwrap();
    let encoder = context.command_encoder();
    encoder.clear_buffer(&slot.bitset, 0, None);
    {
        let attachments = [Some(RenderPassColorAttachment {
            view: if point {
                &targets.id_view
            } else {
                &targets.rect_view
            },
            depth_slice: None,
            resolve_target: None,
            ops: Operations {
                load: LoadOp::Clear(default()),
                store: StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("PuzzleSelectionNode"),
            color_attachments: &attachments,
            depth_stencil_attachment: point.then_some(RenderPassDepthStencilAttachment {
                view: &targets.depth_view,
                depth_ops: Some(Operations {
                    load: LoadOp::Clear(0.0),
                    store: StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..default()
        });
        let viewport = extracted.viewport;
        pass.set_viewport(
            viewport.min.x as f32,
            viewport.min.y as f32,
            viewport.width() as f32,
            viewport.height() as f32,
            0.0,
            1.0,
        );
        pass.set_scissor_rect(region.min.x, region.min.y, region.size.x, region.size.y);
        for ((draw, mesh, pipeline, image), offset) in draws.iter().zip(offsets) {
            pass.set_pipeline(cache.get_render_pipeline(*pipeline).unwrap());
            pass.set_bind_group(0, &group, &[offset]);
            pass.set_bind_group(1, &gpu.texture_groups[image].1, &[]);
            let vertices = allocator.mesh_vertex_slice(&draw.mesh).unwrap();
            pass.set_vertex_buffer(0, *vertices.buffer.slice(..));
            match mesh.buffer_info {
                RenderMeshBufferInfo::Indexed {
                    count,
                    index_format,
                } => {
                    let indices = allocator.mesh_index_slice(&draw.mesh).unwrap();
                    pass.set_index_buffer(*indices.buffer.slice(..), index_format);
                    pass.draw_indexed(
                        indices.range.start..indices.range.start + count,
                        vertices.range.start as i32,
                        0..1,
                    );
                }
                RenderMeshBufferInfo::NonIndexed => pass.draw(vertices.range, 0..1),
            }
        }
    }
    let copy_size = if point {
        encoder.copy_texture_to_buffer(
            TexelCopyTextureInfo {
                texture: &targets.id,
                mip_level: 0,
                origin: Origin3d {
                    x: region.min.x,
                    y: region.min.y,
                    z: 0,
                },
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
        encoder.copy_buffer_to_buffer(&slot.bitset, 0, &slot.staging, 0, bytes);
        bytes
    };
    slot.busy.store(true, Ordering::Release);
    gpu.maps.push(PendingMap {
        index: slot_index,
        request,
        size: copy_size,
    });
    gpu.last_submitted = request.request_id;
}

fn map_results(mut gpu: ResMut<GpuSelection>) {
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
                    Err(error) => (vec![], Some(error.to_string())),
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
mod tests {
    use super::*;
    use crate::gameplay::PieceState;
    use crate::resources::{PieceRenderData, PieceShapeData, StoredPieceData};
    use bevy::{
        asset::RenderAssetUsages,
        camera::{RenderTarget, Viewport},
        mesh::Indices,
    };

    #[test]
    fn coordinates_normalize_all_directions_and_clip_before_scissor() {
        let viewport = URect::new(20, 10, 100, 90);
        for (a, b) in [
            (Vec2::new(15., 10.), Vec2::new(40., 30.)),
            (Vec2::new(40., 30.), Vec2::new(15., 10.)),
            (Vec2::new(15., 30.), Vec2::new(40., 10.)),
            (Vec2::new(40., 10.), Vec2::new(15., 30.)),
        ] {
            let request = SelectionRequest {
                request_id: 1,
                region: Rect { min: a, max: b },
                mode: SelectionMode::Rectangle,
            };
            assert_eq!(
                pixel_region(request, 2., UVec2::splat(128), viewport),
                Some(PixelRegion {
                    min: UVec2::new(30, 20),
                    size: UVec2::new(50, 40)
                })
            );
        }
        let make = |a, b, mode| SelectionRequest {
            request_id: 1,
            region: Rect { min: a, max: b },
            mode,
        };
        assert_eq!(
            pixel_region(
                make(
                    Vec2::splat(-20.),
                    Vec2::splat(80.),
                    SelectionMode::Rectangle
                ),
                2.,
                UVec2::splat(128),
                viewport
            ),
            Some(PixelRegion {
                min: viewport.min,
                size: viewport.size()
            })
        );
        for (a, b) in [
            (Vec2::ZERO, Vec2::ZERO),
            (Vec2::ZERO, Vec2::Y),
            (Vec2::splat(-20.), Vec2::splat(-10.)),
            (Vec2::splat(f32::NAN), Vec2::ONE),
        ] {
            assert!(pixel_region(
                make(a, b, SelectionMode::Rectangle),
                2.,
                UVec2::splat(128),
                viewport
            )
            .is_none());
        }
        assert!(pixel_region(
            make(Vec2::splat(20.), Vec2::splat(40.), SelectionMode::Rectangle),
            2.,
            UVec2::ZERO,
            viewport
        )
        .is_none());
        assert!(pixel_region(
            make(Vec2::splat(-0.1), Vec2::ZERO, SelectionMode::Point),
            1.,
            UVec2::splat(128),
            URect::new(0, 0, 128, 128)
        )
        .is_none());
        assert_eq!(
            pixel_region(
                make(Vec2::splat(20.9), Vec2::ZERO, SelectionMode::Point),
                2.,
                UVec2::splat(128),
                viewport
            )
            .unwrap()
            .size,
            UVec2::ONE
        );
    }
    #[test]
    fn bitset_sizes_and_zero_id_and_word_boundaries() {
        assert_eq!(bitset_bytes(Some(9999)), 1252);
        assert_eq!(bitset_bytes(Some(31)), 4);
        assert_eq!(bitset_bytes(Some(32)), 8);
        let mut bytes = vec![0; 1252];
        for id in [0u32, 31, 32, 9999] {
            let offset = (id / 32 * 4) as usize;
            let mut word = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            word |= 1 << (id % 32);
            bytes[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        }
        assert_eq!(
            decode_ids(SelectionMode::Rectangle, &bytes),
            [0, 31, 32, 9999].map(PieceId)
        );
        assert_eq!(
            decode_ids(SelectionMode::Point, &1u32.to_le_bytes()),
            vec![PieceId(0)]
        );
        assert!(decode_ids(SelectionMode::Point, &0u32.to_le_bytes()).is_empty());
    }
    #[test]
    fn stale_callbacks_do_not_overwrite_latest_or_cross_sessions() {
        let mut app = App::new();
        let (tx, rx) = unbounded();
        app.init_resource::<PuzzleSelection>()
            .init_resource::<PieceDataStore>()
            .insert_resource(ResultInbox(rx))
            .add_systems(Update, receive_results);
        let mut requests = app.world_mut().resource_mut::<PuzzleSelection>();
        let first = requests.request(Rect::default(), SelectionMode::Point);
        let old = requests.latest.unwrap();
        requests.cancel();
        let second = requests.request(Rect::default(), SelectionMode::Point);
        assert!(second > first);
        tx.send(RawResult {
            request: old,
            bytes: 1u32.to_le_bytes().to_vec(),
            error: None,
        })
        .unwrap();
        app.update();
        assert!(app
            .world()
            .resource::<PuzzleSelection>()
            .completed
            .is_none());
    }

    fn spawn_fixture(
        app: &mut App,
        id: u32,
        mesh: Mesh,
        material: Handle<ColorMaterial>,
        position: Vec3,
    ) -> Entity {
        let mut mesh = mesh;
        mesh.insert_attribute(ATTRIBUTE_PIECE_ID, vec![id; mesh.count_vertices()]);
        let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
        let piece = PuzzlePiece {
            id: PieceId(id),
            grid_position: UVec2::ZERO,
            correct_position: Vec2::ZERO,
            initial_position: position.truncate(),
        };
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .add_piece(StoredPieceData {
                definition: piece.clone(),
                state: PieceState::new(position.truncate()),
                render: PieceRenderData {
                    bounds: Rect::default(),
                    shape: PieceShapeData {
                        vertices: vec![],
                        indices: vec![],
                        shape_hash: String::new(),
                    },
                    mesh: mesh.clone(),
                    material: material.clone(),
                },
            });
        app.world_mut().spawn(PuzzlePieceId(PieceId(id)));
        app.world_mut()
            .spawn((
                Mesh2d(mesh),
                MeshMaterial2d(material),
                piece,
                Transform::from_translation(position),
            ))
            .id()
    }
    fn gpu_result(app: &mut App, region: Rect, mode: SelectionMode) -> Vec<PieceId> {
        // Let transform propagation, visibility and asset preparation catch up.
        for _ in 0..4 {
            app.update();
        }
        let id = app
            .world_mut()
            .resource_mut::<PuzzleSelection>()
            .request(region, mode);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            app.update();
            if let Some(result) = app
                .world_mut()
                .resource_mut::<PuzzleSelection>()
                .take_result(id)
            {
                assert!(result.error.is_none(), "{:?}", result.error);
                assert_eq!(result.entities.len(), result.piece_ids.len());
                return result.piece_ids;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "GPU request timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    #[test]
    #[ignore = "requires a real GPU: cargo test --locked gpu_raster_selection -- --ignored --nocapture"]
    fn gpu_raster_selection() {
        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    ..default()
                })
                .disable::<bevy::winit::WinitPlugin>()
                .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>(),
        )
        .init_resource::<PieceDataStore>()
        .add_plugins(PuzzleSelectionPlugin);
        app.finish();
        app.cleanup();
        println!(
            "GPU adapter: {}",
            app.sub_app(RenderApp)
                .world()
                .resource::<bevy::render::renderer::RenderAdapterInfo>()
                .name
        );
        let target =
            app.world_mut()
                .resource_mut::<Assets<Image>>()
                .add(Image::new_target_texture(
                    64,
                    64,
                    TextureFormat::Rgba8UnormSrgb,
                    None,
                ));
        let camera = app
            .world_mut()
            .spawn((
                Camera2d,
                MainCamera,
                RenderTarget::Image(target.clone().into()),
                Msaa::Off,
            ))
            .id();
        let material = app
            .world_mut()
            .resource_mut::<Assets<ColorMaterial>>()
            .add(ColorMaterial::default());
        let a = spawn_fixture(
            &mut app,
            0,
            Rectangle::new(32., 32.).into(),
            material.clone(),
            Vec3::ZERO,
        );
        let b = spawn_fixture(
            &mut app,
            9999,
            Rectangle::new(32., 32.).into(),
            material.clone(),
            Vec3::Z,
        );
        let point = Rect {
            min: Vec2::splat(32.5),
            max: Vec2::splat(32.5),
        };
        let rect = Rect::new(31., 31., 33., 33.);
        assert_eq!(
            gpu_result(&mut app, rect, SelectionMode::Rectangle),
            vec![PieceId(0), PieceId(9999)]
        );
        assert_eq!(
            gpu_result(&mut app, point, SelectionMode::Point),
            vec![PieceId(9999)]
        );
        for (min, max) in [
            (rect.max, rect.min),
            (Vec2::new(31., 33.), Vec2::new(33., 31.)),
            (Vec2::new(33., 31.), Vec2::new(31., 33.)),
        ] {
            assert_eq!(
                gpu_result(&mut app, Rect { min, max }, SelectionMode::Rectangle),
                vec![PieceId(0), PieceId(9999)]
            );
        }
        app.world_mut().entity_mut(b).insert(Visibility::Hidden);
        assert_eq!(
            gpu_result(
                &mut app,
                Rect::new(47., 32., 48., 33.),
                SelectionMode::Rectangle
            ),
            vec![PieceId(0)]
        );
        assert_eq!(
            gpu_result(&mut app, point, SelectionMode::Point),
            vec![PieceId(0)]
        );
        let mut split_alpha = Image::new(
            Extent3d {
                width: 2,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![255, 255, 255, 255, 255, 255, 255, 0],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        split_alpha.sampler = bevy::image::ImageSampler::nearest();
        let split_alpha = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(split_alpha);
        let split_material =
            app.world_mut()
                .resource_mut::<Assets<ColorMaterial>>()
                .add(ColorMaterial {
                    texture: Some(split_alpha),
                    ..default()
                });
        app.world_mut()
            .entity_mut(b)
            .insert((Visibility::Visible, MeshMaterial2d(split_material.clone())));
        assert_eq!(
            gpu_result(
                &mut app,
                Rect::new(24., 32., 25., 33.),
                SelectionMode::Point
            ),
            vec![PieceId(9999)]
        );
        assert_eq!(
            gpu_result(
                &mut app,
                Rect::new(40., 32., 41., 33.),
                SelectionMode::Point
            ),
            vec![PieceId(0)]
        );
        assert_eq!(
            gpu_result(
                &mut app,
                Rect::new(40., 32., 41., 33.),
                SelectionMode::Rectangle
            ),
            vec![PieceId(0)]
        );
        app.world_mut()
            .resource_mut::<Assets<ColorMaterial>>()
            .get_mut(&split_material)
            .unwrap()
            .uv_transform = bevy::math::Affine2::from_translation(Vec2::new(0.5, 0.));
        assert_eq!(
            gpu_result(
                &mut app,
                Rect::new(24., 32., 25., 33.),
                SelectionMode::Point
            ),
            vec![PieceId(0)]
        );
        app.world_mut().entity_mut(b).insert(Visibility::Hidden);
        // Concave/empty mesh regions cannot be filled by its bounding box.
        app.world_mut().entity_mut(a).insert(Visibility::Hidden);
        let mut triangle = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        triangle.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[-16., -16., 0.], [16., -16., 0.], [-16., 16., 0.]],
        );
        triangle.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0., 0.]; 3]);
        triangle.insert_indices(Indices::U32(vec![0, 1, 2]));
        let c = spawn_fixture(&mut app, 32, triangle, material.clone(), Vec3::ZERO);
        assert!(gpu_result(
            &mut app,
            Rect::new(45., 17., 46., 18.),
            SelectionMode::Rectangle
        )
        .is_empty());
        assert_eq!(
            gpu_result(
                &mut app,
                Rect::new(18., 44., 19., 45.),
                SelectionMode::Rectangle
            ),
            vec![PieceId(32)]
        );
        app.world_mut().entity_mut(c).insert(Visibility::Hidden);
        app.world_mut().entity_mut(a).insert(Visibility::Visible);
        // Transparent front piece exposes the back piece for clicks.
        let image = Image::new_fill(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[255, 255, 255, 0],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        let transparent = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        let mask = app
            .world_mut()
            .resource_mut::<Assets<ColorMaterial>>()
            .add(ColorMaterial {
                texture: Some(transparent),
                ..default()
            });
        app.world_mut()
            .entity_mut(b)
            .insert((Visibility::Visible, MeshMaterial2d(mask.clone())));
        assert_eq!(
            gpu_result(&mut app, rect, SelectionMode::Rectangle),
            vec![PieceId(0)]
        );
        assert_eq!(
            gpu_result(&mut app, point, SelectionMode::Point),
            vec![PieceId(0)]
        );
        app.world_mut()
            .resource_mut::<Assets<ColorMaterial>>()
            .get_mut(&mask)
            .unwrap()
            .alpha_mode = AlphaMode2d::Mask(0.5);
        assert_eq!(
            gpu_result(&mut app, point, SelectionMode::Point),
            vec![PieceId(0)]
        );
        // Same projection/transform after translation, zoom and viewport offset.
        app.world_mut().entity_mut(b).insert(Visibility::Hidden);
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation
            .x = 40.;
        assert!(gpu_result(&mut app, point, SelectionMode::Point).is_empty());
        assert_eq!(
            gpu_result(
                &mut app,
                Rect::new(0., 31., 1., 32.),
                SelectionMode::Rectangle
            ),
            vec![PieceId(0)]
        );
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::new(2., 2., 1.);
        assert_eq!(
            gpu_result(
                &mut app,
                Rect::new(12., 31., 13., 32.),
                SelectionMode::Point
            ),
            vec![PieceId(0)]
        );
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation
            .x = 0.;
        app.world_mut().get_mut::<Camera>(camera).unwrap().viewport = Some(Viewport {
            physical_position: UVec2::new(8, 16),
            physical_size: UVec2::splat(32),
            ..default()
        });
        assert_eq!(
            gpu_result(
                &mut app,
                Rect::new(24., 32., 25., 33.),
                SelectionMode::Point
            ),
            vec![PieceId(0)]
        );
        assert!(gpu_result(
            &mut app,
            Rect::new(-100., -100., -10., -10.),
            SelectionMode::Rectangle
        )
        .is_empty());
        // Picking uses the very same combined vertex/index buffers as normal batching.
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale = Vec3::ONE;
        app.world_mut().get_mut::<Camera>(camera).unwrap().viewport = None;
        app.world_mut().entity_mut(a).insert(Visibility::Hidden);
        let inputs: Vec<_> = [a, b]
            .iter()
            .enumerate()
            .map(|(rank, &entity)| {
                let handle = &app.world().get::<Mesh2d>(entity).unwrap().0;
                (
                    app.world()
                        .resource::<Assets<Mesh>>()
                        .get(handle)
                        .unwrap()
                        .clone(),
                    Transform::from_xyz(0., 0., rank as f32),
                )
            })
            .collect();
        let combined = crate::systems::batching::combine_meshes(
            inputs,
            &crate::resources::PerformanceDebugLevel::Off,
        )
        .unwrap();
        let combined = app.world_mut().resource_mut::<Assets<Mesh>>().add(combined);
        app.world_mut().spawn((
            Mesh2d(combined),
            MeshMaterial2d(material),
            Transform::default(),
            BatchedMeshEntity {
                piece_count: 2,
                last_updated: std::time::Instant::now(),
            },
        ));
        assert_eq!(
            gpu_result(&mut app, rect, SelectionMode::Rectangle),
            vec![PieceId(0), PieceId(9999)]
        );
        assert_eq!(
            gpu_result(&mut app, point, SelectionMode::Point),
            vec![PieceId(9999)]
        );
        // Idle frames reuse buffers and do not re-submit the same request.
        let render = app.sub_app(RenderApp);
        let gpu = render.world().resource::<GpuSelection>();
        assert!(gpu.slots.len() <= 3);
        assert!(gpu.slots.iter().all(|s| s.bitset.size() >= 1252));
        let buffers: Vec<_> = gpu
            .slots
            .iter()
            .map(|s| (s.bitset.id(), s.staging.id()))
            .collect();
        let submitted = gpu.last_submitted;
        for _ in 0..8 {
            app.update();
        }
        let gpu = app.sub_app(RenderApp).world().resource::<GpuSelection>();
        assert_eq!(gpu.last_submitted, submitted);
        assert_eq!(
            gpu.slots
                .iter()
                .map(|s| (s.bitset.id(), s.staging.id()))
                .collect::<Vec<_>>(),
            buffers
        );
    }
}
