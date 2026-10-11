//! GPU allocation and revision-based uploads.
use super::*;

/// Three always-present SoA regions. Drag lift adds a fourth region and a
/// compact record tail lazily, keeping the idle allocation and bindings intact.
#[derive(Clone, Copy)]
pub(super) struct PieceMetadataLayout {
    pub(super) capacity: u32,
}
impl PieceMetadataLayout {
    pub(super) fn size(self) -> u64 {
        u64::from(self.capacity) * 3 * 4
    }
    pub(super) fn range_offset(self, region: u64, start: u32, len: usize) -> Option<u64> {
        // Subtract before comparing so malformed ranges cannot overflow or
        // cross into the next region, even in release builds.
        if len > self.capacity.checked_sub(start)? as usize {
            return None;
        }
        Some((u64::from(self.capacity) * region + u64::from(start)) * 4)
    }
    pub(super) fn root_range_offset(self, start: u32, len: usize) -> Option<u64> {
        self.range_offset(0, start, len)
    }
    pub(super) fn remote_range_offset(self, start: u32, len: usize) -> Option<u64> {
        self.range_offset(1, start, len)
    }
    pub(super) fn rotation_range_offset(self, root: u32, len: usize) -> Option<u64> {
        self.range_offset(2, root, len)
    }
    pub(super) fn drag_elevation_range_offset(self, start: u32, len: usize) -> Option<u64> {
        self.range_offset(3, start, len)
    }
    pub(super) fn drag_elevation_records_offset(self) -> u64 {
        u64::from(self.capacity) * 4 * 4
    }
    pub(super) fn drag_elevation_size(self, records: usize) -> u64 {
        self.drag_elevation_records_offset() + records as u64 * 16
    }
}

pub(super) struct StateBuffers {
    pub(super) epoch: u64,
    pub(super) revision: u64,
    pub(super) root_revision: u64,
    pub(super) piece_metadata: Buffer,
    pub(super) drag_elevation_capacity: usize,
    pub(super) drag_elevation_revision: u64,
    pub(super) states: Buffer,
    pub(super) visible: Buffer,
    pub(super) args: Buffer,
    pub(super) sort: Option<RadixBuffers>,
    pub(super) selectable: Buffer,
    pub(super) dummy_selection: Buffer,
    pub(super) drag_members: Buffer,
    pub(super) current_drag: Arc<[u32]>,
    pub(super) remote_deltas: Buffer,
    pub(super) remote_revision: u64,
    pub(super) remote_delta_revision: u64,
    pub(super) rotation_animations: Buffer,
    pub(super) rotation_capacity: usize,
    pub(super) rotation_revision: u64,
    pub(super) selected: Buffer,
    pub(super) current_selected: Arc<[u32]>,
    pub(super) preview: Buffer,
    pub(super) direct_hits: Buffer,
    pub(super) capacity: u32,
    pub(super) pick_visible: Buffer,
    pub(super) pick_args: Buffer,
}
impl StateBuffers {
    pub(super) fn root_binding(&self) -> BufferBinding<'_> {
        BufferBinding {
            buffer: &self.piece_metadata,
            offset: 0,
            size: BufferSize::new(u64::from(self.capacity) * 4),
        }
    }
}
#[derive(Clone)]
pub(super) struct RadixBuffers {
    pub(super) scratch: Buffer,
    pub(super) counts: Buffer,
    pub(super) histogram: Buffer,
    pub(super) dispatch: Buffer,
}
impl RadixBuffers {
    pub(super) fn new(device: &RenderDevice, count: u32) -> Self {
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

pub(super) fn buffer(device: &RenderDevice, label: &str, size: u64, usage: BufferUsages) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: size.max(4),
        usage,
        mapped_at_creation: false,
    })
}
pub(super) fn prepare_buffers(
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
