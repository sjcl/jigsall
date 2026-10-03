//! Workers own codecs, decode and restore preparation. I/O can run on the worker
//! or be dispatched to an external storage owner's event loop.
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
    is_autosave: bool,
}

#[derive(Clone, Debug)]
pub enum PersistenceError {
    WorkerStopped,
    DefinitionUnavailable,
    Checkpoint(CheckpointError),
    ImageImport(SaveError),
    Save(SaveError),
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
    restored: RestoredPuzzle,
    metadata: SaveMetadata,
    hash: ImageHash,
    image: Image,
    opaque: bool,
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
    Load(SaveId),
    Thumbnail(ImageHash),
    Save {
        is_autosave: bool,
        update: Option<(SaveId, u64)>,
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
    Thumbnail(ImageHash, Result<ThumbnailImage, SaveError>),
    Saved(bool, Result<SaveMetadata, SaveError>),
    Deleted(Result<Vec<SaveListEntry>, SaveError>),
}
#[derive(Resource)]
pub struct PersistenceService {
    tx: crossbeam::channel::Sender<(u64, Request)>,
    rx: crossbeam::channel::Receiver<(u64, Reply)>,
    thumbnails: crossbeam::channel::Receiver<ThumbnailReply>,
}
impl Default for PersistenceService {
    fn default() -> Self {
        // Resolve the directory inside the worker; never perform filesystem I/O here.
        Self::spawn(FilesystemStorage::for_user)
    }
}
impl PersistenceService {
    /// Move a Send backend to the repository worker (e.g. filesystem).
    pub fn new<S: SaveStorage + Send + 'static>(storage: S) -> Self {
        Self::spawn(move || Ok(storage))
    }
    /// Keep thread-affine handles with their owner. The owner must poll this
    /// inbox and complete each request, including on asynchronous API failure.
    /// Only the repository worker blocks waiting for I/O replies.
    pub fn with_storage_requests() -> (Self, executor::StorageRequests) {
        let (proxy, requests) = executor::storage_channel();
        (Self::new(proxy), requests)
    }
    fn spawn<S: SaveStorage>(
        storage: impl FnOnce() -> Result<S, StorageError> + Send + 'static,
    ) -> Self {
        let (tx, requests) = crossbeam::channel::unbounded();
        let (results, rx) = crossbeam::channel::unbounded();
        let (thumbnail_results, thumbnails) = crossbeam::channel::unbounded();
        std::thread::spawn(move || {
            let repository = storage().map(SaveRepository::new);
            while let Ok((generation, request)) = requests.recv() {
                let reply = run_request(&repository, request);
                let sent = match reply {
                    Reply::Thumbnail(hash, result) => thumbnail_results
                        .send(ThumbnailReply {
                            generation,
                            hash,
                            result,
                        })
                        .is_ok(),
                    reply => results.send((generation, reply)).is_ok(),
                };
                if !sent {
                    break;
                }
            }
        });
        Self { tx, rx, thumbnails }
    }
    fn submit(&self, state: &mut PersistenceState, request: Request) {
        if state.busy {
            return;
        }
        let is_autosave = matches!(
            request,
            Request::Save {
                is_autosave: true,
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
    pub fn load(&self, state: &mut PersistenceState, id: SaveId) {
        self.submit(state, Request::Load(id));
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
        self.tx
            .send((generation, Request::Thumbnail(hash)))
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
        self.save_as(state, title, checkpoint, bytes, false);
    }
    fn save_as(
        &self,
        state: &mut PersistenceState,
        title: SaveTitle,
        checkpoint: PuzzleCheckpoint,
        bytes: Option<Arc<[u8]>>,
        is_autosave: bool,
    ) {
        let metadata = if is_autosave {
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
                is_autosave,
                update,
                title,
                checkpoint,
                bytes,
            },
        );
    }
    /// UI requests are captured after the frame's queued release commands are applied.
    pub fn request_save(&self, state: &mut PersistenceState, title: SaveTitle) {
        self.request_capture(state, title, false);
    }
    pub(crate) fn request_autosave(&self, state: &mut PersistenceState, title: SaveTitle) {
        self.request_capture(state, title, true);
    }
    fn request_capture(&self, state: &mut PersistenceState, title: SaveTitle, is_autosave: bool) {
        if state.busy {
            return;
        }
        state.capture = Some(SaveCapture { title, is_autosave });
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
    let Some(capture) = state.capture.take() else {
        return;
    };
    state.busy = false;
    state.autosaving = false;
    let (Some(definition), Some(original)) = (definition, original) else {
        save_error(
            &mut state,
            capture.is_autosave,
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
            capture.is_autosave,
        ),
        Err(error) => save_error(
            &mut state,
            capture.is_autosave,
            PersistenceError::Checkpoint(error),
        ),
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
        Request::Thumbnail(hash) => Reply::Thumbnail(
            hash,
            repo().and_then(|r| {
                let bytes = r.read_image(hash)?;
                let decoded = image::load_from_memory(&bytes)
                    .map_err(|error| SaveError::Decode(error.to_string()))?;
                let rgba = decoded
                    .thumbnail(THUMBNAIL_MAX_EDGE, THUMBNAIL_MAX_EDGE)
                    .to_rgba8();
                Ok(ThumbnailImage {
                    size: [rgba.width() as usize, rgba.height() as usize],
                    rgba: rgba.into_raw(),
                })
            }),
        ),
        Request::Delete(id) => Reply::Deleted(repo().and_then(|r| {
            r.delete(id)?;
            r.list()
        })),
        Request::Save {
            is_autosave,
            update,
            title,
            checkpoint,
            bytes,
        } => Reply::Saved(
            is_autosave,
            repo().and_then(|r| {
                let bytes = bytes.as_deref();
                match update {
                    Some((id, expected_revision)) => {
                        r.update_as(id, expected_revision, title, checkpoint, bytes, is_autosave)
                    }
                    None => r.create_as(title, checkpoint, bytes, is_autosave),
                }
            }),
        ),
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
                Ok(metadata) => {
                    if is_autosave {
                        if state
                            .current_save
                            .as_ref()
                            .is_some_and(|save| save.id == metadata.id)
                        {
                            state.current_save = Some(metadata.clone());
                        }
                        state.current_autosave = Some(metadata);
                        state.autosave_error = None;
                    } else {
                        state.current_save = Some(metadata);
                        state.title_dialog_open = false;
                        state.message = Some(PersistenceNotice::Saved);
                    }
                    if let Some(ref mut original) = original {
                        original.encoded = None;
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
                    state.current_autosave =
                        loaded.metadata.is_autosave.then(|| loaded.metadata.clone());
                    state.current_save = Some(loaded.metadata);
                    next.set(AppState::InGame);
                })
            }
            Reply::Imported(..) | Reply::Thumbnail(..) | Reply::Saved(..) => unreachable!(),
        };
        if let Err(error) = result {
            state.error = Some(PersistenceError::Save(error));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use executor::{StorageOperation, StorageValue};
    use std::time::{Duration, Instant};

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
                .map(|i| definition.correct_position(puzzella_core::PieceId(i)) + Vec2::splat(10.0))
                .collect(),
        );
        let mut app = App::new();
        app.insert_resource(PersistenceService::for_test(root))
            .init_resource::<PersistenceState>()
            .insert_resource(definition)
            .insert_resource(store)
            .insert_resource(OriginalPuzzleImage {
                hash: image_hash(&bytes),
                encoded: Some(bytes.into()),
            })
            .init_resource::<Assets<Image>>()
            .insert_resource(State::new(AppState::InGame))
            .insert_resource(NextState::<AppState>::default())
            .add_systems(Update, (capture_requested_save, poll_results).chain());
        app
    }

    fn request_and_wait(app: &mut App, automatic: bool) {
        app.world_mut()
            .resource_scope(|world, service: Mut<PersistenceService>| {
                let mut state = world.resource_mut::<PersistenceState>();
                let title = SaveTitle::new("Puzzle title").unwrap();
                if automatic {
                    service.request_autosave(&mut state, title);
                } else {
                    service.request_save(&mut state, title);
                }
                assert!(state.busy);
                assert_eq!(state.autosaving, automatic);
            });
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.world().resource::<PersistenceState>().busy {
            app.update();
            assert!(Instant::now() < deadline, "save did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!app.world().resource::<PersistenceState>().autosaving);
    }

    #[test]
    fn autosave_updates_its_own_slot_preserves_manual_save_and_reports_conflicts() {
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
            .x += 10.0;
        request_and_wait(&mut app, true);
        let automatic = app
            .world()
            .resource::<PersistenceState>()
            .current_autosave
            .clone()
            .unwrap();
        assert!(automatic.is_autosave);
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
        assert_eq!(updated.id, automatic.id);
        assert_eq!(updated.revision, automatic.revision + 1);
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
    fn loading_autosave_resumes_its_slot_and_manual_save_creates_a_separate_save() {
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
                service.load(&mut state, automatic.id);
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
        assert_ne!(manual.id, automatic.id);
        assert!(repo.read_save(automatic.id).unwrap().metadata.is_autosave);
        assert_eq!(repo.list().unwrap().len(), 2);
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
        assert!(
            matches!(request.operation, StorageOperation::Read(StorageKey::Image(h)) if h == hash)
        );
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
