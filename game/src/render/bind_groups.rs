use super::*;

pub(super) struct CachedBindGroup<K> {
    value: Option<(K, BindGroup)>,
}
impl<K> Default for CachedBindGroup<K> {
    fn default() -> Self {
        Self { value: None }
    }
}
impl<K: PartialEq> CachedBindGroup<K> {
    fn get_or_create(&mut self, key: K, create: impl FnOnce() -> BindGroup) -> &BindGroup {
        if self.value.as_ref().is_none_or(|(old, _)| *old != key) {
            self.value = Some((key, create()));
        }
        &self.value.as_ref().unwrap().1
    }
}
#[cfg(test)]
impl<K> CachedBindGroup<K> {
    pub(super) fn id(&self) -> Option<BindGroupId> {
        self.value.as_ref().map(|(_, group)| group.id())
    }
}

// These caches live with the state buffers, so an epoch change drops every group
// referencing the old puzzle. Uniform uploads reuse groups unless the GPU buffer changes.
#[derive(Default)]
pub(super) struct StateBindGroups {
    compute: CachedBindGroup<(BufferId, BufferId)>,
    draw: CachedBindGroup<BufferId>,
    dummy_selection: CachedBindGroup<()>,
    preview_selection: CachedBindGroup<()>,
    pick_compute: CachedBindGroup<BufferId>,
    pick_draw: CachedBindGroup<BufferId>,
    sort: [CachedBindGroup<BufferId>; 2],
    sort_dispatch: CachedBindGroup<()>,
}
#[cfg(test)]
impl StateBindGroups {
    pub(super) fn radix_ids(&self) -> [Option<BindGroupId>; 2] {
        [self.sort[1].id(), self.sort_dispatch.id()]
    }
    pub(super) fn ids(&self) -> [Option<BindGroupId>; 7] {
        [
            self.compute.id(),
            self.draw.id(),
            self.dummy_selection.id(),
            self.preview_selection.id(),
            self.pick_compute.id(),
            self.pick_draw.id(),
            self.sort[0].id(),
        ]
    }
}

impl GpuRenderer {
    pub(super) fn main_bind_groups(
        &mut self,
        device: &RenderDevice,
        cache: &PipelineCache,
    ) -> (BindGroup, BindGroup, BindGroup) {
        let buffers = self.buffers.as_mut().unwrap();
        let uniform_id = self.uniform.buffer().unwrap().id();
        let counts = buffers
            .sort
            .as_ref()
            .map_or(&buffers.dummy_selection, |s| &s.counts);
        let compute = buffers
            .groups
            .compute
            .get_or_create((uniform_id, counts.id()), || {
                device.create_bind_group(
                    "puzzle compute",
                    &cache.get_bind_group_layout(&self.compute_layout),
                    &BindGroupEntries::sequential((
                        self.uniform.binding().unwrap(),
                        buffers.states.as_entire_buffer_binding(),
                        buffers.visible.as_entire_buffer_binding(),
                        buffers.args.as_entire_buffer_binding(),
                        buffers.selectable.as_entire_buffer_binding(),
                        buffers.drag_members.as_entire_buffer_binding(),
                        counts.as_entire_buffer_binding(),
                    )),
                )
            });
        let draw = buffers.groups.draw.get_or_create(uniform_id, || {
            device.create_bind_group(
                "puzzle draw",
                &cache.get_bind_group_layout(&self.draw_layout),
                &BindGroupEntries::sequential((
                    self.uniform.binding().unwrap(),
                    buffers.states.as_entire_buffer_binding(),
                    buffers.visible.as_entire_buffer_binding(),
                    buffers.drag_members.as_entire_buffer_binding(),
                    buffers.preview.as_entire_buffer_binding(),
                    buffers.selected.as_entire_buffer_binding(),
                )),
            )
        });
        let dummy = buffers.groups.dummy_selection.get_or_create((), || {
            device.create_bind_group(
                "unused selection",
                &cache.get_bind_group_layout(&self.selection_layout),
                &BindGroupEntries::sequential((
                    buffers.dummy_selection.as_entire_buffer_binding(),
                    buffers.selectable.as_entire_buffer_binding(),
                )),
            )
        });
        (compute.clone(), draw.clone(), dummy.clone())
    }

    pub(super) fn image_bind_group(
        &mut self,
        device: &RenderDevice,
        cache: &PipelineCache,
        image: &GpuImage,
    ) -> BindGroup {
        self.image_group
            .get_or_create((image.texture_view.id(), image.sampler.id()), || {
                device.create_bind_group(
                    "puzzle texture",
                    &cache.get_bind_group_layout(&self.texture_layout),
                    &BindGroupEntries::sequential((&image.texture_view, &image.sampler)),
                )
            })
            .clone()
    }

    pub(super) fn sort_bind_groups(
        &mut self,
        device: &RenderDevice,
        cache: &PipelineCache,
    ) -> ([BindGroup; 2], BindGroup) {
        let buffers = self.buffers.as_mut().unwrap();
        let sort = buffers.sort.as_ref().unwrap();
        let uniform_id = self.sort_uniform.buffer().unwrap().id();
        let inputs = [
            (&buffers.visible, &sort.scratch),
            (&sort.scratch, &buffers.visible),
        ];
        let groups = std::array::from_fn(|i| {
            buffers.groups.sort[i]
                .get_or_create(uniform_id, || {
                    device.create_bind_group(
                        "puzzle radix sort",
                        &cache.get_bind_group_layout(&self.sort_layout),
                        &BindGroupEntries::sequential((
                            self.sort_uniform.binding().unwrap(),
                            buffers.states.as_entire_buffer_binding(),
                            inputs[i].0.as_entire_buffer_binding(),
                            inputs[i].1.as_entire_buffer_binding(),
                            buffers.args.as_entire_buffer_binding(),
                            sort.counts.as_entire_buffer_binding(),
                            sort.histogram.as_entire_buffer_binding(),
                        )),
                    )
                })
                .clone()
        });
        let dispatch = buffers.groups.sort_dispatch.get_or_create((), || {
            device.create_bind_group(
                "radix dispatch preparation",
                &cache.get_bind_group_layout(&self.sort_dispatch_layout),
                &BindGroupEntries::single(sort.dispatch.as_entire_buffer_binding()),
            )
        });
        (groups, dispatch.clone())
    }

    pub(super) fn pick_bind_groups(
        &mut self,
        device: &RenderDevice,
        cache: &PipelineCache,
    ) -> (BindGroup, BindGroup) {
        let buffers = self.buffers.as_mut().unwrap();
        let uniform_id = self.pick_uniform.buffer().unwrap().id();
        let compute = buffers.groups.pick_compute.get_or_create(uniform_id, || {
            device.create_bind_group(
                "picking ROI culling",
                &cache.get_bind_group_layout(&self.pick_compute_layout),
                &BindGroupEntries::sequential((
                    self.pick_uniform.binding().unwrap(),
                    buffers.states.as_entire_buffer_binding(),
                    buffers.visible.as_entire_buffer_binding(),
                    buffers.args.as_entire_buffer_binding(),
                    buffers.pick_visible.as_entire_buffer_binding(),
                    buffers.pick_args.as_entire_buffer_binding(),
                    buffers.drag_members.as_entire_buffer_binding(),
                )),
            )
        });
        let draw = buffers.groups.pick_draw.get_or_create(uniform_id, || {
            device.create_bind_group(
                "procedural picking",
                &cache.get_bind_group_layout(&self.draw_layout),
                &BindGroupEntries::sequential((
                    self.pick_uniform.binding().unwrap(),
                    buffers.states.as_entire_buffer_binding(),
                    buffers.pick_visible.as_entire_buffer_binding(),
                    buffers.drag_members.as_entire_buffer_binding(),
                    buffers.dummy_selection.as_entire_buffer_binding(),
                    buffers.selected.as_entire_buffer_binding(),
                )),
            )
        });
        (compute.clone(), draw.clone())
    }

    pub(super) fn selection_bind_group(
        &mut self,
        device: &RenderDevice,
        cache: &PipelineCache,
        point_slot: Option<usize>,
    ) -> BindGroup {
        let buffers = self.buffers.as_mut().unwrap();
        let layout = || cache.get_bind_group_layout(&self.selection_layout);
        if let Some(index) = point_slot {
            let slot = &mut self.slots[index];
            // Slots outlive puzzles. Resizing replaces the entire slot/cache;
            // changing the selectable buffer also invalidates this binding.
            slot.selection_group
                .get_or_create(buffers.selectable.id(), || {
                    device.create_bind_group(
                        "point selection masks",
                        &layout(),
                        &BindGroupEntries::sequential((
                            slot.bitset.as_entire_buffer_binding(),
                            buffers.selectable.as_entire_buffer_binding(),
                        )),
                    )
                })
                .clone()
        } else {
            buffers
                .groups
                .preview_selection
                .get_or_create((), || {
                    device.create_bind_group(
                        "preview selection masks",
                        &layout(),
                        &BindGroupEntries::sequential((
                            buffers.preview.as_entire_buffer_binding(),
                            buffers.selectable.as_entire_buffer_binding(),
                        )),
                    )
                })
                .clone()
        }
    }
}
