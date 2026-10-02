use super::{
    codec::{MAX_IMAGE_BYTES, MAX_SAVE_BYTES},
    ImageHash, SaveId,
};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StorageKey {
    Save(SaveId),
    Image(ImageHash),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageNamespace {
    Saves,
    Images,
}
impl StorageKey {
    pub fn filename(self) -> String {
        match self {
            Self::Save(id) => format!("{}.puzsave", id.hex()),
            Self::Image(hash) => format!(
                "{}.puzimg",
                hash.0
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ),
        }
    }
    fn namespace(self) -> StorageNamespace {
        match self {
            Self::Save(_) => StorageNamespace::Saves,
            Self::Image(_) => StorageNamespace::Images,
        }
    }
    fn max_bytes(self) -> u64 {
        match self {
            Self::Save(_) => MAX_SAVE_BYTES,
            Self::Image(_) => MAX_IMAGE_BYTES,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageError {
    NotFound(StorageKey),
    Unavailable(String),
    Io(String),
    TooLarge,
    InvalidRange,
}
impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(_) => write!(f, "Saved data was not found"),
            Self::Unavailable(e) | Self::Io(e) => write!(f, "Storage error: {e}"),
            Self::TooLarge => write!(f, "Saved data exceeds the supported size"),
            Self::InvalidRange => write!(f, "Invalid storage read range"),
        }
    }
}
impl std::error::Error for StorageError {}
fn io(e: std::io::Error) -> StorageError {
    StorageError::Io(e.to_string())
}

/// Paths/rename are backend internals. A successful write publishes the entire blob.
/// Implementations must preserve the previous blob if replacement fails.
/// Thread affinity belongs to the executor, not to the storage backend.
pub trait SaveStorage {
    fn list(&self, namespace: StorageNamespace) -> Result<Vec<StorageKey>, StorageError>;
    fn read(&self, key: StorageKey) -> Result<Vec<u8>, StorageError>;
    /// Read at most `length` bytes from `offset`; EOF returns a shorter buffer.
    /// Must not fetch the entire blob to implement a bounded read.
    fn read_range(
        &self,
        key: StorageKey,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, StorageError>;
    fn len(&self, key: StorageKey) -> Result<u64, StorageError>;
    fn write(&self, key: StorageKey, bytes: &[u8]) -> Result<(), StorageError>;
    fn delete(&self, key: StorageKey) -> Result<(), StorageError>;
    fn exists(&self, key: StorageKey) -> Result<bool, StorageError>;
}
pub struct FilesystemStorage {
    root: PathBuf,
}
impl FilesystemStorage {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn for_user() -> Result<Self, StorageError> {
        let dirs = directories::BaseDirs::new().ok_or_else(|| {
            StorageError::Unavailable("User application data directory is unavailable".into())
        })?;
        Ok(Self::new(dirs.data_local_dir().join("puzzella")))
    }
    fn directory(&self, namespace: StorageNamespace) -> PathBuf {
        self.root.join(match namespace {
            StorageNamespace::Saves => "saves",
            StorageNamespace::Images => "images",
        })
    }
    fn path(&self, key: StorageKey) -> PathBuf {
        self.directory(key.namespace()).join(key.filename())
    }
}
impl SaveStorage for FilesystemStorage {
    fn list(&self, namespace: StorageNamespace) -> Result<Vec<StorageKey>, StorageError> {
        let entries = match fs::read_dir(self.directory(namespace)) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io(e)),
        };
        let mut keys = Vec::new();
        for entry in entries {
            let entry = entry.map_err(io)?;
            if !entry.file_type().map_err(io)?.is_file() {
                continue;
            }
            if let Some(key) = parse_filename(namespace, &entry.file_name().to_string_lossy()) {
                keys.push(key);
            }
        }
        Ok(keys)
    }
    fn read(&self, key: StorageKey) -> Result<Vec<u8>, StorageError> {
        let file = fs::File::open(self.path(key)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StorageError::NotFound(key)
            } else {
                io(e)
            }
        })?;
        if file.metadata().map_err(io)?.len() > key.max_bytes() {
            return Err(StorageError::TooLarge);
        }
        let mut bytes = Vec::new();
        file.take(key.max_bytes() + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() as u64 > key.max_bytes() {
            return Err(StorageError::TooLarge);
        }
        Ok(bytes)
    }
    fn read_range(
        &self,
        key: StorageKey,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, StorageError> {
        if length as u64 > key.max_bytes() || offset.checked_add(length as u64).is_none() {
            return Err(StorageError::InvalidRange);
        }
        let mut file = fs::File::open(self.path(key)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StorageError::NotFound(key)
            } else {
                io(e)
            }
        })?;
        if file.metadata().map_err(io)?.len() > key.max_bytes() {
            return Err(StorageError::TooLarge);
        }
        file.seek(SeekFrom::Start(offset)).map_err(io)?;
        let mut bytes = Vec::new();
        file.take(length as u64)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        Ok(bytes)
    }
    fn len(&self, key: StorageKey) -> Result<u64, StorageError> {
        let metadata = fs::metadata(self.path(key)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                StorageError::NotFound(key)
            } else {
                io(e)
            }
        })?;
        if metadata.len() > key.max_bytes() {
            return Err(StorageError::TooLarge);
        }
        Ok(metadata.len())
    }
    fn write(&self, key: StorageKey, bytes: &[u8]) -> Result<(), StorageError> {
        if bytes.len() as u64 > key.max_bytes() {
            return Err(StorageError::TooLarge);
        }
        let directory = self.directory(key.namespace());
        fs::create_dir_all(&directory).map_err(io)?;
        let mut temp = tempfile::Builder::new()
            .prefix(".puzzella-")
            .suffix(".tmp")
            .tempfile_in(&directory)
            .map_err(io)?;
        temp.write_all(bytes).map_err(io)?;
        temp.flush().map_err(io)?;
        temp.as_file().sync_all().map_err(io)?;
        // tempfile uses atomic overwrite (MoveFileExW on Windows), never delete + rename.
        temp.persist(self.path(key)).map_err(|e| io(e.error))?;
        sync_directory(&directory)?;
        Ok(())
    }
    fn delete(&self, key: StorageKey) -> Result<(), StorageError> {
        match fs::remove_file(self.path(key)) {
            Ok(()) => sync_directory(&self.directory(key.namespace())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(e)),
        }
    }
    fn exists(&self, key: StorageKey) -> Result<bool, StorageError> {
        match fs::metadata(self.path(key)) {
            Ok(m) => Ok(m.is_file()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(io(e)),
        }
    }
}
fn sync_directory(path: &Path) -> Result<(), StorageError> {
    #[cfg(unix)]
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(io)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn parse_filename(namespace: StorageNamespace, name: &str) -> Option<StorageKey> {
    let (extension, len) = match namespace {
        StorageNamespace::Saves => (".puzsave", 32),
        StorageNamespace::Images => (".puzimg", 64),
    };
    let hex = name.strip_suffix(extension)?;
    if hex.len() != len
        || !hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    match namespace {
        StorageNamespace::Saves => Some(StorageKey::Save(SaveId(
            u128::from_str_radix(hex, 16).ok()?,
        ))),
        StorageNamespace::Images => {
            let mut hash = [0; 32];
            for (i, byte) in hash.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
            }
            Some(StorageKey::Image(ImageHash(hash)))
        }
    }
}
