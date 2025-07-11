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
        let extension = file_path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("bin");
        let virtual_key = format!("external_file_{}.{}", id, extension);
        
        // マッピングを保存
        let mut paths_guard = self.registered_paths.write().unwrap();
        paths_guard.insert(virtual_key.clone(), file_path.clone());
        drop(paths_guard);
        
        println!("📁 Registered external file: {} -> {}", virtual_key, file_path.display());
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
            real_path.file_name()
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

/// 外部ファイルまたは通常のアセットを読み込む
/// 外部ファイルは直接読み込んでImage assetとして登録
pub fn load_external_image(
    file_registry: &ExternalFileRegistry,
    asset_server: &AssetServer,
    images: &mut ResMut<Assets<Image>>,
    virtual_key: &str,
) -> Handle<Image> {
    if file_registry.is_external_key(virtual_key) {
        // 外部ファイルの場合：実際のパスを解決して直接イメージデータを読み込み
        if let Some(real_path) = file_registry.resolve_path(virtual_key) {
            println!("📖 Loading external image directly: {} -> {}", virtual_key, real_path.display());
            
            // ファイルからイメージデータを直接読み込み
            match load_image_from_file(&real_path) {
                Ok(image) => {
                    // サイズ情報を先に取得
                    let image_size = image.size();
                    
                    // Imageアセットとして登録
                    let handle = images.add(image);
                    println!("✅ Successfully loaded external image: {} (size: {}x{})", 
                        real_path.display(), image_size.x, image_size.y);
                    println!("📝 External image handle: {:?}", handle.id());
                    handle
                }
                Err(e) => {
                    println!("❌ Failed to load external image {}: {}", real_path.display(), e);
                    // フォールバック：空のイメージを作成
                    let fallback_image = Image::default();
                    let handle = images.add(fallback_image);
                    println!("📝 Fallback image handle: {:?}", handle.id());
                    handle
                }
            }
        } else {
            println!("❌ External file path not found for key: {}", virtual_key);
            // フォールバック：空のイメージを作成
            images.add(Image::default())
        }
    } else {
        // 通常のアセットパスの場合
        asset_server.load(virtual_key)
    }
}

/// ファイルパスから直接Imageを読み込む
fn load_image_from_file(file_path: &Path) -> Result<Image, Box<dyn std::error::Error>> {
    // ファイルの拡張子を確認
    let extension = file_path.extension()
        .and_then(|ext| ext.to_str())
        .ok_or("Invalid file extension")?;
    
    // ファイルデータを読み込み
    let image_bytes = std::fs::read(file_path)?;
    
    // image crateを使用してデコード
    let dynamic_image = image::load_from_memory(&image_bytes)?;
    
    // RGBAフォーマットに変換
    let rgba_image = dynamic_image.to_rgba8();
    let (width, height) = rgba_image.dimensions();
    
    // BevyのImageを作成
    let image = Image::new(
        bevy::render::render_resource::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        rgba_image.into_raw(),
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::render::render_asset::RenderAssetUsages::all(),
    );
    
    println!("🖼️ Decoded image: {}x{} pixels", width, height);
    Ok(image)
}