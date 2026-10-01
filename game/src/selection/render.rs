//! GPU selection of the *rendered* Mesh2d buffers, including combined batches.
//! Bevy 0.19 render graph nodes are systems in the RenderGraph schedule.
use crate::{components::*, resources::PieceDataStore};
use bevy::{
    camera::visibility::VisibleEntities,
    mesh::VertexBufferLayout,
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
use crossbeam::channel::Sender;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use super::{api::*, coordinates::*, RawResult};

pub(super) fn install(app: &mut App, tx: Sender<RawResult>) {
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
#[path = "render_tests.rs"]
mod tests;
