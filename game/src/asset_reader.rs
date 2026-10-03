use bevy::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Maps virtual image keys to the selected source files.
#[derive(Resource, Default)]
pub struct ExternalFileRegistry {
    registered_paths: RwLock<HashMap<String, PathBuf>>,
    // Never reset between selections or sessions: late load results keep distinct keys.
    next_id: AtomicU64,
}

impl ExternalFileRegistry {
    /// Registers a source file and returns its unique virtual key.
    pub fn register_file<P: AsRef<Path>>(&self, file_path: P) -> String {
        let file_path = file_path.as_ref().to_path_buf();
        // The map lock synchronizes paths; the counter only allocates distinct IDs.
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let extension = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("bin");
        let virtual_key = format!("external_file_{id}.{extension}");
        debug!(
            "Registered external file: {} -> {}",
            virtual_key,
            file_path.display()
        );
        self.write_paths().insert(virtual_key.clone(), file_path);
        virtual_key
    }

    /// Returns the source path for a registered virtual key.
    pub fn resolve_path(&self, virtual_key: &str) -> Option<PathBuf> {
        self.read_paths().get(virtual_key).cloned()
    }

    /// Releases a mapping when its image selection is replaced.
    pub fn unregister_file(&self, virtual_key: &str) -> Option<PathBuf> {
        self.write_paths().remove(virtual_key)
    }

    /// Releases all source paths when returning to the menu, preserving the ID counter.
    pub fn clear(&self) {
        self.write_paths().clear();
    }

    pub fn is_empty(&self) -> bool {
        self.read_paths().is_empty()
    }

    // Each map operation is independent, so a prior panic need not discard other entries.
    fn read_paths(&self) -> RwLockReadGuard<'_, HashMap<String, PathBuf>> {
        self.registered_paths.read().unwrap_or_else(|error| {
            warn!("Recovering poisoned external file registry");
            self.registered_paths.clear_poison();
            error.into_inner()
        })
    }

    fn write_paths(&self) -> RwLockWriteGuard<'_, HashMap<String, PathBuf>> {
        self.registered_paths.write().unwrap_or_else(|error| {
            warn!("Recovering poisoned external file registry");
            self.registered_paths.clear_poison();
            error.into_inner()
        })
    }

    /// Identifies virtual keys, including those whose mappings have been released.
    pub fn is_external_key(&self, key: &str) -> bool {
        key.starts_with("external_file_")
    }

    /// Checks whether a puzzle image path is an external file key.
    pub fn is_external_image_path(&self, image_path: &str) -> bool {
        self.is_external_key(image_path)
    }

    /// Returns the original filename for display in the image picker.
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

/// Initializes the registry for images loaded directly from source files.
pub struct DirectFileAssetPlugin;

impl Plugin for DirectFileAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExternalFileRegistry>();
        debug!("External file registry initialized");
    }
}

/// A decoded image and its original bytes, identified by the selection's virtual key.
pub struct ImageLoadResult {
    pub virtual_key: String,
    pub image: Result<Image, String>,
    pub original: Option<crate::persistence::runtime::OriginalPuzzleImage>,
}

/// Decode only; both selected source files and verified .puzimg payloads use this.
pub fn decode_image_bytes(encoded: &[u8]) -> Result<Image, String> {
    let decoded = image::load_from_memory(encoded).map_err(|e| e.to_string())?;
    let rgba = decoded.into_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(Image::new(
        bevy::render::render_resource::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        rgba.into_raw(),
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    ))
}

pub fn start_thread_image_load(
    virtual_key: String,
    file_path: PathBuf,
    sender: crossbeam::channel::Sender<ImageLoadResult>,
) {
    std::thread::spawn(move || {
        use std::io::Read;
        let result =
            (|| -> Result<(Image, crate::persistence::runtime::OriginalPuzzleImage), String> {
                let file = std::fs::File::open(file_path).map_err(|e| e.to_string())?;
                let limit = 512 * 1024 * 1024;
                let mut bytes = Vec::new();
                file.take(limit + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() as u64 > limit {
                    return Err("Image is too large".into());
                }
                let image = decode_image_bytes(&bytes)?;
                let original = crate::persistence::runtime::OriginalPuzzleImage {
                    hash: crate::persistence::image_hash(&bytes),
                    encoded: Some(bytes.into()),
                };
                Ok((image, original))
            })();
        let (image, original) = match result {
            Ok((image, original)) => (Ok(image), Some(original)),
            Err(error) => (Err(error), None),
        };
        let _ = sender.send(ImageLoadResult {
            virtual_key,
            image,
            original,
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        asset::RenderAssetUsages,
        render::{render_asset::RenderAsset, texture::GpuImage},
    };

    #[test]
    fn concurrent_registrations_keep_unique_keys_and_source_paths() {
        let registry = ExternalFileRegistry::default();
        let registrations = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|worker| {
                    let registry = &registry;
                    scope.spawn(move || {
                        (0..64)
                            .map(|index| {
                                let path = PathBuf::from(format!("source-{worker}-{index}.png"));
                                (registry.register_file(&path), path)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().unwrap())
                .collect::<Vec<_>>()
        });
        let keys: std::collections::HashSet<_> = registrations.iter().map(|(key, _)| key).collect();
        assert_eq!(keys.len(), 8 * 64);
        for (key, path) in registrations {
            assert!(key.ends_with(".png"));
            assert_eq!(registry.resolve_path(&key), Some(path));
        }
    }

    #[test]
    fn releasing_paths_preserves_other_entries_and_does_not_reuse_keys() {
        let registry = ExternalFileRegistry::default();
        let first = registry.register_file("first.png");
        let second = registry.register_file("second.jpg");
        assert_eq!(
            registry.get_original_filename(&first).as_deref(),
            Some("first.png")
        );
        assert_eq!(
            registry.unregister_file(&first),
            Some(PathBuf::from("first.png"))
        );
        assert_eq!(registry.unregister_file(&first), None);
        assert_eq!(registry.resolve_path(&first), None);
        assert_eq!(registry.get_original_filename(&first), None);
        assert!(registry.is_external_image_path(&first));
        assert_eq!(
            registry.resolve_path(&second),
            Some(PathBuf::from("second.jpg"))
        );

        registry.clear();
        assert!(registry.is_empty());
        assert_eq!(registry.resolve_path(&second), None);
        let next = registry.register_file("first.png");
        assert_ne!(next, first);
        assert_ne!(next, second);
        assert_eq!(registry.resolve_path(&first), None);
    }

    fn poison_paths(registry: &ExternalFileRegistry) {
        std::thread::scope(|scope| {
            assert!(scope
                .spawn(|| {
                    let _guard = registry.registered_paths.write().unwrap();
                    panic!("poison registry for recovery test");
                })
                .join()
                .is_err());
        });
        assert!(registry.registered_paths.is_poisoned());
    }

    #[test]
    fn poisoned_registry_recovers_for_reads_registration_and_cleanup() {
        let registry = ExternalFileRegistry::default();
        let first = registry.register_file("first.png");
        poison_paths(&registry);
        assert_eq!(
            registry.resolve_path(&first),
            Some(PathBuf::from("first.png"))
        );
        assert!(!registry.registered_paths.is_poisoned());

        poison_paths(&registry);
        let second = registry.register_file("second.jpg");
        assert_eq!(
            registry.resolve_path(&second),
            Some(PathBuf::from("second.jpg"))
        );
        assert_eq!(
            registry.resolve_path(&first),
            Some(PathBuf::from("first.png"))
        );

        poison_paths(&registry);
        assert_eq!(
            registry.unregister_file(&first),
            Some(PathBuf::from("first.png"))
        );
        poison_paths(&registry);
        registry.clear();
        assert!(registry.is_empty());
        assert!(!registry.registered_paths.is_poisoned());
    }

    #[test]
    fn selected_original_can_be_saved_after_source_file_is_deleted() {
        use crate::persistence::*;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.jpg");
        image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]))
            .save(&path)
            .unwrap();
        let original_bytes = std::fs::read(&path).unwrap();
        let (tx, rx) = crossbeam::channel::bounded(1);
        start_thread_image_load("fixture.jpg".into(), path.clone(), tx);
        let loaded = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(loaded.image.is_ok());
        std::fs::remove_file(path).unwrap();
        let original = loaded.original.unwrap();
        assert_eq!(original.hash, image_hash(&original_bytes));
        let repo = SaveRepository::new(FilesystemStorage::new(dir.path().join("data")));
        repo.import_image(original.hash, original.encoded.as_deref().unwrap())
            .unwrap();
        assert_eq!(repo.read_image(original.hash).unwrap(), original_bytes);
    }

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
