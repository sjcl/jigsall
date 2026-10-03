use crate::resources::*;
use bevy::log::{debug, debug_once, error_once, warn, warn_once};
use bevy::prelude::*;

/// 画像読み込みシステムをセットアップ
pub fn setup_image_load_system(mut commands: Commands) {
    // crossbeam unboundedチャネルを作成
    let (tx_results, rx_results) = crossbeam::channel::unbounded();

    // Bevyシステム側のチャネルをリソースとして追加
    commands.insert_resource(ImageLoadChannels { rx_results });

    commands.insert_resource(ImageLoadSender { tx_results });
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
            Ok(image) => {
                commands.remove_resource::<ImageLoadError>();
                if let Some(original) = result.original {
                    if let Some(bytes) = &original.encoded {
                        service.import(persistence.generation, original.hash, bytes.clone());
                    }
                    commands.insert_resource(original);
                }
                // 画像のサイズを取得
                let image_size = image.size();
                let size_vec2 = Vec2::new(image_size.x as f32, image_size.y as f32);

                // Imageアセットとして登録
                let opaque = crate::resources::images::image_is_opaque(&image);
                let handle = images.add(image);

                // PuzzleImageリソースを作成
                commands.insert_resource(PuzzleImage {
                    handle: handle.clone(),
                    size: size_vec2,
                    opaque,
                });

                debug!(size = ?size_vec2, "Loaded puzzle image");
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

pub fn update_puzzle_image_size(
    puzzle_image: Option<ResMut<PuzzleImage>>,
    images: Res<Assets<Image>>,
    asset_server: Res<AssetServer>,
    puzzle_config: Res<PuzzleConfig>,
    file_registry: Res<crate::asset_reader::ExternalFileRegistry>,
) {
    let _span = info_span!("update_puzzle_image_size").entered();
    if let Some(mut puzzle_image) = puzzle_image {
        // 外部ファイルかどうかチェック
        let is_external = file_registry.is_external_image_path(&puzzle_config.image_path);

        if is_external {
            // 外部ファイルの場合：AssetServerの状態チェックをスキップして直接Imageをチェック
            if let Some(image) = images.get(&puzzle_image.handle) {
                let actual_size = image.size();
                let new_size = Vec2::new(actual_size.x as f32, actual_size.y as f32);

                // サイズが変更された場合のみ更新
                if puzzle_image.size != new_size {
                    debug!(
                        old_size = ?puzzle_image.size,
                        ?new_size,
                        "Updated external image size"
                    );
                    puzzle_image.size = new_size;
                }
            }
        } else {
            // 通常のアセットファイルの場合：従来のAssetServer状態チェック
            let load_state = asset_server.load_state(&puzzle_image.handle);

            match load_state {
                bevy::asset::LoadState::Loaded => {
                    if let Some(image) = images.get(&puzzle_image.handle) {
                        // Bevy 0.16では image.size() メソッドを使用
                        let actual_size = image.size();
                        let new_size = Vec2::new(actual_size.x as f32, actual_size.y as f32);

                        // サイズが変更された場合のみ更新
                        if puzzle_image.size != new_size {
                            debug!(
                                old_size = ?puzzle_image.size,
                                ?new_size,
                                "Updated asset image size"
                            );
                            puzzle_image.size = new_size;
                        }
                    } else {
                        warn_once!("Loaded puzzle image asset is missing from Assets<Image>");
                    }
                }
                bevy::asset::LoadState::Loading => {}
                bevy::asset::LoadState::Failed(error) => {
                    error_once!("Failed to load puzzle image asset");
                    debug_once!(?error, "Puzzle image asset loading failure details");
                }
                bevy::asset::LoadState::NotLoaded => {}
            }
        }
    }
}
