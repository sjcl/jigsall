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
            is_autosave: false,
        },
        checkpoint: checkpoint(&encoded_image(image::ImageFormat::Png)),
    }
}
fn resign_header(bytes: &mut [u8]) {
    let len = u32::from_le_bytes(bytes[10..14].try_into().unwrap()) as usize;
    let digest = image_hash(&bytes[..len - 32]);
    bytes[len - 32..len].copy_from_slice(&digest.0);
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
        205 + save.metadata.title.as_str().len() + 16 * 4
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
fn autosave_flag_round_trips_in_full_save_and_list_header_and_rejects_invalid_values() {
    for is_autosave in [false, true] {
        let mut save = save();
        save.metadata.is_autosave = is_autosave;
        let mut bytes = SaveCodec::encode(&save).unwrap();
        assert_eq!(
            SaveCodec::decode(&bytes).unwrap().metadata.is_autosave,
            is_autosave
        );
        assert_eq!(
            SaveCodec::decode_header(&bytes, bytes.len() as u64)
                .unwrap()
                .metadata
                .is_autosave,
            is_autosave
        );
        let header_len = u32::from_le_bytes(bytes[10..14].try_into().unwrap()) as usize;
        bytes[header_len - 33] = 2;
        resign_header(&mut bytes);
        resign(&mut bytes);
        assert_eq!(
            SaveCodec::decode(&bytes),
            Err(SaveError::CorruptSave("Invalid autosave flag"))
        );
    }
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
        16_000_000 + 205 + save.metadata.title.as_str().len()
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
    bytes[62..66].copy_from_slice(&u32::MAX.to_le_bytes());
    resign_header(&mut bytes);
    resign(&mut bytes);
    assert!(SaveCodec::decode(&bytes).is_err());
    let title_len = save().metadata.title.as_str().len();
    bytes = original.clone();
    bytes[98 + title_len..100 + title_len].copy_from_slice(&0u16.to_le_bytes());
    resign_header(&mut bytes);
    resign(&mut bytes);
    assert_eq!(
        SaveCodec::decode(&bytes),
        Err(SaveError::UnsupportedGenerator(0))
    );
    bytes = original.clone();
    bytes[132 + title_len..136 + title_len].copy_from_slice(&u32::MAX.to_le_bytes());
    resign_header(&mut bytes);
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
}

#[test]
fn checkpoint_during_drag_captures_canonical_state_and_leaves_drag_untouched() {
    use puzzella_core::{PieceCommand, LOCAL_PLAYER};
    let expected = save().checkpoint;
    let mut store = PieceDataStore::default();
    expected.install(&mut store).unwrap();
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::Grab(PieceId(1)),
        Some(&expected.definition),
        LOCAL_PLAYER,
    );
    store.drag.members = Arc::from([0b1010]);
    let states = store.states.clone();
    let members = store.drag.members.clone();
    let held_by = store.held_by.clone();
    let dirty = store.dirty_pieces.clone();
    let epoch = store.epoch;
    assert!(!members.is_empty());
    assert_eq!(held_by.len(), 2);
    for delta in [Vec2::ZERO, Vec2::new(100.0, 200.0)] {
        store.drag.delta = delta;
        let checkpoint =
            PuzzleCheckpoint::capture(&store, &expected.definition, expected.image_hash).unwrap();
        for (piece, canonical) in checkpoint.pieces.iter().zip(states.iter()) {
            assert_eq!(piece.position, canonical.position);
            assert_eq!(piece.z_order, canonical.z_order);
        }
        assert_eq!(checkpoint.pieces[0], expected.pieces[0]);
        assert_eq!(checkpoint.pieces[1].flags, expected.pieces[1].flags);
        assert_eq!(store.states, states);
        assert_eq!(store.held_by, held_by);
        assert!(Arc::ptr_eq(&store.drag.members, &members));
        assert_eq!(store.drag.delta, delta);
        assert_eq!(store.dirty_pieces, dirty);
        assert_eq!(store.epoch, epoch);
    }
}

#[test]
fn save_during_rebased_drag_restores_committed_rotation_and_discards_only_transient_delta() {
    use puzzella_core::{decode_rotation, PieceBitSet, PieceCommand, LOCAL_PLAYER};
    let mut save = save();
    let mut store = PieceDataStore::default();
    save.checkpoint.install(&mut store).unwrap();
    let before_grab = store.states.clone();
    store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::Grab(PieceId(1)),
        Some(&save.checkpoint.definition),
        LOCAL_PLAYER,
    );
    let mut members = PieceBitSet::new(store.len());
    members.extend([PieceId(1), PieceId(3)]);
    store.drag.members = members.words().clone();
    store.drag.delta = Vec2::new(100.0, 0.0);
    let result = store.apply_command(
        LOCAL_PLAYER,
        &PieceCommand::RotateDrag {
            members: members.clone(),
            delta: store.drag.delta,
            quarter_turns: 1,
        },
        Some(&save.checkpoint.definition),
        LOCAL_PLAYER,
    );
    assert!(result.drag_rebased);
    assert_eq!(result.rotated, 2);
    assert_eq!(store.drag.delta, Vec2::ZERO);
    assert_ne!(store.states[1].position, before_grab[1].position);
    store.drag.delta = Vec2::new(50.0, 0.0);
    store.selected_pieces = members.clone();
    store.sync_highlights();
    let rebased = store.states.clone();
    save.checkpoint = PuzzleCheckpoint::capture(
        &store,
        &save.checkpoint.definition,
        save.checkpoint.image_hash,
    )
    .unwrap();
    let loaded = SaveCodec::decode(&SaveCodec::encode(&save).unwrap()).unwrap();
    loaded.checkpoint.install(&mut store).unwrap();
    for (piece, canonical) in store.states.iter().zip(rebased.iter()) {
        assert_eq!(piece.position, canonical.position);
        assert_eq!(piece.z_order, canonical.z_order);
        assert_eq!(
            decode_rotation(piece.flags),
            decode_rotation(canonical.flags)
        );
        assert_eq!(piece.flags & PLACED, canonical.flags & PLACED);
        assert_eq!(piece.flags & (HELD | SELECTED | PREVIEW), 0);
    }
    assert_eq!(decode_rotation(store.states[1].flags), 1);
    assert_eq!(store.states[0].position, before_grab[0].position);
    assert_eq!(store.placed_count, 1);
    assert!(store.connectivity.same_component(PieceId(1), PieceId(3)));
    assert!(!store.connectivity.same_component(PieceId(0), PieceId(1)));
    assert!(store.held_by.is_empty());
    assert!(store.selected_pieces.is_empty());
    assert!(store.drag.members.is_empty());
    assert_eq!(store.drag.delta, Vec2::ZERO);
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
            first.revision,
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
    storage.write(key, b"old data".to_vec()).unwrap();
    let path = dir.path().join("saves").join(key.filename());
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(storage.write(key, b"new data".to_vec()).is_err());
        assert_eq!(storage.read(key).unwrap(), b"old data");
        drop(locked);
    }
    #[cfg(not(windows))]
    let _ = path;
    storage.write(key, b"new data".to_vec()).unwrap();
    assert_eq!(storage.read(key).unwrap(), b"new data");
    assert_eq!(storage.list(StorageNamespace::Saves).unwrap(), vec![key]);
}

#[derive(Clone, Default)]
struct MemoryStorage {
    blobs: Arc<Mutex<HashMap<StorageKey, Vec<u8>>>>,
    writes: Arc<Mutex<Vec<StorageKey>>>,
    fail: Arc<Mutex<Option<StorageKey>>>,
    reads: Arc<Mutex<Vec<StorageKey>>>,
    ranges: Arc<Mutex<Vec<(StorageKey, u64, usize)>>>,
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
        self.reads.lock().unwrap().push(key);
        self.blobs
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .ok_or(StorageError::NotFound(key))
    }
    fn read_range(
        &self,
        key: StorageKey,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, StorageError> {
        self.ranges.lock().unwrap().push((key, offset, length));
        let blobs = self.blobs.lock().unwrap();
        let bytes = blobs.get(&key).ok_or(StorageError::NotFound(key))?;
        let start = usize::try_from(offset)
            .map_err(|_| StorageError::InvalidRange)?
            .min(bytes.len());
        Ok(bytes[start..start.saturating_add(length).min(bytes.len())].to_vec())
    }
    fn len(&self, key: StorageKey) -> Result<u64, StorageError> {
        self.blobs
            .lock()
            .unwrap()
            .get(&key)
            .map(|bytes| bytes.len() as u64)
            .ok_or(StorageError::NotFound(key))
    }
    fn write(&self, key: StorageKey, bytes: Vec<u8>) -> Result<(), StorageError> {
        if *self.fail.lock().unwrap() == Some(key) {
            return Err(StorageError::Io("injected failure".into()));
        }
        self.writes.lock().unwrap().push(key);
        self.blobs.lock().unwrap().insert(key, bytes);
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
        .write(StorageKey::Save(SaveId(5)), b"broken".to_vec())
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
            SaveCodec::encode(&save()).unwrap(),
        )
        .unwrap();
    let repo = SaveRepository::new(storage);
    assert!(matches!(
        repo.read_save(SaveId(7)),
        Err(SaveError::CorruptSave("Save ID does not match storage key"))
    ));
    assert!(matches!(
        repo.read_header(SaveId(7)),
        Err(SaveError::CorruptSave("Save ID does not match storage key"))
    ));
    assert!(repo.list().unwrap()[0].summary.is_err());
}
#[test]
fn snapshot_serialized_field_order_preserves_dense_records() {
    use crate::multiplayer::*;
    use puzzella_core::session::{AuthorityCursor, SessionId};
    let c = save().checkpoint;
    let wire = GameSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
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
    assert_eq!(SNAPSHOT_SCHEMA_VERSION, 4);
}

#[test]
fn preview_validates_header_but_load_checks_body_and_placed_cache() {
    let mut longest = save();
    longest.metadata.title = SaveTitle::new(&"🧩".repeat(80)).unwrap();
    let encoded = SaveCodec::encode(&longest).unwrap();
    assert_eq!(
        u32::from_le_bytes(encoded[10..14].try_into().unwrap()) as usize,
        MAX_SAVE_HEADER_BYTES
    );
    assert_eq!(
        SaveCodec::decode_header(&encoded[..MAX_SAVE_HEADER_BYTES], encoded.len() as u64)
            .unwrap()
            .metadata,
        longest.metadata
    );
    let original = SaveCodec::encode(&save()).unwrap();
    let len = original.len() as u64;
    let header = SaveCodec::decode_header(&original, len).unwrap();
    assert_eq!(header.placed_count, 1);
    assert_eq!(header.piece_count, 4);
    let header_len = u32::from_le_bytes(original[10..14].try_into().unwrap()) as usize;
    for n in 0..header_len {
        assert!(SaveCodec::decode_header(&original[..n], len).is_err());
    }
    let mut bytes = original.clone();
    bytes[38] ^= 1;
    assert!(matches!(
        SaveCodec::decode_header(&bytes, len),
        Err(SaveError::CorruptSave("Header checksum mismatch"))
    ));
    let mut bytes = original.clone();
    bytes[header_len] ^= 1;
    assert!(SaveCodec::decode_header(&bytes[..header_len], len).is_ok());
    assert!(SaveCodec::decode(&bytes).is_err());
    let mut bytes = original.clone();
    let placed_offset = 136 + save().metadata.title.as_str().len();
    bytes[placed_offset..placed_offset + 4].copy_from_slice(&2u32.to_le_bytes());
    resign_header(&mut bytes);
    resign(&mut bytes);
    assert_eq!(
        SaveCodec::decode_header(&bytes[..header_len], len)
            .unwrap()
            .placed_count,
        2
    );
    assert_eq!(
        SaveCodec::decode(&bytes),
        Err(SaveError::CorruptSave("Placed count cache mismatch"))
    );
    bytes[placed_offset..placed_offset + 4].copy_from_slice(&5u32.to_le_bytes());
    resign_header(&mut bytes);
    assert!(SaveCodec::decode_header(&bytes, len).is_err());
    assert!(SaveCodec::decode_header(&original, len - 1).is_err());
    assert!(SaveCodec::decode_header(&original, u64::MAX).is_err());
}
#[test]
fn normal_updates_never_read_image_or_previous_piece_state() {
    let storage = MemoryStorage::default();
    let repo = SaveRepository::new(storage.clone());
    let bytes = encoded_image(image::ImageFormat::Png);
    let saved = repo
        .create(
            SaveTitle::new("save").unwrap(),
            checkpoint(&bytes),
            Some(&bytes),
        )
        .unwrap();
    storage.reads.lock().unwrap().clear();
    let key = StorageKey::Image(image_hash(&bytes));
    // External corruption after import is detected on load, not each Save.
    storage.blobs.lock().unwrap().get_mut(&key).unwrap()[50] ^= 1;
    repo.update(
        saved.id,
        saved.revision,
        saved.title.clone(),
        checkpoint(&bytes),
        None,
    )
    .unwrap();
    assert!(storage.reads.lock().unwrap().is_empty());
    assert_eq!(
        *storage.ranges.lock().unwrap(),
        vec![(StorageKey::Save(saved.id), 0, MAX_SAVE_HEADER_BYTES)]
    );
    assert!(matches!(
        repo.load(saved.id),
        Err(SaveError::CorruptImage(_))
    ));
    assert!(matches!(
        repo.import_image(image_hash(&bytes), &bytes),
        Err(SaveError::CorruptImage(_))
    ));
    let before = storage.blobs.lock().unwrap()[&StorageKey::Save(saved.id)].clone();
    storage.delete(key).unwrap();
    assert!(matches!(
        repo.update(
            saved.id,
            saved.revision + 1,
            saved.title,
            checkpoint(&bytes),
            None
        ),
        Err(SaveError::MissingImage(_))
    ));
    assert_eq!(
        storage.blobs.lock().unwrap()[&StorageKey::Save(saved.id)],
        before
    );
}
#[test]
fn fifty_million_piece_previews_read_only_bounded_headers() {
    use std::cell::Cell;
    struct HeadersOnly {
        headers: HashMap<StorageKey, Vec<u8>>,
        length: u64,
        bytes_read: std::rc::Rc<Cell<usize>>,
    }
    impl SaveStorage for HeadersOnly {
        fn list(&self, _: StorageNamespace) -> Result<Vec<StorageKey>, StorageError> {
            Ok(self.headers.keys().copied().collect())
        }
        fn read(&self, _: StorageKey) -> Result<Vec<u8>, StorageError> {
            panic!("Listing fetched the entire save")
        }
        fn read_range(
            &self,
            key: StorageKey,
            offset: u64,
            length: usize,
        ) -> Result<Vec<u8>, StorageError> {
            assert_eq!(offset, 0);
            assert_eq!(length, MAX_SAVE_HEADER_BYTES);
            let bytes = self.headers[&key].clone();
            self.bytes_read.set(self.bytes_read.get() + bytes.len());
            Ok(bytes)
        }
        fn len(&self, _: StorageKey) -> Result<u64, StorageError> {
            Ok(self.length)
        }
        fn write(&self, _: StorageKey, _: Vec<u8>) -> Result<(), StorageError> {
            unreachable!()
        }
        fn delete(&self, _: StorageKey) -> Result<(), StorageError> {
            unreachable!()
        }
        fn exists(&self, _: StorageKey) -> Result<bool, StorageError> {
            Ok(true)
        }
    }
    let mut header = SaveCodec::encode(&save()).unwrap();
    let t = save().metadata.title.as_str().len();
    let header_len = 173 + t;
    let length = header_len as u64 + 16_000_000 + 32;
    header.truncate(header_len);
    header[14..22].copy_from_slice(&length.to_le_bytes());
    for (offset, value) in [
        (108 + t, 1000u32),
        (112 + t, 1000),
        (116 + t, 4096),
        (120 + t, 4096),
        (128 + t, 1_000_000),
        (132 + t, 1_000_000),
    ] {
        header[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut headers = HashMap::new();
    for id in 0u128..50 {
        let mut bytes = header.clone();
        bytes[22..38].copy_from_slice(&id.to_le_bytes());
        resign_header(&mut bytes);
        headers.insert(StorageKey::Save(SaveId(id)), bytes);
    }
    let bytes_read = std::rc::Rc::new(Cell::new(0));
    let storage = HeadersOnly {
        headers,
        length,
        bytes_read: bytes_read.clone(),
    };
    let repo = SaveRepository::new(storage);
    let list = repo.list().unwrap();
    assert_eq!(list.len(), 50);
    for entry in list {
        let summary = entry.summary.unwrap();
        assert_eq!(summary.piece_count, 1_000_000);
        assert_eq!(summary.placed_count, 1);
    }
    assert_eq!(bytes_read.get(), 50 * header_len);
    assert!(bytes_read.get() <= 50 * MAX_SAVE_HEADER_BYTES);
}
#[test]
fn filesystem_range_reads_are_bounded_and_handle_eof() {
    let dir = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(dir.path());
    let key = StorageKey::Save(SaveId(1));
    storage.write(key, b"0123456789".to_vec()).unwrap();
    assert_eq!(storage.len(key).unwrap(), 10);
    assert_eq!(storage.read_range(key, 3, 4).unwrap(), b"3456");
    assert_eq!(storage.read_range(key, 9, 4).unwrap(), b"9");
    assert!(storage.read_range(key, 50, 4).unwrap().is_empty());
    assert!(storage.read_range(key, 0, 0).unwrap().is_empty());
    assert_eq!(
        storage.read_range(key, u64::MAX, 1),
        Err(StorageError::InvalidRange)
    );
    assert!(matches!(
        storage.len(StorageKey::Save(SaveId(2))),
        Err(StorageError::NotFound(_))
    ));
}
#[test]
fn thread_affine_backend_stays_with_owner_through_repository_operations() {
    use std::{
        rc::Rc,
        time::{Duration, Instant},
    };
    struct LocalStorage {
        owner: Rc<std::thread::ThreadId>,
        memory: MemoryStorage,
    }
    impl LocalStorage {
        fn check(&self) {
            assert_eq!(*self.owner, std::thread::current().id());
        }
    }
    impl SaveStorage for LocalStorage {
        fn list(&self, ns: StorageNamespace) -> Result<Vec<StorageKey>, StorageError> {
            self.check();
            self.memory.list(ns)
        }
        fn read(&self, key: StorageKey) -> Result<Vec<u8>, StorageError> {
            self.check();
            self.memory.read(key)
        }
        fn read_range(
            &self,
            key: StorageKey,
            offset: u64,
            length: usize,
        ) -> Result<Vec<u8>, StorageError> {
            self.check();
            self.memory.read_range(key, offset, length)
        }
        fn len(&self, key: StorageKey) -> Result<u64, StorageError> {
            self.check();
            self.memory.len(key)
        }
        fn write(&self, key: StorageKey, bytes: Vec<u8>) -> Result<(), StorageError> {
            self.check();
            self.memory.write(key, bytes)
        }
        fn delete(&self, key: StorageKey) -> Result<(), StorageError> {
            self.check();
            self.memory.delete(key)
        }
        fn exists(&self, key: StorageKey) -> Result<bool, StorageError> {
            self.check();
            self.memory.exists(key)
        }
    }
    let storage = LocalStorage {
        owner: Rc::new(std::thread::current().id()),
        memory: MemoryStorage::default(),
    };
    let (proxy, inbox) = executor::storage_channel();
    let worker = std::thread::spawn(move || {
        let repo = SaveRepository::new(proxy);
        let bytes = encoded_image(image::ImageFormat::Png);
        let saved = repo
            .create(
                SaveTitle::new("local owner").unwrap(),
                checkpoint(&bytes),
                Some(&bytes),
            )
            .unwrap();
        assert_eq!(
            repo.list().unwrap()[0]
                .summary
                .as_ref()
                .unwrap()
                .placed_count,
            1
        );
        assert_eq!(repo.load(saved.id).unwrap().image_bytes, bytes);
        repo.update(
            saved.id,
            saved.revision,
            saved.title,
            checkpoint(&bytes),
            None,
        )
        .unwrap();
        repo.delete(saved.id).unwrap();
        assert!(repo.list().unwrap().is_empty());
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !worker.is_finished() {
        assert!(Instant::now() < deadline, "Storage worker did not complete");
        while let Ok(request) = inbox.try_recv() {
            request.execute(&storage).unwrap();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    worker.join().unwrap();
}

#[test]
fn stale_revision_cannot_overwrite_a_newer_save_or_import_an_image() {
    let storage = MemoryStorage::default();
    let repo = SaveRepository::new(storage.clone());
    let mut initial = save();
    initial.metadata.revision = 10;
    let bytes = encoded_image(image::ImageFormat::Png);
    repo.import_image(initial.checkpoint.image_hash, &bytes)
        .unwrap();
    storage
        .write(
            StorageKey::Save(initial.metadata.id),
            SaveCodec::encode(&initial).unwrap(),
        )
        .unwrap();
    let pc_a = repo.load(initial.metadata.id).unwrap().save;
    let pc_b = repo
        .update(
            initial.metadata.id,
            10,
            SaveTitle::new("PC B").unwrap(),
            initial.checkpoint.clone(),
            None,
        )
        .unwrap();
    assert_eq!(pc_b.revision, 11);
    let key = StorageKey::Save(pc_b.id);
    let before = storage.blobs.lock().unwrap()[&key].clone();
    storage.writes.lock().unwrap().clear();
    storage.reads.lock().unwrap().clear();
    // Conflict must take precedence even over an invalid image import.
    let result = repo.update(
        pc_a.metadata.id,
        pc_a.metadata.revision,
        SaveTitle::new("Stale PC A").unwrap(),
        pc_a.checkpoint,
        Some(b"unrelated image"),
    );
    assert_eq!(
        result,
        Err(SaveError::Conflict {
            id: pc_b.id,
            expected_revision: 10,
            actual_revision: 11,
        })
    );
    assert!(storage.writes.lock().unwrap().is_empty());
    assert!(storage.reads.lock().unwrap().is_empty());
    assert_eq!(storage.blobs.lock().unwrap()[&key], before);
    let loaded = repo.load(pc_b.id).unwrap().save;
    assert_eq!(loaded.metadata, pc_b);
    let next = repo
        .update(
            loaded.metadata.id,
            loaded.metadata.revision,
            SaveTitle::new("Reloaded PC A").unwrap(),
            loaded.checkpoint,
            None,
        )
        .unwrap();
    assert_eq!(next.revision, 12);
    assert_eq!(next.created_at, initial.metadata.created_at);
    assert_eq!(repo.load(next.id).unwrap().save.metadata, next);
}
#[test]
fn exhausted_revision_does_not_publish_or_wrap() {
    let storage = MemoryStorage::default();
    let repo = SaveRepository::new(storage.clone());
    let mut initial = save();
    initial.metadata.revision = u64::MAX;
    let key = StorageKey::Save(initial.metadata.id);
    let encoded = SaveCodec::encode(&initial).unwrap();
    storage.write(key, encoded.clone()).unwrap();
    storage.writes.lock().unwrap().clear();
    assert_eq!(
        repo.update(
            initial.metadata.id,
            u64::MAX,
            initial.metadata.title,
            initial.checkpoint,
            None
        ),
        Err(SaveError::CounterExhausted)
    );
    assert!(storage.writes.lock().unwrap().is_empty());
    assert_eq!(storage.blobs.lock().unwrap()[&key], encoded);
}
#[test]
fn save_format_two_is_the_only_supported_layout() {
    let original = SaveCodec::encode(&save()).unwrap();
    assert_eq!(SAVE_FORMAT_VERSION, 2);
    assert_eq!(&original[8..10], &2u16.to_le_bytes());
    assert_eq!(SaveCodec::decode(&original).unwrap(), save());
    for version in [0u16, 1, 9] {
        let mut bytes = original.clone();
        bytes[8..10].copy_from_slice(&version.to_le_bytes());
        assert_eq!(
            SaveCodec::decode(&bytes),
            Err(SaveError::UnsupportedSaveFormat(version))
        );
        assert!(
            matches!(SaveCodec::decode_header(&bytes, bytes.len() as u64),
            Err(SaveError::UnsupportedSaveFormat(v)) if v == version)
        );
    }
    // The abandoned layout had metadata immediately after version. There is
    // no fallback parser even when its version field is also 1.
    let mut abandoned = b"PUZSAVE\0".to_vec();
    abandoned.extend_from_slice(&1u16.to_le_bytes());
    abandoned.extend_from_slice(&save().metadata.id.0.to_le_bytes());
    abandoned.resize(220, 0);
    resign(&mut abandoned);
    assert!(SaveCodec::decode(&abandoned).is_err());
    assert!(SaveCodec::decode_header(&abandoned, abandoned.len() as u64).is_err());
}
#[test]
fn proxy_transfers_the_same_blob_allocation_to_owner_backend() {
    use std::time::{Duration, Instant};
    struct AllocationStorage {
        pointer: usize,
        length: usize,
    }
    impl SaveStorage for AllocationStorage {
        fn list(&self, _: StorageNamespace) -> Result<Vec<StorageKey>, StorageError> {
            unreachable!()
        }
        fn read(&self, _: StorageKey) -> Result<Vec<u8>, StorageError> {
            unreachable!()
        }
        fn read_range(&self, _: StorageKey, _: u64, _: usize) -> Result<Vec<u8>, StorageError> {
            unreachable!()
        }
        fn len(&self, _: StorageKey) -> Result<u64, StorageError> {
            unreachable!()
        }
        fn write(&self, _: StorageKey, bytes: Vec<u8>) -> Result<(), StorageError> {
            assert_eq!(bytes.as_ptr() as usize, self.pointer);
            assert_eq!(bytes.len(), self.length);
            assert!(bytes.iter().all(|byte| *byte == 0x5a));
            Ok(())
        }
        fn delete(&self, _: StorageKey) -> Result<(), StorageError> {
            unreachable!()
        }
        fn exists(&self, _: StorageKey) -> Result<bool, StorageError> {
            unreachable!()
        }
    }
    let bytes = vec![0x5a; 1024 * 1024];
    let storage = AllocationStorage {
        pointer: bytes.as_ptr() as usize,
        length: bytes.len(),
    };
    let (proxy, inbox) = executor::storage_channel();
    let worker =
        std::thread::spawn(move || proxy.write(StorageKey::Image(ImageHash([1; 32])), bytes));
    let deadline = Instant::now() + Duration::from_secs(10);
    let request = loop {
        if let Ok(request) = inbox.try_recv() {
            break request;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    };
    let executor::StorageOperation::Write(_, bytes) = &request.operation else {
        panic!("Expected write request");
    };
    assert_eq!(bytes.as_ptr() as usize, storage.pointer);
    request.execute(&storage).unwrap();
    worker.join().unwrap().unwrap();
}

#[test]
fn rotated_save_codec_keeps_sixteen_byte_records_and_restores_components() {
    use puzzella_core::{
        protocol::{ComponentRef, PieceTarget},
        PieceCommand, LOCAL_PLAYER,
    };
    let mut save = save();
    let mut store = PieceDataStore::default();
    save.checkpoint.install(&mut store).unwrap();
    let reference = ComponentRef::from_member(&store.connectivity, PieceId(1)).unwrap();
    assert_eq!(
        store
            .apply_command(
                LOCAL_PLAYER,
                &PieceCommand::Rotate {
                    target: PieceTarget::Component(reference),
                    quarter_turns: 1
                },
                Some(&save.checkpoint.definition),
                puzzella_core::LOCAL_PLAYER
            )
            .rotated,
        2
    );
    save.checkpoint = PuzzleCheckpoint::capture(
        &store,
        &save.checkpoint.definition,
        save.checkpoint.image_hash,
    )
    .unwrap();
    let bytes = SaveCodec::encode(&save).unwrap();
    let header = u32::from_le_bytes(bytes[10..14].try_into().unwrap()) as usize;
    assert_eq!(bytes.len() - header - 32, 16 * 4);
    let loaded = SaveCodec::decode(&bytes).unwrap();
    assert_eq!(loaded, save);
    let mut restored = PieceDataStore::default();
    loaded.checkpoint.install(&mut restored).unwrap();
    assert_eq!(&*restored.states, &*store.states);
    assert!(restored.connectivity.same_component(PieceId(1), PieceId(3)));
}
