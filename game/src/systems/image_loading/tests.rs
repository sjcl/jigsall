use super::*;
use crate::asset_reader::{decode_image_bytes, ImageLoadResult};
use crate::persistence::runtime::{OriginalPuzzleImage, PersistenceService, PersistenceState};

fn app() -> (App, crossbeam::channel::Sender<ImageLoadResult>) {
    let mut app = App::new();
    let (sender, rx_results) = crossbeam::channel::unbounded();
    let (service, _requests) = PersistenceService::with_storage_requests();
    app.insert_resource(ImageLoadChannels { rx_results })
        .insert_resource(PuzzleConfig {
            image_path: "current.png".into(),
            ..default()
        })
        .insert_resource(service)
        .init_resource::<PersistenceState>()
        .init_resource::<Assets<Image>>()
        .add_systems(Update, handle_image_load_results);
    (app, sender)
}

#[test]
fn image_failure_paths_require_debug_logging() {
    use bevy::{
        ecs::system::RunSystemOnce,
        log::{tracing, tracing_subscriber, Level},
    };
    #[derive(Clone)]
    struct LogBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    for path in [
        r"C:\Users\private-user\Pictures\private-puzzle.png",
        "/home/private-user/Pictures/private-puzzle.png",
    ] {
        for level in [Level::INFO, Level::DEBUG] {
            let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
            let writer = LogBuffer(output.clone());
            let subscriber = tracing_subscriber::fmt()
                .with_max_level(level)
                .without_time()
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .finish();
            let (mut app, sender) = app();
            app.world_mut().resource_mut::<PuzzleConfig>().image_path = path.into();
            let reason = format!("Could not read {path}");
            sender
                .send(ImageLoadResult {
                    virtual_key: path.into(),
                    image: Err(reason.clone()),
                    original: None,
                })
                .unwrap();

            tracing::subscriber::with_default(subscriber, || {
                app.world_mut()
                    .run_system_once(handle_image_load_results)
                    .unwrap();
            });

            let output = String::from_utf8(output.lock().unwrap().clone()).unwrap();
            assert!(output.contains("Image loading failed"));
            assert_eq!(output.contains(path), level == Level::DEBUG, "{output}");
            assert_eq!(
                output.contains("Image loading failure details"),
                level == Level::DEBUG,
                "{output}"
            );
            assert_eq!(app.world().resource::<ImageLoadError>().reason, reason);
        }
    }
}

#[test]
fn current_failure_keeps_reason_and_discards_previous_image() {
    for reason in [
        decode_image_bytes(b"corrupt image").unwrap_err(),
        "Image is too large".into(),
    ] {
        let (mut app, sender) = app();
        app.insert_resource(PuzzleImage {
            handle: Handle::default(),
            size: Vec2::splat(100.0),
            opaque: true,
        })
        .insert_resource(OriginalPuzzleImage {
            hash: crate::persistence::image_hash(b"previous image"),
            encoded: None,
        });
        sender
            .send(ImageLoadResult {
                virtual_key: "current.png".into(),
                image: Err(reason.clone()),
                original: None,
            })
            .unwrap();
        app.update();
        app.update();

        let error = app.world().resource::<ImageLoadError>();
        assert_eq!(error.virtual_key, "current.png");
        assert_eq!(error.reason, reason);
        assert!(!app.world().contains_resource::<PuzzleImage>());
        assert!(!app.world().contains_resource::<OriginalPuzzleImage>());
    }
}

#[test]
fn successful_retry_clears_failure_and_sets_preview() {
    let (mut app, sender) = app();
    app.insert_resource(ImageLoadError {
        virtual_key: "current.png".into(),
        reason: "previous failure".into(),
    });
    let image = Image::default();
    let expected_size = image.size().as_vec2();
    sender
        .send(ImageLoadResult {
            virtual_key: "current.png".into(),
            image: Ok(image),
            original: None,
        })
        .unwrap();
    sender
        .send(ImageLoadResult {
            virtual_key: "old.png".into(),
            image: Err("stale failure".into()),
            original: None,
        })
        .unwrap();
    app.update();

    assert!(!app.world().contains_resource::<ImageLoadError>());
    let preview = app.world().resource::<PuzzleImage>();
    assert_eq!(preview.size, expected_size);
    assert!(app
        .world()
        .resource::<Assets<Image>>()
        .contains(&preview.handle));
}

#[test]
fn stale_results_cannot_replace_current_failure() {
    let (mut app, sender) = app();
    app.insert_resource(ImageLoadError {
        virtual_key: "current.png".into(),
        reason: "current failure".into(),
    });
    for image in [Err("stale failure".into()), Ok(Image::default())] {
        sender
            .send(ImageLoadResult {
                virtual_key: "old.png".into(),
                image,
                original: None,
            })
            .unwrap();
    }
    app.update();

    let error = app.world().resource::<ImageLoadError>();
    assert_eq!(error.virtual_key, "current.png");
    assert_eq!(error.reason, "current failure");
    assert!(!app.world().contains_resource::<PuzzleImage>());
    assert!(!app.world().contains_resource::<OriginalPuzzleImage>());
    assert!(app.world().resource::<Assets<Image>>().is_empty());
}
