//! Backend-neutral binary formats, logical storage and repository operations.
mod codec;
pub mod executor;
mod repository;
pub mod runtime;
mod storage;
use crate::checkpoint::{CheckpointError, PuzzleCheckpoint};
pub use codec::{
    image_hash, PuzImage, SaveCodec, SaveHeader, MAX_SAVE_HEADER_BYTES, PUZIMG_FORMAT_VERSION,
    SAVE_FORMAT_VERSION,
};
use puzzella_core::session::ImageHash;
pub use repository::{LoadedSave, SaveListEntry, SaveRepository, SaveSummary};
pub use storage::{FilesystemStorage, SaveStorage, StorageError, StorageKey, StorageNamespace};

pub const MAX_SAVE_TITLE_CHARS: usize = 80;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SaveId(pub u128);
impl SaveId {
    pub fn hex(self) -> String {
        format!("{:032x}", self.0)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveTitle(String);
impl SaveTitle {
    pub fn new(value: &str) -> Result<Self, SaveError> {
        if value.chars().any(char::is_control) || value.contains(['\u{2028}', '\u{2029}']) {
            return Err(SaveError::InvalidTitle(SaveTitleError::ControlCharacters));
        }
        let value = value.trim();
        if value.is_empty() {
            return Err(SaveError::InvalidTitle(SaveTitleError::Empty));
        }
        if value.chars().count() > MAX_SAVE_TITLE_CHARS {
            return Err(SaveError::InvalidTitle(SaveTitleError::TooLong));
        }
        Ok(Self(value.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveMetadata {
    pub id: SaveId,
    pub title: SaveTitle,
    pub revision: u64,
    /// Unix seconds (UTC), independent of the storage backend.
    pub created_at: u64,
    pub updated_at: u64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct PuzzleSave {
    pub metadata: SaveMetadata,
    pub checkpoint: PuzzleCheckpoint,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveTitleError {
    ControlCharacters,
    Empty,
    TooLong,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveError {
    InvalidTitle(SaveTitleError),
    UnsupportedSaveFormat(u16),
    UnsupportedImageFormat(u16),
    UnsupportedGenerator(u16),
    CorruptSave(&'static str),
    CorruptImage(&'static str),
    MissingImage(ImageHash),
    Conflict {
        id: SaveId,
        expected_revision: u64,
        actual_revision: u64,
    },
    Checkpoint(CheckpointError),
    Storage(StorageError),
    Decode(String),
    CounterExhausted,
    IdCollision,
}
impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTitle(s) => write!(f, "Invalid save title: {s:?}"),
            Self::UnsupportedSaveFormat(v) => write!(f, "Unsupported save format: {v}"),
            Self::UnsupportedImageFormat(v) => write!(f, "Unsupported image container format: {v}"),
            Self::UnsupportedGenerator(v) => write!(f, "Unsupported puzzle generator version: {v}"),
            Self::CorruptSave(s) => write!(f, "Corrupt save: {s}"),
            Self::CorruptImage(s) => write!(f, "Corrupt image: {s}"),
            Self::MissingImage(_) => write!(f, "Missing referenced image"),
            Self::Conflict { expected_revision, actual_revision, .. } => write!(f,
                "This save changed elsewhere (expected revision {expected_revision}, found {actual_revision}). Reload it before saving again."),
            Self::Checkpoint(e) => write!(f, "Corrupt save: {e}"),
            Self::Storage(e) => e.fmt(f),
            Self::Decode(e) => write!(f, "Image decode failed: {e}"),
            Self::CounterExhausted => write!(f, "Save revision counter exhausted"),
            Self::IdCollision => write!(f, "Could not allocate a unique save ID"),
        }
    }
}
impl std::error::Error for SaveError {}
impl From<StorageError> for SaveError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
}
impl From<CheckpointError> for SaveError {
    fn from(e: CheckpointError) -> Self {
        Self::Checkpoint(e)
    }
}

#[cfg(test)]
mod tests;
