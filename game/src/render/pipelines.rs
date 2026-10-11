//! Bind-group layouts and cached render/compute pipelines.
use super::*;

impl GpuRenderer {
    pub(super) fn new(
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
    pub(super) fn sort_ready(&self, cache: &PipelineCache) -> bool {
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
    pub(super) fn queue_pipelines(
        &mut self,
        cache: &PipelineCache,
        format: TextureFormat,
        opaque: bool,
    ) {
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

    pub(super) fn queue_shadow_pipelines(&mut self, cache: &PipelineCache, format: TextureFormat) {
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

    pub(super) fn queue_side_pipeline(
        &mut self,
        cache: &PipelineCache,
        format: TextureFormat,
        opaque: bool,
    ) {
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
pub(super) fn optional_render_pipelines<'a, const N: usize>(
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
