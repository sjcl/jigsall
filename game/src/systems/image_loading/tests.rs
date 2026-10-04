use super::*;
use crate::asset_reader::{decode_image_bytes, DecodedPuzzleImage, ImageLoadResult};
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
    use tracing_subscriber::fmt::{writer::MutexGuardWriter, MakeWriter};

    #[derive(Clone, Default)]
    struct LogOutput(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl<'writer> MakeWriter<'writer> for LogOutput {
        type Writer = MutexGuardWriter<'writer, Vec<u8>>;

        fn make_writer(&'writer self) -> Self::Writer {
            self.0.make_writer()
        }
    }

    // Keep both dispatches alive: tracing's single-dispatch fast path can cache
    // Interest::never when another test first registers a callsite without our
    // scoped subscriber. With two dispatches, registration consults both filters.
    let captures = [Level::INFO, Level::DEBUG].map(|level| {
        let output = LogOutput::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(level)
            .without_time()
            .with_ansi(false)
            .with_writer(output.clone())
            .finish();
        (level, output, tracing::Dispatch::new(subscriber))
    });

    for path in [
        r"C:\Users\private-user\Pictures\private-puzzle.png",
        "/home/private-user/Pictures/private-puzzle.png",
    ] {
        for (level, output, dispatch) in &captures {
            output.0.lock().unwrap().clear();
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

            tracing::dispatcher::with_default(dispatch, || {
                // Register the same log callsites from a thread without a subscriber,
                // as other image-loading tests can do when the suite runs in parallel.
                std::thread::spawn(|| {
                    let (mut app, sender) = self::app();
                    sender
                        .send(ImageLoadResult {
                            virtual_key: "current.png".into(),
                            image: Err("unrelated failure".into()),
                            original: None,
                        })
                        .unwrap();
                    app.world_mut()
                        .run_system_once(handle_image_load_results)
                        .unwrap();
                })
                .join()
                .unwrap();
                app.world_mut()
                    .run_system_once(handle_image_load_results)
                    .unwrap();
            });

            let output = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
            assert!(output.contains("Image loading failed"), "{level}: {output}");
            assert_eq!(output.contains(path), *level == Level::DEBUG, "{output}");
            assert_eq!(
                output.contains("Image loading failure details"),
                *level == Level::DEBUG,
                "{output}"
            );
            assert_eq!(app.world().resource::<ImageLoadError>().reason, reason);
        }
    }
}

#[test]
fn current_failure_keeps_reason_and_discards_previous_image() {
    for reason in [
        decode_image_bytes(
            b"corrupt image",
            ImageDecodeLimits {
                max_texture_dimension: 8192,
            },
        )
        .unwrap_err(),
        "Image is too large".into(),
    ] {
        let (mut app, sender) = app();
        app.insert_resource(PuzzleImage {
            handle: Handle::default(),
            logical_size: UVec2::splat(100),
            texture_size: UVec2::splat(100),
            opaque: true,
        })
        .insert_resource(OriginalPuzzleImage {
            hash: crate::persistence::image_hash(b"previous image"),
            encoded: None,
            image_lease: None,
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
    let image = DecodedPuzzleImage {
        image: Image::default(),
        source_size: UVec2::splat(20000),
        logical_size: UVec2::splat(16384),
    };
    let expected_texture_size = image.image.size();
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
    assert_eq!(preview.logical_size, UVec2::splat(16384));
    assert_eq!(preview.texture_size, expected_texture_size);
    assert!(app
        .world()
        .resource::<Assets<Image>>()
        .contains(&preview.handle));
    app.update();
    assert_eq!(
        app.world().resource::<PuzzleImage>().logical_size,
        UVec2::splat(16384)
    );
}

#[test]
fn stale_results_cannot_replace_current_failure() {
    let (mut app, sender) = app();
    app.insert_resource(ImageLoadError {
        virtual_key: "current.png".into(),
        reason: "current failure".into(),
    });
    for image in [
        Err("stale failure".into()),
        Ok(DecodedPuzzleImage {
            image: Image::default(),
            source_size: UVec2::ONE,
            logical_size: UVec2::ONE,
        }),
    ] {
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
