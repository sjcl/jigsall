use crate::resources::*;
use bevy::log::{debug, warn};
use bevy::prelude::*;
use bevy::render::renderer::{RenderAdapter, RenderDevice};

/// 画像読み込みシステムをセットアップ
pub fn setup_image_load_system(
    mut commands: Commands,
    device: Option<Res<RenderDevice>>,
    adapter: Option<Res<RenderAdapter>>,
) {
    // crossbeam unboundedチャネルを作成
    let (tx_results, rx_results) = crossbeam::channel::unbounded();

    // Bevyシステム側のチャネルをリソースとして追加
    commands.insert_resource(ImageLoadChannels { rx_results });

    commands.insert_resource(ImageLoadSender { tx_results });
    // Headless CPU fixtures have no renderer and cannot upload textures.
    if let Some(device) = device {
        commands.insert_resource(PuzzleImageLimits {
            device_max_dimension: device.limits().max_texture_dimension_2d,
            gpu_memory_bytes: adapter
                .as_deref()
                .and_then(|adapter| crate::gpu_memory::capacity_bytes(adapter, &device)),
        });
    }
}

/// 画像読み込み結果を処理（crossbeam-channel受信）
pub fn handle_image_load_results(
    image_channels: Res<ImageLoadChannels>,
    config: Res<PuzzleConfig>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
    service: Res<crate::persistence::runtime::PersistenceService>,
    persistence: Res<crate::persistence::runtime::PersistenceState>,
) {
    // crossbeam-channelから直接try_recv
    while let Ok(result) = image_channels.rx_results.try_recv() {
        // An older worker can finish after the user selects another image.
        if result.virtual_key != config.image_path {
            continue;
        }
        match result.image {
            Ok(decoded) => {
                commands.remove_resource::<ImageLoadError>();
                if let Some(original) = result.original {
                    if let Some(bytes) = &original.encoded {
                        service.import(persistence.generation, original.hash, bytes.clone());
                    }
                    commands.insert_resource(original);
                }
                let texture_size = decoded.image.size();
                let logical_size = decoded.logical_size;

                // Imageアセットとして登録
                let opaque = crate::resources::images::image_is_opaque(&decoded.image);
                let handle = images.add(decoded.image);

                // PuzzleImageリソースを作成
                commands.insert_resource(PuzzleImage {
                    handle: handle.clone(),
                    logical_size,
                    texture_size,
                    opaque,
                });

                debug!(source_size = ?decoded.source_size, ?logical_size, ?texture_size, "Loaded puzzle image");
            }
            Err(e) => {
                warn!("Image loading failed");
                debug!(
                    image_key = %result.virtual_key,
                    error = %e,
                    "Image loading failure details"
                );
                commands.remove_resource::<PuzzleImage>();
                commands.remove_resource::<crate::persistence::runtime::OriginalPuzzleImage>();
                commands.insert_resource(ImageLoadError {
                    virtual_key: result.virtual_key,
                    reason: e,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
