//! Point/rectangle targets and asynchronous selection readback.
use super::*;

pub(super) struct Slot {
    pub(super) bitset: Buffer,
    pub(super) staging: Buffer,
    pub(super) busy: Arc<AtomicBool>,
}
pub(super) struct PendingMap {
    pub(super) index: usize,
    pub(super) request: SelectionRequest,
    pub(super) size: u64,
}
pub(super) struct PointTargets {
    pub(super) id: Texture,
    pub(super) id_view: TextureView,
    pub(super) depth_view: TextureView,
}
pub(super) struct ScreenTarget {
    pub(super) size: UVec2,
    pub(super) view: TextureView,
}

pub(super) fn texture(
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
pub(super) fn screen_target(
    device: &RenderDevice,
    size: UVec2,
    format: TextureFormat,
) -> ScreenTarget {
    ScreenTarget {
        size,
        view: texture(device, size, format, TextureUsages::RENDER_ATTACHMENT)
            .create_view(&default()),
    }
}
pub(super) fn point_targets(device: &RenderDevice) -> PointTargets {
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
pub(super) fn new_slot(device: &RenderDevice, bytes: u64) -> Slot {
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
pub(super) fn draw_selection(
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
pub(super) fn map_results(mut gpu: ResMut<GpuRenderer>) {
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
                        let result = buffer
                            .slice(0..map.size)
                            .get_mapped_range()
                            .map(|view| view.to_vec());
                        buffer.unmap();
                        match result {
                            Ok(bytes) => (bytes, None),
                            Err(e) => (vec![], Some(e.to_string())),
                        }
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
