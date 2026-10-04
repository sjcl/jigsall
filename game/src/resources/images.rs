use bevy::prelude::*;

#[derive(Resource)]
pub struct PuzzleImage {
    pub handle: Handle<Image>,
    /// Frozen game coordinates, shared by save files and all players.
    pub logical_size: UVec2,
    /// Local texture resolution, bounded before Assets<Image> registration.
    pub texture_size: UVec2,
    pub opaque: bool,
}

/// Actual device limits, captured on the main thread after renderer startup.
#[derive(Resource, Clone, Copy, Debug)]
pub struct PuzzleImageLimits {
    pub device_max_dimension: u32,
    /// Dedicated VRAM or the active adapter's shared memory capacity, in bytes.
    /// None means this backend cannot report it; the UI displays the fallback.
    pub gpu_memory_bytes: Option<u64>,
}

impl PuzzleImageLimits {
    /// Copy just the effective cap into worker requests; workers never access GPU resources.
    pub fn decode_limits(
        &self,
        settings: &crate::image_settings::ImageSettings,
    ) -> ImageDecodeLimits {
        ImageDecodeLimits {
            max_texture_dimension: self
                .device_max_dimension
                .min(settings.max_texture_dimension(self)),
        }
    }

    pub fn max_budget_mib(&self) -> u64 {
        self.gpu_memory_bytes
            .map(|bytes| bytes / (1024 * 1024))
            .unwrap_or_else(|| {
                let edge = self
                    .device_max_dimension
                    .min(jigsall_core::MAX_PUZZLE_IMAGE_DIMENSION);
                u64::from(edge).pow(2) * 4 / (1024 * 1024)
            })
            .max(1)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ImageDecodeLimits {
    pub max_texture_dimension: u32,
}

/// A failed selection stays visible until the user selects another image.
#[derive(Resource, Debug)]
pub struct ImageLoadError {
    pub virtual_key: String,
    pub reason: String,
}

/// 画像読み込みチャネル（crossbeam-channel）
#[derive(Resource)]
pub struct ImageLoadChannels {
    /// 画像読み込み結果を受信するチャネル
    pub rx_results: crossbeam::channel::Receiver<crate::asset_reader::ImageLoadResult>,
}

/// 画像読み込み送信チャネル（スレッド間通信用）
#[derive(Resource)]
pub struct ImageLoadSender {
    pub tx_results: crossbeam::channel::Sender<crate::asset_reader::ImageLoadResult>,
}

/// Decode outputs RGBA8. Unknown formats conservatively use the transparent path.
pub fn image_is_opaque(image: &Image) -> bool {
    use bevy::render::render_resource::TextureFormat;
    matches!(
        image.texture_descriptor.format,
        TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb
    ) && image
        .data
        .as_ref()
        .is_some_and(|bytes| bytes.chunks_exact(4).all(|p| p[3] == 255))
}
