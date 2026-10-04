use super::*;
use crate::{
    checkpoint::PuzzleCheckpoint,
    persistence::{
        runtime::{capture_requested_save, poll_results, PersistenceService, PersistenceState},
        FilesystemStorage, SaveRepository, SaveTitle,
    },
};

fn save_and_wait(client: &mut App) {
    client
        .world_mut()
        .resource_scope(|world, service: Mut<PersistenceService>| {
            service.request_save(
                &mut world.resource_mut::<PersistenceState>(),
                SaveTitle::new("Last synced game").unwrap(),
            );
        });
    let deadline = Instant::now() + Duration::from_secs(5);
    while client.world().resource::<PersistenceState>().busy {
        assert!(
            Instant::now() < deadline,
            "disconnected save did not finish"
        );
        client.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(client
        .world()
        .resource::<PersistenceState>()
        .error
        .is_none());
}

#[test]
fn disconnected_client_save_roundtrip_keeps_confirmed_state_and_original_image() {
    for pending_release in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut pair = Pair::new();
        pair.client
            .insert_resource(PersistenceService::new(FilesystemStorage::new(dir.path())))
            .init_resource::<PersistenceState>()
            .add_systems(Update, poll_results)
            .add_systems(PostUpdate, capture_requested_save);
        pair.ready();
        begin_gesture(&mut pair.client);
        pair.converge();
        pointer(&mut pair.client, Vec2::new(31.0, -13.0), true, false);
        pair.converge();
        let expected = PuzzleCheckpoint::capture(
            pair.client.world().resource::<PieceDataStore>(),
            pair.client.world().resource::<PuzzleDefinition>(),
            pair.client.world().resource::<OriginalPuzzleImage>().hash,
        )
        .unwrap();
        let game_id = pair.client.world().resource::<PersistenceState>().game_id;
        if pending_release {
            pointer(&mut pair.client, Vec2::new(31.0, -13.0), false, false);
            pair.client.update(); // Release sent, but the host has not committed it.
        }
        assert!(!pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .drag
            .members
            .is_empty());
        stop_session(pair.host.world_mut());
        pair.client.update();
        pair.client.update();
        assert!(pair
            .client
            .world()
            .resource::<NetworkStatus>()
            .has_disconnected_game());
        assert_eq!(
            *pair.client.world().resource::<State<AppState>>().get(),
            AppState::InGame
        );
        let store = pair.client.world().resource::<PieceDataStore>();
        assert!(store.held_by.is_empty());
        assert!(store.drag.members.is_empty());
        assert!(store.local_rotation.poses.is_empty());
        assert!(pair.client.world().resource::<LocalGameplayBlocked>().0);
        // Even a queued command cannot turn a disconnected replica into authority.
        send(&mut pair.client, PieceCommand::Grab(PieceId(0)));
        pair.client.update();
        assert!(pair
            .client
            .world()
            .resource::<PieceDataStore>()
            .held_by
            .is_empty());
        save_and_wait(&mut pair.client);
        let first = pair
            .client
            .world()
            .resource::<PersistenceState>()
            .current_save
            .clone()
            .unwrap();
        assert_eq!(first.game_id, game_id);
        assert!(!first.is_autosave);
        let repo = SaveRepository::new(FilesystemStorage::new(dir.path()));
        let loaded = repo.load(first.id).unwrap();
        assert_eq!(loaded.save.checkpoint, expected);
        assert_eq!(loaded.image_bytes.as_slice(), encoded().as_ref());
        let mut restored = PieceDataStore::default();
        loaded.save.checkpoint.install(&mut restored).unwrap();
        assert!(restored.held_by.is_empty());
        assert_eq!(
            PuzzleCheckpoint::capture(&restored, &expected.definition, expected.image_hash)
                .unwrap(),
            expected,
        );
        // A second save updates this local copy using its image lease.
        assert!(pair
            .client
            .world()
            .resource::<OriginalPuzzleImage>()
            .encoded
            .is_none());
        save_and_wait(&mut pair.client);
        let second = pair
            .client
            .world()
            .resource::<PersistenceState>()
            .current_save
            .clone()
            .unwrap();
        assert_eq!(second.id, first.id);
        assert_eq!(second.revision, first.revision + 1);
        assert_eq!(repo.read_save(second.id).unwrap().checkpoint, expected);
    }
}
