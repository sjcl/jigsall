use super::{
    codec::{MAX_IMAGE_BYTES, MAX_SAVE_BYTES},
    ImageHash, SaveId,
};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const TEMP_FILE_PREFIX: &str = ".puzzella-";
const TEMP_FILE_SUFFIX: &str = ".tmp";
const STALE_TEMP_FILE_AGE: Duration = Duration::from_secs(24 * 60 * 60);

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
    /// Transfer the encoded allocation to the executor without a blob-sized copy.
    fn write(&self, key: StorageKey, bytes: Vec<u8>) -> Result<(), StorageError>;
    fn delete(&self, key: StorageKey) -> Result<(), StorageError>;
    fn exists(&self, key: StorageKey) -> Result<bool, StorageError>;
}
#[derive(Clone)]
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
    /// Run once on the persistence worker before accepting requests. Recent
    /// files may belong to another running instance and must be left alone.
    pub(crate) fn cleanup_stale_temp_files(&self) {
        let Some(cutoff) = SystemTime::now().checked_sub(STALE_TEMP_FILE_AGE) else {
            return;
        };
        for namespace in [StorageNamespace::Saves, StorageNamespace::Images] {
            let directory = self.directory(namespace);
            let entries = match fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    bevy::log::warn!("Could not clean up temporary files");
                    bevy::log::debug!(
                        directory = %directory.display(),
                        error = %e,
                        "Temporary file cleanup failure details"
                    );
                    continue;
                }
            };
            for entry in entries {
                if let Err(e) = entry.and_then(|entry| remove_stale_temp_file(entry, cutoff)) {
                    bevy::log::warn!("Could not clean up a temporary file");
                    bevy::log::debug!(
                        directory = %directory.display(),
                        error = %e,
                        "Temporary file cleanup failure details"
                    );
                }
            }
        }
    }
}

fn remove_stale_temp_file(entry: fs::DirEntry, cutoff: SystemTime) -> std::io::Result<()> {
    let name = entry.file_name();
    let Some(name) = name.to_str() else {
        return Ok(());
    };
    if !name.starts_with(TEMP_FILE_PREFIX) || !name.ends_with(TEMP_FILE_SUFFIX) {
        return Ok(());
    }
    // DirEntry::metadata does not follow symlinks. Never recurse into directories.
    let metadata = match entry.metadata() {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if !metadata.is_file() || metadata.modified()? > cutoff {
        return Ok(());
    }
    match fs::remove_file(entry.path()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
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
    fn write(&self, key: StorageKey, bytes: Vec<u8>) -> Result<(), StorageError> {
        if bytes.len() as u64 > key.max_bytes() {
            return Err(StorageError::TooLarge);
        }
        let directory = self.directory(key.namespace());
        fs::create_dir_all(&directory).map_err(io)?;
        let mut temp = tempfile::Builder::new()
            .prefix(TEMP_FILE_PREFIX)
            .suffix(TEMP_FILE_SUFFIX)
            .tempfile_in(&directory)
            .map_err(io)?;
        temp.write_all(&bytes).map_err(io)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_at(path: &Path, modified: SystemTime) {
        fs::write(path, b"partial").unwrap();
        set_modified(path, modified);
    }

    fn set_modified(path: &Path, modified: SystemTime) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
    }

    #[test]
    fn cleanup_removes_only_old_matching_files_in_both_namespaces() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FilesystemStorage::new(dir.path());
        let now = SystemTime::now();
        let old = now - STALE_TEMP_FILE_AGE * 2;
        for namespace in [StorageNamespace::Saves, StorageNamespace::Images] {
            let directory = storage.directory(namespace);
            fs::create_dir_all(&directory).unwrap();
            let stale = directory.join(".puzzella-stale.tmp");
            write_at(&stale, old);
            let key = match namespace {
                StorageNamespace::Saves => StorageKey::Save(SaveId(1)),
                StorageNamespace::Images => StorageKey::Image(ImageHash([1; 32])),
            };
            storage.write(key, b"saved data".to_vec()).unwrap();
            set_modified(&storage.path(key), old);
            let preserved = [
                (".puzzella-recent.tmp", now),
                (".puzzella-future.tmp", now + STALE_TEMP_FILE_AGE),
                ("other.tmp", old),
                (".puzzella-other.tmp.bak", old),
                (".puzzella-other.puzsave", old),
            ];
            for (name, modified) in preserved {
                write_at(&directory.join(name), modified);
            }
            let nested = directory.join(".puzzella-directory.tmp");
            fs::create_dir(&nested).unwrap();
            let nested_temp = nested.join(".puzzella-nested.tmp");
            write_at(&nested_temp, old);

            storage.cleanup_stale_temp_files();

            assert!(!stale.exists());
            assert_eq!(storage.read(key).unwrap(), b"saved data");
            for (name, _) in preserved {
                assert!(directory.join(name).exists(), "{name}");
            }
            assert!(nested_temp.exists());
        }
        // Cleanup is safe to repeat and a recent, open temp can still be published.
        let directory = storage.directory(StorageNamespace::Saves);
        let mut active = tempfile::Builder::new()
            .prefix(TEMP_FILE_PREFIX)
            .suffix(TEMP_FILE_SUFFIX)
            .tempfile_in(&directory)
            .unwrap();
        active.write_all(b"active write").unwrap();
        let key = StorageKey::Save(SaveId(1));
        storage.cleanup_stale_temp_files();
        assert!(active.path().exists());
        active.persist(storage.path(key)).unwrap();
        assert_eq!(storage.read(key).unwrap(), b"active write");
    }

    #[test]
    fn cleanup_does_not_create_missing_directories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("missing");
        FilesystemStorage::new(&root).cleanup_stale_temp_files();
        assert!(!root.exists());
    }

    #[test]
    fn cleanup_continues_after_a_namespace_cannot_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FilesystemStorage::new(dir.path());
        fs::write(
            storage.directory(StorageNamespace::Saves),
            b"not a directory",
        )
        .unwrap();
        let images = storage.directory(StorageNamespace::Images);
        fs::create_dir(&images).unwrap();
        let stale = images.join(".puzzella-stale.tmp");
        write_at(&stale, SystemTime::now() - STALE_TEMP_FILE_AGE * 2);

        storage.cleanup_stale_temp_files();

        assert!(!stale.exists());
        let key = StorageKey::Image(ImageHash([1; 32]));
        storage.write(key, b"image data".to_vec()).unwrap();
        assert_eq!(storage.read(key).unwrap(), b"image data");
    }

    #[cfg(windows)]
    #[test]
    fn cleanup_continues_after_a_temp_file_cannot_be_deleted() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let storage = FilesystemStorage::new(dir.path());
        let old = SystemTime::now() - STALE_TEMP_FILE_AGE * 2;
        let saves = storage.directory(StorageNamespace::Saves);
        fs::create_dir(&saves).unwrap();
        let locked_path = saves.join(".puzzella-locked.tmp");
        write_at(&locked_path, old);
        let locked = fs::File::options()
            .read(true)
            .share_mode(1)
            .open(&locked_path)
            .unwrap();
        let removable = saves.join(".puzzella-removable.tmp");
        write_at(&removable, old);
        let images = storage.directory(StorageNamespace::Images);
        fs::create_dir(&images).unwrap();
        let image_temp = images.join(".puzzella-removable.tmp");
        write_at(&image_temp, old);

        storage.cleanup_stale_temp_files();

        assert!(locked_path.exists());
        assert!(!removable.exists());
        assert!(!image_temp.exists());
        let key = StorageKey::Save(SaveId(1));
        storage.write(key, b"saved data".to_vec()).unwrap();
        assert_eq!(storage.read(key).unwrap(), b"saved data");
        drop(locked);
        storage.cleanup_stale_temp_files();
        assert!(!locked_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_preserves_symlinks_and_their_targets() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FilesystemStorage::new(dir.path());
        let saves = storage.directory(StorageNamespace::Saves);
        fs::create_dir(&saves).unwrap();
        let target = dir.path().join("target");
        write_at(&target, SystemTime::now() - STALE_TEMP_FILE_AGE * 2);
        let link = saves.join(".puzzella-symlink.tmp");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        storage.cleanup_stale_temp_files();

        assert!(link.is_symlink());
        assert_eq!(fs::read(target).unwrap(), b"partial");
    }
}
