use super::*;

#[derive(Resource, Default)]
struct EventLog(Vec<GameEvent>);
fn record(mut reader: MessageReader<GameEvent>, mut log: ResMut<EventLog>) {
    log.0.extend(reader.read().cloned());
}
fn track(app: &mut App) {
    app.init_resource::<EventLog>().add_systems(Last, record);
}
fn take(app: &mut App) -> Vec<GameEvent> {
    std::mem::take(&mut app.world_mut().resource_mut::<EventLog>().0)
}

#[test]
fn autosave_events_use_capture_and_worker_results_including_partial_success() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FilesystemStorage::new(dir.path());
    let mut app = save_app(dir.path().to_owned());
    let (service, inbox) = PersistenceService::with_storage_requests();
    app.insert_resource(service);
    track(&mut app);
    request_and_wait_with_storage(&mut app, false, Some((&inbox, &backend, None)));
    assert!(take(&mut app).is_empty(), "manual saves are silent");
    request_and_wait_with_storage(&mut app, true, Some((&inbox, &backend, None)));
    assert!(matches!(
        take(&mut app).as_slice(),
        [GameEvent::AutosaveStarted, GameEvent::AutosaveSucceeded]
    ));
    let first = app
        .world()
        .resource::<PersistenceState>()
        .current_autosave
        .clone()
        .unwrap();
    request_and_wait_with_storage(&mut app, true, Some((&inbox, &backend, Some(first.id))));
    assert!(matches!(
        take(&mut app).as_slice(),
        [
            GameEvent::AutosaveStarted,
            GameEvent::AutosaveRotationFailed {
                error: PersistenceError::AutosaveRotation(_)
            }
        ]
    ));
    assert_ne!(
        app.world()
            .resource::<PersistenceState>()
            .current_autosave
            .as_ref()
            .unwrap()
            .id,
        first.id
    );
    request_and_wait_with_storage(&mut app, true, Some((&inbox, &backend, None)));
    assert!(matches!(
        take(&mut app).as_slice(),
        [GameEvent::AutosaveStarted, GameEvent::AutosaveSucceeded]
    ));
    app.update();
    assert!(
        take(&mut app).is_empty(),
        "idle frames do not repeat completion"
    );
}

#[test]
fn queued_request_is_silent_until_capture_and_capture_failure_ends_progress() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = save_app(dir.path().to_owned());
    track(&mut app);
    app.world_mut()
        .resource_scope(|world, service: Mut<PersistenceService>| {
            service.request_autosave(
                &mut world.resource_mut::<PersistenceState>(),
                SaveTitle::new("Puzzle").unwrap(),
                NonZeroU32::new(1).unwrap(),
            );
        });
    assert!(app.world().resource::<Messages<GameEvent>>().is_empty());
    app.world_mut().remove_resource::<OriginalPuzzleImage>();
    app.update();
    assert!(matches!(
        take(&mut app).as_slice(),
        [
            GameEvent::AutosaveStarted,
            GameEvent::AutosaveFailed {
                error: PersistenceError::DefinitionUnavailable
            }
        ]
    ));
    app.update();
    assert!(take(&mut app).is_empty());
}

#[test]
fn worker_failure_stale_result_and_disconnection_cannot_report_success() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = save_app(dir.path().to_owned());
    track(&mut app);
    let (tx, rx) = crossbeam::channel::unbounded();
    app.world_mut().resource_mut::<PersistenceService>().rx = rx;
    let generation = app.world().resource::<PersistenceState>().generation;
    tx.send((
        generation.wrapping_add(1),
        Reply::Saved(true, Err(StorageError::LockTimeout.into())),
    ))
    .unwrap();
    app.update();
    assert!(
        take(&mut app).is_empty(),
        "previous-session results are silent"
    );
    {
        let mut state = app.world_mut().resource_mut::<PersistenceState>();
        state.busy = true;
        state.autosaving = true;
    }
    tx.send((
        generation,
        Reply::Saved(true, Err(StorageError::LockTimeout.into())),
    ))
    .unwrap();
    app.update();
    assert!(matches!(
        take(&mut app).as_slice(),
        [GameEvent::AutosaveFailed {
            error: PersistenceError::Save(SaveError::Storage(StorageError::LockTimeout))
        }]
    ));
    {
        let mut state = app.world_mut().resource_mut::<PersistenceState>();
        state.busy = true;
        state.autosaving = true;
    }
    drop(tx);
    app.update();
    assert!(matches!(
        take(&mut app).as_slice(),
        [GameEvent::AutosaveFailed {
            error: PersistenceError::WorkerStopped
        }]
    ));
    app.update();
    assert!(take(&mut app).is_empty());
}

#[test]
fn worker_submission_failure_reports_start_and_failure_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = save_app(dir.path().to_owned());
    track(&mut app);
    let (tx, rx) = crossbeam::channel::unbounded();
    drop(rx);
    app.world_mut().resource_mut::<PersistenceService>().tx = tx;
    request_and_wait(&mut app, true);
    assert!(matches!(
        take(&mut app).as_slice(),
        [
            GameEvent::AutosaveStarted,
            GameEvent::AutosaveFailed {
                error: PersistenceError::WorkerStopped
            }
        ]
    ));
}
