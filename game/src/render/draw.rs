//! Visibility, sorting and indirect main-pass drawing.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn puzzle_node(
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
