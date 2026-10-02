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
    input_frame(
        ctx,
        thumbnails,
        dialogs,
        state,
        service,
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events: vec![egui::Event::PointerMoved(size.to_pos2() * 0.5)],
            ..Default::default()
        },
        scroll,
    )
}

fn input_frame(
    ctx: &egui::Context,
    thumbnails: &mut SaveThumbnails,
    dialogs: &mut SaveDialogs,
    state: &mut PersistenceState,
    service: &PersistenceService,
    input: egui::RawInput,
    scroll: f32,
) -> egui::FullOutput {
    ctx.run_ui(input, |ui| {
        crate::theme::prepare(ui.ctx());
        thumbnails.begin_frame(ui.ctx(), service, state.generation, dialogs.load_open);
        // Feed the same per-frame scroll input consumed by ScrollArea.
        ui.ctx()
            .input_mut(|input| input.smooth_scroll_delta.y = scroll);
        paint_load_dialog(ui.ctx(), dialogs, state, service, thumbnails);
    })
}

fn text_rects(output: &egui::FullOutput, label: &str) -> Vec<(egui::Rect, egui::Rect)> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == label => Some((
                text.galley.rect.translate(text.pos.to_vec2()),
                shape.clip_rect,
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn delete_button_shares_resume_row_at_the_right_edge() {
    for size in [egui::vec2(1280.0, 720.0), egui::vec2(320.0, 900.0)] {
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
        let (resume, _) = text_rects(&output, "Resume Puzzle")[0];
        let (delete, _) = text_rects(&output, "Delete save")[0];
        assert!(
            (resume.center().y - delete.center().y).abs() < 1.0,
            "Buttons are on different rows: {resume:?}, {delete:?}"
        );
        assert!(delete.left() > resume.right());
        let delete_button = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.corner_radius.nw == 10 && rect.rect.contains(delete.center()) =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .min_by(|a, b| a.height().total_cmp(&b.height()))
            .unwrap();
        assert!(
            delete_button.right() - delete.right() < 50.0,
            "Delete is not at the card's right edge"
        );
        output.drop_without_applying_deltas();
        dialogs.pending_delete = Some(SaveId(0));
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
        let (resume, _) = text_rects(&output, "Resume Puzzle")[0];
        let (caption, _) = text_rects(&output, "Permanently delete this save?")[0];
        let (delete, _) = text_rects(&output, "Delete")[0];
        let (cancel, _) = text_rects(&output, "Cancel")[0];
        assert!(
            caption.left() >= resume.right() && delete.left() >= resume.right(),
            "Confirmation must be beside Resume: {resume:?}, {caption:?}, {delete:?}"
        );
        assert!((delete.center().y - cancel.center().y).abs() < 1.0);
        assert!(cancel.left() > delete.right());
        assert!(
            caption.top() < resume.bottom(),
            "Confirmation must start in the action row"
        );
        let card = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.corner_radius.nw == 10 && rect.rect.contains(resume.center()) =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .unwrap();
        assert!(
            card.contains_rect(caption) && card.contains_rect(cancel),
            "Confirmation exceeds its card: {card:?}, {caption:?}, {cancel:?}"
        );
        assert!(card.right() - cancel.right() < 50.0);
        output.drop_without_applying_deltas();
    }
}

#[test]
fn opening_delete_confirmation_scrolls_only_when_needed() {
    for (size, last_visible) in [
        (egui::vec2(1280.0, 720.0), false),
        (egui::vec2(1280.0, 720.0), true),
        (egui::vec2(640.0, 360.0), true),
        (egui::vec2(320.0, 360.0), true),
    ] {
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
        let mut output = frame(
            &ctx,
            &mut thumbnails,
            &mut dialogs,
            &mut state,
            &service,
            size,
            0.0,
        );
        if last_visible {
            output.drop_without_applying_deltas();
            frame(
                &ctx,
                &mut thumbnails,
                &mut dialogs,
                &mut state,
                &service,
                size,
                -1000.0,
            )
            .drop_without_applying_deltas();
            // Find an action row even when the viewport is shorter than a card.
            let mut button = None;
            for _ in 0..40 {
                output = frame(
                    &ctx,
                    &mut thumbnails,
                    &mut dialogs,
                    &mut state,
                    &service,
                    size,
                    0.0,
                );
                button = text_rects(&output, "Delete save")
                    .into_iter()
                    .find(|(rect, clip)| clip.contains_rect(*rect));
                output.drop_without_applying_deltas();
                if button.is_some() {
                    break;
                }
                frame(
                    &ctx,
                    &mut thumbnails,
                    &mut dialogs,
                    &mut state,
                    &service,
                    size,
                    -20.0,
                )
                .drop_without_applying_deltas();
            }
            let (button, clip) = button.expect("No visible delete button");
            // Keep the action visible while its newly expanded section will be clipped.
            let scroll = button.center().y - (clip.bottom() - 24.0);
            frame(
                &ctx,
                &mut thumbnails,
                &mut dialogs,
                &mut state,
                &service,
                size,
                -scroll,
            )
            .drop_without_applying_deltas();
            output = frame(
                &ctx,
                &mut thumbnails,
                &mut dialogs,
                &mut state,
                &service,
                size,
                0.0,
            );
        }
        let buttons: Vec<_> = text_rects(&output, "Delete save")
            .into_iter()
            .filter(|(rect, clip)| clip.contains_rect(*rect))
            .collect();
        let position = if last_visible {
            buttons.last().unwrap()
        } else {
            &buttons[0]
        }
        .0
        .center();
        let card_top = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.corner_radius.nw == 10 && rect.rect.contains(position) =>
                {
                    Some(rect.rect.top())
                }
                _ => None,
            })
            .unwrap();
        output.drop_without_applying_deltas();
        for pressed in [true, false] {
            input_frame(
                &ctx,
                &mut thumbnails,
                &mut dialogs,
                &mut state,
                &service,
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    events: vec![
                        egui::Event::PointerMoved(position),
                        egui::Event::PointerButton {
                            pos: position,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                    ..Default::default()
                },
                0.0,
            )
            .drop_without_applying_deltas();
        }
        assert!(
            dialogs.pending_delete.is_some(),
            "Delete save must open confirmation"
        );
        for _ in 0..30 {
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
        for label in ["Permanently delete this save?", "Delete", "Cancel"] {
            let (rect, clip) = text_rects(&output, label)
                .first()
                .copied()
                .unwrap_or_else(|| panic!("Missing confirmation: {label}"));
            assert!(
                clip.contains_rect(rect),
                "Confirmation is clipped: {label}, {rect:?}, {clip:?}"
            );
        }
        let position = text_rects(&output, "Permanently delete this save?")[0]
            .0
            .center();
        let expanded_card_top = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.corner_radius.nw == 10 && rect.rect.contains(position) =>
                {
                    Some(rect.rect.top())
                }
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!(
                    "Confirmation outside cards: {position:?}, cards: {:?}",
                    output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::Shape::Rect(rect) if rect.corner_radius.nw == 10 =>
                                Some(rect.rect),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                )
            });
        if !last_visible {
            assert_eq!(
                expanded_card_top, card_top,
                "Visible confirmation should not move the scroll position"
            );
        } else {
            assert!(
                expanded_card_top < card_top,
                "Hidden confirmation must scroll into view"
            );
        }
        output.drop_without_applying_deltas();
        // Once opened, manual scrolling must remain under the user's control.
        frame(
            &ctx,
            &mut thumbnails,
            &mut dialogs,
            &mut state,
            &service,
            size,
            -1000.0,
        )
        .drop_without_applying_deltas();
        for _ in 0..30 {
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
        assert!(
            text_rects(&output, "Permanently delete this save?").is_empty(),
            "Open confirmation should not force scrolling again"
        );
        output.drop_without_applying_deltas();
    }
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
