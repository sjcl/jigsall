//! Convert domain outcomes to UI text. Technical details remain diagnostic arguments.
use crate::localization::Localization;
use puzzella_game::{
    persistence::{
        runtime::{PersistenceError, PersistenceNotice},
        SaveError, SaveTitleError, StorageError, MAX_SAVE_TITLE_CHARS,
    },
    resources::GenerationError,
    settings::{DisplaySettingsError, DisplayValidationError},
};

impl Localization {
    fn reason(&self, key: &str, reason: &str) -> String {
        self.format(key, &[("reason", reason.into())])
    }

    pub(crate) fn display_validation(&self, error: &DisplayValidationError) -> String {
        self.text(match error {
            DisplayValidationError::InvalidResolution => "settings-invalid-resolution",
            DisplayValidationError::InvalidFps => "settings-invalid-fps",
        })
    }

    pub(crate) fn display_error(&self, error: &DisplaySettingsError) -> String {
        match error {
            DisplaySettingsError::Invalid(error) => self.display_validation(error),
            DisplaySettingsError::InvalidSaved(error) => {
                self.reason("settings-invalid-saved", &self.display_validation(error))
            }
            DisplaySettingsError::ReadFailed(reason) => self.reason("settings-read-failed", reason),
            DisplaySettingsError::SaveFailed(reason) => self.reason("settings-save-failed", reason),
            DisplaySettingsError::DirectoryUnavailable => {
                self.text("settings-directory-unavailable")
            }
            DisplaySettingsError::UnsupportedFullscreen => {
                self.text("settings-unsupported-fullscreen")
            }
            DisplaySettingsError::NoDisplay => self.text("settings-no-display"),
            DisplaySettingsError::FullscreenUnavailable => {
                self.text("settings-fullscreen-unavailable")
            }
        }
    }

    pub(crate) fn save_error(&self, error: &SaveError) -> String {
        match error {
            SaveError::InvalidTitle(error) => match error {
                SaveTitleError::ControlCharacters => self.text("save-title-controls"),
                SaveTitleError::Empty => self.text("save-title-empty"),
                SaveTitleError::TooLong => self.format(
                    "save-title-too-long",
                    &[("max", MAX_SAVE_TITLE_CHARS.into())],
                ),
            },
            SaveError::UnsupportedSaveFormat(version) => self.format(
                "save-unsupported-format",
                &[("version", (*version as u32).into())],
            ),
            SaveError::UnsupportedImageFormat(version) => self.format(
                "save-unsupported-image",
                &[("version", (*version as u32).into())],
            ),
            SaveError::UnsupportedGenerator(version) => self.format(
                "save-unsupported-generator",
                &[("version", (*version as u32).into())],
            ),
            SaveError::CorruptSave(reason) => self.reason("save-corrupt", reason),
            SaveError::CorruptImage(reason) => self.reason("save-corrupt-image", reason),
            SaveError::MissingImage(_) => self.text("save-missing-image"),
            SaveError::Conflict {
                expected_revision,
                actual_revision,
                ..
            } => self.format(
                "save-conflict",
                &[
                    ("expected", expected_revision.to_string().as_str().into()),
                    ("actual", actual_revision.to_string().as_str().into()),
                ],
            ),
            SaveError::Checkpoint(error) => self.reason("save-corrupt", &error.to_string()),
            SaveError::Storage(error) => match error {
                StorageError::NotFound(_) => self.text("save-data-not-found"),
                StorageError::Unavailable(reason) | StorageError::Io(reason) => {
                    self.reason("save-storage-error", reason)
                }
                StorageError::TooLarge => self.text("save-too-large"),
                StorageError::InvalidRange => self.text("save-invalid-range"),
                StorageError::LockTimeout => self.text("save-lock-timeout"),
            },
            SaveError::Decode(reason) => self.reason("save-decode-failed", reason),
            SaveError::CounterExhausted => self.text("save-counter-exhausted"),
            SaveError::IdCollision => self.text("save-id-collision"),
        }
    }

    pub(crate) fn persistence_error(&self, error: &PersistenceError) -> String {
        match error {
            PersistenceError::WorkerStopped => self.text("save-worker-stopped"),
            PersistenceError::DefinitionUnavailable => self.text("save-definition-unavailable"),
            PersistenceError::Checkpoint(error) => self.reason("save-corrupt", &error.to_string()),
            PersistenceError::ImageImport(error) => {
                self.reason("save-import-failed", &self.save_error(error))
            }
            PersistenceError::Save(error) => self.save_error(error),
            PersistenceError::AutosaveRotation(error) => self.reason(
                "game-autosave-rotation-failed-detail",
                &self.save_error(error),
            ),
        }
    }

    pub(crate) fn persistence_notice(&self, notice: &PersistenceNotice) -> String {
        self.text(match notice {
            PersistenceNotice::Saved => "save-saved",
        })
    }

    pub(crate) fn generation_error(&self, error: &GenerationError) -> String {
        match error {
            GenerationError::WorkerStopped => self.text("generation-worker-stopped"),
            GenerationError::InvalidDefinition(reason)
            | GenerationError::State(reason)
            | GenerationError::Renderer(reason) => self.reason("generation-error", reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localization::{LanguagePreference, Locale};
    use puzzella_game::persistence::SaveId;

    #[test]
    fn retained_domain_outcomes_follow_language_changes_and_keep_technical_details() {
        let mut i18n = crate::localization::tests::english();
        let error = DisplaySettingsError::SaveFailed("disk detail".into());
        let notice = PersistenceNotice::Saved;
        let lock_timeout = SaveError::Storage(StorageError::LockTimeout);
        assert_eq!(
            i18n.save_error(&lock_timeout),
            "Timed out waiting for access to saved data. Close other Puzzella instances and try again."
        );
        assert_eq!(
            i18n.display_error(&error),
            "Could not save settings: disk detail"
        );
        assert_eq!(i18n.persistence_notice(&notice), "Game saved");
        i18n.set_preference(LanguagePreference::Locale(Locale::JA));
        assert_eq!(
            i18n.save_error(&lock_timeout),
            "保存データのロック待ちがタイムアウトしました。他の Puzzella を閉じてから再試行してください。"
        );
        assert_eq!(
            i18n.display_error(&error),
            "設定を保存できませんでした：disk detail"
        );
        assert_eq!(i18n.persistence_notice(&notice), "ゲームを保存しました");
        assert_eq!(
            i18n.save_error(&SaveError::InvalidTitle(SaveTitleError::Empty)),
            "タイトルを入力してください。"
        );
        let conflict = i18n.save_error(&SaveError::Conflict {
            id: SaveId(1),
            expected_revision: u64::MAX - 1,
            actual_revision: u64::MAX,
        });
        assert!(conflict.contains("18446744073709551614"));
        assert!(conflict.contains("18446744073709551615"));
    }
}
