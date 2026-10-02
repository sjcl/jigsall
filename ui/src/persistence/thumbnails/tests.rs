use super::*;
use crate::persistence::{paint_load_dialog, SaveDialogs};
use puzzella_game::persistence::{
    executor::{StorageOperation, StorageRequests},
    runtime::{PersistenceState, ThumbnailImage},
    SaveError, SaveId, SaveListEntry, SaveMetadata, SaveSummary, SaveTitle, StorageError,
    StorageKey,
};
use std::time::{Duration, Instant};

fn hash(index: u32) -> ImageHash {
    let mut bytes = [0; 32];
    bytes[..4].copy_from_slice(&index.to_le_bytes());
    ImageHash(bytes)
}

fn state() -> PersistenceState {
    let mut state = PersistenceState::default();
    state.entries = (0..100)
        .map(|index| SaveListEntry {
            id: SaveId(index as u128),
            summary: Ok(SaveSummary {
                metadata: SaveMetadata {
                    id: SaveId(index as u128),
                    title: SaveTitle::new(&format!("Puzzle {index}")).unwrap(),
                    revision: 1,
                    created_at: 0,
                    updated_at: 0,
                },
                image_hash: hash(index),
                piece_count: 1000,
                placed_count: 250,
            }),
        })
        .collect();
    state
}

fn frame(
    ctx: &egui::Context,
    thumbnails: &mut SaveThumbnails,
    dialogs: &mut SaveDialogs,
    state: &mut PersistenceState,
    service: &PersistenceService,
    size: egui::Vec2,
    scroll: f32,
) -> egui::FullOutput {
    ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events: vec![egui::Event::PointerMoved(size.to_pos2() * 0.5)],
            ..Default::default()
        },
        |ui| {
            crate::theme::prepare(ui.ctx());
            thumbnails.begin_frame(ui.ctx(), service, state.generation, dialogs.load_open);
            // Feed the same per-frame scroll input consumed by ScrollArea.
            ui.ctx()
                .input_mut(|input| input.smooth_scroll_delta.y = scroll);
            paint_load_dialog(ui.ctx(), dialogs, state, service, thumbnails);
        },
    )
}

fn receive(inbox: &StorageRequests) -> puzzella_game::persistence::executor::StorageRequest {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(request) = inbox.try_recv() {
            return request;
        }
        assert!(Instant::now() < deadline, "No thumbnail request");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn real_load_dialog_requests_only_visible_images_after_scroll() {
    let ctx = egui::Context::default();
    let (service, inbox) = PersistenceService::with_storage_requests();
    let mut thumbnails = SaveThumbnails::default();
    let mut dialogs = SaveDialogs {
        load_open: true,
        ..Default::default()
    };
    let mut state = state();
    let size = egui::vec2(1280.0, 720.0);
    for _ in 0..3 {
        frame(
            &ctx,
            &mut thumbnails,
            &mut dialogs,
            &mut state,
            &service,
            size,
            0.0,
        )
        .drop_without_applying_deltas();
    }
    let request = receive(&inbox);
    assert!(
        matches!(request.operation, StorageOperation::Read(StorageKey::Image(h)) if h == hash(0))
    );
    assert!(!state.busy);
    assert!(inbox.try_recv().is_err(), "Only one image may be in flight");
    // Move far beyond the initially visible cards while the first read is pending.
    frame(
        &ctx,
        &mut thumbnails,
        &mut dialogs,
        &mut state,
        &service,
        size,
        -3000.0,
    )
    .drop_without_applying_deltas();
    for _ in 0..3 {
        frame(
            &ctx,
            &mut thumbnails,
            &mut dialogs,
            &mut state,
            &service,
            size,
            0.0,
        )
        .drop_without_applying_deltas();
    }
    assert!(inbox.try_recv().is_err());
    request
        .reply
        .complete(Err(StorageError::NotFound(StorageKey::Image(hash(0)))))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while thumbnails
        .in_flight
        .as_ref()
        .is_some_and(|pending| pending.hash == hash(0))
    {
        frame(
            &ctx,
            &mut thumbnails,
            &mut dialogs,
            &mut state,
            &service,
            size,
            0.0,
        )
        .drop_without_applying_deltas();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let request = receive(&inbox);
    let StorageOperation::Read(StorageKey::Image(requested)) = request.operation else {
        panic!("Unexpected request")
    };
    let index = u32::from_le_bytes(requested.0[..4].try_into().unwrap());
    assert!(
        index > 5 && index < 100,
        "Requested an offscreen card after scrolling: {index}"
    );
    assert!(inbox.try_recv().is_err());
    assert!(state.entries.iter().all(|entry| entry.summary.is_ok()));
    assert!(state.error.is_none());
}

#[test]
fn load_dialog_fits_small_windows_and_shows_loading_placeholders() {
    for size in [egui::vec2(640.0, 360.0), egui::vec2(320.0, 360.0)] {
        let ctx = egui::Context::default();
        let (service, _inbox) = PersistenceService::with_storage_requests();
        let mut thumbnails = SaveThumbnails::default();
        let mut dialogs = SaveDialogs {
            load_open: true,
            ..Default::default()
        };
        let mut state = state();
        for _ in 0..3 {
            frame(
                &ctx,
                &mut thumbnails,
                &mut dialogs,
                &mut state,
                &service,
                size,
                0.0,
            )
            .drop_without_applying_deltas();
        }
        let output = frame(
            &ctx,
            &mut thumbnails,
            &mut dialogs,
            &mut state,
            &service,
            size,
            0.0,
        );
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let panel = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.corner_radius.nw == 16 => Some(rect.rect),
                _ => None,
            })
            .unwrap();
        assert!(
            viewport.contains_rect(panel),
            "Panel exceeds window: {panel:?}"
        );
        for label in ["Loading image...", "Back to Title", "Refresh"] {
            let position = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("Missing label: {label}"));
            assert!(viewport.contains(position), "{label} exceeds window");
        }
        output.drop_without_applying_deltas();
        state.busy = true;
        dialogs.loading_save = Some(SaveId(0));
        let output = frame(
            &ctx,
            &mut thumbnails,
            &mut dialogs,
            &mut state,
            &service,
            size,
            0.0,
        );
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == "Loading puzzle...")));
        let panel = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect) if rect.corner_radius.nw == 16 => Some(rect.rect),
                _ => None,
            })
            .unwrap();
        assert!(
            viewport.contains_rect(panel),
            "Loading panel exceeds window: {panel:?}"
        );
        output.drop_without_applying_deltas();
    }
}

#[test]
fn cache_reuses_images_limits_memory_and_ignores_obsolete_replies() {
    let ctx = egui::Context::default();
    let mut thumbnails = SaveThumbnails {
        active: true,
        ..Default::default()
    };
    let image = || ThumbnailImage {
        size: [2, 1],
        rgba: vec![255; 8],
    };
    thumbnails.in_flight = Some(PendingThumbnail {
        hash: hash(0),
        generation: 0,
        epoch: 0,
    });
    thumbnails.complete(
        &ctx,
        ThumbnailReply {
            generation: 0,
            hash: hash(0),
            result: Ok(image()),
        },
    );
    assert!(matches!(
        thumbnails.cache[&hash(0)].thumbnail,
        Thumbnail::Ready(_)
    ));
    assert_eq!(
        thumbnails.next_visible(&[hash(0), hash(1)], false),
        Some(hash(1))
    );
    assert_eq!(thumbnails.next_visible(&[hash(1)], true), None);
    let Thumbnail::Ready(texture) = &thumbnails.cache[&hash(0)].thumbnail else {
        unreachable!()
    };
    let texture_id = texture.id();
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 360.0),
            )),
            ..Default::default()
        },
        |ui| thumbnails.paint(ui, hash(0), &mut Vec::new()),
    );
    let image_rect = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) if mesh.texture_id == texture_id => Some(mesh.calc_bounds()),
            _ => None,
        })
        .expect("Loaded thumbnail must paint its texture");
    assert_eq!(image_rect.size(), egui::vec2(112.0, 56.0));
    output.drop_without_applying_deltas();
    for index in 1..=CACHE_CAPACITY as u32 {
        thumbnails.frame += 1;
        thumbnails.insert(hash(index), Thumbnail::Failed("Unavailable".into()));
    }
    assert_eq!(thumbnails.cache.len(), CACHE_CAPACITY);
    assert!(!thumbnails.cache.contains_key(&hash(0)));
    // Failed previews are cached, without retrying every frame.
    assert_eq!(thumbnails.next_visible(&[hash(1)], false), None);
    thumbnails.in_flight = Some(PendingThumbnail {
        hash: hash(0),
        generation: 0,
        epoch: 0,
    });
    thumbnails.invalidate();
    assert_eq!(thumbnails.next_visible(&[hash(1)], false), None);
    thumbnails.complete(
        &ctx,
        ThumbnailReply {
            generation: 0,
            hash: hash(0),
            result: Ok(image()),
        },
    );
    assert!(thumbnails.cache.is_empty());
    assert!(thumbnails.in_flight.is_none());
    assert_eq!(thumbnails.next_visible(&[hash(1)], false), Some(hash(1)));
    thumbnails.in_flight = Some(PendingThumbnail {
        hash: hash(1),
        generation: 0,
        epoch: thumbnails.epoch,
    });
    thumbnails.generation = 1;
    thumbnails.complete(
        &ctx,
        ThumbnailReply {
            generation: 0,
            hash: hash(1),
            result: Err(SaveError::MissingImage(hash(1))),
        },
    );
    assert!(thumbnails.cache.is_empty());
}
