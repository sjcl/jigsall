use bevy::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// 外部ファイルパス管理
#[derive(Resource, Default)]
pub struct ExternalFileRegistry {
    /// 仮想パス -> 実際のファイルパス のマッピング
    pub registered_paths: Arc<RwLock<HashMap<String, PathBuf>>>,
    /// 次に使用するIDカウンター
    pub next_id: Arc<RwLock<u64>>,
}

impl ExternalFileRegistry {
    /// 外部ファイルを登録して一意なキーを返す
    pub fn register_file<P: AsRef<Path>>(&self, file_path: P) -> String {
        let file_path = file_path.as_ref().to_path_buf();

        // IDを取得
        let mut id_guard = self.next_id.write().unwrap();
        let id = *id_guard;
        *id_guard += 1;
        drop(id_guard);

        // 一意なキーを生成
        let extension = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("bin");
        let virtual_key = format!("external_file_{}.{}", id, extension);

        // マッピングを保存
        let mut paths_guard = self.registered_paths.write().unwrap();
        paths_guard.insert(virtual_key.clone(), file_path.clone());
        drop(paths_guard);

        println!(
            "📁 Registered external file: {} -> {}",
            virtual_key,
            file_path.display()
        );
        virtual_key
    }

    /// 仮想キーから実際のファイルパスを取得
    pub fn resolve_path(&self, virtual_key: &str) -> Option<PathBuf> {
        let paths_guard = self.registered_paths.read().unwrap();
        paths_guard.get(virtual_key).cloned()
    }

    /// 外部ファイルキーかどうかをチェック
    pub fn is_external_key(&self, key: &str) -> bool {
        key.starts_with("external_file_")
    }

    /// パズル設定のimage_pathが外部ファイルかどうかチェック
    pub fn is_external_image_path(&self, image_path: &str) -> bool {
        self.is_external_key(image_path)
    }

    /// 仮想キーからオリジナルのファイル名を取得
    pub fn get_original_filename(&self, virtual_key: &str) -> Option<String> {
        if let Some(real_path) = self.resolve_path(virtual_key) {
            real_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|s| s.to_string())
        } else {
            None
        }
    }
}

/// ExternalFileRegistryを初期化するプラグイン
pub struct DirectFileAssetPlugin;

impl Plugin for DirectFileAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExternalFileRegistry>();
        println!("🔧 DirectFileAssetPlugin: ExternalFileRegistry initialized");
    }
}

/// 画像読み込み結果
#[derive(Debug)]
pub struct ImageLoadResult {
    pub virtual_key: String,
    pub image: Result<Image, String>,
}

/// std::thread::spawnを使った画像読み込みスレッドを開始
pub fn start_thread_image_load(
    virtual_key: String,
    file_path: PathBuf,
    sender: crossbeam::channel::Sender<ImageLoadResult>,
) {
    println!(
        "🔄 MAIN THREAD [{:?}]: Starting thread image load for: {}",
        std::thread::current().id(),
        file_path.display()
    );

    // ネイティブスレッドを起動
    std::thread::spawn(move || {
        println!(
            "🔍 WORKER THREAD [{:?}]: Starting image processing...",
            std::thread::current().id()
        );

        // ファイル読み込みからImage作成まで全て同期的に実行
        let result = (|| -> Result<Image, String> {
            // ファイルメタデータ取得
            println!(
                "🔍 WORKER THREAD [{:?}]: Getting file metadata...",
                std::thread::current().id()
            );
            let metadata = std::fs::metadata(&file_path)
                .map_err(|e| format!("Failed to get file metadata: {}", e))?;
            let file_size = metadata.len();
            println!(
                "📂 WORKER THREAD [{:?}]: Starting file read: {} ({} MB)",
                std::thread::current().id(),
                file_path.display(),
                file_size / 1024 / 1024
            );

            // ファイル読み込み
            let image_bytes =
                std::fs::read(&file_path).map_err(|e| format!("Failed to read file: {}", e))?;
            println!(
                "📊 WORKER THREAD [{:?}]: Read {} bytes ({} MB) from {}",
                std::thread::current().id(),
                image_bytes.len(),
                file_size / 1024 / 1024,
                file_path.display()
            );

            // 画像デコード
            println!(
                "🎨 WORKER THREAD [{:?}]: Starting image decode...",
                std::thread::current().id()
            );
            let dynamic_image = image::load_from_memory(&image_bytes)
                .map_err(|e| format!("Failed to decode image: {}", e))?;
            drop(image_bytes);

            // RGBA変換
            println!(
                "🎨 WORKER THREAD [{:?}]: Image decode complete, starting RGBA conversion...",
                std::thread::current().id()
            );
            let rgba_image = dynamic_image.into_rgba8();
            let (width, height) = rgba_image.dimensions();
            let rgba_data = rgba_image.into_raw();

            // BevyのImageを作成
            let image = Image::new(
                bevy::render::render_resource::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                bevy::render::render_resource::TextureDimension::D2,
                rgba_data,
                bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
                // Bevy moves pixels into the render world and retains Image metadata.
                bevy::asset::RenderAssetUsages::RENDER_WORLD,
            );

            println!(
                "✅ WORKER THREAD [{:?}]: Image processing complete: {}x{} pixels",
                std::thread::current().id(),
                width,
                height
            );
            Ok(image)
        })();

        // 結果をチャネルに送信
        let load_result = ImageLoadResult {
            virtual_key: virtual_key.clone(),
            image: result,
        };

        println!(
            "🔍 WORKER THREAD [{:?}]: Sending result to main thread...",
            std::thread::current().id()
        );
        match sender.send(load_result) {
            Ok(()) => println!(
                "📤 WORKER THREAD [{:?}]: Result sent successfully",
                std::thread::current().id()
            ),
            Err(e) => println!(
                "❌ WORKER THREAD [{:?}]: Failed to send result: {:?}",
                std::thread::current().id(),
                e
            ),
        }
        println!(
            "🔍 WORKER THREAD [{:?}]: Thread completing",
            std::thread::current().id()
        );
    });

    println!(
        "🔍 MAIN THREAD [{:?}]: Worker thread spawned",
        std::thread::current().id()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        asset::RenderAssetUsages,
        render::{render_asset::RenderAsset, texture::GpuImage},
    };

    #[test]
    fn decoded_image_moves_pixels_to_render_world_and_keeps_metadata() {
        let path = std::env::temp_dir().join(format!(
            "puzzella-image-load-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let pixels = vec![
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 255, 255, 255, 255,
        ];
        image::RgbaImage::from_raw(2, 2, pixels.clone())
            .unwrap()
            .save(&path)
            .unwrap();
        let (tx, rx) = crossbeam::channel::bounded(1);
        start_thread_image_load("fixture.png".into(), path.clone(), tx);
        let result = rx.recv_timeout(std::time::Duration::from_secs(10));
        std::fs::remove_file(path).unwrap();
        let mut image = result.unwrap().image.unwrap();
        assert_eq!(image.asset_usage, RenderAssetUsages::RENDER_WORLD);
        assert!(!crate::resources::images::image_is_opaque(&image));
        let allocation = image.data.as_ref().unwrap().as_ptr();
        let extracted = GpuImage::take_gpu_data(&mut image, None).unwrap();
        assert_eq!(extracted.data.as_ref().unwrap().as_ptr(), allocation);
        assert_eq!(extracted.data.as_ref().unwrap(), &pixels);
        assert!(image.data.is_none());
        assert_eq!(image.size(), UVec2::new(2, 2));
        assert_eq!(
            image.texture_descriptor.format,
            extracted.texture_descriptor.format
        );
    }

    #[test]
    #[ignore = "requires a real GPU"]
    fn render_only_image_upload_keeps_metadata_and_pixel_values() {
        use bevy::render::{
            render_asset::RenderAssets,
            render_resource::*,
            renderer::{RenderDevice, RenderQueue},
            RenderApp,
        };
        use std::time::Duration;

        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    ..default()
                })
                .disable::<bevy::log::LogPlugin>()
                .disable::<bevy::winit::WinitPlugin>()
                .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>(),
        );
        app.finish();
        app.cleanup();
        let pixels = [
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 255, 255, 255, 255,
        ];
        let mut image = Image::new(
            Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            pixels.to_vec(),
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        app.update();
        let image = app
            .world()
            .resource::<Assets<Image>>()
            .get(&handle)
            .unwrap();
        assert!(image.data.is_none());
        assert_eq!(image.size(), UVec2::splat(2));

        let world = app.sub_app(RenderApp).world();
        let image = world
            .resource::<RenderAssets<GpuImage>>()
            .get(&handle)
            .unwrap();
        let device = world.resource::<RenderDevice>();
        let queue = world.resource::<RenderQueue>();
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("render-only image verification"),
            size: 512,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&default());
        encoder.copy_texture_to_buffer(
            image.texture.as_image_copy(),
            TexelCopyBufferInfo {
                buffer: &staging,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(2),
                },
            },
            Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let (tx, rx) = crossbeam::channel::bounded(1);
        staging.slice(..).map_async(MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
        device
            .poll(PollType::Wait {
                timeout: Some(Duration::from_secs(20)),
                submission_index: None,
            })
            .unwrap();
        rx.recv_timeout(Duration::from_secs(20)).unwrap().unwrap();
        let bytes = staging.slice(..).get_mapped_range();
        assert_eq!(&bytes[..8], &pixels[..8]);
        assert_eq!(&bytes[256..264], &pixels[8..]);
        drop(bytes);
        staging.unmap();
    }
}
