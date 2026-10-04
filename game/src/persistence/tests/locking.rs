use super::*;
use crate::persistence::repository::wait_for_lock;
use std::{
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant},
};

#[test]
fn lock_wait_times_out_without_releasing_the_other_holders_lock() {
    let directory = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(directory.path());
    let hash = image_hash(b"locked image");
    for image_lock in [false, true] {
        let holder = if image_lock {
            storage.try_lock_image(hash)
        } else {
            storage.try_lock_repository()
        }
        .unwrap()
        .unwrap();
        let waiter = storage.clone();
        let (done, result) = crossbeam::channel::bounded(1);
        let worker = std::thread::spawn(move || {
            done.send(wait_for_lock(
                || {
                    if image_lock {
                        waiter.try_retain_image(hash)
                    } else {
                        waiter.try_lock_repository()
                    }
                },
                Duration::from_millis(25),
            ))
            .unwrap();
        });
        let result = result.recv_timeout(Duration::from_secs(5));
        let still_locked = if image_lock {
            storage.try_retain_image(hash)
        } else {
            storage.try_lock_repository()
        }
        .unwrap()
        .is_none();
        // Release before asserting so an unbounded wait can still finish.
        drop(holder);
        worker.join().unwrap();
        assert!(matches!(
            result.unwrap(),
            Err(SaveError::Storage(StorageError::LockTimeout))
        ));
        assert!(still_locked);
        let guard = wait_for_lock(
            || {
                if image_lock {
                    storage.try_retain_image(hash)
                } else {
                    storage.try_lock_repository()
                }
            },
            Duration::ZERO,
        )
        .unwrap();
        drop(guard);
    }
}

#[test]
fn lock_wait_retries_transient_contention_and_acquires_the_guard() {
    let mut attempts = 0;
    let guard = wait_for_lock(
        || {
            attempts += 1;
            Ok((attempts == 3).then(StorageGuard::default))
        },
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(attempts, 3);
    drop(guard);
}

#[test]
fn lock_wait_reports_backend_errors_without_retrying() {
    let mut attempts = 0;
    let error = StorageError::Io("lock failure".into());
    let result = wait_for_lock(
        || {
            attempts += 1;
            Err(error.clone())
        },
        Duration::from_secs(5),
    );
    assert!(matches!(result, Err(SaveError::Storage(actual)) if actual == error));
    assert_eq!(attempts, 1);
}

#[test]
fn shared_image_leases_preserve_unsaved_images_until_the_last_user_releases_them() {
    let directory = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(directory.path());
    let first = SaveRepository::new(storage.clone());
    let second = SaveRepository::new(storage.clone());
    let bytes = encoded_image(image::ImageFormat::Png);
    let hash = image_hash(&bytes);
    let first_lease = first.import_image_retained(hash, &bytes).unwrap();
    let second_lease = second.import_image_retained(hash, &bytes).unwrap();
    let pending_request = second_lease.clone();
    first
        .import_image(image_hash(b"orphan"), b"orphan")
        .unwrap();

    second.delete(SaveId(99)).unwrap();
    assert_eq!(
        storage.list(StorageNamespace::Images).unwrap(),
        vec![StorageKey::Image(hash)]
    );
    drop(first_lease);
    drop(second_lease);
    first.delete(SaveId(99)).unwrap();
    assert!(storage.exists(StorageKey::Image(hash)).unwrap());
    drop(pending_request);
    first.delete(SaveId(99)).unwrap();
    assert!(storage.list(StorageNamespace::Images).unwrap().is_empty());
    // Reusing the hash must use the same permanent lock file.
    let lease = first.import_image_retained(hash, &bytes).unwrap();
    assert!(storage.try_lock_image(hash).unwrap().is_none());
    drop(lease);
    assert!(storage.try_lock_image(hash).unwrap().is_some());
}

#[test]
fn loaded_image_remains_available_after_another_repository_deletes_its_only_save() {
    let directory = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(directory.path());
    let first = SaveRepository::new(storage.clone());
    let second = SaveRepository::new(storage.clone());
    let bytes = encoded_image(image::ImageFormat::Png);
    let saved = first
        .create(
            SaveTitle::new("original").unwrap(),
            checkpoint(&bytes),
            Some(&bytes),
        )
        .unwrap();
    let loaded = second.load(saved.id).unwrap();
    first.delete(saved.id).unwrap();
    assert_eq!(second.read_image(loaded.image_lease.hash).unwrap(), bytes);
    let next = second
        .create(
            SaveTitle::new("another save").unwrap(),
            loaded.save.checkpoint,
            None,
        )
        .unwrap();
    drop(loaded.image_lease);
    first.delete(SaveId(99)).unwrap();
    assert_eq!(first.load(next.id).unwrap().image_bytes, bytes);
    first.delete(next.id).unwrap();
    assert!(storage.list(StorageNamespace::Images).unwrap().is_empty());
}

#[test]
fn simultaneous_revision_updates_publish_once_and_report_a_conflict() {
    let directory = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(directory.path());
    let repository = SaveRepository::new(storage.clone());
    let bytes = encoded_image(image::ImageFormat::Png);
    let saved = repository
        .create(
            SaveTitle::new("original").unwrap(),
            checkpoint(&bytes),
            Some(&bytes),
        )
        .unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|index| {
            let repository = SaveRepository::new(storage.clone());
            let checkpoint = checkpoint(&bytes);
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                repository.update(
                    saved.id,
                    saved.revision,
                    SaveTitle::new(&format!("writer {index}")).unwrap(),
                    checkpoint,
                    None,
                )
            })
        })
        .collect();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(
                result,
                Err(SaveError::Conflict {
                    expected_revision: 1,
                    actual_revision: 2,
                    ..
                })
            ))
            .count(),
        1
    );
    assert_eq!(
        repository.read_header(saved.id).unwrap().metadata.revision,
        2
    );
}

#[test]
fn lock_contention_keeps_the_storage_executor_available_for_other_io() {
    let directory = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(directory.path());
    let key = StorageKey::Image(image_hash(b"existing"));
    storage.write(key, b"existing".to_vec()).unwrap();
    let transaction = storage.try_lock_repository().unwrap().unwrap();
    let proxy = executor::spawn_storage(storage);
    let repository = SaveRepository::new(proxy.clone());
    let writer = std::thread::spawn(move || repository.import_image(image_hash(b"new"), b"new"));
    let (done, result) = crossbeam::channel::bounded(1);
    let reader = std::thread::spawn(move || done.send(proxy.read(key)).unwrap());
    let read = result.recv_timeout(Duration::from_secs(10));
    // Release before asserting, so a broken executor can still shut down.
    drop(transaction);
    assert_eq!(read.unwrap().unwrap(), b"existing");
    reader.join().unwrap();
    writer.join().unwrap().unwrap();
}

#[test]
fn collection_waits_for_image_and_save_publication_to_finish() {
    let directory = tempfile::tempdir().unwrap();
    let storage = FilesystemStorage::new(directory.path());
    let repository = SaveRepository::new(storage.clone());
    let old = repository
        .create(
            SaveTitle::new("old").unwrap(),
            checkpoint(b"old"),
            Some(b"old"),
        )
        .unwrap();
    let (proxy, inbox) = executor::storage_channel();
    let bytes = encoded_image(image::ImageFormat::Png);
    let hash = image_hash(&bytes);
    let writer = std::thread::spawn(move || {
        SaveRepository::new(proxy).create_with_ids(
            SaveTitle::new("new").unwrap(),
            checkpoint(&bytes),
            Some(&bytes),
            || 99,
        )
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    // Pause after the image is on disk, before its first save is published.
    let publication = loop {
        if let Ok(request) = inbox.try_recv() {
            if matches!(
                request.operation,
                executor::StorageOperation::Write(StorageKey::Save(_), _)
            ) {
                break request;
            }
            request.execute(&storage).unwrap();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(storage.exists(StorageKey::Image(hash)).unwrap());
    let locked = storage.try_lock_repository().unwrap().is_none();
    let (done, deleted) = crossbeam::channel::bounded(1);
    let cleaner = std::thread::spawn(move || done.send(repository.delete(old.id)).unwrap());
    let waited = deleted.recv_timeout(Duration::from_millis(50)).is_err();
    publication.execute(&storage).unwrap();
    let saved = writer.join().unwrap().unwrap();
    cleaner.join().unwrap();
    if waited {
        deleted
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
    }
    assert!(locked && waited, "Collection bypassed the publication lock");
    assert_eq!(
        SaveRepository::new(storage)
            .load(saved.id)
            .unwrap()
            .image_bytes,
        encoded_image(image::ImageFormat::Png)
    );
}

struct TestProcess(Child);
impl Drop for TestProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn child(root: &Path, action: &str) -> TestProcess {
    TestProcess(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "persistence::tests::locking::image_lock_child_process",
                "--ignored",
                "--nocapture",
            ])
            .env("PUZZELLA_LOCK_TEST_ROOT", root)
            .env("PUZZELLA_LOCK_TEST_ACTION", action)
            .spawn()
            .unwrap(),
    )
}
fn wait_for_file(root: &Path, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !root.join(name).exists() {
        assert!(Instant::now() < deadline, "Timed out waiting for {name}");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn wait_for_child(child: &mut TestProcess) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        assert!(Instant::now() < deadline, "Child did not finish saving");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn another_process_cannot_collect_an_unsaved_image_and_the_next_save_succeeds() {
    let directory = tempfile::tempdir().unwrap();
    let mut child = child(directory.path(), "save");
    wait_for_file(directory.path(), "ready");
    let storage = FilesystemStorage::new(directory.path());
    let repository = SaveRepository::new(storage.clone());
    let other = repository
        .create(
            SaveTitle::new("other").unwrap(),
            checkpoint(b"other"),
            Some(b"other"),
        )
        .unwrap();
    repository.delete(other.id).unwrap();
    let hash = image_hash(&encoded_image(image::ImageFormat::Png));
    assert!(storage.exists(StorageKey::Image(hash)).unwrap());
    assert!(!storage
        .exists(StorageKey::Image(image_hash(b"other")))
        .unwrap());
    std::fs::write(directory.path().join("continue"), []).unwrap();
    wait_for_child(&mut child);
    let entries = repository.list().unwrap();
    assert_eq!(entries.len(), 1);
    let loaded = repository.load(entries[0].id).unwrap();
    assert_eq!(loaded.save.checkpoint.image_hash, hash);
    assert_eq!(loaded.image_bytes, encoded_image(image::ImageFormat::Png));
}

#[test]
fn process_termination_releases_image_and_repository_locks_without_stale_lock_cleanup() {
    for action in ["hold", "hold_repository"] {
        let directory = tempfile::tempdir().unwrap();
        let mut child = child(directory.path(), action);
        wait_for_file(directory.path(), "ready");
        let storage = FilesystemStorage::new(directory.path());
        let hash = image_hash(&encoded_image(image::ImageFormat::Png));
        if action == "hold_repository" {
            assert!(storage.try_lock_repository().unwrap().is_none());
        } else {
            SaveRepository::new(storage.clone())
                .delete(SaveId(99))
                .unwrap();
            assert!(storage.try_lock_image(hash).unwrap().is_none());
            assert!(storage.exists(StorageKey::Image(hash)).unwrap());
        }
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        SaveRepository::new(storage.clone())
            .delete(SaveId(99))
            .unwrap();
        assert!(!storage.exists(StorageKey::Image(hash)).unwrap());
        assert!(storage.try_lock_repository().unwrap().is_some());
        assert!(storage.try_lock_image(hash).unwrap().is_some());
    }
}

// Subprocess fixtures synchronize using files only; no process messaging is
// involved in either the product or this test's coordination.
#[test]
#[ignore = "invoked by the process lock tests"]
fn image_lock_child_process() {
    let Some(root) = std::env::var_os("PUZZELLA_LOCK_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let action = std::env::var("PUZZELLA_LOCK_TEST_ACTION").unwrap();
    let storage = FilesystemStorage::new(&root);
    let repository = SaveRepository::new(storage.clone());
    let bytes = encoded_image(image::ImageFormat::Png);
    let checkpoint = checkpoint(&bytes);
    let _lease = repository
        .import_image_retained(image_hash(&bytes), &bytes)
        .unwrap();
    drop(bytes);
    let _transaction =
        (action == "hold_repository").then(|| storage.try_lock_repository().unwrap().unwrap());
    std::fs::write(root.join("ready"), []).unwrap();
    wait_for_file(&root, "continue");
    if action == "save" {
        repository
            .create(
                SaveTitle::new("child's first save").unwrap(),
                checkpoint,
                None,
            )
            .unwrap();
    }
}
