use super::*;
use std::{collections::HashSet, num::NonZeroU32};

pub struct SaveRepository<S: SaveStorage> {
    storage: S,
}
#[derive(Clone, Debug)]
pub struct SaveSummary {
    pub metadata: SaveMetadata,
    pub image_hash: ImageHash,
    pub piece_count: usize,
    pub placed_count: usize,
}
#[derive(Clone, Debug)]
pub struct SaveListEntry {
    pub id: SaveId,
    pub summary: Result<SaveSummary, SaveError>,
}
pub struct LoadedSave {
    pub save: PuzzleSave,
    pub image_bytes: Vec<u8>,
    pub image_lease: ImageLease,
}
/// A live image reference, including while no save refers to it yet.
#[derive(Clone, Debug)]
#[must_use = "keep the lease alive while the image is in use"]
pub struct ImageLease {
    pub hash: ImageHash,
    _guard: StorageGuard,
}
pub(crate) struct SaveOutcome {
    pub metadata: SaveMetadata,
    /// Publishing succeeded even if removing an older checkpoint failed.
    pub rotation_error: Option<SaveError>,
}
impl<S: SaveStorage> SaveRepository<S> {
    pub fn new(storage: S) -> Self {
        Self { storage }
    }
    pub fn import_image(&self, hash: ImageHash, bytes: &[u8]) -> Result<(), SaveError> {
        let _transaction = self.lock_repository()?;
        self.import_image_unlocked(hash, bytes)
    }
    pub fn import_image_retained(
        &self,
        hash: ImageHash,
        bytes: &[u8],
    ) -> Result<ImageLease, SaveError> {
        let _transaction = self.lock_repository()?;
        let lease = self.retain_image(hash)?;
        self.import_image_unlocked(hash, bytes)?;
        Ok(lease)
    }
    pub(crate) fn retain_image(&self, hash: ImageHash) -> Result<ImageLease, SaveError> {
        Ok(ImageLease {
            hash,
            _guard: wait_for_lock(|| self.storage.try_retain_image(hash))?,
        })
    }
    fn lock_repository(&self) -> Result<StorageGuard, SaveError> {
        wait_for_lock(|| self.storage.try_lock_repository())
    }
    /// Caller holds the repository lock through image and save publication.
    fn import_image_unlocked(&self, hash: ImageHash, bytes: &[u8]) -> Result<(), SaveError> {
        if image_hash(bytes) != hash {
            return Err(SaveError::CorruptImage(
                "Original bytes do not match image identity",
            ));
        }
        let key = StorageKey::Image(hash);
        if self.storage.exists(key)? {
            // A corrupt existing blob must never be used to publish a save.
            let existing = self.storage.read(key)?;
            PuzImage::decode(&existing, hash)?;
        } else {
            self.storage.write(key, PuzImage::encode(bytes)?)?;
        }
        Ok(())
    }
    pub fn read_image(&self, hash: ImageHash) -> Result<Vec<u8>, SaveError> {
        let mut bytes = self
            .storage
            .read(StorageKey::Image(hash))
            .map_err(|e| match e {
                StorageError::NotFound(_) => SaveError::MissingImage(hash),
                e => SaveError::Storage(e),
            })?;
        let length = PuzImage::decode(&bytes, hash)?.len();
        // Reuse the storage allocation instead of cloning a potentially large image.
        let start = bytes.len() - length;
        bytes.copy_within(start.., 0);
        bytes.truncate(length);
        Ok(bytes)
    }
    pub fn create(
        &self,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        original_bytes: Option<&[u8]>,
    ) -> Result<SaveMetadata, SaveError> {
        self.create_as(title, checkpoint, original_bytes, false)
    }
    pub fn create_for_game(
        &self,
        game_id: GameId,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        original_bytes: Option<&[u8]>,
    ) -> Result<SaveMetadata, SaveError> {
        let _transaction = self.lock_repository()?;
        let now = timestamp();
        self.create_with_metadata(
            PuzzleSave {
                metadata: SaveMetadata {
                    id: SaveId(0),
                    game_id,
                    title,
                    revision: 1,
                    created_at: now,
                    updated_at: now,
                    is_autosave: false,
                },
                checkpoint,
            },
            original_bytes,
            rand::random,
        )
    }
    pub fn create_as(
        &self,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        original_bytes: Option<&[u8]>,
        is_autosave: bool,
    ) -> Result<SaveMetadata, SaveError> {
        self.create_as_with_ids(title, checkpoint, original_bytes, is_autosave, rand::random)
    }
    #[cfg(test)]
    pub(crate) fn create_with_ids(
        &self,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        original_bytes: Option<&[u8]>,
        id_source: impl FnMut() -> u128,
    ) -> Result<SaveMetadata, SaveError> {
        self.create_as_with_ids(title, checkpoint, original_bytes, false, id_source)
    }
    fn create_as_with_ids(
        &self,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        original_bytes: Option<&[u8]>,
        is_autosave: bool,
        id_source: impl FnMut() -> u128,
    ) -> Result<SaveMetadata, SaveError> {
        let _transaction = self.lock_repository()?;
        let now = timestamp();
        self.create_with_metadata(
            PuzzleSave {
                metadata: SaveMetadata {
                    id: SaveId(0),
                    game_id: GameId::default(),
                    title,
                    revision: 1,
                    created_at: now,
                    updated_at: now,
                    is_autosave,
                },
                checkpoint,
            },
            original_bytes,
            id_source,
        )
    }
    fn create_with_metadata(
        &self,
        mut save: PuzzleSave,
        original_bytes: Option<&[u8]>,
        mut id_source: impl FnMut() -> u128,
    ) -> Result<SaveMetadata, SaveError> {
        for _ in 0..32 {
            let id = SaveId(id_source());
            if self.storage.exists(StorageKey::Save(id))? {
                continue;
            }
            save.metadata.id = id;
            return self.publish(save, original_bytes);
        }
        Err(SaveError::IdCollision)
    }
    /// Rotate only checkpoints belonging to the same persistent game ID.
    pub(crate) fn autosave(
        &self,
        game_id: GameId,
        previous: Option<(SaveId, u64)>,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        original_bytes: Option<&[u8]>,
        max_saves_per_game: NonZeroU32,
    ) -> Result<SaveOutcome, SaveError> {
        let _transaction = self.lock_repository()?;
        if let Some((id, expected_revision)) = previous {
            let header = self.read_header(id)?;
            if header.metadata.revision != expected_revision {
                return Err(SaveError::Conflict {
                    id,
                    expected_revision,
                    actual_revision: header.metadata.revision,
                });
            }
            if !header.metadata.is_autosave || header.metadata.game_id != game_id {
                return Err(SaveError::CorruptSave("Autosave game identity changed"));
            }
        }
        let mut history = Vec::new();
        for key in self.storage.list(StorageNamespace::Saves)? {
            let StorageKey::Save(id) = key else {
                continue;
            };
            let header = match self.read_header(id) {
                Ok(header) => header,
                // Leave damaged or unsupported saves untouched; their identity is unknown.
                Err(SaveError::Storage(error)) => return Err(error.into()),
                Err(_) => continue,
            };
            if header.metadata.is_autosave && header.metadata.game_id == game_id {
                history.push(header.metadata);
            }
        }
        history.sort_by_key(|metadata| {
            (
                metadata.updated_at,
                metadata.revision,
                metadata.created_at,
                metadata.id,
            )
        });
        // A sequence across this game's checkpoints orders saves made in the same
        // second. Clamp time like update() so a clock rollback cannot reorder them.
        let revision = history
            .iter()
            .map(|metadata| metadata.revision)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(SaveError::CounterExhausted)?;
        let now = timestamp().max(history.last().map_or(0, |metadata| metadata.updated_at));
        let metadata = self.create_with_metadata(
            PuzzleSave {
                metadata: SaveMetadata {
                    id: SaveId(0),
                    game_id,
                    title,
                    revision,
                    created_at: now,
                    updated_at: now,
                    is_autosave: true,
                },
                checkpoint,
            },
            original_bytes,
            rand::random,
        )?;
        // Never delete history before publishing the replacement. Keep the freshly
        // published save even when cleanup fails, and retry cleanup next interval.
        let mut rotation_error = None;
        let excess = history
            .len()
            .saturating_sub(max_saves_per_game.get() as usize - 1);
        for old in history.into_iter().take(excess) {
            if let Err(error) = self.delete_unlocked(old.id) {
                rotation_error.get_or_insert(error);
            }
        }
        Ok(SaveOutcome {
            metadata,
            rotation_error,
        })
    }
    /// Reject a stale session before encoding or publishing any data.
    /// The repository lock keeps the revision check and publication atomic
    /// with respect to cooperating writers and image collection.
    pub fn update(
        &self,
        id: SaveId,
        expected_revision: u64,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        original_bytes: Option<&[u8]>,
    ) -> Result<SaveMetadata, SaveError> {
        self.update_as(
            id,
            expected_revision,
            title,
            checkpoint,
            original_bytes,
            false,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn update_as(
        &self,
        id: SaveId,
        expected_revision: u64,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        original_bytes: Option<&[u8]>,
        is_autosave: bool,
    ) -> Result<SaveMetadata, SaveError> {
        let _transaction = self.lock_repository()?;
        let previous = self.read_header(id)?;
        if previous.metadata.revision != expected_revision {
            return Err(SaveError::Conflict {
                id,
                expected_revision,
                actual_revision: previous.metadata.revision,
            });
        }
        let metadata = SaveMetadata {
            id,
            game_id: previous.metadata.game_id,
            title,
            revision: previous
                .metadata
                .revision
                .checked_add(1)
                .ok_or(SaveError::CounterExhausted)?,
            created_at: previous.metadata.created_at,
            updated_at: timestamp().max(previous.metadata.updated_at),
            is_autosave,
        };
        self.publish(
            PuzzleSave {
                metadata,
                checkpoint,
            },
            original_bytes,
        )
    }
    fn publish(
        &self,
        save: PuzzleSave,
        original_bytes: Option<&[u8]>,
    ) -> Result<SaveMetadata, SaveError> {
        let encoded = SaveCodec::encode(&save)?;
        let hash = save.checkpoint.image_hash;
        if let Some(bytes) = original_bytes {
            self.import_image_unlocked(hash, bytes)?;
        } else if !self.storage.exists(StorageKey::Image(hash))? {
            return Err(SaveError::MissingImage(hash));
        }
        // Publish image first; any failure leaves at most a harmless orphan image.
        self.storage
            .write(StorageKey::Save(save.metadata.id), encoded)?;
        Ok(save.metadata)
    }
    pub fn read_save(&self, id: SaveId) -> Result<PuzzleSave, SaveError> {
        let save = SaveCodec::decode(&self.storage.read(StorageKey::Save(id))?)?;
        if save.metadata.id != id {
            return Err(SaveError::CorruptSave("Save ID does not match storage key"));
        }
        Ok(save)
    }
    pub fn read_header(&self, id: SaveId) -> Result<SaveHeader, SaveError> {
        let key = StorageKey::Save(id);
        let prefix = self.storage.read_range(key, 0, MAX_SAVE_HEADER_BYTES)?;
        let header = SaveCodec::decode_header(&prefix, self.storage.len(key)?)?;
        if header.metadata.id != id {
            return Err(SaveError::CorruptSave("Save ID does not match storage key"));
        }
        Ok(header)
    }
    pub fn load(&self, id: SaveId) -> Result<LoadedSave, SaveError> {
        let transaction = self.lock_repository()?;
        let save = self.read_save(id)?;
        let image_lease = self.retain_image(save.checkpoint.image_hash)?;
        // The lease protects the image across decode and reply delivery. No
        // repository-wide lock is needed for the potentially large image read.
        drop(transaction);
        let image_bytes = self.read_image(save.checkpoint.image_hash)?;
        Ok(LoadedSave {
            save,
            image_bytes,
            image_lease,
        })
    }
    pub fn list(&self) -> Result<Vec<SaveListEntry>, SaveError> {
        let mut entries = Vec::new();
        for key in self.storage.list(StorageNamespace::Saves)? {
            let StorageKey::Save(id) = key else {
                continue;
            };
            let summary = self.read_header(id).and_then(|save| {
                if !self.storage.exists(StorageKey::Image(save.image_hash))? {
                    return Err(SaveError::MissingImage(save.image_hash));
                }
                Ok(SaveSummary {
                    image_hash: save.image_hash,
                    piece_count: save.piece_count,
                    placed_count: save.placed_count,
                    metadata: save.metadata,
                })
            });
            entries.push(SaveListEntry { id, summary });
        }
        entries.sort_by(|a, b| {
            let order = |e: &SaveListEntry| {
                e.summary
                    .as_ref()
                    .map(|s| {
                        (
                            s.metadata.updated_at,
                            s.metadata.revision,
                            s.metadata.created_at,
                        )
                    })
                    .unwrap_or_default()
            };
            order(b).cmp(&order(a)).then_with(|| a.id.cmp(&b.id))
        });
        Ok(entries)
    }
    pub fn delete(&self, id: SaveId) -> Result<(), SaveError> {
        let _transaction = self.lock_repository()?;
        self.delete_unlocked(id)
    }
    fn delete_unlocked(&self, id: SaveId) -> Result<(), SaveError> {
        self.storage.delete(StorageKey::Save(id))?;
        // Cleanup must not turn an already successful save deletion into a
        // failure. Leave any images we cannot safely remove for the next delete.
        if let Err(error) = self.cleanup_unreferenced_images() {
            bevy::log::warn!("Could not clean up unreferenced puzzle images");
            bevy::log::debug!(%error, "Unreferenced puzzle image cleanup failure details");
        }
        Ok(())
    }
    fn cleanup_unreferenced_images(&self) -> Result<(), SaveError> {
        let mut referenced = HashSet::new();
        for key in self.storage.list(StorageNamespace::Saves)? {
            if let StorageKey::Save(id) = key {
                // Validate every remaining header before deleting any images.
                // An unreadable save may still reference any of the blobs.
                referenced.insert(self.read_header(id)?.image_hash);
            }
        }
        for key in self.storage.list(StorageNamespace::Images)? {
            if let StorageKey::Image(hash) = key {
                if !referenced.contains(&hash) {
                    let result = self.storage.try_lock_image(hash).and_then(|guard| {
                        let Some(_guard) = guard else {
                            return Ok(());
                        };
                        self.storage.delete(key)
                    });
                    if let Err(error) = result {
                        bevy::log::warn!("Could not delete an unreferenced puzzle image");
                        bevy::log::debug!(
                            filename = %key.filename(),
                            %error,
                            "Unreferenced puzzle image deletion failure details"
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
fn wait_for_lock(
    mut acquire: impl FnMut() -> Result<Option<StorageGuard>, StorageError>,
) -> Result<StorageGuard, SaveError> {
    loop {
        if let Some(guard) = acquire()? {
            return Ok(guard);
        }
        // Only a repository worker waits. Storage executor operations stay
        // nonblocking so another holder can finish I/O and release its lock.
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
