use super::*;
use std::num::NonZeroU32;

fn limit(count: u32) -> NonZeroU32 {
    NonZeroU32::new(count).unwrap()
}

#[test]
fn rotation_keeps_newest_checkpoints_and_reduces_the_limit_across_sessions() {
    let storage = MemoryStorage::default();
    let repo = SaveRepository::new(storage.clone());
    let bytes = encoded_image(image::ImageFormat::Png);
    let game_id = GameId(0x5678);
    let title = SaveTitle::new("Puzzle").unwrap();
    let manual = repo
        .create_for_game(game_id, title.clone(), checkpoint(&bytes), Some(&bytes))
        .unwrap();
    let original_manual = repo.read_save(manual.id).unwrap();
    let mut previous = None;
    let mut snapshots = Vec::new();
    for index in 0..6 {
        let mut state = checkpoint(&bytes);
        state.pieces[2].position.x += index as f32;
        let outcome = repo
            .autosave(
                game_id,
                previous,
                title.clone(),
                state.clone(),
                None,
                limit(3),
            )
            .unwrap();
        assert!(outcome.rotation_error.is_none());
        assert_eq!(outcome.metadata.revision, index + 1);
        previous = Some((outcome.metadata.id, outcome.metadata.revision));
        snapshots.push((outcome.metadata.id, state));
        let entries = repo.list().unwrap();
        assert_eq!(entries.len(), 1 + (index as usize + 1).min(3));
        for (id, state) in snapshots.iter().rev().take(3) {
            assert_eq!(&repo.read_save(*id).unwrap().checkpoint, state);
        }
    }
    // A new session or a manual-load resume has no current autosave pointer.
    let resumed = repo
        .autosave(
            game_id,
            None,
            SaveTitle::new("Renamed puzzle").unwrap(),
            checkpoint(&bytes),
            None,
            limit(1),
        )
        .unwrap();
    assert!(resumed.rotation_error.is_none());
    let entries = repo.list().unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|entry| entry.id == resumed.metadata.id));
    assert_eq!(repo.read_save(manual.id).unwrap(), original_manual);
    assert_eq!(repo.read_image(image_hash(&bytes)).unwrap(), bytes);
}

#[test]
fn separate_game_ids_keep_identical_puzzles_independent_and_bad_headers_untouched() {
    let storage = MemoryStorage::default();
    let repo = SaveRepository::new(storage.clone());
    let bytes = encoded_image(image::ImageFormat::Png);
    let base = checkpoint(&bytes);
    repo.import_image(base.image_hash, &bytes).unwrap();
    let game_id = GameId(0x5678);
    let title = SaveTitle::new("Same title").unwrap();
    let others: Vec<_> = [GameId(1), GameId(2)]
        .into_iter()
        .map(|other_id| {
            repo.autosave(other_id, None, title.clone(), base.clone(), None, limit(1))
                .unwrap()
                .metadata
        })
        .collect();
    let corrupt = StorageKey::Save(SaveId(123));
    storage.write(corrupt, b"broken header".to_vec()).unwrap();
    for _ in 0..3 {
        repo.autosave(game_id, None, title.clone(), base.clone(), None, limit(1))
            .unwrap();
    }
    assert_eq!(repo.list().unwrap().len(), others.len() + 2);
    for metadata in others {
        assert_eq!(repo.read_save(metadata.id).unwrap().metadata, metadata);
    }
    assert_eq!(storage.read(corrupt).unwrap(), b"broken header");
}

#[test]
fn mismatched_game_id_is_rejected_before_publishing_or_rotating() {
    let storage = MemoryStorage::default();
    let repo = SaveRepository::new(storage.clone());
    let bytes = encoded_image(image::ImageFormat::Png);
    let title = SaveTitle::new("Puzzle").unwrap();
    let first = repo
        .autosave(
            GameId(1),
            None,
            title.clone(),
            checkpoint(&bytes),
            Some(&bytes),
            limit(1),
        )
        .unwrap()
        .metadata;
    storage.writes.lock().unwrap().clear();
    assert!(matches!(
        repo.autosave(
            GameId(2),
            Some((first.id, first.revision)),
            title,
            checkpoint(&bytes),
            None,
            limit(1)
        ),
        Err(SaveError::CorruptSave("Autosave game identity changed"))
    ));
    assert!(storage.writes.lock().unwrap().is_empty());
    assert_eq!(repo.list().unwrap().len(), 1);
}

#[test]
fn same_second_and_clock_rollback_keep_the_latest_history_without_full_save_reads() {
    let storage = MemoryStorage::default();
    let repo = SaveRepository::new(storage.clone());
    let bytes = encoded_image(image::ImageFormat::Png);
    repo.import_image(image_hash(&bytes), &bytes).unwrap();
    let game_id = GameId(0x5678);
    for (revision, id) in [(1, 30), (2, 20), (3, 10)] {
        let mut legacy = save();
        legacy.metadata.id = SaveId(id);
        legacy.metadata.revision = revision;
        legacy.metadata.created_at = 4_000_000_000;
        legacy.metadata.updated_at = 4_000_000_000;
        legacy.metadata.is_autosave = true;
        storage
            .write(
                StorageKey::Save(legacy.metadata.id),
                SaveCodec::encode(&legacy).unwrap(),
            )
            .unwrap();
    }
    for revision in [4, 5] {
        let outcome = repo
            .autosave(
                game_id,
                None,
                SaveTitle::new("Puzzle").unwrap(),
                checkpoint(&bytes),
                None,
                limit(2),
            )
            .unwrap();
        assert_eq!(outcome.metadata.revision, revision);
        assert_eq!(outcome.metadata.updated_at, 4_000_000_000);
    }
    let entries = repo.list().unwrap();
    assert_eq!(entries[0].summary.as_ref().unwrap().metadata.revision, 5);
    let mut revisions: Vec<_> = entries
        .iter()
        .map(|entry| entry.summary.as_ref().unwrap().metadata.revision)
        .collect();
    revisions.sort();
    assert_eq!(revisions, vec![4, 5]);
    assert!(storage.reads.lock().unwrap().is_empty());
    assert!(storage
        .ranges
        .lock()
        .unwrap()
        .iter()
        .all(|(_, offset, len)| *offset == 0 && *len == MAX_SAVE_HEADER_BYTES));
}

#[test]
fn publish_failure_preserves_history_and_cleanup_failure_keeps_the_new_save_for_retry() {
    let storage = MemoryStorage::default();
    let repo = SaveRepository::new(storage.clone());
    let bytes = encoded_image(image::ImageFormat::Png);
    let game_id = GameId(0x5678);
    let title = SaveTitle::new("Puzzle").unwrap();
    let first = repo
        .autosave(
            game_id,
            None,
            title.clone(),
            checkpoint(&bytes),
            Some(&bytes),
            limit(1),
        )
        .unwrap()
        .metadata;
    let original = repo.read_save(first.id).unwrap();
    *storage.fail_save_write.lock().unwrap() = true;
    assert!(repo
        .autosave(
            game_id,
            Some((first.id, first.revision)),
            title.clone(),
            checkpoint(&bytes),
            None,
            limit(1)
        )
        .is_err());
    assert_eq!(repo.read_save(first.id).unwrap(), original);
    assert_eq!(repo.list().unwrap().len(), 1);

    *storage.fail_save_write.lock().unwrap() = false;
    *storage.fail_delete.lock().unwrap() = Some(StorageKey::Save(first.id));
    let second = repo
        .autosave(
            game_id,
            Some((first.id, first.revision)),
            title.clone(),
            checkpoint(&bytes),
            None,
            limit(1),
        )
        .unwrap();
    assert!(matches!(
        second.rotation_error,
        Some(SaveError::Storage(StorageError::Io(_)))
    ));
    assert!(
        repo.read_save(second.metadata.id)
            .unwrap()
            .metadata
            .is_autosave
    );
    assert_eq!(repo.read_save(first.id).unwrap(), original);
    assert_eq!(repo.list().unwrap().len(), 2);

    *storage.fail_delete.lock().unwrap() = None;
    let third = repo
        .autosave(
            game_id,
            Some((second.metadata.id, second.metadata.revision)),
            title,
            checkpoint(&bytes),
            None,
            limit(1),
        )
        .unwrap();
    assert!(third.rotation_error.is_none());
    assert_eq!(repo.list().unwrap()[0].id, third.metadata.id);
    assert_eq!(repo.list().unwrap().len(), 1);
}
