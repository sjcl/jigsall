//! Workers own codecs, decode and restore preparation. I/O can run on the worker
//! or be dispatched to an external storage owner's event loop.
use super::repository::SaveOutcome;
use super::*;
use crate::{checkpoint::PuzzleCheckpoint, resources::*};
use bevy::prelude::*;
use puzzella_core::PuzzleDefinition;
use std::{num::NonZeroU32, sync::Arc};

#[derive(Resource)]
pub struct OriginalPuzzleImage {
    pub hash: ImageHash,
    /// Released after successful persistence with a live image lease, unless
    /// retained for hosting.
    /// Saving never rereads the source path.
    pub encoded: Option<Arc<[u8]>>,
    /// Held through selection, play, pause and pending persistence requests.
    pub image_lease: Option<ImageLease>,
}
#[derive(Resource, Default)]
pub struct PersistenceState {
    /// Keep encoded source bytes only while preparing/running a user-hosted game.
    pub retain_image_for_host: bool,
    pub game_id: GameId,
    pub current_save: Option<SaveMetadata>,
    pub current_autosave: Option<SaveMetadata>,
    pub autosaving: bool,
    pub autosave_error: Option<PersistenceError>,
    pub entries: Vec<SaveListEntry>,
    pub busy: bool,
    pub title_dialog_open: bool,
    pub error: Option<PersistenceError>,
    pub message: Option<PersistenceNotice>,
    pub generation: u64,
    pub(crate) capture: Option<SaveCapture>,
}

pub(crate) struct SaveCapture {
    title: SaveTitle,
    autosave_limit: Option<NonZeroU32>,
}

#[derive(Clone, Debug)]
pub enum PersistenceError {
    WorkerStopped,
    DefinitionUnavailable,
    Checkpoint(CheckpointError),
    ImageImport(SaveError),
    Save(SaveError),
    AutosaveRotation(SaveError),
}

#[derive(Clone, Copy, Debug)]
pub enum PersistenceNotice {
    Saved,
}
#[derive(Resource)]
pub(crate) struct PendingRestore(pub Option<RestoredPuzzle>);
pub(crate) struct RestoredPuzzle {
    pub definition: PuzzleDefinition,
    pub store: PieceDataStore,
}
struct LoadedPuzzle {
    encoded: Option<Arc<[u8]>>,
    restored: RestoredPuzzle,
    metadata: SaveMetadata,
    hash: ImageHash,
    image: Image,
    logical_size: UVec2,
    opaque: bool,
    image_lease: ImageLease,
}
/// Small CPU pixels for a menu texture; full decoded images stay on the worker.
pub struct ThumbnailImage {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}
pub struct ThumbnailReply {
    pub generation: u64,
    pub hash: ImageHash,
    pub result: Result<ThumbnailImage, SaveError>,
}
pub const THUMBNAIL_MAX_EDGE: u32 = 224;
enum Request {
    Import(ImageHash, Arc<[u8]>),
    List,
    Load(SaveId, ImageDecodeLimits),
    LoadForHost(SaveId, ImageDecodeLimits),
    Save {
        game_id: GameId,
        autosave_limit: Option<NonZeroU32>,
        update: Option<(SaveId, u64)>,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        bytes: Option<Arc<[u8]>>,
        image_lease: Option<Box<ImageLease>>,
    },
    Delete(SaveId),
}
enum Reply {
    Imported(ImageHash, Result<ImageLease, SaveError>),
    Listed(Result<Vec<SaveListEntry>, SaveError>),
    Loaded(SaveId, Result<Box<LoadedPuzzle>, SaveError>),
    Saved(bool, Result<(SaveOutcome, ImageLease), SaveError>),
    Deleted(Result<Vec<SaveListEntry>, SaveError>),
}
#[derive(Resource)]
pub struct PersistenceService {
    tx: crossbeam::channel::Sender<(u64, Request)>,
    rx: crossbeam::channel::Receiver<(u64, Reply)>,
    thumbnail_tx: crossbeam::channel::Sender<(u64, ImageHash)>,
    thumbnails: crossbeam::channel::Receiver<ThumbnailReply>,
}
impl Default for PersistenceService {
    fn default() -> Self {
        // Resolve and clean up the directory on the worker, never on the main thread.
        Self::spawn_filesystem(FilesystemStorage::for_user)
    }
}
impl PersistenceService {
    /// Keep a Send backend on an I/O thread, with separate foreground and thumbnail
    /// workers. Synchronous backend I/O is serialized; decode and hashing are not.
    pub fn new<S: SaveStorage + Send + 'static>(storage: S) -> Self {
        let proxy = executor::spawn_storage(storage);
        Self::spawn(move || Ok((proxy.clone(), proxy)))
    }
    /// Keep thread-affine handles with their owner. The owner must poll this
    /// inbox and complete each request, including on asynchronous API failure.
    /// Foreground and thumbnail requests may be outstanding together. Only their
    /// workers block waiting for I/O replies; the owner can dispatch both asynchronously.
    pub fn with_storage_requests() -> (Self, executor::StorageRequests) {
        let (proxy, requests) = executor::storage_channel();
        (Self::spawn(move || Ok((proxy.clone(), proxy))), requests)
    }
    fn spawn_filesystem(
        storage: impl FnOnce() -> Result<FilesystemStorage, StorageError> + Send + 'static,
    ) -> Self {
        Self::spawn(move || {
            let storage = storage()?;
            storage.cleanup_stale_temp_files();
            // Independent filesystem handles let thumbnail reads proceed without
            // occupying the foreground worker, including while it loads another image.
            Ok((storage.clone(), storage))
        })
    }
    fn spawn<S: SaveStorage, T: SaveStorage + Send + 'static>(
        storage: impl FnOnce() -> Result<(S, T), StorageError> + Send + 'static,
    ) -> Self {
        let (tx, requests) = crossbeam::channel::unbounded();
        let (results, rx) = crossbeam::channel::unbounded();
        let (thumbnail_tx, thumbnail_requests) = crossbeam::channel::unbounded();
        let (thumbnail_results, thumbnails) = crossbeam::channel::unbounded();
        std::thread::spawn(move || {
            let (repository, thumbnail_repository) = match storage() {
                Ok((foreground, thumbnails)) => (
                    Ok(SaveRepository::new(foreground)),
                    Ok(SaveRepository::new(thumbnails)),
                ),
                Err(error) => (Err(error.clone()), Err(error)),
            };
            std::thread::spawn(move || {
                while let Ok((generation, hash)) = thumbnail_requests.recv() {
                    let result = run_thumbnail(&thumbnail_repository, hash);
                    if thumbnail_results
                        .send(ThumbnailReply {
                            generation,
                            hash,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
            while let Ok((generation, request)) = requests.recv() {
                let reply = run_request(&repository, request);
                if results.send((generation, reply)).is_err() {
                    break;
                }
            }
        });
        Self {
            tx,
            rx,
            thumbnail_tx,
            thumbnails,
        }
    }
    fn submit(&self, state: &mut PersistenceState, request: Request) {
        if state.busy {
            return;
        }
        let is_autosave = matches!(
            request,
            Request::Save {
                autosave_limit: Some(_),
                ..
            }
        );
        if is_autosave {
            state.autosave_error = None;
        } else {
            state.error = None;
            state.message = None;
        }
        if self.tx.send((state.generation, request)).is_ok() {
            state.busy = true;
            state.autosaving = is_autosave;
        } else {
            state.autosaving = false;
            save_error(state, is_autosave, PersistenceError::WorkerStopped);
        }
    }
    pub fn list(&self, state: &mut PersistenceState) {
        self.submit(state, Request::List);
    }
    pub fn load(&self, state: &mut PersistenceState, id: SaveId, limits: ImageDecodeLimits) {
        self.submit(state, Request::Load(id, limits));
    }
    pub fn load_for_host(
        &self,
        state: &mut PersistenceState,
        id: SaveId,
        limits: ImageDecodeLimits,
    ) {
        state.retain_image_for_host = true;
        self.submit(state, Request::LoadForHost(id, limits));
    }
    pub fn delete(&self, state: &mut PersistenceState, id: SaveId) {
        self.submit(state, Request::Delete(id));
    }
    /// The UI limits this to one in-flight request and selects only visible images.
    /// Thumbnail replies never change the foreground operation's busy/error state.
    pub fn request_thumbnail(
        &self,
        generation: u64,
        hash: ImageHash,
    ) -> Result<(), PersistenceError> {
        self.thumbnail_tx
            .send((generation, hash))
            .map_err(|_| PersistenceError::WorkerStopped)
    }
    pub fn try_recv_thumbnail(&self) -> Option<ThumbnailReply> {
        self.thumbnails.try_recv().ok()
    }
    pub fn save(
        &self,
        state: &mut PersistenceState,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        bytes: Option<Arc<[u8]>>,
    ) {
        self.save_as(state, title, checkpoint, bytes, None, None);
    }
    fn save_as(
        &self,
        state: &mut PersistenceState,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        bytes: Option<Arc<[u8]>>,
        autosave_limit: Option<NonZeroU32>,
        image_lease: Option<ImageLease>,
    ) {
        let metadata = if autosave_limit.is_some() {
            state.current_autosave.as_ref()
        } else {
            state
                .current_save
                .as_ref()
                .filter(|metadata| !metadata.is_autosave)
        };
        let update = metadata.map(|metadata| (metadata.id, metadata.revision));
        self.submit(
            state,
            Request::Save {
                game_id: state.game_id,
                autosave_limit,
                update,
                title,
                checkpoint,
                bytes,
                image_lease: image_lease.map(Box::new),
            },
        );
    }
    /// UI requests are captured after the frame's queued release commands are applied.
    pub fn request_save(&self, state: &mut PersistenceState, title: SaveTitle) {
        self.request_capture(state, title, None);
    }
    pub(crate) fn request_autosave(
        &self,
        state: &mut PersistenceState,
        title: SaveTitle,
        max_saves_per_game: NonZeroU32,
    ) {
        self.request_capture(state, title, Some(max_saves_per_game));
    }
    fn request_capture(
        &self,
        state: &mut PersistenceState,
        title: SaveTitle,
        autosave_limit: Option<NonZeroU32>,
    ) {
        if state.busy {
            return;
        }
        let is_autosave = autosave_limit.is_some();
        state.capture = Some(SaveCapture {
            title,
            autosave_limit,
        });
        state.busy = true;
        state.autosaving = is_autosave;
        if is_autosave {
            state.autosave_error = None;
        } else {
            state.error = None;
            state.message = None;
        }
    }
    pub(crate) fn import(&self, generation: u64, hash: ImageHash, bytes: Arc<[u8]>) {
        let _ = self.tx.send((generation, Request::Import(hash, bytes)));
    }
    #[cfg(test)]
    pub(crate) fn for_test(root: std::path::PathBuf) -> Self {
        Self::spawn_filesystem(move || Ok(FilesystemStorage::new(root)))
    }
}

pub(crate) fn capture_requested_save(
    service: Res<PersistenceService>,
    mut state: ResMut<PersistenceState>,
    store: Res<PieceDataStore>,
    definition: Option<Res<PuzzleDefinition>>,
    original: Option<Res<OriginalPuzzleImage>>,
) {
    let Some(capture) = state.capture.take() else {
        return;
    };
    let is_autosave = capture.autosave_limit.is_some();
    state.busy = false;
    state.autosaving = false;
    let (Some(definition), Some(original)) = (definition, original) else {
        save_error(
            &mut state,
            is_autosave,
            PersistenceError::DefinitionUnavailable,
        );
        return;
    };
    match PuzzleCheckpoint::capture(&store, &definition, original.hash) {
        Ok(checkpoint) => service.save_as(
            &mut state,
            capture.title,
            checkpoint,
            original.encoded.clone(),
            capture.autosave_limit,
            original.image_lease.clone(),
        ),
        Err(error) => save_error(&mut state, is_autosave, PersistenceError::Checkpoint(error)),
    }
}
fn save_error(state: &mut PersistenceState, is_autosave: bool, error: PersistenceError) {
    if is_autosave {
        state.autosave_error = Some(error);
    } else {
        state.error = Some(error);
    }
}
fn run_request<S: SaveStorage>(
    repository: &Result<SaveRepository<S>, StorageError>,
    request: Request,
) -> Reply {
    let retain_image = matches!(request, Request::LoadForHost(..));
    let repo = || {
        repository
            .as_ref()
            .map_err(|e| SaveError::Storage(e.clone()))
    };
    match request {
        Request::Import(hash, bytes) => Reply::Imported(
            hash,
            repo().and_then(|r| r.import_image_retained(hash, &bytes)),
        ),
        Request::List => Reply::Listed(repo().and_then(SaveRepository::list)),
        Request::Delete(id) => Reply::Deleted(repo().and_then(|r| {
            r.delete(id)?;
            r.list()
        })),
        Request::Save {
            game_id,
            autosave_limit,
            update,
            title,
            checkpoint,
            bytes,
            image_lease: _previous_lease,
        } => Reply::Saved(
            autosave_limit.is_some(),
            repo().and_then(|r| {
                let image_lease = r.retain_image(checkpoint.image_hash)?;
                let bytes = bytes.as_deref();
                if let Some(limit) = autosave_limit {
                    return r
                        .autosave(game_id, update, title, checkpoint, bytes, limit)
                        .map(|outcome| (outcome, image_lease));
                }
                match update {
                    Some((id, expected_revision)) => {
                        r.update(id, expected_revision, title, checkpoint, bytes)
                    }
                    None => r.create_for_game(game_id, title, checkpoint, bytes),
                }
                .map(|metadata| {
                    (
                        SaveOutcome {
                            metadata,
                            rotation_error: None,
                        },
                        image_lease,
                    )
                })
            }),
        ),
        Request::Load(id, limits) | Request::LoadForHost(id, limits) => Reply::Loaded(
            id,
            repo().and_then(|r| {
                let loaded = r.load(id)?;
                let decoded = crate::asset_reader::decode_image_bytes(&loaded.image_bytes, limits)
                    .map_err(SaveError::Decode)?;
                if decoded.logical_size != loaded.save.checkpoint.definition.image_size {
                    return Err(SaveError::CorruptSave(
                        "Image dimensions do not match puzzle definition",
                    ));
                }
                let opaque = images::image_is_opaque(&decoded.image);
                let mut store = PieceDataStore::default();
                loaded.save.checkpoint.install(&mut store)?;
                Ok(Box::new(LoadedPuzzle {
                    encoded: retain_image.then(|| Arc::from(loaded.image_bytes)),
                    restored: RestoredPuzzle {
                        definition: loaded.save.checkpoint.definition,
                        store,
                    },
                    metadata: loaded.save.metadata,
                    hash: loaded.save.checkpoint.image_hash,
                    image: decoded.image,
                    logical_size: decoded.logical_size,
                    opaque,
                    image_lease: loaded.image_lease,
                }))
            }),
        ),
    }
}
fn run_thumbnail<S: SaveStorage>(
    repository: &Result<SaveRepository<S>, StorageError>,
    hash: ImageHash,
) -> Result<ThumbnailImage, SaveError> {
    let repository = repository
        .as_ref()
        .map_err(|error| SaveError::Storage(error.clone()))?;
    let bytes = repository.read_image(hash)?;
    let decoded = crate::asset_reader::decode_puzzle_image_bytes(&bytes)
        .map_err(|error| SaveError::Decode(error.to_string()))?;
    let rgba = decoded
        .thumbnail(THUMBNAIL_MAX_EDGE, THUMBNAIL_MAX_EDGE)
        .to_rgba8();
    Ok(ThumbnailImage {
        size: [rgba.width() as usize, rgba.height() as usize],
        rgba: rgba.into_raw(),
    })
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
                        Ok(lease) => {
                            original.image_lease = Some(lease);
                            if !state.retain_image_for_host {
                                original.encoded = None;
                            }
                        }
                        Err(e) => state.error = Some(PersistenceError::ImageImport(e)),
                    }
                }
            }
            continue;
        }
        state.busy = false;
        state.autosaving = false;
        if let Reply::Saved(is_autosave, result) = reply {
            match result {
                Ok((
                    SaveOutcome {
                        metadata,
                        rotation_error,
                    },
                    lease,
                )) => {
                    if is_autosave {
                        if state
                            .current_save
                            .as_ref()
                            .is_some_and(|save| save.is_autosave)
                        {
                            state.current_save = Some(metadata.clone());
                        }
                        state.current_autosave = Some(metadata);
                        state.autosave_error =
                            rotation_error.map(PersistenceError::AutosaveRotation);
                    } else {
                        state.current_save = Some(metadata);
                        state.title_dialog_open = false;
                        state.message = Some(PersistenceNotice::Saved);
                    }
                    if let Some(ref mut original) = original {
                        if original.hash == lease.hash {
                            original.image_lease = Some(lease);
                            if !state.retain_image_for_host {
                                original.encoded = None;
                            }
                        }
                    }
                }
                Err(error) => save_error(&mut state, is_autosave, PersistenceError::Save(error)),
            }
            continue;
        }
        let result = match reply {
            Reply::Listed(result) | Reply::Deleted(result) => {
                result.map(|entries| state.entries = entries)
            }
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
                    let texture_size = loaded.image.size();
                    let handle = images.add(loaded.image);
                    commands.insert_resource(PuzzleImage {
                        handle,
                        logical_size: loaded.logical_size,
                        texture_size,
                        opaque: loaded.opaque,
                    });
                    commands.insert_resource(OriginalPuzzleImage {
                        hash: loaded.hash,
                        encoded: loaded.encoded,
                        image_lease: Some(loaded.image_lease),
                    });
                    commands.insert_resource(PendingRestore(Some(loaded.restored)));
                    state.current_autosave =
                        loaded.metadata.is_autosave.then(|| loaded.metadata.clone());
                    state.game_id = loaded.metadata.game_id;
                    state.current_save = Some(loaded.metadata);
                    next.set(AppState::InGame);
                })
            }
            Reply::Imported(..) | Reply::Saved(..) => unreachable!(),
        };
        if let Err(error) = result {
            state.error = Some(PersistenceError::Save(error));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::autosave::AutosaveSettingsState;
    use executor::{StorageOperation, StorageValue};
    use std::time::{Duration, Instant};

    #[test]
    fn restore_matches_logical_dimensions_and_installs_a_smaller_local_texture() {
        use puzzella_core::{PieceId, GENERATOR_VERSION, MAX_PUZZLE_IMAGE_DIMENSION};
        let dir = tempfile::tempdir().unwrap();
        let repository = Ok(SaveRepository::new(FilesystemStorage::new(dir.path())));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            20000,
            20,
            image::Rgb([12, 34, 56]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        let bytes = bytes.into_inner();
        let hash = image_hash(&bytes);
        let definition = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: UVec2::new(2, 1),
            image_size: UVec2::new(MAX_PUZZLE_IMAGE_DIMENSION, 16),
            snap_distance: 5.0,
        };
        let mut store = PieceDataStore::default();
        store.initialize(
            (0..2)
                .map(|id| definition.correct_position(PieceId(id)) + Vec2::splat(100.0))
                .collect(),
        );
        let checkpoint = PuzzleCheckpoint::capture(&store, &definition, hash).unwrap();
        let metadata = repository
            .as_ref()
            .unwrap()
            .create(
                SaveTitle::new("Large original").unwrap(),
                checkpoint.clone(),
                Some(&bytes),
            )
            .unwrap();
        let (service, _storage) = PersistenceService::with_storage_requests();
        let mut app = App::new();
        app.insert_resource(service)
            .init_resource::<PersistenceState>()
            .init_resource::<Assets<Image>>()
            .insert_resource(State::new(AppState::Menu))
            .insert_resource(NextState::<AppState>::default())
            .add_systems(Update, poll_results);
        for cap in [128, 8192] {
            let reply = run_request(
                &repository,
                Request::Load(
                    metadata.id,
                    ImageDecodeLimits {
                        max_texture_dimension: cap,
                    },
                ),
            );
            // Inspect the actual worker reply before applying the identical UI path.
            let Reply::Loaded(_, Ok(ref loaded)) = reply else {
                panic!("restore failed")
            };
            assert_eq!(loaded.logical_size, definition.image_size);
            assert_eq!(loaded.image.size().x, cap);
            assert_eq!(loaded.restored.store.states, store.states);
            let (tx, rx) = crossbeam::channel::unbounded();
            app.world_mut().resource_mut::<PersistenceService>().rx = rx;
            tx.send((0, reply)).unwrap();
            app.update();
            let image = app.world().resource::<PuzzleImage>();
            assert_eq!(image.logical_size, definition.image_size);
            assert_eq!(image.texture_size.x, cap);
            assert_eq!(app.world().resource::<OriginalPuzzleImage>().hash, hash);
        }
        assert_eq!(
            repository.as_ref().unwrap().read_image(hash).unwrap(),
            bytes
        );
        let mut wrong = checkpoint;
        wrong.definition.image_size.x -= 1;
        let wrong = repository
            .as_ref()
            .unwrap()
            .create(SaveTitle::new("Wrong dimensions").unwrap(), wrong, None)
            .unwrap();
        assert!(matches!(
            run_request(
                &repository,
                Request::Load(
                    wrong.id,
                    ImageDecodeLimits {
                        max_texture_dimension: 128
                    }
                )
            ),
            Reply::Loaded(
                _,
                Err(SaveError::CorruptSave(
                    "Image dimensions do not match puzzle definition"
                ))
            )
        ));
    }

    #[test]
    fn filesystem_startup_cleans_stale_temps_on_worker_before_requests() {
        use std::{fs, time::SystemTime};

        let dir = tempfile::tempdir().unwrap();
        let mut stale_paths = Vec::new();
        let mut recent_paths = Vec::new();
        for namespace in ["saves", "images"] {
            let directory = dir.path().join(namespace);
            fs::create_dir(&directory).unwrap();
            let stale = directory.join(".puzzella-stale.tmp");
            fs::write(&stale, b"partial").unwrap();
            fs::File::options()
                .write(true)
                .open(&stale)
                .unwrap()
                .set_times(
                    fs::FileTimes::new()
                        .set_modified(SystemTime::now() - Duration::from_secs(48 * 60 * 60)),
                )
                .unwrap();
            stale_paths.push(stale);
            let recent = directory.join(".puzzella-recent.tmp");
            fs::write(&recent, b"partial").unwrap();
            recent_paths.push(recent);
        }
        let root = dir.path().to_path_buf();
        let (initialized, worker_id) = crossbeam::channel::bounded(1);
        let service = PersistenceService::spawn_filesystem(move || {
            initialized.send(std::thread::current().id()).unwrap();
            Ok(FilesystemStorage::new(root))
        });
        assert_ne!(
            worker_id.recv_timeout(Duration::from_secs(10)).unwrap(),
            std::thread::current().id()
        );
        service.tx.send((0, Request::List)).unwrap();
        assert!(matches!(
            service.rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            (0, Reply::Listed(Ok(entries))) if entries.is_empty()
        ));
        assert!(stale_paths.iter().all(|path| !path.exists()));
        assert!(recent_paths.iter().all(|path| path.exists()));
    }

    fn save_app(root: std::path::PathBuf) -> App {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3])))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let bytes = bytes.into_inner();
        let definition = PuzzleDefinition {
            generator_version: puzzella_core::GENERATOR_VERSION,
            seed: 1,
            grid_size: UVec2::splat(2),
            image_size: UVec2::splat(2),
            snap_distance: 1.0,
        };
        let mut store = PieceDataStore::default();
        store.initialize(
            (0..4)
                .map(|i| definition.correct_position(puzzella_core::PieceId(i)) + Vec2::splat(2.0))
                .collect(),
        );
        let mut app = App::new();
        app.insert_resource(PersistenceService::for_test(root))
            .init_resource::<PersistenceState>()
            .insert_resource(AutosaveSettingsState::load(None))
            .insert_resource(definition)
            .insert_resource(store)
            .insert_resource(OriginalPuzzleImage {
                hash: image_hash(&bytes),
                encoded: Some(bytes.into()),
                image_lease: None,
            })
            .init_resource::<Assets<Image>>()
            .insert_resource(State::new(AppState::InGame))
            .insert_resource(NextState::<AppState>::default())
            .add_systems(Update, (capture_requested_save, poll_results).chain());
        app
    }

    #[test]
    fn imported_image_is_protected_in_transit_and_while_the_resource_is_alive() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = save_app(directory.path().to_owned());
        let storage = FilesystemStorage::new(directory.path());
        let repository = Ok(SaveRepository::new(storage.clone()));
        let original = app.world().resource::<OriginalPuzzleImage>();
        let hash = original.hash;
        let reply = run_request(
            &repository,
            Request::Import(hash, original.encoded.clone().unwrap()),
        );
        repository.as_ref().unwrap().delete(SaveId(99)).unwrap();
        assert!(storage.exists(StorageKey::Image(hash)).unwrap());
        let (tx, rx) = crossbeam::channel::unbounded();
        app.world_mut().resource_mut::<PersistenceService>().rx = rx;
        tx.send((0, reply)).unwrap();
        app.update();
        let original = app.world().resource::<OriginalPuzzleImage>();
        assert!(original.encoded.is_none());
        assert!(original.image_lease.is_some());
        repository.as_ref().unwrap().delete(SaveId(99)).unwrap();
        assert!(storage.exists(StorageKey::Image(hash)).unwrap());
        app.world_mut().remove_resource::<OriginalPuzzleImage>();
        repository.as_ref().unwrap().delete(SaveId(99)).unwrap();
        assert!(!storage.exists(StorageKey::Image(hash)).unwrap());
    }

    #[test]
    fn discarded_import_replies_release_their_image_leases() {
        for stale_generation in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let mut app = save_app(directory.path().to_owned());
            let storage = FilesystemStorage::new(directory.path());
            let repository = Ok(SaveRepository::new(storage.clone()));
            let original = app.world().resource::<OriginalPuzzleImage>();
            let hash = original.hash;
            let reply = run_request(
                &repository,
                Request::Import(hash, original.encoded.clone().unwrap()),
            );
            if stale_generation {
                app.world_mut()
                    .resource_mut::<PersistenceState>()
                    .generation = 1;
            } else {
                app.world_mut().resource_mut::<OriginalPuzzleImage>().hash =
                    image_hash(b"replacement");
            }
            let (tx, rx) = crossbeam::channel::unbounded();
            app.world_mut().resource_mut::<PersistenceService>().rx = rx;
            tx.send((0, reply)).unwrap();
            app.update();
            let original = app.world().resource::<OriginalPuzzleImage>();
            assert!(original.encoded.is_some());
            assert!(original.image_lease.is_none());
            repository.as_ref().unwrap().delete(SaveId(99)).unwrap();
            assert!(!storage.exists(StorageKey::Image(hash)).unwrap());
        }
    }

    #[test]
    fn pending_save_keeps_its_image_after_the_session_resource_is_removed() {
        let directory = tempfile::tempdir().unwrap();
        let storage = FilesystemStorage::new(directory.path());
        let repository = SaveRepository::new(storage.clone());
        let mut app = save_app(directory.path().to_owned());
        let original = app.world().resource::<OriginalPuzzleImage>();
        let hash = original.hash;
        let lease = repository
            .import_image_retained(hash, original.encoded.as_deref().unwrap())
            .unwrap();
        let mut original = app.world_mut().resource_mut::<OriginalPuzzleImage>();
        original.image_lease = Some(lease);
        original.encoded = None;
        let (service, inbox) = PersistenceService::with_storage_requests();
        app.insert_resource(service);
        app.world_mut()
            .resource_scope(|world, service: Mut<PersistenceService>| {
                service.request_save(
                    &mut world.resource_mut::<PersistenceState>(),
                    SaveTitle::new("pending").unwrap(),
                );
            });
        app.update();
        app.world_mut().remove_resource::<OriginalPuzzleImage>();
        repository.delete(SaveId(99)).unwrap();
        assert!(storage.exists(StorageKey::Image(hash)).unwrap());
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.world().resource::<PersistenceState>().busy {
            while let Ok(request) = inbox.try_recv() {
                request.execute(&storage).unwrap();
            }
            app.update();
            assert!(Instant::now() < deadline, "pending save did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
        let metadata = app
            .world()
            .resource::<PersistenceState>()
            .current_save
            .as_ref()
            .unwrap();
        assert_eq!(
            repository
                .load(metadata.id)
                .unwrap()
                .save
                .checkpoint
                .image_hash,
            hash
        );
        repository.delete(metadata.id).unwrap();
        assert!(!storage.exists(StorageKey::Image(hash)).unwrap());
    }

    #[test]
    fn failed_image_lock_keeps_original_bytes_for_a_later_save() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("locks"), b"not a directory").unwrap();
        let mut app = save_app(directory.path().to_owned());
        let repository = Ok(SaveRepository::new(FilesystemStorage::new(
            directory.path(),
        )));
        let original = app.world().resource::<OriginalPuzzleImage>();
        let reply = run_request(
            &repository,
            Request::Import(original.hash, original.encoded.clone().unwrap()),
        );
        let (tx, rx) = crossbeam::channel::unbounded();
        app.world_mut().resource_mut::<PersistenceService>().rx = rx;
        tx.send((0, reply)).unwrap();
        app.update();
        assert!(app
            .world()
            .resource::<OriginalPuzzleImage>()
            .encoded
            .is_some());
        assert!(app
            .world()
            .resource::<OriginalPuzzleImage>()
            .image_lease
            .is_none());
        assert!(matches!(
            app.world().resource::<PersistenceState>().error,
            Some(PersistenceError::ImageImport(SaveError::Storage(_)))
        ));
    }

    fn request_and_wait(app: &mut App, automatic: bool) {
        request_and_wait_with_storage(app, automatic, None);
    }

    #[test]
    fn host_images_survive_import_and_saves_but_offline_images_are_released() {
        for retain in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = save_app(dir.path().to_path_buf());
            app.world_mut()
                .resource_mut::<PersistenceState>()
                .retain_image_for_host = retain;
            let original = app.world().resource::<OriginalPuzzleImage>();
            let hash = original.hash;
            let bytes = original.encoded.as_ref().unwrap().clone();
            let repo = Ok(SaveRepository::new(FilesystemStorage::new(dir.path())));
            // Apply a real import reply, including the shared image lease.
            let imported = run_request(&repo, Request::Import(hash, bytes.clone()));
            let (tx, rx) = crossbeam::channel::unbounded();
            let old_rx = std::mem::replace(
                &mut app.world_mut().resource_mut::<PersistenceService>().rx,
                rx,
            );
            tx.send((0, imported)).unwrap();
            app.update();
            assert_eq!(
                app.world()
                    .resource::<OriginalPuzzleImage>()
                    .encoded
                    .is_some(),
                retain
            );
            assert!(app
                .world()
                .resource::<OriginalPuzzleImage>()
                .image_lease
                .is_some());
            app.world_mut().resource_mut::<PersistenceService>().rx = old_rx;
            // Put the source back so this is also a real successful save/import.
            app.world_mut()
                .resource_mut::<OriginalPuzzleImage>()
                .encoded = Some(bytes.clone());
            request_and_wait(&mut app, false);
            assert!(app.world().resource::<PersistenceState>().error.is_none());
            let image = app.world().resource::<OriginalPuzzleImage>();
            assert_eq!(image.encoded.is_some(), retain);
            assert!(image.image_lease.is_some());
            if retain {
                assert!(Arc::ptr_eq(image.encoded.as_ref().unwrap(), &bytes));
            }
            let id = app
                .world()
                .resource::<PersistenceState>()
                .current_save
                .as_ref()
                .unwrap()
                .id;
            let limits = ImageDecodeLimits {
                max_texture_dimension: 128,
            };
            let request = if retain {
                Request::LoadForHost(id, limits)
            } else {
                Request::Load(id, limits)
            };
            let Reply::Loaded(_, Ok(loaded)) = run_request(&repo, request) else {
                panic!("load failed");
            };
            assert_eq!(loaded.encoded.is_some(), retain);
            if retain {
                assert_eq!(loaded.encoded.as_deref().unwrap(), bytes.as_ref());
            }
            // Apply the worker's reply through the actual main-thread restore path.
            app.world_mut().insert_resource(State::new(AppState::Menu));
            let (tx, rx) = crossbeam::channel::unbounded();
            app.world_mut().resource_mut::<PersistenceService>().rx = rx;
            tx.send((0, Reply::Loaded(id, Ok(loaded)))).unwrap();
            app.update();
            assert_eq!(
                app.world()
                    .resource::<OriginalPuzzleImage>()
                    .encoded
                    .is_some(),
                retain
            );
            assert!(app
                .world()
                .resource::<OriginalPuzzleImage>()
                .image_lease
                .is_some());
        }
    }

    fn request_and_wait_with_storage(
        app: &mut App,
        automatic: bool,
        storage: Option<(
            &executor::StorageRequests,
            &FilesystemStorage,
            Option<SaveId>,
        )>,
    ) {
        app.world_mut()
            .resource_scope(|world, service: Mut<PersistenceService>| {
                let limit = world
                    .resource::<AutosaveSettingsState>()
                    .current
                    .max_saves_per_game;
                let mut state = world.resource_mut::<PersistenceState>();
                let title = SaveTitle::new("Puzzle title").unwrap();
                if automatic {
                    service.request_autosave(&mut state, title, limit);
                } else {
                    service.request_save(&mut state, title);
                }
                assert!(state.busy);
                assert_eq!(state.autosaving, automatic);
            });
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.world().resource::<PersistenceState>().busy {
            if let Some((inbox, backend, fail_delete)) = storage {
                while let Ok(request) = inbox.try_recv() {
                    if matches!(request.operation, StorageOperation::Delete(StorageKey::Save(id)) if Some(id) == fail_delete)
                    {
                        request
                            .reply
                            .complete(Err(StorageError::Io("injected deletion failure".into())))
                            .unwrap();
                    } else {
                        request.execute(backend).unwrap();
                    }
                }
            }
            app.update();
            assert!(Instant::now() < deadline, "save did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!app.world().resource::<PersistenceState>().autosaving);
    }

    #[test]
    fn autosave_rotates_its_history_preserves_manual_save_and_reports_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let repo = SaveRepository::new(FilesystemStorage::new(dir.path()));
        let mut app = save_app(dir.path().to_owned());
        request_and_wait(&mut app, false);
        let manual = app
            .world()
            .resource::<PersistenceState>()
            .current_save
            .clone()
            .unwrap();
        assert!(!manual.is_autosave);
        let original = repo.read_save(manual.id).unwrap();
        app.world_mut().resource_mut::<PieceDataStore>().states[0]
            .position
            .x += 1.0;
        request_and_wait(&mut app, true);
        let automatic = app
            .world()
            .resource::<PersistenceState>()
            .current_autosave
            .clone()
            .unwrap();
        assert!(automatic.is_autosave);
        assert_eq!(automatic.game_id, manual.game_id);
        assert_ne!(manual.id, automatic.id);
        assert_eq!(
            app.world()
                .resource::<PersistenceState>()
                .current_save
                .as_ref(),
            Some(&manual)
        );
        assert_eq!(repo.read_save(manual.id).unwrap(), original);
        request_and_wait(&mut app, true);
        let updated = app
            .world()
            .resource::<PersistenceState>()
            .current_autosave
            .clone()
            .unwrap();
        assert_ne!(updated.id, automatic.id);
        assert_eq!(updated.revision, automatic.revision + 1);
        assert!(matches!(
            repo.read_save(automatic.id),
            Err(SaveError::Storage(StorageError::NotFound(_)))
        ));
        assert_eq!(repo.list().unwrap().len(), 2);
        let saved = repo.read_save(updated.id).unwrap();
        repo.update_as(
            updated.id,
            updated.revision,
            updated.title.clone(),
            saved.checkpoint,
            None,
            true,
        )
        .unwrap();
        request_and_wait(&mut app, true);
        let state = app.world().resource::<PersistenceState>();
        assert!(matches!(
            state.autosave_error,
            Some(PersistenceError::Save(SaveError::Conflict { .. }))
        ));
        assert!(state.error.is_none());
        assert_eq!(state.current_autosave.as_ref(), Some(&updated));
        assert_eq!(repo.read_save(manual.id).unwrap(), original);
    }

    #[test]
    fn loading_autosave_rotates_history_and_manual_save_creates_a_separate_save() {
        let dir = tempfile::tempdir().unwrap();
        let repo = SaveRepository::new(FilesystemStorage::new(dir.path()));
        let mut app = save_app(dir.path().to_owned());
        request_and_wait(&mut app, true);
        let automatic = app
            .world()
            .resource::<PersistenceState>()
            .current_autosave
            .clone()
            .unwrap();
        app.insert_resource(State::new(AppState::Menu));
        app.world_mut()
            .resource_scope(|world, service: Mut<PersistenceService>| {
                let mut state = world.resource_mut::<PersistenceState>();
                state.current_autosave = None;
                state.game_id = GameId::default();
                service.load(
                    &mut state,
                    automatic.id,
                    ImageDecodeLimits {
                        max_texture_dimension: 8192,
                    },
                );
            });
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.world().resource::<PersistenceState>().busy {
            app.update();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let state = app.world().resource::<PersistenceState>();
        assert_eq!(state.current_save.as_ref(), Some(&automatic));
        assert_eq!(state.current_autosave.as_ref(), Some(&automatic));
        assert_eq!(state.game_id, automatic.game_id);
        request_and_wait(&mut app, true);
        assert_eq!(
            app.world()
                .resource::<PersistenceState>()
                .current_save
                .as_ref()
                .unwrap()
                .revision,
            2
        );
        request_and_wait(&mut app, false);
        let manual = app
            .world()
            .resource::<PersistenceState>()
            .current_save
            .clone()
            .unwrap();
        assert!(!manual.is_autosave);
        assert_eq!(manual.game_id, automatic.game_id);
        assert_ne!(manual.id, automatic.id);
        assert!(matches!(
            repo.read_save(automatic.id),
            Err(SaveError::Storage(StorageError::NotFound(_)))
        ));
        let latest = app
            .world()
            .resource::<PersistenceState>()
            .current_autosave
            .as_ref()
            .unwrap();
        assert!(repo.read_save(latest.id).unwrap().metadata.is_autosave);
        assert_eq!(repo.list().unwrap().len(), 2);
    }

    #[test]
    fn cleanup_failure_keeps_the_latest_metadata_and_recovers_next_interval() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FilesystemStorage::new(dir.path());
        let repo = SaveRepository::new(FilesystemStorage::new(dir.path()));
        let mut app = save_app(dir.path().to_owned());
        let (service, inbox) = PersistenceService::with_storage_requests();
        app.insert_resource(service);
        request_and_wait_with_storage(&mut app, true, Some((&inbox, &backend, None)));
        let first = app
            .world()
            .resource::<PersistenceState>()
            .current_autosave
            .clone()
            .unwrap();
        app.world_mut()
            .resource_mut::<PersistenceState>()
            .current_save = Some(first.clone());
        request_and_wait_with_storage(&mut app, true, Some((&inbox, &backend, Some(first.id))));
        let state = app.world().resource::<PersistenceState>();
        let latest = state.current_autosave.clone().unwrap();
        assert_ne!(latest.id, first.id);
        assert_eq!(state.current_save.as_ref(), Some(&latest));
        assert!(matches!(
            state.autosave_error,
            Some(PersistenceError::AutosaveRotation(_))
        ));
        assert_eq!(repo.list().unwrap().len(), 2);
        request_and_wait_with_storage(&mut app, true, Some((&inbox, &backend, None)));
        let state = app.world().resource::<PersistenceState>();
        assert!(state.autosave_error.is_none());
        assert_eq!(repo.list().unwrap().len(), 1);
        assert_eq!(
            repo.list().unwrap()[0].id,
            state.current_autosave.as_ref().unwrap().id
        );
    }

    #[test]
    fn manual_resume_preserves_game_id_and_rotates_existing_history_with_a_new_limit() {
        let dir = tempfile::tempdir().unwrap();
        let repo = SaveRepository::new(FilesystemStorage::new(dir.path()));
        let mut app = save_app(dir.path().to_owned());
        app.world_mut()
            .resource_mut::<AutosaveSettingsState>()
            .set_max_saves_per_game(NonZeroU32::new(3).unwrap());
        request_and_wait(&mut app, false);
        let manual = app
            .world()
            .resource::<PersistenceState>()
            .current_save
            .clone()
            .unwrap();
        request_and_wait(&mut app, true);
        request_and_wait(&mut app, true);
        let old_latest = app
            .world()
            .resource::<PersistenceState>()
            .current_autosave
            .clone()
            .unwrap();
        let mut resumed = save_app(dir.path().to_owned());
        resumed.insert_resource(State::new(AppState::Menu));
        assert_ne!(
            resumed.world().resource::<PersistenceState>().game_id,
            manual.game_id
        );
        resumed
            .world_mut()
            .resource_scope(|world, service: Mut<PersistenceService>| {
                service.load(
                    &mut world.resource_mut::<PersistenceState>(),
                    manual.id,
                    ImageDecodeLimits {
                        max_texture_dimension: 8192,
                    },
                );
            });
        let deadline = Instant::now() + Duration::from_secs(10);
        while resumed.world().resource::<PersistenceState>().busy {
            resumed.update();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let state = resumed.world().resource::<PersistenceState>();
        assert_eq!(state.game_id, manual.game_id);
        assert!(state.current_autosave.is_none());
        resumed
            .world_mut()
            .resource_mut::<AutosaveSettingsState>()
            .set_max_saves_per_game(NonZeroU32::new(2).unwrap());
        request_and_wait(&mut resumed, true);
        assert!(resumed
            .world()
            .resource::<PersistenceState>()
            .autosave_error
            .is_none());
        assert_eq!(repo.list().unwrap().len(), 3);
        assert_eq!(repo.read_save(old_latest.id).unwrap().metadata, old_latest);
        assert_eq!(repo.read_save(manual.id).unwrap().metadata, manual);
    }

    #[test]
    fn capture_failure_clears_autosaving_and_is_visible_without_a_dialog() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = save_app(dir.path().to_owned());
        app.world_mut().remove_resource::<OriginalPuzzleImage>();
        request_and_wait(&mut app, true);
        let state = app.world().resource::<PersistenceState>();
        assert!(matches!(
            state.autosave_error,
            Some(PersistenceError::DefinitionUnavailable)
        ));
        assert!(state.current_autosave.is_none());
    }

    #[test]
    fn thumbnail_worker_rejects_unsupported_formats_in_verified_containers() {
        let dir = tempfile::tempdir().unwrap();
        let repo = SaveRepository::new(FilesystemStorage::new(dir.path()));
        let bytes = b"P6\n1 1\n255\n\x49\x64\xb5";
        let hash = image_hash(bytes);
        repo.import_image(hash, bytes).unwrap();
        assert!(matches!(
            run_thumbnail(&Ok(repo), hash),
            Err(SaveError::Decode(reason))
                if reason == "The image format Pnm is not supported"
        ));
    }

    #[test]
    fn load_and_thumbnail_reject_oversized_sources_in_verified_containers() {
        let dir = tempfile::tempdir().unwrap();
        let repo = SaveRepository::new(FilesystemStorage::new(dir.path()));
        // Pixel data must never be decoded even though the container hash is valid.
        let bytes = crate::asset_reader::tests::image_with_claimed_dimensions(
            image::ImageFormat::Gif,
            24000,
            16000,
        );
        let hash = image_hash(&bytes);
        let definition = PuzzleDefinition {
            generator_version: puzzella_core::GENERATOR_VERSION,
            seed: 42,
            grid_size: UVec2::ONE,
            image_size: UVec2::new(16384, 10923),
            snap_distance: 5.0,
        };
        let mut store = PieceDataStore::default();
        store.initialize(vec![Vec2::ZERO]);
        let checkpoint = PuzzleCheckpoint::capture(&store, &definition, hash).unwrap();
        let metadata = repo
            .create(
                SaveTitle::new("Oversized source").unwrap(),
                checkpoint,
                Some(&bytes),
            )
            .unwrap();
        let repository = Ok(repo);
        assert!(
            matches!(run_thumbnail(&repository, hash), Err(SaveError::Decode(reason)) if reason == "Image size exceeds limit")
        );
        for cap in [128, 16384] {
            assert!(
                matches!(run_request(&repository, Request::Load(metadata.id, ImageDecodeLimits { max_texture_dimension: cap })), Reply::Loaded(_, Err(SaveError::Decode(reason))) if reason == "Image size exceeds limit")
            );
        }
    }

    #[test]
    fn thumbnail_worker_reads_only_the_image_and_keeps_foreground_replies_separate() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            800,
            400,
            image::Rgba([73, 100, 181, 128]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        let bytes = bytes.into_inner();
        let hash = image_hash(&bytes);
        let (service, inbox) = PersistenceService::with_storage_requests();
        let mut state = PersistenceState {
            generation: 7,
            ..default()
        };
        service.request_thumbnail(state.generation, hash).unwrap();
        assert!(!state.busy);
        let receive = || {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(request) = inbox.try_recv() {
                    break request;
                }
                assert!(Instant::now() < deadline, "Missing storage request");
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        let request = receive();
        assert!(
            matches!(request.operation, StorageOperation::Read(StorageKey::Image(h)) if h == hash)
        );
        service.list(&mut state);
        request
            .reply
            .complete(Ok(StorageValue::Bytes(PuzImage::encode(&bytes).unwrap())))
            .unwrap();
        let reply = service
            .thumbnails
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert_eq!(reply.generation, 7);
        assert_eq!(reply.hash, hash);
        let image = reply.result.unwrap();
        assert_eq!(image.size, [224, 112]);
        assert_eq!(image.rgba.len(), 224 * 112 * 4);
        assert_eq!(&image.rgba[..4], &[73, 100, 181, 128]);
        assert!(state.busy);
        assert!(state.error.is_none());
        assert!(service.rx.try_recv().is_err());
        let request = receive();
        assert!(matches!(
            request.operation,
            StorageOperation::List(StorageNamespace::Saves)
        ));
        request
            .reply
            .complete(Ok(StorageValue::Keys(Vec::new())))
            .unwrap();
        assert!(
            matches!(service.rx.recv_timeout(Duration::from_secs(10)).unwrap().1, Reply::Listed(Ok(entries)) if entries.is_empty())
        );
    }

    #[test]
    fn pending_thumbnail_read_does_not_delay_load_or_save() {
        let dir = tempfile::tempdir().unwrap();
        let backend = FilesystemStorage::new(dir.path());
        let repo = SaveRepository::new(backend.clone());
        let mut app = save_app(dir.path().to_owned());
        request_and_wait(&mut app, false);
        let metadata = app
            .world()
            .resource::<PersistenceState>()
            .current_save
            .clone()
            .unwrap();
        let checkpoint = repo.read_save(metadata.id).unwrap().checkpoint;
        let hash = checkpoint.image_hash;
        let (service, inbox) = PersistenceService::with_storage_requests();
        let mut state = PersistenceState {
            generation: 7,
            ..default()
        };
        service.request_thumbnail(state.generation, hash).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let pending_thumbnail = loop {
            if let Ok(request) = inbox.try_recv() {
                break request;
            }
            assert!(Instant::now() < deadline, "Missing thumbnail read");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(matches!(
            pending_thumbnail.operation,
            StorageOperation::Read(StorageKey::Image(h)) if h == hash
        ));
        // Retain its reply like a slow asynchronous read. Foreground requests
        // must finish without completing this read, even for the same image.
        let foreground_reply = || {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                while let Ok(request) = inbox.try_recv() {
                    request.execute(&backend).unwrap();
                }
                if let Ok(reply) = service.rx.try_recv() {
                    break reply;
                }
                assert!(
                    Instant::now() < deadline,
                    "Thumbnail blocked foreground work"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        service.load(
            &mut state,
            metadata.id,
            ImageDecodeLimits {
                max_texture_dimension: 8192,
            },
        );
        let (generation, reply) = foreground_reply();
        assert_eq!(generation, 7);
        let Reply::Loaded(id, Ok(loaded)) = reply else {
            panic!("Load failed while thumbnail read was pending")
        };
        assert_eq!(id, metadata.id);
        assert_eq!(loaded.metadata, metadata);
        assert_eq!(loaded.hash, hash);
        assert_eq!(
            PuzzleCheckpoint::capture(&loaded.restored.store, &loaded.restored.definition, hash)
                .unwrap(),
            checkpoint
        );
        assert!(service.thumbnails.try_recv().is_err());
        state.busy = false;
        service.save(
            &mut state,
            SaveTitle::new("While thumbnail is pending").unwrap(),
            checkpoint.clone(),
            None,
        );
        let (generation, reply) = foreground_reply();
        assert_eq!(generation, 7);
        let Reply::Saved(false, Ok((saved, _lease))) = reply else {
            panic!("Save failed while thumbnail read was pending")
        };
        assert_eq!(
            repo.read_save(saved.metadata.id).unwrap().checkpoint,
            checkpoint
        );
        assert!(service.thumbnails.try_recv().is_err());
        pending_thumbnail.execute(&backend).unwrap();
        let reply = service
            .thumbnails
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert_eq!(reply.generation, 7);
        assert_eq!(reply.hash, hash);
        assert!(reply.result.is_ok());
    }

    #[test]
    fn storage_initialization_failure_reaches_both_workers() {
        let error = StorageError::Unavailable("No save directory".into());
        let failure = error.clone();
        let service = PersistenceService::spawn_filesystem(move || Err(failure));
        let mut state = PersistenceState::default();
        service.list(&mut state);
        service.request_thumbnail(0, ImageHash([0; 32])).unwrap();
        assert!(matches!(
            service.rx.recv_timeout(Duration::from_secs(10)).unwrap().1,
            Reply::Listed(Err(SaveError::Storage(actual))) if actual == error
        ));
        assert!(matches!(
            service.thumbnails.recv_timeout(Duration::from_secs(10)).unwrap().result,
            Err(SaveError::Storage(actual)) if actual == error
        ));
    }

    #[test]
    fn thumbnail_errors_do_not_change_the_save_list_or_restore_a_puzzle() {
        let (service, inbox) = PersistenceService::with_storage_requests();
        let hash = ImageHash([9; 32]);
        service.request_thumbnail(0, hash).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let request = loop {
            if let Ok(request) = inbox.try_recv() {
                break request;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        };
        // A container with different original bytes must fail verification.
        request
            .reply
            .complete(Ok(StorageValue::Bytes(
                PuzImage::encode(b"bad image").unwrap(),
            )))
            .unwrap();
        assert!(matches!(
            service
                .thumbnails
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .result,
            Err(SaveError::CorruptImage(_))
        ));
        assert!(service.rx.try_recv().is_err());
    }

    #[test]
    fn owner_executor_can_reply_asynchronously_and_report_disconnects() {
        let (service, inbox) = PersistenceService::with_storage_requests();
        let mut state = PersistenceState::default();
        service.list(&mut state);
        let receive = || {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(request) = inbox.try_recv() {
                    break request;
                }
                assert!(Instant::now() < deadline, "Missing storage request");
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        let request = receive();
        assert!(matches!(
            request.operation,
            StorageOperation::List(StorageNamespace::Saves)
        ));
        // Dispatch returns to the owner event loop; an API callback owns reply.
        let callback = request.reply;
        assert!(service.rx.try_recv().is_err());
        callback
            .complete(Ok(StorageValue::Keys(Vec::new())))
            .unwrap();
        let (generation, reply) = service.rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(generation, state.generation);
        assert!(matches!(reply, Reply::Listed(Ok(entries)) if entries.is_empty()));
        state.busy = false;
        service.list(&mut state);
        drop(receive());
        assert!(matches!(
            service.rx.recv_timeout(Duration::from_secs(10)).unwrap().1,
            Reply::Listed(Err(SaveError::Storage(StorageError::Unavailable(_))))
        ));
        state.busy = false;
        drop(inbox);
        service.list(&mut state);
        assert!(matches!(
            service.rx.recv_timeout(Duration::from_secs(10)).unwrap().1,
            Reply::Listed(Err(SaveError::Storage(StorageError::Unavailable(_))))
        ));
    }
}
