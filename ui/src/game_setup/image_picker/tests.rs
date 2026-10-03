use super::*;
use bevy::state::app::StatesPlugin;
use puzzella_game::resources::{AppState, ImageLoadChannels};
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
        .insert_resource(ImageLoadSender { tx_results })
        .insert_resource(ImageLoadChannels { rx_results })
        .insert_resource(PuzzleConfig {
            image_path: "previous.png".into(),
            ..default()
        })
        .insert_resource(PuzzleImage {
            handle: Handle::default(),
            size: Vec2::new(100.0, 80.0),
            opaque: true,
        })
        .insert_resource(OriginalPuzzleImage {
            hash: puzzella_game::persistence::image_hash(b"previous image"),
            encoded: Some(Arc::from(b"previous image".as_slice())),
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
        app.world().resource::<PuzzleImage>().size,
        Vec2::new(100.0, 80.0)
    );
    assert_eq!(
        app.world().resource::<OriginalPuzzleImage>().hash,
        puzzella_game::persistence::image_hash(b"previous image")
    );
    assert!(app
        .world()
        .resource::<ExternalFileRegistry>()
        .registered_paths
        .read()
        .unwrap()
        .is_empty());
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
    let result = app
        .world()
        .resource::<ImageLoadChannels>()
        .rx_results
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert_eq!(&result.virtual_key, key);
    assert_eq!(result.image.unwrap().size(), UVec2::new(2, 3));
    let original = result.original.unwrap();
    assert_eq!(
        original.hash,
        puzzella_game::persistence::image_hash(&bytes)
    );
    assert_eq!(original.encoded.unwrap().as_ref(), bytes.as_slice());
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
