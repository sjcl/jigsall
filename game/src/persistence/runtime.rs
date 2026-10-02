//! Channel adapter: workers own disk I/O, codecs, decode and restore preparation.
use super::*;
use crate::{checkpoint::PuzzleCheckpoint, resources::*};
use bevy::prelude::*;
use puzzella_core::PuzzleDefinition;
use std::sync::Arc;

#[derive(Resource)]
pub struct OriginalPuzzleImage {
    pub hash: ImageHash,
    /// Released only after a successful import. Saving never rereads the source path.
    pub encoded: Option<Arc<[u8]>>,
}
#[derive(Resource, Default)]
pub struct PersistenceState {
    pub current_save: Option<SaveMetadata>,
    pub entries: Vec<SaveListEntry>,
    pub busy: bool,
    pub title_dialog_open: bool,
    pub error: Option<String>,
    pub message: Option<String>,
    pub generation: u64,
    pub(crate) capture_title: Option<SaveTitle>,
}
#[derive(Resource)]
pub(crate) struct PendingRestore(pub Option<RestoredPuzzle>);
pub(crate) struct RestoredPuzzle {
    pub definition: PuzzleDefinition,
    pub store: PieceDataStore,
}
struct LoadedPuzzle {
    restored: RestoredPuzzle,
    metadata: SaveMetadata,
    hash: ImageHash,
    image: Image,
    opaque: bool,
}
enum Request {
    Import(ImageHash, Arc<[u8]>),
    List,
    Load(SaveId),
    Save {
        id: Option<SaveId>,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        bytes: Option<Arc<[u8]>>,
    },
    Delete(SaveId),
}
enum Reply {
    Imported(ImageHash, Result<(), SaveError>),
    Listed(Result<Vec<SaveListEntry>, SaveError>),
    Loaded(SaveId, Result<Box<LoadedPuzzle>, SaveError>),
    Saved(Result<SaveMetadata, SaveError>),
    Deleted(Result<Vec<SaveListEntry>, SaveError>),
}
#[derive(Resource)]
pub struct PersistenceService {
    tx: crossbeam::channel::Sender<(u64, Request)>,
    rx: crossbeam::channel::Receiver<(u64, Reply)>,
}
impl Default for PersistenceService {
    fn default() -> Self {
        // Resolve the directory inside the worker; never perform filesystem I/O here.
        Self::spawn(FilesystemStorage::for_user)
    }
}
impl PersistenceService {
    /// Inject a backend without changing the UI, formats, or gameplay restore.
    pub fn new<S: SaveStorage>(storage: S) -> Self {
        Self::spawn(move || Ok(storage))
    }
    fn spawn<S: SaveStorage>(
        storage: impl FnOnce() -> Result<S, StorageError> + Send + 'static,
    ) -> Self {
        let (tx, requests) = crossbeam::channel::unbounded();
        let (results, rx) = crossbeam::channel::unbounded();
        std::thread::spawn(move || {
            let repository = storage().map(SaveRepository::new);
            while let Ok((generation, request)) = requests.recv() {
                let reply = run_request(&repository, request);
                if results.send((generation, reply)).is_err() {
                    break;
                }
            }
        });
        Self { tx, rx }
    }
    fn submit(&self, state: &mut PersistenceState, request: Request) {
        if state.busy {
            return;
        }
        state.error = None;
        state.message = None;
        if self.tx.send((state.generation, request)).is_ok() {
            state.busy = true;
        } else {
            state.error = Some("Save worker stopped".into());
        }
    }
    pub fn list(&self, state: &mut PersistenceState) {
        self.submit(state, Request::List);
    }
    pub fn load(&self, state: &mut PersistenceState, id: SaveId) {
        self.submit(state, Request::Load(id));
    }
    pub fn delete(&self, state: &mut PersistenceState, id: SaveId) {
        self.submit(state, Request::Delete(id));
    }
    pub fn save(
        &self,
        state: &mut PersistenceState,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        bytes: Option<Arc<[u8]>>,
    ) {
        self.submit(
            state,
            Request::Save {
                id: state.current_save.as_ref().map(|m| m.id),
                title,
                checkpoint,
                bytes,
            },
        );
    }
    /// UI requests are captured after the frame's queued release commands are applied.
    pub fn request_save(&self, state: &mut PersistenceState, title: SaveTitle) {
        if state.busy {
            return;
        }
        state.capture_title = Some(title);
        state.busy = true;
        state.error = None;
        state.message = None;
    }
    pub(crate) fn import(&self, generation: u64, hash: ImageHash, bytes: Arc<[u8]>) {
        let _ = self.tx.send((generation, Request::Import(hash, bytes)));
    }
    #[cfg(test)]
    pub(crate) fn for_test(root: std::path::PathBuf) -> Self {
        Self::new(FilesystemStorage::new(root))
    }
}

pub(crate) fn capture_requested_save(
    service: Res<PersistenceService>,
    mut state: ResMut<PersistenceState>,
    store: Res<PieceDataStore>,
    definition: Option<Res<PuzzleDefinition>>,
    original: Option<Res<OriginalPuzzleImage>>,
) {
    let Some(title) = state.capture_title.take() else {
        return;
    };
    state.busy = false;
    let (Some(definition), Some(original)) = (definition, original) else {
        state.error = Some("Puzzle definition or original image is unavailable".into());
        return;
    };
    match PuzzleCheckpoint::capture(&store, &definition, original.hash) {
        Ok(checkpoint) => service.save(&mut state, title, checkpoint, original.encoded.clone()),
        Err(error) => state.error = Some(error.to_string()),
    }
}
fn run_request<S: SaveStorage>(
    repository: &Result<SaveRepository<S>, StorageError>,
    request: Request,
) -> Reply {
    let repo = || {
        repository
            .as_ref()
            .map_err(|e| SaveError::Storage(e.clone()))
    };
    match request {
        Request::Import(hash, bytes) => {
            Reply::Imported(hash, repo().and_then(|r| r.import_image(hash, &bytes)))
        }
        Request::List => Reply::Listed(repo().and_then(SaveRepository::list)),
        Request::Delete(id) => Reply::Deleted(repo().and_then(|r| {
            r.delete(id)?;
            r.list()
        })),
        Request::Save {
            id,
            title,
            checkpoint,
            bytes,
        } => Reply::Saved(repo().and_then(|r| {
            let bytes = bytes.as_deref();
            match id {
                Some(id) => r.update(id, title, checkpoint, bytes),
                None => r.create(title, checkpoint, bytes),
            }
        })),
        Request::Load(id) => Reply::Loaded(
            id,
            repo().and_then(|r| {
                let loaded = r.load(id)?;
                let image = crate::asset_reader::decode_image_bytes(&loaded.image_bytes)
                    .map_err(SaveError::Decode)?;
                if image.size() != loaded.save.checkpoint.definition.image_size {
                    return Err(SaveError::CorruptSave(
                        "Image dimensions do not match puzzle definition",
                    ));
                }
                let opaque = images::image_is_opaque(&image);
                let mut store = PieceDataStore::default();
                loaded.save.checkpoint.install(&mut store)?;
                Ok(Box::new(LoadedPuzzle {
                    restored: RestoredPuzzle {
                        definition: loaded.save.checkpoint.definition,
                        store,
                    },
                    metadata: loaded.save.metadata,
                    hash: loaded.save.checkpoint.image_hash,
                    image,
                    opaque,
                }))
            }),
        ),
    }
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn poll_results(
    service: Res<PersistenceService>,
    mut state: ResMut<PersistenceState>,
    original: Option<ResMut<OriginalPuzzleImage>>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    app_state: Res<State<AppState>>,
    mut next: ResMut<NextState<AppState>>,
) {
    let mut original = original;
    while let Ok((generation, reply)) = service.rx.try_recv() {
        if generation != state.generation {
            continue;
        }
        if let Reply::Imported(hash, result) = reply {
            if let Some(ref mut original) = original {
                if original.hash == hash {
                    match result {
                        Ok(()) => original.encoded = None,
                        Err(e) => {
                            state.error =
                                Some(format!("Image import failed; original bytes retained: {e}"))
                        }
                    }
                }
            }
            continue;
        }
        state.busy = false;
        let result = match reply {
            Reply::Listed(result) | Reply::Deleted(result) => {
                result.map(|entries| state.entries = entries)
            }
            Reply::Saved(result) => result.map(|metadata| {
                state.current_save = Some(metadata);
                state.message = Some("Game saved".into());
                if let Some(ref mut original) = original {
                    original.encoded = None;
                }
            }),
            Reply::Loaded(id, result) => {
                if let Err(error) = &result {
                    if let Some(entry) = state.entries.iter_mut().find(|entry| entry.id == id) {
                        entry.summary = Err(error.clone());
                    }
                }
                result.map(|loaded| {
                    if *app_state.get() != AppState::Menu {
                        return;
                    }
                    let loaded = *loaded;
                    let size = loaded.image.size().as_vec2();
                    let handle = images.add(loaded.image);
                    commands.insert_resource(PuzzleImage {
                        handle,
                        size,
                        opaque: loaded.opaque,
                    });
                    commands.insert_resource(OriginalPuzzleImage {
                        hash: loaded.hash,
                        encoded: None,
                    });
                    commands.insert_resource(PendingRestore(Some(loaded.restored)));
                    state.current_save = Some(loaded.metadata);
                    next.set(AppState::InGame);
                })
            }
            Reply::Imported(..) => unreachable!(),
        };
        if let Err(error) = result {
            state.error = Some(error.to_string());
        }
    }
}
