//! Sections of the per-user settings.json, shared by the game and UI crates.
use crossbeam::channel::{self, Receiver, Sender, TryRecvError};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, Weak},
    thread::JoinHandle,
};

#[derive(Clone, Copy)]
pub enum SettingsSection {
    Display,
    KeyBindings,
    Autosave,
    Image,
    Preferences,
}

impl SettingsSection {
    fn key(self) -> &'static str {
        match self {
            Self::Display => "display",
            Self::KeyBindings => "keybindings",
            Self::Autosave => "autosave",
            Self::Image => "image",
            Self::Preferences => "preferences",
        }
    }
}

pub struct SettingsFile {
    path: Option<PathBuf>,
    writer: Option<Arc<SettingsWriter>>,
    pending: Option<Receiver<Result<(), String>>>,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self::new(
            directories::BaseDirs::new()
                .map(|dirs| dirs.data_local_dir().join("puzzella/settings.json")),
        )
    }
}

impl SettingsFile {
    /// None disables filesystem access for tests and probes.
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            writer: None,
            pending: None,
        }
    }

    /// Missing or invalid sections use their defaults. An invalid section cannot
    /// prevent another section from loading.
    pub fn load<T: DeserializeOwned + Default>(
        &self,
        section: SettingsSection,
    ) -> (T, Option<String>) {
        let read = || -> Result<_, String> {
            self.document()?
                .remove(section.key())
                .map(serde_json::from_value)
                .transpose()
                .map(|current| current.unwrap_or_default())
                .map_err(|error| error.to_string())
        };
        match read() {
            Ok(current) => (current, None),
            Err(error) => (T::default(), Some(error)),
        }
    }

    /// Queue a section snapshot. Only the worker reads, writes, syncs and replaces
    /// the file; Ok means queued, not persisted. Poll for the latest save's result.
    /// Unconfirmed display previews never enter this queue.
    pub fn save<T: Serialize>(
        &mut self,
        section: SettingsSection,
        current: &T,
    ) -> Result<(), String> {
        let (reply, result) = channel::bounded(1);
        if let Some(path) = &self.path {
            let value = serde_json::to_value(current).map_err(|error| error.to_string())?;
            if self.writer.is_none() {
                self.writer = Some(shared_writer()?);
            }
            self.writer
                .as_ref()
                .unwrap()
                .requests
                .as_ref()
                .unwrap()
                .send(WriteRequest::Save {
                    path: path.clone(),
                    section,
                    value,
                    reply,
                })
                .map_err(|_| "settings writer stopped".to_owned())?;
        } else {
            let _ = reply.send(Ok(()));
        }
        // Discard obsolete replies, while keeping their already queued writes ordered.
        self.pending = Some(result);
        Ok(())
    }

    pub fn is_save_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Nonblocking, including when the worker is waiting on a slow filesystem.
    pub fn poll_save(&mut self) -> Option<Result<(), String>> {
        let result = match self.pending.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("settings writer stopped".to_owned()),
        };
        self.pending = None;
        Some(result)
    }

    fn document(&self) -> Result<Map<String, Value>, String> {
        Ok(match &self.path {
            Some(path) => read_json(path)?.unwrap_or_default(),
            None => Map::new(),
        })
    }
}

enum WriteRequest {
    Save {
        path: PathBuf,
        section: SettingsSection,
        value: Value,
        reply: Sender<Result<(), String>>,
    },
    #[cfg(test)]
    Pause {
        entered: Sender<()>,
        resume: Receiver<()>,
    },
}

struct SettingsWriter {
    requests: Option<Sender<WriteRequest>>,
    worker: Option<JoinHandle<()>>,
}

// All resources share one FIFO writer, so section read/modify/replace operations
// cannot race. Weak ownership lets the last settings resource drain it at shutdown.
static WRITER: (Mutex<Option<Weak<SettingsWriter>>>, Condvar) = (Mutex::new(None), Condvar::new());

fn shared_writer() -> Result<Arc<SettingsWriter>, String> {
    let mut slot = WRITER.0.lock().unwrap();
    loop {
        match slot.as_ref() {
            Some(writer) => {
                if let Some(writer) = writer.upgrade() {
                    return Ok(writer);
                }
                // The last owner is draining the old queue. Do not start a competing writer.
                slot = WRITER.1.wait(slot).unwrap();
            }
            None => {
                let writer = Arc::new(SettingsWriter::start()?);
                *slot = Some(Arc::downgrade(&writer));
                return Ok(writer);
            }
        }
    }
}

impl SettingsWriter {
    fn start() -> Result<Self, String> {
        let (requests, incoming) = channel::unbounded();
        let worker = std::thread::Builder::new()
            .name("settings-save".into())
            .spawn(move || {
                for request in incoming {
                    match request {
                        WriteRequest::Save {
                            path,
                            section,
                            value,
                            reply,
                        } => {
                            let result = write_section(&path, section, value);
                            if let Err(error) = &result {
                                bevy::log::warn!("Could not save settings: {error}");
                            }
                            let _ = reply.send(result);
                        }
                        #[cfg(test)]
                        WriteRequest::Pause { entered, resume } => {
                            let _ = entered.send(());
                            let _ = resume.recv_timeout(std::time::Duration::from_secs(5));
                        }
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            requests: Some(requests),
            worker: Some(worker),
        })
    }
}

impl Drop for SettingsWriter {
    fn drop(&mut self) {
        // Closing the sender drains accepted saves before joining. This wait is
        // confined to resource teardown, never an Apply/Keep/language UI action.
        drop(self.requests.take());
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                bevy::log::warn!("Settings writer panicked");
            }
        }
        let mut slot = WRITER.0.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|writer| std::ptr::eq(writer.as_ptr(), self))
        {
            *slot = None;
            WRITER.1.notify_all();
        }
    }
}

fn write_section(path: &Path, section: SettingsSection, value: Value) -> Result<(), String> {
    // Reread here, after earlier queued sections have been persisted. Keep unknown
    // sections and refuse to overwrite a corrupt document.
    let mut document: Map<String, Value> = read_json(path)?.unwrap_or_default();
    document.insert(section.key().into(), value);
    let write = || -> Result<(), Box<dyn std::error::Error>> {
        let parent = path.parent().ok_or("missing settings directory")?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&serde_json::to_vec_pretty(&document)?)?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        Ok(())
    };
    write().map_err(|error| error.to_string())
}

fn read_json<T: DeserializeOwned>(path: &std::path::Path) -> Result<Option<T>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
pub(crate) fn wait_for_save(mut poll: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while poll() {
        assert!(
            std::time::Instant::now() < deadline,
            "settings save timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[cfg(test)]
impl SettingsFile {
    pub(crate) fn paused(path: PathBuf) -> (Self, Sender<()>) {
        let writer = Arc::new(SettingsWriter::start().unwrap());
        let (entered, ready) = channel::bounded(1);
        let (resume, gate) = channel::bounded(1);
        writer
            .requests
            .as_ref()
            .unwrap()
            .send(WriteRequest::Pause {
                entered,
                resume: gate,
            })
            .unwrap();
        ready
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        (
            Self {
                path: Some(path),
                writer: Some(writer),
                pending: None,
            },
            resume,
        )
    }
}

#[cfg(test)]
mod tests;
