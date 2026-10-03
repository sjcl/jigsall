use bevy::prelude::*;
use bevy::tasks::{block_on, poll_once, IoTaskPool, Task};
use puzzella_game::asset_reader::{start_thread_image_load, ExternalFileRegistry};
use puzzella_game::persistence::runtime::OriginalPuzzleImage;
use puzzella_game::resources::{ImageLoadError, ImageLoadSender, PuzzleConfig, PuzzleImage};
use std::{future::Future, path::PathBuf};

#[derive(Resource, Default)]
pub(crate) struct ImagePicker {
    task: Option<Task<Option<PathBuf>>>,
    accept_result: bool,
}

impl ImagePicker {
    pub(super) fn is_open(&self) -> bool {
        self.task.is_some()
    }

    pub(super) fn open(&mut self, filter_name: String) {
        if self.is_open() {
            return;
        }
        let dialog = rfd::AsyncFileDialog::new()
            .add_filter(filter_name, &["png", "jpg", "jpeg", "bmp", "gif", "webp"])
            .pick_file();
        self.start(async move { dialog.await.map(|file| file.path().to_path_buf()) });
    }

    fn start(&mut self, selection: impl Future<Output = Option<PathBuf>> + Send + 'static) {
        if self.is_open() {
            return;
        }
        self.task = Some(IoTaskPool::get().spawn(selection));
        self.accept_result = true;
    }

    fn take_result(&mut self) -> Option<PathBuf> {
        // poll_once returns immediately even while the native dialog is still open.
        let result = block_on(poll_once(self.task.as_mut()?))?;
        self.task = None;
        if self.accept_result {
            result
        } else {
            None
        }
    }
}

pub(crate) fn discard_image_selection(mut picker: ResMut<ImagePicker>) {
    // Dropping a future does not close the native dialog. Keep tracking it until
    // it finishes, preventing overlapping dialogs and results from an old setup.
    picker.accept_result = false;
}

pub(crate) fn finish_image_selection(
    mut picker: ResMut<ImagePicker>,
    mut config: ResMut<PuzzleConfig>,
    registry: Res<ExternalFileRegistry>,
    sender: Res<ImageLoadSender>,
    mut commands: Commands,
) {
    if let Some(path) = picker.take_result() {
        let key = registry.register_file(&path);
        config.image_path = key.clone();
        start_thread_image_load(key, path, sender.tx_results.clone());
        commands.remove_resource::<PuzzleImage>();
        commands.remove_resource::<OriginalPuzzleImage>();
        commands.remove_resource::<ImageLoadError>();
    }
}

#[cfg(test)]
mod tests;
