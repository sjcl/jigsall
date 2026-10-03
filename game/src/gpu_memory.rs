//! Read the selected renderer's adapter, never the first GPU in an OS inventory.
use bevy::render::renderer::{RenderAdapter, RenderDevice};

pub(crate) fn capacity_bytes(adapter: &RenderAdapter, _device: &RenderDevice) -> Option<u64> {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        // SAFETY: retain the HAL guard while reading immutable properties from
        // its own physical device / instance. No resource is changed or destroyed.
        if let Some(vulkan) = unsafe { adapter.as_hal::<wgpu_hal::api::Vulkan>() } {
            let properties = unsafe {
                vulkan
                    .shared_instance()
                    .raw_instance()
                    .get_physical_device_memory_properties(vulkan.raw_physical_device())
            };
            // The largest DEVICE_LOCAL heap is VRAM on discrete GPUs and the
            // driver-advertised shared capacity on unified-memory GPUs. Do not
            // add alias / multi-instance heaps and overestimate the capacity.
            return properties
                .memory_heaps_as_slice()
                .iter()
                .filter(|heap| heap.flags.contains(ash::vk::MemoryHeapFlags::DEVICE_LOCAL))
                .map(|heap| heap.size)
                .max()
                .filter(|bytes| *bytes > 0);
        }
    }
    #[cfg(windows)]
    {
        // SAFETY: the HAL guard owns the active DXGI adapter throughout GetDesc.
        if let Some(dx12) = unsafe { adapter.as_hal::<wgpu_hal::api::Dx12>() } {
            let desc = unsafe { dx12.raw_adapter().GetDesc() }.ok()?;
            let bytes = if adapter.get_info().device_type == wgpu_types::DeviceType::IntegratedGpu {
                desc.SharedSystemMemory
            } else {
                desc.DedicatedVideoMemory
            };
            return (bytes > 0).then_some(bytes as u64);
        }
    }
    #[cfg(target_os = "macos")]
    {
        use objc2_metal::MTLDevice;
        // SAFETY: only read a property of the active Metal device while holding
        // its HAL guard. Apple's recommended working set covers unified memory.
        if let Some(metal) = unsafe { _device.wgpu_device().as_hal::<wgpu_hal::api::Metal>() } {
            let bytes = metal.raw_device().recommendedMaxWorkingSetSize();
            return (bytes > 0).then_some(bytes);
        }
    }
    let _ = adapter;
    None
}
