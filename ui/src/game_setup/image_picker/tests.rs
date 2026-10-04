use super::*;
use bevy::state::app::StatesPlugin;
use jigsall_game::resources::{AppState, ImageLoadChannels};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Selection {
    result: Option<Option<PathBuf>>,
    waker: Option<Waker>,
}

fn begin_selection(app: &mut App) -> Arc<Mutex<Selection>> {
    let selection = Arc::new(Mutex::new(Selection::default()));
    let pending = selection.clone();
    app.world_mut()
        .resource_mut::<ImagePicker>()
        .start(std::future::poll_fn(move |cx| {
            let mut selection = pending.lock().unwrap();
            if let Some(result) = selection.result.take() {
                Poll::Ready(result)
            } else {
                selection.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }));
    selection
}

fn complete(selection: &Mutex<Selection>, result: Option<PathBuf>) {
    let mut selection = selection.lock().unwrap();
    selection.result = Some(result);
    if let Some(waker) = selection.waker.take() {
        waker.wake();
    }
}

#[derive(Resource, Default)]
struct Frames(usize);

fn app() -> App {
    let mut app = App::new();
    let (tx_results, rx_results) = crossbeam::channel::unbounded();
    app.add_plugins((MinimalPlugins, StatesPlugin))
        .init_state::<AppState>()
        .init_resource::<ImagePicker>()
        .init_resource::<ExternalFileRegistry>()
        .init_resource::<Frames>()
        .insert_resource(PuzzleImageLimits {
            device_max_dimension: 8192,
            gpu_memory_bytes: Some(8 * 1024 * 1024 * 1024),
        })
        .insert_resource(ImageSettingsState::load(None))
        .insert_resource(ImageLoadSender { tx_results })
        .insert_resource(ImageLoadChannels { rx_results })
        .insert_resource(PuzzleConfig {
            image_path: "previous.png".into(),
            ..default()
        })
        .insert_resource(PuzzleImage {
            handle: Handle::default(),
            logical_size: UVec2::new(100, 80),
            texture_size: UVec2::new(100, 80),
            opaque: true,
        })
        .insert_resource(OriginalPuzzleImage {
            hash: jigsall_game::persistence::image_hash(b"previous image"),
            encoded: Some(Arc::from(b"previous image".as_slice())),
            image_lease: None,
        })
        .add_systems(
            Update,
            (finish_image_selection, |mut frames: ResMut<Frames>| {
                frames.0 += 1;
            }),
        )
        .add_systems(OnExit(AppState::GameSetup), discard_image_selection);
    app.update();
    transition(&mut app, AppState::GameSetup);
    app
}

fn transition(app: &mut App, state: AppState) {
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(state);
    app.update();
    assert_eq!(*app.world().resource::<State<AppState>>().get(), state);
}

fn finish(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.world().resource::<ImagePicker>().is_open() {
        assert!(Instant::now() < deadline, "selection did not finish");
        app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn assert_previous_image(app: &App) {
    assert_eq!(
        app.world().resource::<PuzzleConfig>().image_path,
        "previous.png"
    );
    assert_eq!(
        app.world().resource::<PuzzleImage>().logical_size,
        UVec2::new(100, 80)
    );
    assert_eq!(
        app.world().resource::<OriginalPuzzleImage>().hash,
        jigsall_game::persistence::image_hash(b"previous image")
    );
    assert!(app.world().resource::<ExternalFileRegistry>().is_empty());
    assert!(app
        .world()
        .resource::<ImageLoadChannels>()
        .rx_results
        .try_recv()
        .is_err());
}

#[test]
fn pending_dialog_keeps_frames_running_and_cancel_preserves_image() {
    let mut app = app();
    let selection = begin_selection(&mut app);
    let before = app.world().resource::<Frames>().0;
    for _ in 0..32 {
        app.update();
    }
    assert_eq!(app.world().resource::<Frames>().0, before + 32);
    assert_previous_image(&app);

    // A repeated request cannot replace the still-open dialog.
    app.world_mut()
        .resource_mut::<ImagePicker>()
        .start(std::future::ready(Some(PathBuf::from("unexpected.png"))));
    for _ in 0..8 {
        app.update();
    }
    assert!(app.world().resource::<ImagePicker>().is_open());
    assert_previous_image(&app);

    complete(&selection, None);
    finish(&mut app);
    assert_previous_image(&app);

    let reopened = begin_selection(&mut app);
    assert!(app.world().resource::<ImagePicker>().is_open());
    complete(&reopened, None);
    finish(&mut app);
}

#[test]
fn selected_file_uses_existing_decode_and_original_image_pipeline() {
    let mut app = app();
    app.insert_resource(ImageLoadError {
        virtual_key: "previous.png".into(),
        reason: "previous failure".into(),
    });
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("selected.png");
    image::RgbImage::from_pixel(2, 3, image::Rgb([23, 45, 67]))
        .save(&path)
        .unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let selection = begin_selection(&mut app);
    complete(&selection, Some(path.clone()));
    finish(&mut app);

    let key = &app.world().resource::<PuzzleConfig>().image_path;
    assert_eq!(
        app.world()
            .resource::<ExternalFileRegistry>()
            .resolve_path(key),
        Some(path)
    );
    assert!(!app.world().contains_resource::<PuzzleImage>());
    assert!(!app.world().contains_resource::<OriginalPuzzleImage>());
    assert!(!app.world().contains_resource::<ImageLoadError>());
    let result = app
        .world()
        .resource::<ImageLoadChannels>()
        .rx_results
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert_eq!(&result.virtual_key, key);
    assert_eq!(result.image.unwrap().image.size(), UVec2::new(2, 3));
    let original = result.original.unwrap();
    assert_eq!(original.hash, jigsall_game::persistence::image_hash(&bytes));
    assert_eq!(original.encoded.unwrap().as_ref(), bytes.as_slice());
}

#[test]
fn replacing_image_releases_previous_path_and_keeps_worker_results() {
    let mut app = app();
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("first.png");
    let second_path = directory.path().join("second.png");
    for (path, height) in [(&first_path, 3), (&second_path, 4)] {
        image::RgbImage::from_pixel(2, height, image::Rgb([23, 45, 67]))
            .save(path)
            .unwrap();
    }
    let selection = begin_selection(&mut app);
    complete(&selection, Some(first_path.clone()));
    finish(&mut app);
    let first_key = app.world().resource::<PuzzleConfig>().image_path.clone();

    let cancelled = begin_selection(&mut app);
    complete(&cancelled, None);
    finish(&mut app);
    assert_eq!(app.world().resource::<PuzzleConfig>().image_path, first_key);
    assert_eq!(
        app.world()
            .resource::<ExternalFileRegistry>()
            .resolve_path(&first_key),
        Some(first_path)
    );

    let replacement = begin_selection(&mut app);
    complete(&replacement, Some(second_path.clone()));
    finish(&mut app);
    let second_key = app.world().resource::<PuzzleConfig>().image_path.clone();
    let registry = app.world().resource::<ExternalFileRegistry>();
    assert_ne!(second_key, first_key);
    assert_eq!(registry.resolve_path(&first_key), None);
    assert_eq!(registry.resolve_path(&second_key), Some(second_path));
    assert_eq!(
        registry.get_original_filename(&second_key).as_deref(),
        Some("second.png")
    );

    // Workers own their source paths even after the registry releases a mapping.
    let results = &app.world().resource::<ImageLoadChannels>().rx_results;
    let loaded: std::collections::HashMap<_, _> = (0..2)
        .map(|_| {
            let result = results.recv_timeout(Duration::from_secs(5)).unwrap();
            (result.virtual_key, result.image.unwrap().image.size())
        })
        .collect();
    assert_eq!(loaded.get(&first_key), Some(&UVec2::new(2, 3)));
    assert_eq!(loaded.get(&second_key), Some(&UVec2::new(2, 4)));
}

#[test]
fn cancelled_retry_preserves_image_load_error() {
    let mut app = app();
    app.insert_resource(ImageLoadError {
        virtual_key: "previous.png".into(),
        reason: "previous failure".into(),
    });
    let selection = begin_selection(&mut app);
    complete(&selection, None);
    finish(&mut app);

    assert_previous_image(&app);
    let error = app.world().resource::<ImageLoadError>();
    assert_eq!(error.virtual_key, "previous.png");
    assert_eq!(error.reason, "previous failure");
}

#[test]
fn leaving_setup_discards_late_result_even_after_reentering() {
    let mut app = app();
    let selection = begin_selection(&mut app);
    transition(&mut app, AppState::Menu);
    transition(&mut app, AppState::GameSetup);
    assert!(app.world().resource::<ImagePicker>().is_open());
    complete(&selection, Some(PathBuf::from("stale.png")));
    finish(&mut app);
    assert_previous_image(&app);

    let reopened = begin_selection(&mut app);
    assert!(app.world().resource::<ImagePicker>().is_open());
    complete(&reopened, None);
    finish(&mut app);
}
