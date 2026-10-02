use super::*;
use crate::{
    checkpoint::{
        PuzzleCheckpoint, SNAPSHOT_CONNECTED_DOWN, SNAPSHOT_CONNECTED_RIGHT, SNAPSHOT_PLACED,
    },
    resources::{pieces::*, PieceDataStore},
};
use bevy::math::{UVec2, Vec2};
use puzzella_core::{PieceId, PuzzleDefinition, GENERATOR_VERSION};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

fn encoded_image(format: image::ImageFormat) -> Vec<u8> {
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        2,
        2,
        image::Rgb([73, 100, 181]),
    ));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).unwrap();
    bytes.into_inner()
}
fn checkpoint(bytes: &[u8]) -> PuzzleCheckpoint {
    let definition = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 987,
        grid_size: UVec2::splat(2),
        image_size: UVec2::splat(2),
        snap_distance: 12.5,
    };
    let mut store = PieceDataStore::default();
    store.initialize(
        (0..4)
            .map(|i| definition.correct_position(PieceId(i)) + Vec2::splat(50.25))
            .collect(),
    );
    store.states[0].position = definition.correct_position(PieceId(0));
    store.states[0].flags |= PLACED;
    store.placed_count = 1;
    store.states[2].position.x = f32::from_bits(0x42c80001);
    store.connectivity.union(PieceId(1), PieceId(3));
    PuzzleCheckpoint::capture(&store, &definition, image_hash(bytes)).unwrap()
}
fn save() -> PuzzleSave {
    PuzzleSave {
        metadata: SaveMetadata {
            id: SaveId(0x1234),
            title: SaveTitle::new("  星のパズル  ").unwrap(),
            revision: 1,
            created_at: 100,
            updated_at: 101,
        },
        checkpoint: checkpoint(&encoded_image(image::ImageFormat::Png)),
    }
}
fn resign(bytes: &mut [u8]) {
    let end = bytes.len() - 32;
    let digest = image_hash(&bytes[..end]);
    bytes[end..].copy_from_slice(&digest.0);
}

#[test]
fn title_validates_scalars_trims_and_allows_duplicates() {
    for title in [
        "",
        "   ",
        "\t",
        "line\nbreak",
        "\rtitle",
        "t\0itle",
        "title\u{7f}",
        "x\u{2028}y",
    ] {
        assert!(SaveTitle::new(title).is_err(), "{title:?}");
    }
    assert!(SaveTitle::new(&"猫".repeat(80)).is_ok());
    assert!(SaveTitle::new(&"猫".repeat(81)).is_err());
    assert!(SaveTitle::new(&"a".repeat(80)).is_ok());
    assert!(SaveTitle::new(&"a".repeat(81)).is_err());
    assert_eq!(SaveTitle::new("  星 ⭐  ").unwrap().as_str(), "星 ⭐");
    assert_eq!(SaveTitle::new("same"), SaveTitle::new("same"));
}
#[test]
fn keys_are_stable_id_based_and_cannot_contain_title_paths() {
    let id = SaveId(0x1234);
    assert_eq!(
        StorageKey::Save(id).filename(),
        "00000000000000000000000000001234.puzsave"
    );
    let mut s = save();
    let key = StorageKey::Save(s.metadata.id);
    s.metadata.title = SaveTitle::new("../../also legal as a title").unwrap();
    assert_eq!(key, StorageKey::Save(s.metadata.id));
    assert!(!key.filename().contains('/'));
}
#[test]
fn image_original_png_and_jpeg_payloads_are_bit_identical() {
    for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
        let bytes = encoded_image(format);
        let hash = image_hash(&bytes);
        assert_eq!(hash, image_hash(&bytes.clone()));
        let container = PuzImage::encode(&bytes).unwrap();
        assert_eq!(PuzImage::decode(&container, hash).unwrap(), bytes);
        assert_eq!(&container[18..50], &hash.0);
        assert!(crate::asset_reader::decode_image_bytes(
            PuzImage::decode(&container, hash).unwrap()
        )
        .is_ok());
    }
    // Known SHA-256 vector, independent of our codec.
    assert_eq!(
        StorageKey::Image(image_hash(b"abc")).filename(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad.puzimg"
    );
}
#[test]
fn image_rejects_magic_version_length_hash_and_key_mismatches() {
    let payload = encoded_image(image::ImageFormat::Png);
    let hash = image_hash(&payload);
    let original = PuzImage::encode(&payload).unwrap();
    for len in 0..original.len() {
        assert!(PuzImage::decode(&original[..len], hash).is_err());
    }
    let mut bytes = original.clone();
    bytes[0] ^= 1;
    assert!(matches!(
        PuzImage::decode(&bytes, hash),
        Err(SaveError::CorruptImage(_))
    ));
    bytes = original.clone();
    bytes[8..10].copy_from_slice(&9u16.to_le_bytes());
    assert_eq!(
        PuzImage::decode(&bytes, hash),
        Err(SaveError::UnsupportedImageFormat(9))
    );
    bytes = original.clone();
    bytes[10..18].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(PuzImage::decode(&bytes, hash).is_err());
    bytes = original.clone();
    bytes[50] ^= 1;
    assert!(matches!(
        PuzImage::decode(&bytes, hash),
        Err(SaveError::CorruptImage("Payload hash mismatch"))
    ));
    assert!(matches!(
        PuzImage::decode(&original, ImageHash([0; 32])),
        Err(SaveError::CorruptImage("Storage key hash mismatch"))
    ));
    bytes = original.clone();
    bytes.push(0);
    assert!(PuzImage::decode(&bytes, hash).is_err());
}
#[test]
fn binary_save_round_trip_keeps_metadata_definition_flags_and_float_bits() {
    let save = save();
    let encoded = SaveCodec::encode(&save).unwrap();
    assert_eq!(
        encoded.len(),
        156 + save.metadata.title.as_str().len() + 16 * 4
    );
    let restored = SaveCodec::decode(&encoded).unwrap();
    assert_eq!(save, restored);
    for (a, b) in save
        .checkpoint
        .pieces
        .iter()
        .zip(&restored.checkpoint.pieces)
    {
        assert_eq!(a.position.x.to_bits(), b.position.x.to_bits());
        assert_eq!(a.position.y.to_bits(), b.position.y.to_bits());
        assert_eq!(a.z_order, b.z_order);
        assert_eq!(a.flags, b.flags);
    }
    assert_eq!(restored.checkpoint.pieces[0].flags, SNAPSHOT_PLACED);
    assert_ne!(
        restored.checkpoint.pieces[1].flags & SNAPSHOT_CONNECTED_DOWN,
        0
    );
}

#[test]
fn binary_save_restores_both_right_and_down_edges() {
    let mut save = save();
    save.checkpoint.pieces[2].position =
        save.checkpoint.definition.correct_position(PieceId(2)) + Vec2::splat(50.25);
    save.checkpoint.pieces[2].flags = SNAPSHOT_CONNECTED_RIGHT;
    let decoded = SaveCodec::decode(&SaveCodec::encode(&save).unwrap()).unwrap();
    assert_eq!(save, decoded);
    let mut restored = PieceDataStore::default();
    decoded.checkpoint.install(&mut restored).unwrap();
    assert!(restored.connectivity.same_component(PieceId(1), PieceId(2)));
    assert!(restored.connectivity.same_component(PieceId(2), PieceId(3)));
    assert_ne!(restored.states[2].flags & CONNECTED_RIGHT, 0);
    assert_ne!(restored.states[3].flags & CONNECTED_LEFT, 0);
}

#[test]
fn million_piece_save_keeps_sixteen_byte_records_and_restores_directly() {
    let mut save = save();
    let definition = &mut save.checkpoint.definition;
    definition.grid_size = UVec2::splat(1000);
    definition.image_size = UVec2::splat(4096);
    let mut store = PieceDataStore::default();
    store.initialize_dense(crate::resources::DensePieceStates::generate(definition));
    save.checkpoint =
        PuzzleCheckpoint::capture(&store, definition, save.checkpoint.image_hash).unwrap();
    let bytes = SaveCodec::encode(&save).unwrap();
    assert_eq!(
        bytes.len(),
        16_000_000 + 156 + save.metadata.title.as_str().len()
    );
    let decoded = SaveCodec::decode(&bytes).unwrap();
    assert_eq!(save, decoded);
    let mut restored = PieceDataStore::default();
    decoded.checkpoint.install(&mut restored).unwrap();
    assert_eq!(store.states, restored.states);
    assert_eq!(store.next_z_order, restored.next_z_order);
    assert_eq!(restored.len(), 1_000_000);
}
#[test]
fn binary_save_rejects_truncation_checksum_unknown_version_and_malicious_lengths() {
    let original = SaveCodec::encode(&save()).unwrap();
    for len in 0..original.len() {
        assert!(SaveCodec::decode(&original[..len]).is_err());
    }
    let mut bytes = original.clone();
    bytes[0] ^= 1;
    assert!(matches!(
        SaveCodec::decode(&bytes),
        Err(SaveError::CorruptSave("Magic mismatch"))
    ));
    bytes = original.clone();
    bytes[8..10].copy_from_slice(&9u16.to_le_bytes());
    assert_eq!(
        SaveCodec::decode(&bytes),
        Err(SaveError::UnsupportedSaveFormat(9))
    );
    bytes = original.clone();
    bytes[100] ^= 1;
    assert!(matches!(
        SaveCodec::decode(&bytes),
        Err(SaveError::CorruptSave("Checksum mismatch"))
    ));
    bytes = original.clone();
    bytes[50..54].copy_from_slice(&u32::MAX.to_le_bytes());
    resign(&mut bytes);
    assert!(SaveCodec::decode(&bytes).is_err());
    let title_len = save().metadata.title.as_str().len();
    bytes = original.clone();
    bytes[86 + title_len..88 + title_len].copy_from_slice(&0u16.to_le_bytes());
    resign(&mut bytes);
    assert_eq!(
        SaveCodec::decode(&bytes),
        Err(SaveError::UnsupportedGenerator(0))
    );
    bytes = original.clone();
    bytes[120 + title_len..124 + title_len].copy_from_slice(&u32::MAX.to_le_bytes());
    resign(&mut bytes);
    assert!(SaveCodec::decode(&bytes).is_err());
    bytes = original.clone();
    bytes.push(0);
    resign(&mut bytes);
    assert!(SaveCodec::decode(&bytes).is_err());
}
#[test]
fn restore_uses_shared_dsu_validation_and_resets_presentation() {
    let save = SaveCodec::decode(&SaveCodec::encode(&save()).unwrap()).unwrap();
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::ZERO; 4]);
    let epoch = store.epoch;
    store.selected_pieces.insert(PieceId(2));
    store
        .held_by
        .insert(PieceId(2), puzzella_core::LOCAL_PLAYER);
    store.drag.members = Arc::from([4]);
    store.drag.delta = Vec2::splat(18.0);
    save.checkpoint.install(&mut store).unwrap();
    assert_ne!(epoch, store.epoch);
    assert!(store.selected_pieces.is_empty());
    assert!(store.held_by.is_empty());
    assert!(store.drag.members.is_empty());
    assert_eq!(store.drag.delta, Vec2::ZERO);
    assert_eq!(store.placed_count, 1);
    assert_eq!(store.next_z_order, save.checkpoint.next_z_order);
    assert!(store.connectivity.same_component(PieceId(1), PieceId(3)));
    assert!(!store.connectivity.same_component(PieceId(0), PieceId(1)));
    assert_ne!(store.states[1].flags & CONNECTED_BOTTOM, 0);
    assert_ne!(store.states[3].flags & CONNECTED_TOP, 0);
    assert_eq!(
        PuzzleCheckpoint::capture(
            &store,
            &save.checkpoint.definition,
            save.checkpoint.image_hash
        )
        .unwrap(),
        save.checkpoint
    );
    store.drag.members = Arc::from([1]);
    assert!(matches!(
        PuzzleCheckpoint::capture(
            &store,
            &save.checkpoint.definition,
            save.checkpoint.image_hash
        ),
        Err(crate::checkpoint::CheckpointError::ActiveLocalDrag)
    ));
}
#[test]
fn invalid_checkpoint_install_is_transactional() {
    let good = save().checkpoint;
    let mut store = PieceDataStore::default();
    good.install(&mut store).unwrap();
    let states = store.states.clone();
    let epoch = store.epoch;
    for case in 0..6 {
        let mut invalid = good.clone();
        match case {
            0 => invalid.pieces[1].flags |= SNAPSHOT_CONNECTED_RIGHT,
            1 => invalid.pieces[3].position += Vec2::ONE,
            2 => invalid.pieces[0].position += Vec2::ONE,
            3 => invalid.pieces[0].position.x = f32::NAN,
            4 => invalid.pieces[0].z_order = invalid.next_z_order,
            _ => invalid.pieces[0].flags |= 1 << 10,
        }
        assert!(invalid.install(&mut store).is_err());
        assert_eq!(store.states, states);
        assert_eq!(store.epoch, epoch);
    }
}
#[test]
fn filesystem_create_update_list_load_delete_deduplicate_and_ignore_temps() {
    let dir = tempfile::tempdir().unwrap();
    let repo = SaveRepository::new(FilesystemStorage::new(dir.path()));
    let bytes = encoded_image(image::ImageFormat::Png);
    let title = SaveTitle::new("../shared title").unwrap();
    let first = repo
        .create(title.clone(), checkpoint(&bytes), Some(&bytes))
        .unwrap();
    let second = repo.create(title, checkpoint(&bytes), None).unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(
        std::fs::read_dir(dir.path().join("images"))
            .unwrap()
            .count(),
        1
    );
    let path = dir
        .path()
        .join("saves")
        .join(StorageKey::Save(first.id).filename());
    assert!(path.exists());
    let old = std::fs::read(&path).unwrap();
    let updated = repo
        .update(
            first.id,
            SaveTitle::new("new title").unwrap(),
            checkpoint(&bytes),
            None,
        )
        .unwrap();
    assert_eq!(updated.id, first.id);
    assert_eq!(updated.created_at, first.created_at);
    assert_eq!(updated.revision, first.revision + 1);
    assert!(updated.updated_at >= first.updated_at);
    assert_ne!(old, std::fs::read(&path).unwrap());
    assert_eq!(repo.load(first.id).unwrap().save.metadata, updated);
    assert_eq!(repo.load(first.id).unwrap().image_bytes, bytes);
    std::fs::write(
        dir.path().join("saves").join(".puzzella-ignored.tmp"),
        b"partial",
    )
    .unwrap();
    std::fs::write(dir.path().join("saves").join("wrong.puzsave"), b"partial").unwrap();
    let list = repo.list().unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().all(|e| e.summary.is_ok()));
    repo.delete(first.id).unwrap();
    assert_eq!(repo.list().unwrap().len(), 1);
    repo.delete(first.id).unwrap();
    assert!(repo.load(first.id).is_err());
}
#[test]
fn failed_filesystem_replace_preserves_previous_file_and_temp_is_not_listed() {
    let dir = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(dir.path());
    let key = StorageKey::Save(SaveId(1));
    storage.write(key, b"old data").unwrap();
    let path = dir.path().join("saves").join(key.filename());
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(storage.write(key, b"new data").is_err());
        assert_eq!(storage.read(key).unwrap(), b"old data");
        drop(locked);
    }
    #[cfg(not(windows))]
    let _ = path;
    storage.write(key, b"new data").unwrap();
    assert_eq!(storage.read(key).unwrap(), b"new data");
    assert_eq!(storage.list(StorageNamespace::Saves).unwrap(), vec![key]);
}

#[derive(Clone, Default)]
struct MemoryStorage {
    blobs: Arc<Mutex<HashMap<StorageKey, Vec<u8>>>>,
    writes: Arc<Mutex<Vec<StorageKey>>>,
    fail: Arc<Mutex<Option<StorageKey>>>,
}
impl SaveStorage for MemoryStorage {
    fn list(&self, n: StorageNamespace) -> Result<Vec<StorageKey>, StorageError> {
        Ok(self
            .blobs
            .lock()
            .unwrap()
            .keys()
            .filter(|key| {
                matches!(
                    (n, key),
                    (StorageNamespace::Saves, StorageKey::Save(_))
                        | (StorageNamespace::Images, StorageKey::Image(_))
                )
            })
            .copied()
            .collect())
    }
    fn read(&self, key: StorageKey) -> Result<Vec<u8>, StorageError> {
        self.blobs
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or(StorageError::NotFound(key))
    }
    fn write(&self, key: StorageKey, bytes: &[u8]) -> Result<(), StorageError> {
        if *self.fail.lock().unwrap() == Some(key) {
            return Err(StorageError::Io("injected failure".into()));
        }
        self.writes.lock().unwrap().push(key);
        self.blobs.lock().unwrap().insert(key, bytes.to_vec());
        Ok(())
    }
    fn delete(&self, key: StorageKey) -> Result<(), StorageError> {
        self.blobs.lock().unwrap().remove(&key);
        Ok(())
    }
    fn exists(&self, key: StorageKey) -> Result<bool, StorageError> {
        Ok(self.blobs.lock().unwrap().contains_key(&key))
    }
}
#[test]
fn backend_is_path_free_handles_collisions_and_never_publishes_before_image() {
    let storage = MemoryStorage::default();
    let repo = SaveRepository::new(storage.clone());
    let bytes = encoded_image(image::ImageFormat::Png);
    let title = SaveTitle::new("same").unwrap();
    let first = repo
        .create_with_ids(title.clone(), checkpoint(&bytes), Some(&bytes), || 1)
        .unwrap();
    let mut ids = [1, 2].into_iter();
    let second = repo
        .create_with_ids(title.clone(), checkpoint(&bytes), None, || {
            ids.next().unwrap()
        })
        .unwrap();
    assert_eq!(first.id, SaveId(1));
    assert_eq!(second.id, SaveId(2));
    assert_eq!(
        *storage.writes.lock().unwrap(),
        vec![
            StorageKey::Image(image_hash(&bytes)),
            StorageKey::Save(first.id),
            StorageKey::Save(second.id)
        ]
    );
    assert_eq!(
        repo.create_with_ids(title, checkpoint(&bytes), None, || 1),
        Err(SaveError::IdCollision)
    );
    let other = b"missing original";
    *storage.fail.lock().unwrap() = Some(StorageKey::Image(image_hash(other)));
    assert!(repo
        .create_with_ids(
            SaveTitle::new("fail").unwrap(),
            checkpoint(other),
            Some(other),
            || 3
        )
        .is_err());
    assert!(!storage.exists(StorageKey::Save(SaveId(3))).unwrap());
    *storage.fail.lock().unwrap() = Some(StorageKey::Save(SaveId(4)));
    assert!(repo
        .create_with_ids(
            SaveTitle::new("fail").unwrap(),
            checkpoint(other),
            Some(other),
            || 4
        )
        .is_err());
    assert!(storage
        .exists(StorageKey::Image(image_hash(other)))
        .unwrap());
    assert!(!storage.exists(StorageKey::Save(SaveId(4))).unwrap());
}
#[test]
fn corrupt_save_and_missing_image_are_individual_list_errors() {
    let storage = MemoryStorage::default();
    let repo = SaveRepository::new(storage.clone());
    let bytes = encoded_image(image::ImageFormat::Png);
    let good = repo
        .create(
            SaveTitle::new("valid").unwrap(),
            checkpoint(&bytes),
            Some(&bytes),
        )
        .unwrap();
    storage
        .write(StorageKey::Save(SaveId(5)), b"broken")
        .unwrap();
    let list = repo.list().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list.iter().filter(|e| e.summary.is_err()).count(), 1);
    storage
        .delete(StorageKey::Image(image_hash(&bytes)))
        .unwrap();
    assert!(matches!(
        repo.load(good.id),
        Err(SaveError::MissingImage(_))
    ));
    assert_eq!(
        repo.list()
            .unwrap()
            .iter()
            .filter(|e| e.summary.is_err())
            .count(),
        2
    );
}
#[test]
fn save_identity_must_match_its_storage_key() {
    let storage = MemoryStorage::default();
    storage
        .write(
            StorageKey::Save(SaveId(7)),
            &SaveCodec::encode(&save()).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        SaveRepository::new(storage).read_save(SaveId(7)),
        Err(SaveError::CorruptSave("Save ID does not match storage key"))
    ));
}
#[test]
fn schema3_serialized_field_order_and_semantics_are_frozen() {
    use crate::multiplayer::*;
    use puzzella_core::session::{AuthorityCursor, SessionId};
    let c = save().checkpoint;
    let wire = GameSnapshot {
        schema_version: 3,
        session: SessionId(9),
        image_hash: c.image_hash,
        cursor: AuthorityCursor::new(1, 2),
        definition: c.definition.clone(),
        next_z_order: c.next_z_order,
        pieces: c.pieces.clone(),
    };
    let json = serde_json::to_string(&wire).unwrap();
    let fields = [
        "schema_version",
        "session",
        "image_hash",
        "cursor",
        "definition",
        "next_z_order",
        "pieces",
    ];
    let offsets: Vec<_> = fields
        .iter()
        .map(|f| json.find(&format!("\"{f}\":")).unwrap())
        .collect();
    assert!(offsets.windows(2).all(|w| w[0] < w[1]));
    assert!(!json.contains("checkpoint"));
    assert_eq!(serde_json::from_str::<GameSnapshot>(&json).unwrap(), wire);
    assert_eq!(wire.clone().into_checkpoint(), c);
    assert_eq!(SNAPSHOT_SCHEMA_VERSION, 3);
}
