use super::*;
use std::collections::HashSet;

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
}
impl<S: SaveStorage> SaveRepository<S> {
    pub fn new(storage: S) -> Self {
        Self { storage }
    }
    pub fn import_image(&self, hash: ImageHash, bytes: &[u8]) -> Result<(), SaveError> {
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
        mut id_source: impl FnMut() -> u128,
    ) -> Result<SaveMetadata, SaveError> {
        for _ in 0..32 {
            let id = SaveId(id_source());
            if self.storage.exists(StorageKey::Save(id))? {
                continue;
            }
            let now = timestamp();
            let metadata = SaveMetadata {
                id,
                title,
                revision: 1,
                created_at: now,
                updated_at: now,
                is_autosave,
            };
            return self.publish(
                PuzzleSave {
                    metadata,
                    checkpoint,
                },
                original_bytes,
            );
        }
        Err(SaveError::IdCollision)
    }
    /// Reject a stale session before encoding or publishing any data.
    /// Concurrent writers still need backend-specific atomic conflict handling.
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
            self.import_image(hash, bytes)?;
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
        let save = self.read_save(id)?;
        let image_bytes = self.read_image(save.checkpoint.image_hash)?;
        Ok(LoadedSave { save, image_bytes })
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
            let time = |e: &SaveListEntry| {
                e.summary
                    .as_ref()
                    .map(|s| s.metadata.updated_at)
                    .unwrap_or(0)
            };
            time(b).cmp(&time(a)).then_with(|| a.id.cmp(&b.id))
        });
        Ok(entries)
    }
    pub fn delete(&self, id: SaveId) -> Result<(), SaveError> {
        self.storage.delete(StorageKey::Save(id))?;
        // Cleanup must not turn an already successful save deletion into a
        // failure. Leave any images we cannot safely remove for the next delete.
        if let Err(error) = self.cleanup_unreferenced_images() {
            bevy::log::warn!("Could not clean up unreferenced puzzle images: {error}");
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
                    if let Err(error) = self.storage.delete(key) {
                        bevy::log::warn!(
                            "Could not delete unreferenced puzzle image {}: {error}",
                            key.filename()
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
