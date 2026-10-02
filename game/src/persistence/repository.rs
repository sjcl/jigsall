use super::*;
use crate::checkpoint::SNAPSHOT_PLACED;

pub struct SaveRepository<S: SaveStorage> {
    storage: S,
}
#[derive(Clone, Debug)]
pub struct SaveSummary {
    pub metadata: SaveMetadata,
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
            self.storage.write(key, &PuzImage::encode(bytes)?)?;
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
        self.create_with_ids(title, checkpoint, original_bytes, rand::random)
    }
    pub(crate) fn create_with_ids(
        &self,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        original_bytes: Option<&[u8]>,
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
    pub fn update(
        &self,
        id: SaveId,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        original_bytes: Option<&[u8]>,
    ) -> Result<SaveMetadata, SaveError> {
        let previous = self.read_save(id)?;
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
        } else {
            let container = self
                .storage
                .read(StorageKey::Image(hash))
                .map_err(|e| match e {
                    StorageError::NotFound(_) => SaveError::MissingImage(hash),
                    e => SaveError::Storage(e),
                })?;
            PuzImage::decode(&container, hash)?;
        }
        // Publish image first; any failure leaves at most a harmless orphan image.
        self.storage
            .write(StorageKey::Save(save.metadata.id), &encoded)?;
        Ok(save.metadata)
    }
    pub fn read_save(&self, id: SaveId) -> Result<PuzzleSave, SaveError> {
        let save = SaveCodec::decode(&self.storage.read(StorageKey::Save(id))?)?;
        if save.metadata.id != id {
            return Err(SaveError::CorruptSave("Save ID does not match storage key"));
        }
        Ok(save)
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
            let summary = self.read_save(id).and_then(|save| {
                if !self
                    .storage
                    .exists(StorageKey::Image(save.checkpoint.image_hash))?
                {
                    return Err(SaveError::MissingImage(save.checkpoint.image_hash));
                }
                Ok(SaveSummary {
                    piece_count: save.checkpoint.pieces.len(),
                    placed_count: save
                        .checkpoint
                        .pieces
                        .iter()
                        .filter(|p| p.flags & SNAPSHOT_PLACED != 0)
                        .count(),
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
        Ok(self.storage.delete(StorageKey::Save(id))?)
    }
}
fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
