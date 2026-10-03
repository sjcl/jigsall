use super::*;
use puzzella_core::PlayerId;

fn timer_app() -> App {
    let (service, _inbox) = PersistenceService::with_storage_requests();
    let mut app = App::new();
    app.insert_resource(service)
        .insert_resource(AutosaveSettingsState::load(None))
        .insert_resource(Time::<Real>::default())
        .insert_resource(State::new(GameSubState::Playing))
        .init_resource::<LocalPlayerId>()
        .init_resource::<SessionHostId>()
        .init_resource::<AutosaveTimer>()
        .init_resource::<PersistenceState>()
        .add_systems(Update, tick_autosave);
    app.world_mut()
        .resource_mut::<AutosaveSettingsState>()
        .set_interval(NonZeroU32::new(1));
    app.update();
    app
}

fn advance(app: &mut App, seconds: u64) {
    app.world_mut()
        .resource_mut::<Time<Real>>()
        .advance_by(Duration::from_secs(seconds));
    app.update();
}

#[test]
fn autosave_counts_only_host_play_time_and_suspends_during_pause() {
    let mut app = timer_app();
    app.world_mut().resource_mut::<SessionHostId>().0 = PlayerId(99);
    advance(&mut app, 600);
    assert!(!app.world().resource::<PersistenceState>().busy);
    app.world_mut().resource_mut::<SessionHostId>().0 = app.world().resource::<LocalPlayerId>().0;
    advance(&mut app, 59);
    assert!(!app.world().resource::<PersistenceState>().autosaving);
    app.insert_resource(State::new(GameSubState::Paused));
    advance(&mut app, 600);
    assert!(!app.world().resource::<PersistenceState>().busy);
    app.insert_resource(State::new(GameSubState::Playing));
    advance(&mut app, 1);
    let state = app.world().resource::<PersistenceState>();
    assert!(state.autosaving && state.busy && state.capture.is_some());
}

#[test]
fn due_autosave_waits_for_foreground_operations_and_title_dialog() {
    for dialog in [false, true] {
        let mut app = timer_app();
        {
            let mut state = app.world_mut().resource_mut::<PersistenceState>();
            state.busy = !dialog;
            state.title_dialog_open = dialog;
        }
        advance(&mut app, 120);
        assert!(app.world().resource::<PersistenceState>().capture.is_none());
        {
            let mut state = app.world_mut().resource_mut::<PersistenceState>();
            state.busy = false;
            state.title_dialog_open = false;
        }
        advance(&mut app, 0);
        assert!(app.world().resource::<PersistenceState>().autosaving);
    }
}

#[test]
fn disabling_and_changing_interval_restart_the_timer() {
    let mut app = timer_app();
    advance(&mut app, 59);
    app.world_mut()
        .resource_mut::<AutosaveSettingsState>()
        .set_interval(None);
    advance(&mut app, 600);
    assert!(!app.world().resource::<PersistenceState>().busy);
    app.world_mut()
        .resource_mut::<AutosaveSettingsState>()
        .set_interval(NonZeroU32::new(2));
    advance(&mut app, 0);
    advance(&mut app, 119);
    assert!(!app.world().resource::<PersistenceState>().busy);
    advance(&mut app, 1);
    assert!(app.world().resource::<PersistenceState>().autosaving);
}

#[test]
fn settings_persist_interval_and_disable_and_reject_zero() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let mut settings = AutosaveSettingsState::load(Some(path.clone()));
    assert_eq!(settings.current.interval_minutes, NonZeroU32::new(5));
    assert_eq!(settings.current.max_saves_per_game, NonZeroU32::MIN);
    settings.set_max_saves_per_game(NonZeroU32::new(3).unwrap());
    settings.set_interval(NonZeroU32::new(12));
    assert!(settings.error.is_none());
    assert_eq!(
        AutosaveSettingsState::load(Some(path.clone()))
            .current
            .interval_minutes,
        NonZeroU32::new(12)
    );
    settings.set_interval(None);
    assert_eq!(
        AutosaveSettingsState::load(Some(path.clone()))
            .current
            .max_saves_per_game,
        NonZeroU32::new(3).unwrap()
    );
    assert_eq!(
        AutosaveSettingsState::load(Some(path.clone()))
            .current
            .interval_minutes,
        None
    );
    std::fs::write(&path, br#"{"autosave":{"interval_minutes":0}}"#).unwrap();
    let invalid = AutosaveSettingsState::load(Some(path));
    assert!(matches!(
        invalid.error,
        Some(AutosaveSettingsError::Read(_))
    ));
    assert_eq!(invalid.current, AutosaveSettings::default());
}

#[test]
fn missing_save_limit_defaults_to_one_and_zero_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, br#"{"autosave":{"interval_minutes":12}}"#).unwrap();
    let settings = AutosaveSettingsState::load(Some(path.clone()));
    assert!(settings.error.is_none());
    assert_eq!(settings.current.interval_minutes, NonZeroU32::new(12));
    assert_eq!(settings.current.max_saves_per_game, NonZeroU32::MIN);
    std::fs::write(
        &path,
        br#"{"autosave":{"interval_minutes":12,"max_saves_per_game":0}}"#,
    )
    .unwrap();
    let invalid = AutosaveSettingsState::load(Some(path));
    assert!(matches!(
        invalid.error,
        Some(AutosaveSettingsError::Read(_))
    ));
    assert_eq!(invalid.current, AutosaveSettings::default());
}
