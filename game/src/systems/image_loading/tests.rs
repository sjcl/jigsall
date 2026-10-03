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
