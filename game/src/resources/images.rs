use bevy::prelude::*;

#[derive(Resource)]
pub struct PuzzleImage {
    pub handle: Handle<Image>,
    pub size: Vec2,
    pub opaque: bool,
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
