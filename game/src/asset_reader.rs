use crate::resources::ImageDecodeLimits;
use bevy::prelude::*;
use jigsall_core::{fit_image_size, MAX_PUZZLE_IMAGE_DIMENSION};
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
    pub image: Result<DecodedPuzzleImage, String>,
    pub original: Option<crate::persistence::runtime::OriginalPuzzleImage>,
}

#[derive(Debug)]
pub struct DecodedPuzzleImage {
    pub image: Image,
    pub source_size: UVec2,
    pub logical_size: UVec2,
}

// Source limits apply to files, saves, thumbnails and peer-supplied images alike.
// Bound pixel storage independently of compressed size and the local GPU budget.
const MAX_SOURCE_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_ENCODED_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SOURCE_IMAGE_DIMENSION: u32 = 32768;
const MAX_IMAGE_DECODER_ALLOC: u64 = 512 * 1024 * 1024;

/// Restrict puzzle inputs before decoding their pixels, including thumbnail requests.
pub(crate) fn decode_puzzle_image_bytes(encoded: &[u8]) -> image::ImageResult<image::DynamicImage> {
    image::DynamicImage::from_decoder(puzzle_image_decoder(encoded)?)
}

fn puzzle_image_decoder(encoded: &[u8]) -> image::ImageResult<impl image::ImageDecoder + '_> {
    use image::{
        error::{ImageFormatHint, LimitError, LimitErrorKind},
        ImageDecoder, ImageError, ImageFormat,
    };

    if encoded.len() as u64 > MAX_ENCODED_IMAGE_BYTES {
        return Err(ImageError::Limits(LimitError::from_kind(
            LimitErrorKind::InsufficientMemory,
        )));
    }
    let format = image::guess_format(encoded)?;
    if !matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::Bmp
            | ImageFormat::Gif
            | ImageFormat::WebP
    ) {
        return Err(ImageError::Unsupported(
            ImageFormatHint::Exact(format).into(),
        ));
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(encoded), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_SOURCE_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_DECODER_ALLOC);
    reader.limits(limits.clone());
    // Reuse this decoder: a second reader would parse the input again (JPEG also
    // copies the encoded bytes). No full pixel buffer exists at this point.
    let mut decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_SOURCE_IMAGE_PIXELS {
        return Err(ImageError::Limits(LimitError::from_kind(
            LimitErrorKind::DimensionError,
        )));
    }
    // from_decoder does not reserve its output against max_alloc. Preserve the
    // reservation performed by ImageReader::decode before allocating pixels.
    limits.reserve(decoder.total_bytes())?;
    decoder.set_limits(limits)?;
    Ok(decoder)
}

fn resize_puzzle_rgba(
    rgba: image::RgbaImage,
    size: UVec2,
    resizer: &mut fast_image_resize::Resizer,
) -> Result<image::RgbaImage, String> {
    use fast_image_resize::{images, FilterType, PixelType, ResizeAlg, ResizeOptions};

    if rgba.dimensions() == (size.x, size.y) {
        return Ok(rgba);
    }
    let source = images::ImageRef::new(rgba.width(), rgba.height(), &rgba, PixelType::U8x4)
        .map_err(|error| error.to_string())?;
    let mut destination = images::Image::new(size.x, size.y, PixelType::U8x4);
    // Match image's independent RGBA channel filtering. Alpha premultiplication
    // would both change existing coverage and allocate another source-size copy.
    let options = ResizeOptions::new()
        .resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3))
        .use_alpha(false);
    resizer
        .resize(&source, &mut destination, &options)
        .map_err(|error| error.to_string())?;
    // U8x4 convolution needs at most source_width * destination_height * 4
    // bytes (+ alignment), rather than image's 16-byte Rgba32F intermediate.
    Ok(image::RgbaImage::from_raw(size.x, size.y, destination.into_vec()).unwrap())
}

/// Decode and resize on the worker, before any Bevy asset can be uploaded.
/// Selected source files and verified .puzimg payloads use the same size policy.
pub fn decode_image_bytes(
    encoded: &[u8],
    limits: ImageDecodeLimits,
) -> Result<DecodedPuzzleImage, String> {
    if limits.max_texture_dimension == 0 {
        return Err("Puzzle texture dimension limit must be positive".into());
    }
    let decoded = decode_puzzle_image_bytes(encoded).map_err(|e| e.to_string())?;
    let source_size = UVec2::new(decoded.width(), decoded.height());
    let logical_size = fit_image_size(source_size, MAX_PUZZLE_IMAGE_DIMENSION);
    if logical_size == UVec2::ZERO {
        return Err("Image dimensions must be positive".into());
    }
    let texture_size = fit_image_size(logical_size, limits.max_texture_dimension);
    // Normalize to the final GPU format before resizing, so 16-bit inputs also
    // use bounded U8x4 scratch storage. Consuming RGBA8 reuses its pixel buffer.
    let rgba = resize_puzzle_rgba(
        decoded.into_rgba8(),
        texture_size,
        &mut fast_image_resize::Resizer::new(),
    )?;
    let image = Image::new(
        bevy::render::render_resource::Extent3d {
            width: texture_size.x,
            height: texture_size.y,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        rgba.into_raw(),
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    );
    Ok(DecodedPuzzleImage {
        image,
        source_size,
        logical_size,
    })
}

pub fn start_thread_image_load(
    virtual_key: String,
    file_path: PathBuf,
    sender: crossbeam::channel::Sender<ImageLoadResult>,
    limits: ImageDecodeLimits,
) {
    std::thread::spawn(move || {
        use std::io::Read;
        let result =
            (|| -> Result<(DecodedPuzzleImage, crate::persistence::runtime::OriginalPuzzleImage), String> {
                let file = std::fs::File::open(file_path).map_err(|e| e.to_string())?;
                let limit = MAX_ENCODED_IMAGE_BYTES;
                let mut bytes = Vec::new();
                file.take(limit + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() as u64 > limit {
                    return Err("Image is too large".into());
                }
                let image = decode_image_bytes(&bytes, limits)?;
                let original = crate::persistence::runtime::OriginalPuzzleImage {
                    hash: crate::persistence::image_hash(&bytes),
                    encoded: Some(bytes.into()),
                    image_lease: None,
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
pub(crate) mod tests {
    use super::*;
    use bevy::{
        asset::RenderAssetUsages,
        render::{render_asset::RenderAsset, texture::GpuImage},
    };

    pub(crate) fn image_with_claimed_dimensions(
        format: image::ImageFormat,
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        use image::ImageFormat;
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1, 1, image::Rgb([30, 60, 90])))
            .write_to(&mut bytes, format)
            .unwrap();
        let mut bytes = bytes.into_inner();
        match format {
            ImageFormat::Bmp => {
                bytes[18..22].copy_from_slice(&width.to_le_bytes());
                bytes[22..26].copy_from_slice(&height.to_le_bytes());
            }
            ImageFormat::Gif => {
                bytes[6..8].copy_from_slice(&(width as u16).to_le_bytes());
                bytes[8..10].copy_from_slice(&(height as u16).to_le_bytes());
            }
            ImageFormat::Jpeg => {
                let sof = bytes
                    .windows(2)
                    .position(|bytes| bytes == [0xff, 0xc0])
                    .unwrap();
                bytes[sof + 5..sof + 7].copy_from_slice(&(height as u16).to_be_bytes());
                bytes[sof + 7..sof + 9].copy_from_slice(&(width as u16).to_be_bytes());
            }
            ImageFormat::Png => {
                bytes[16..20].copy_from_slice(&width.to_be_bytes());
                bytes[20..24].copy_from_slice(&height.to_be_bytes());
                // Keep IHDR valid, so the limit check is reached before IDAT decode.
                let mut crc = u32::MAX;
                for byte in &bytes[12..29] {
                    crc ^= u32::from(*byte);
                    for _ in 0..8 {
                        crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
                    }
                }
                bytes[29..33].copy_from_slice(&(!crc).to_be_bytes());
            }
            ImageFormat::WebP => {
                assert_eq!(&bytes[12..16], b"VP8L");
                assert_eq!(bytes[20], 0x2f);
                let bits = u32::from_le_bytes(bytes[21..25].try_into().unwrap());
                let bits = (bits & 0xf000_0000) | (width - 1) | ((height - 1) << 14);
                bytes[21..25].copy_from_slice(&bits.to_le_bytes());
            }
            _ => unreachable!(),
        }
        bytes
    }

    #[test]
    fn source_limits_reject_oversized_headers_before_pixel_decode_in_every_codec() {
        use image::{error::LimitErrorKind, ImageError, ImageFormat, ImageReader};
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Bmp,
            ImageFormat::Gif,
            ImageFormat::WebP,
        ] {
            for (width, height) in [(9000, 9000), (16000, 8192), (8192, 16000)] {
                let bytes = image_with_claimed_dimensions(format, width, height);
                let mut reader = ImageReader::with_format(std::io::Cursor::new(&bytes), format);
                reader.no_limits();
                assert_eq!(
                    reader.into_dimensions().unwrap(),
                    (width, height),
                    "{format:?}"
                );
                let ImageError::Limits(error) = decode_puzzle_image_bytes(&bytes).unwrap_err()
                else {
                    panic!("{format:?} did not reject the source dimensions");
                };
                assert_eq!(error.kind(), LimitErrorKind::DimensionError);
                for cap in [128, 8192, 16384] {
                    assert!(decode_image_bytes(
                        &bytes,
                        ImageDecodeLimits {
                            max_texture_dimension: cap
                        }
                    )
                    .is_err());
                }
            }
        }
        for (width, height) in [
            (24000, 16000),
            (16000, 24000),
            (32769, 1),
            (1, 32769),
            (65535, 65535),
        ] {
            let bytes = image_with_claimed_dimensions(ImageFormat::Bmp, width, height);
            assert!(matches!(
                decode_puzzle_image_bytes(&bytes),
                Err(ImageError::Limits(_))
            ));
        }
    }

    #[test]
    fn source_limits_allow_the_pixel_boundary_and_long_narrow_images() {
        use image::{ImageDecoder, ImageFormat};
        for (width, height) in [
            (8192, 8192),
            (32768, 2048),
            (2048, 32768),
            (32768, 1),
            (1, 32768),
        ] {
            let bytes = image_with_claimed_dimensions(ImageFormat::Bmp, width, height);
            // Inspect preflight directly: boundary tests need no large pixel buffers.
            assert_eq!(
                puzzle_image_decoder(&bytes).unwrap().dimensions(),
                (width, height)
            );
        }
    }

    #[test]
    fn zero_or_overflowing_source_dimensions_are_rejected() {
        use image::{ImageError, ImageFormat};
        for (width, height) in [(0, 1), (1, 0), (i32::MAX as u32, i32::MAX as u32)] {
            let bytes = image_with_claimed_dimensions(ImageFormat::Bmp, width, height);
            assert!(decode_puzzle_image_bytes(&bytes).is_err());
        }
        // The pinned WebP decoder wraps its maximum VP8L width to zero.
        let bytes = image_with_claimed_dimensions(ImageFormat::WebP, 16384, 8192);
        assert!(matches!(
            decode_puzzle_image_bytes(&bytes),
            Err(ImageError::Limits(_))
        ));
    }

    #[test]
    fn rgba_resize_scratch_is_four_bytes_per_pixel_without_an_alpha_copy() {
        for (source, destination) in [
            (UVec2::new(1024, 768), UVec2::new(512, 384)),
            (UVec2::new(768, 1024), UVec2::new(384, 512)),
        ] {
            let rgba =
                image::RgbaImage::from_pixel(source.x, source.y, image::Rgba([30, 60, 90, 128]));
            let mut resizer = fast_image_resize::Resizer::new();
            let resized = resize_puzzle_rgba(rgba, destination, &mut resizer).unwrap();
            assert_eq!(resized.dimensions(), (destination.x, destination.y));
            assert!(resized.pixels().all(|pixel| pixel.0 == [30, 60, 90, 128]));
            let scratch = resizer.size_of_internal_buffers();
            assert!(scratch > 0);
            assert!(
                scratch <= source.x as usize * destination.y as usize * 4 + 4,
                "scratch={scratch}"
            );
        }
        let rgba = image::RgbaImage::from_pixel(8, 4, image::Rgba([30, 60, 90, 128]));
        let pointer = rgba.as_ptr();
        let mut resizer = fast_image_resize::Resizer::new();
        let resized = resize_puzzle_rgba(rgba, UVec2::new(8, 4), &mut resizer).unwrap();
        assert_eq!(resized.as_ptr(), pointer);
        assert_eq!(resizer.size_of_internal_buffers(), 0);
    }

    #[test]
    fn accepted_16_bit_png_is_normalized_to_the_gpu_format_before_resize() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba16(image::ImageBuffer::from_pixel(
            8,
            4,
            image::Rgba([0x1212u16, 0x3434, 0x5656, 0x8080]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        assert_eq!(
            decode_puzzle_image_bytes(bytes.get_ref()).unwrap().color(),
            image::ColorType::Rgba16
        );
        for cap in [2, 16384] {
            let decoded = decode_image_bytes(
                bytes.get_ref(),
                ImageDecodeLimits {
                    max_texture_dimension: cap,
                },
            )
            .unwrap();
            assert_eq!(decoded.logical_size, UVec2::new(8, 4));
            assert!(decoded
                .image
                .data
                .unwrap()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| *pixel == [0x12, 0x34, 0x56, 0x80]));
        }
    }

    #[test]
    fn supported_codecs_still_decode_ordinary_images() {
        for format in [
            image::ImageFormat::Png,
            image::ImageFormat::Jpeg,
            image::ImageFormat::Bmp,
            image::ImageFormat::Gif,
            image::ImageFormat::WebP,
        ] {
            let bytes = image_with_claimed_dimensions(format, 1, 1);
            let decoded = decode_image_bytes(
                &bytes,
                ImageDecodeLimits {
                    max_texture_dimension: 8192,
                },
            )
            .unwrap();
            assert_eq!(decoded.image.size(), UVec2::ONE);
            assert_eq!(decoded.logical_size, UVec2::ONE);
            assert_eq!(decoded.image.data.unwrap()[3], 255);
        }
    }

    #[test]
    fn file_worker_rejects_oversized_source_without_retaining_original_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oversized.bmp");
        std::fs::write(
            &path,
            image_with_claimed_dimensions(image::ImageFormat::Bmp, 24000, 16000),
        )
        .unwrap();
        let (tx, rx) = crossbeam::channel::bounded(1);
        start_thread_image_load(
            "oversized.bmp".into(),
            path,
            tx,
            ImageDecodeLimits {
                max_texture_dimension: 128,
            },
        );
        let loaded = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert_eq!(loaded.image.unwrap_err(), "Image size exceeds limit");
        assert!(loaded.original.is_none());
    }

    #[test]
    fn oversized_source_has_common_logical_size_and_local_texture_sizes() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            24000,
            16,
            image::Rgba([30, 60, 90, 255]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        let hash = crate::persistence::image_hash(bytes.get_ref());
        for (cap, expected) in [
            (16384, UVec2::new(16384, 11)),
            (8192, UVec2::new(8192, 6)),
            (128, UVec2::new(128, 1)),
        ] {
            let decoded = decode_image_bytes(
                bytes.get_ref(),
                ImageDecodeLimits {
                    max_texture_dimension: cap,
                },
            )
            .unwrap();
            assert_eq!(decoded.source_size, UVec2::new(24000, 16));
            assert_eq!(decoded.logical_size, UVec2::new(16384, 11));
            assert_eq!(decoded.image.size(), expected);
            assert_eq!(
                decoded.image.data.as_ref().unwrap().len(),
                expected.x as usize * expected.y as usize * 4
            );
            assert!(crate::resources::images::image_is_opaque(&decoded.image));
            assert_eq!(decoded.image.asset_usage, RenderAssetUsages::RENDER_WORLD);
            assert_eq!(crate::persistence::image_hash(bytes.get_ref()), hash);
        }
        assert!(decode_image_bytes(
            bytes.get_ref(),
            ImageDecodeLimits {
                max_texture_dimension: 0
            }
        )
        .is_err());
    }

    #[test]
    fn resized_rgba_preserves_transparency_and_small_images_are_not_upscaled() {
        for alpha in [0, 128, 255] {
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                8,
                4,
                image::Rgba([30, 60, 90, alpha]),
            ))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
            for cap in [2, 16384] {
                let decoded = decode_image_bytes(
                    bytes.get_ref(),
                    ImageDecodeLimits {
                        max_texture_dimension: cap,
                    },
                )
                .unwrap();
                assert_eq!(decoded.logical_size, UVec2::new(8, 4));
                assert_eq!(
                    decoded.image.size(),
                    if cap == 2 {
                        UVec2::new(2, 1)
                    } else {
                        UVec2::new(8, 4)
                    }
                );
                assert!(decoded
                    .image
                    .data
                    .unwrap()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|pixel| pixel[3] == alpha));
            }
        }
    }

    #[test]
    fn unused_image_formats_have_no_decoder() {
        use image::{error::ImageFormatHint, ImageError, ImageFormat};

        // Exercise decoder construction: ImageFormat::can_read ignores Cargo features.
        for format in ImageFormat::all().filter(|format| {
            !matches!(
                format,
                ImageFormat::Png
                    | ImageFormat::Jpeg
                    | ImageFormat::Bmp
                    | ImageFormat::Gif
                    | ImageFormat::WebP
            )
        }) {
            // arboard needs TIFF for native clipboard images on macOS.
            #[cfg(target_os = "macos")]
            if format == ImageFormat::Tiff {
                continue;
            }
            let error = image::load_from_memory_with_format(&[], format).unwrap_err();
            let ImageError::Unsupported(error) = error else {
                panic!("{format:?} reached a decoder: {error}");
            };
            assert_eq!(error.format_hint(), ImageFormatHint::Exact(format));
        }
    }

    #[test]
    fn content_detection_cannot_reach_unused_image_decoders() {
        use image::{error::ImageFormatHint, ImageError, ImageFormat};

        for (header, format) in [
            (b"II*\0".as_slice(), ImageFormat::Tiff),
            (b"MM\0*".as_slice(), ImageFormat::Tiff),
            (b"\x76\x2f\x31\x01".as_slice(), ImageFormat::OpenExr),
            (b"#?RADIANCE".as_slice(), ImageFormat::Hdr),
            (b"DDS ".as_slice(), ImageFormat::Dds),
            (b"\0\0\x01\0".as_slice(), ImageFormat::Ico),
            (b"P6\n1 1\n255\n\x49\x64\xb5".as_slice(), ImageFormat::Pnm),
            (b"farbfeld".as_slice(), ImageFormat::Farbfeld),
            (b"qoif".as_slice(), ImageFormat::Qoi),
            (b"\0\0\0\0ftypavif".as_slice(), ImageFormat::Avif),
        ] {
            assert_eq!(image::guess_format(header).unwrap(), format);
            let ImageError::Unsupported(error) = decode_puzzle_image_bytes(header).unwrap_err()
            else {
                panic!("{format:?} reached a decoder");
            };
            assert_eq!(error.format_hint(), ImageFormatHint::Exact(format));
            assert!(decode_image_bytes(
                header,
                ImageDecodeLimits {
                    max_texture_dimension: 8192
                }
            )
            .is_err());
        }
    }

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
        start_thread_image_load(
            "fixture.jpg".into(),
            path.clone(),
            tx,
            ImageDecodeLimits {
                max_texture_dimension: 8192,
            },
        );
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
            "jigsall-image-load-{}-{}.png",
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
        start_thread_image_load(
            "fixture.png".into(),
            path.clone(),
            tx,
            ImageDecodeLimits {
                max_texture_dimension: 8192,
            },
        );
        let result = rx.recv_timeout(std::time::Duration::from_secs(10));
        std::fs::remove_file(path).unwrap();
        let mut image = result.unwrap().image.unwrap().image;
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
