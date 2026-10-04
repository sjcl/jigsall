use super::*;
use jigsall_game::network::runtime::{NetworkStatus, RuntimeRole};

#[derive(Clone, Copy)]
pub(crate) enum DepartureAction {
    Title,
    Exit,
}

impl DepartureAction {
    pub(super) fn prompt_key(self) -> &'static str {
        match self {
            Self::Title => "save-before-title",
            Self::Exit => "save-before-exit",
        }
    }

    pub(super) fn save_key(self) -> &'static str {
        match self {
            Self::Title => "save-and-title",
            Self::Exit => "save-and-exit",
        }
    }

    pub(super) fn discard_key(self) -> &'static str {
        match self {
            Self::Title => "save-discard-title",
            Self::Exit => "save-discard-exit",
        }
    }

    fn confirmation_key(self) -> &'static str {
        match self {
            Self::Title => "save-confirm-discard-title",
            Self::Exit => "save-confirm-discard-exit",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum DepartureFlow {
    Prompt(DepartureAction),
    Confirm(DepartureAction),
    Saving(DepartureAction),
    Ready(DepartureAction),
}

impl DepartureFlow {
    pub(super) fn action(self) -> DepartureAction {
        match self {
            Self::Prompt(action)
            | Self::Confirm(action)
            | Self::Saving(action)
            | Self::Ready(action) => action,
        }
    }
}

impl SaveDialogs {
    /// A repeated OS close must not bypass confirmation or restart a save.
    pub(crate) fn request_window_exit(
        &mut self,
        state: &mut PersistenceState,
        i18n: &Localization,
    ) {
        match self.departure {
            Some(
                DepartureFlow::Prompt(DepartureAction::Exit)
                | DepartureFlow::Confirm(DepartureAction::Exit)
                | DepartureFlow::Saving(DepartureAction::Exit)
                | DepartureFlow::Ready(DepartureAction::Exit),
            ) => return,
            Some(DepartureFlow::Saving(DepartureAction::Title)) => {
                self.departure = Some(DepartureFlow::Saving(DepartureAction::Exit));
                return;
            }
            Some(DepartureFlow::Ready(DepartureAction::Title)) => {
                self.departure = Some(DepartureFlow::Ready(DepartureAction::Exit));
                return;
            }
            Some(
                DepartureFlow::Prompt(DepartureAction::Title)
                | DepartureFlow::Confirm(DepartureAction::Title),
            ) => {
                self.departure = Some(DepartureFlow::Prompt(DepartureAction::Exit));
                return;
            }
            None => {}
        }
        if state.title_dialog_open {
            self.departure = Some(if state.busy && !state.autosaving {
                DepartureFlow::Saving(DepartureAction::Exit)
            } else {
                DepartureFlow::Prompt(DepartureAction::Exit)
            });
        } else {
            self.request_departure(DepartureAction::Exit, None, state, i18n);
        }
    }

    pub(crate) fn departure_pending(&self) -> bool {
        self.departure.is_some()
    }

    pub(crate) fn exit_pending(&self) -> bool {
        self.departure
            .is_some_and(|flow| matches!(flow.action(), DepartureAction::Exit))
    }

    pub(crate) fn request_departure(
        &mut self,
        action: DepartureAction,
        role: Option<RuntimeRole>,
        state: &mut PersistenceState,
        i18n: &Localization,
    ) {
        if role == Some(RuntimeRole::Client) {
            self.departure = Some(DepartureFlow::Ready(action));
        } else {
            self.open_title(state, i18n);
            self.departure = Some(DepartureFlow::Prompt(action));
        }
    }
}

/// Saving must finish successfully before session teardown or AppExit.
pub(crate) fn process_departure(
    mut dialogs: ResMut<SaveDialogs>,
    state: Res<PersistenceState>,
    app_state: Res<State<AppState>>,
    status: Res<NetworkStatus>,
    mut next: ResMut<NextState<AppState>>,
    mut multiplayer: ResMut<crate::multiplayer::MultiplayerUi>,
    mut exit: MessageWriter<AppExit>,
) {
    if !matches!(app_state.get(), AppState::InGame | AppState::GameComplete) {
        dialogs.departure = None;
        return;
    }
    if let Some(DepartureFlow::Saving(action)) = dialogs.departure {
        if state.busy {
            return;
        }
        // Only a successful manual save closes the title dialog and reports Saved.
        // Autosave and image-import replies must never complete a departure.
        dialogs.departure = Some(
            if !state.title_dialog_open && matches!(state.message, Some(PersistenceNotice::Saved)) {
                DepartureFlow::Ready(action)
            } else {
                DepartureFlow::Prompt(action)
            },
        );
    }
    let Some(DepartureFlow::Ready(action)) = dialogs.departure else {
        return;
    };
    dialogs.departure = None;
    match action {
        DepartureAction::Title => {
            if status.role.is_some() {
                multiplayer.return_to_title();
            } else {
                next.set(AppState::Menu);
            }
        }
        DepartureAction::Exit => {
            exit.write(AppExit::Success);
        }
    }
}

pub(super) fn paint_confirmation(
    ctx: &egui::Context,
    dialogs: &mut SaveDialogs,
    state: &mut PersistenceState,
    action: DepartureAction,
    i18n: &Localization,
) {
    let screen = ctx.content_rect();
    let response = egui::Modal::new("discard_game_confirmation".into())
        .backdrop_color(egui::Color32::from_black_alpha(185))
        .frame(theme::frame())
        .show(ctx, |ui| {
            ui.set_width((screen.width() - 96.0).clamp(160.0, 460.0));
            theme::heading(ui, i18n.text("save-discard-confirm-title"));
            ui.label(i18n.text(action.confirmation_key()));
            theme::hint(ui, i18n.text("save-discard-warning"));
            ui.add_space(8.0);
            let width = ui.available_width();
            if theme::button(ui, i18n.text("common-cancel"), width, true).clicked() {
                dialogs.departure = Some(DepartureFlow::Prompt(action));
            }
            if theme::danger_button(ui, i18n.text(action.discard_key()), width).clicked() {
                state.title_dialog_open = false;
                dialogs.departure = Some(DepartureFlow::Ready(action));
            }
        });
    if state.title_dialog_open && response.should_close() {
        dialogs.departure = Some(DepartureFlow::Prompt(action));
    }
}
