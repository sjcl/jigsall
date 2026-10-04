//! Transport for storage owned by another thread. Requests, replies and
//! transferable lock guards cross threads; thread-affine backend handles and
//! asynchronous API callbacks stay with their owner.
use super::{ImageHash, SaveStorage, StorageError, StorageGuard, StorageKey, StorageNamespace};
use crossbeam::channel::{self, Receiver, Sender, TryRecvError};

#[derive(Debug)]
pub enum StorageOperation {
    TryLockRepository,
    TryRetainImage(ImageHash),
    TryLockImage(ImageHash),
    List(StorageNamespace),
    Read(StorageKey),
    ReadRange(StorageKey, u64, usize),
    Len(StorageKey),
    Write(StorageKey, Vec<u8>),
    Delete(StorageKey),
    Exists(StorageKey),
}
#[derive(Debug)]
pub enum StorageValue {
    Guard(Option<StorageGuard>),
    Keys(Vec<StorageKey>),
    Bytes(Vec<u8>),
    Len(u64),
    Exists(bool),
    Done,
}
pub struct StorageReply(Sender<Result<StorageValue, StorageError>>);
impl StorageReply {
    /// May be retained until an owner-thread asynchronous callback completes.
    pub fn complete(self, result: Result<StorageValue, StorageError>) -> Result<(), StorageError> {
        self.0.send(result).map_err(|_| stopped())
    }
}
pub struct StorageRequest {
    pub operation: StorageOperation,
    pub reply: StorageReply,
}
impl StorageRequest {
    /// Convenience for a synchronous owner-thread backend. Async integrations
    /// dispatch `operation` and complete `reply` from their API callback instead.
    pub fn execute(self, storage: &impl SaveStorage) -> Result<(), StorageError> {
        let result = match self.operation {
            StorageOperation::TryLockRepository => {
                storage.try_lock_repository().map(StorageValue::Guard)
            }
            StorageOperation::TryRetainImage(hash) => {
                storage.try_retain_image(hash).map(StorageValue::Guard)
            }
            StorageOperation::TryLockImage(hash) => {
                storage.try_lock_image(hash).map(StorageValue::Guard)
            }
            StorageOperation::List(ns) => storage.list(ns).map(StorageValue::Keys),
            StorageOperation::Read(key) => storage.read(key).map(StorageValue::Bytes),
            StorageOperation::ReadRange(key, offset, length) => storage
                .read_range(key, offset, length)
                .map(StorageValue::Bytes),
            StorageOperation::Len(key) => storage.len(key).map(StorageValue::Len),
            StorageOperation::Write(key, bytes) => {
                storage.write(key, bytes).map(|()| StorageValue::Done)
            }
            StorageOperation::Delete(key) => storage.delete(key).map(|()| StorageValue::Done),
            StorageOperation::Exists(key) => storage.exists(key).map(StorageValue::Exists),
        };
        self.reply.complete(result)
    }
}
pub struct StorageRequests(Receiver<StorageRequest>);
impl StorageRequests {
    /// Poll from the storage owner's event loop; never wait on the main thread.
    pub fn try_recv(&self) -> Result<StorageRequest, TryRecvError> {
        self.0.try_recv()
    }
}
#[derive(Clone)]
pub struct StorageProxy(Sender<StorageRequest>);
pub fn storage_channel() -> (StorageProxy, StorageRequests) {
    let (tx, rx) = channel::unbounded();
    (StorageProxy(tx), StorageRequests(rx))
}
/// Keep a Send backend on one I/O thread without requiring Clone or Sync.
pub(crate) fn spawn_storage<S: SaveStorage + Send + 'static>(storage: S) -> StorageProxy {
    let (proxy, StorageRequests(requests)) = storage_channel();
    std::thread::spawn(move || {
        while let Ok(request) = requests.recv() {
            // A stopped requester must not prevent the other worker from using storage.
            let _ = request.execute(&storage);
        }
    });
    proxy
}
fn stopped() -> StorageError {
    StorageError::Unavailable("Storage executor stopped".into())
}
fn unexpected() -> StorageError {
    StorageError::Io("Storage executor returned an unexpected reply type".into())
}
impl StorageProxy {
    fn request(&self, operation: StorageOperation) -> Result<StorageValue, StorageError> {
        let (tx, rx) = channel::bounded(1);
        self.0
            .send(StorageRequest {
                operation,
                reply: StorageReply(tx),
            })
            .map_err(|_| stopped())?;
        // Only the requesting worker waits. The owner thread continues pumping
        // its API/event loop while an asynchronous operation is outstanding.
        rx.recv().map_err(|_| stopped())?
    }
}
impl SaveStorage for StorageProxy {
    fn try_lock_repository(&self) -> Result<Option<StorageGuard>, StorageError> {
        match self.request(StorageOperation::TryLockRepository)? {
            StorageValue::Guard(guard) => Ok(guard),
            _ => Err(unexpected()),
        }
    }
    fn try_retain_image(&self, hash: ImageHash) -> Result<Option<StorageGuard>, StorageError> {
        match self.request(StorageOperation::TryRetainImage(hash))? {
            StorageValue::Guard(guard) => Ok(guard),
            _ => Err(unexpected()),
        }
    }
    fn try_lock_image(&self, hash: ImageHash) -> Result<Option<StorageGuard>, StorageError> {
        match self.request(StorageOperation::TryLockImage(hash))? {
            StorageValue::Guard(guard) => Ok(guard),
            _ => Err(unexpected()),
        }
    }
    fn list(&self, namespace: StorageNamespace) -> Result<Vec<StorageKey>, StorageError> {
        match self.request(StorageOperation::List(namespace))? {
            StorageValue::Keys(keys) => Ok(keys),
            _ => Err(unexpected()),
        }
    }
    fn read(&self, key: StorageKey) -> Result<Vec<u8>, StorageError> {
        match self.request(StorageOperation::Read(key))? {
            StorageValue::Bytes(bytes) => Ok(bytes),
            _ => Err(unexpected()),
        }
    }
    fn read_range(
        &self,
        key: StorageKey,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, StorageError> {
        match self.request(StorageOperation::ReadRange(key, offset, length))? {
            StorageValue::Bytes(bytes) => Ok(bytes),
            _ => Err(unexpected()),
        }
    }
    fn len(&self, key: StorageKey) -> Result<u64, StorageError> {
        match self.request(StorageOperation::Len(key))? {
            StorageValue::Len(length) => Ok(length),
            _ => Err(unexpected()),
        }
    }
    fn write(&self, key: StorageKey, bytes: Vec<u8>) -> Result<(), StorageError> {
        match self.request(StorageOperation::Write(key, bytes))? {
            StorageValue::Done => Ok(()),
            _ => Err(unexpected()),
        }
    }
    fn delete(&self, key: StorageKey) -> Result<(), StorageError> {
        match self.request(StorageOperation::Delete(key))? {
            StorageValue::Done => Ok(()),
            _ => Err(unexpected()),
        }
    }
    fn exists(&self, key: StorageKey) -> Result<bool, StorageError> {
        match self.request(StorageOperation::Exists(key))? {
            StorageValue::Exists(exists) => Ok(exists),
            _ => Err(unexpected()),
        }
    }
}
