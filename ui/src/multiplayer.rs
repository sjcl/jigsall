//! Memory-only menu drafts and the handoff to the existing network runtime.
use crate::{localization::Localization, theme};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use puzzella_game::{
    network::{
        auth::{SessionPassword, MAX_PASSWORD_BYTES, MIN_PASSWORD_BYTES},
        runtime::{JoinOptions, NetworkStatus, RuntimePhase, RuntimeRole, RuntimeStartError},
        syncing::SyncPhase,
    },
    persistence::{runtime::*, SaveId},
    player_settings::PlayerSettingsState,
    resources::*,
};
use std::net::SocketAddr;
use zeroize::Zeroizing;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuScreen {
    #[default]
    Title,
    SinglePlayer,
    Multiplayer,
    Host,
    Join,
    HostLoadSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UiError {
    Address,
    Password,
    WrongPassword,
    Timeout,
    ServerFull,
    ConnectionFailed,
    ProtocolMismatch,
    ImageUnavailable,
    PuzzleUnavailable,
    AlreadyActive,
}
impl UiError {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Address => "multiplayer-error-address",
            Self::Password => "multiplayer-error-password",
            Self::WrongPassword => "multiplayer-error-wrong-password",
            Self::Timeout => "multiplayer-error-timeout",
            Self::ServerFull => "multiplayer-error-full",
            Self::ConnectionFailed => "multiplayer-error-connection",
            Self::ProtocolMismatch => "multiplayer-error-protocol",
            Self::ImageUnavailable => "multiplayer-error-image",
            Self::PuzzleUnavailable => "multiplayer-error-puzzle",
            Self::AlreadyActive => "multiplayer-error-active",
        }
    }
    // The runtime retains diagnostics for logging. Only these categories reach UI.
    fn diagnostic(error: Option<&str>) -> Self {
        let error = error.unwrap_or_default();
        if error.contains("AuthenticationFailed") {
            Self::WrongPassword
        } else if error.contains("JoinCapacity")
            || error.contains("TooManyJoins")
            || error.contains("CapacityWaitTimeout")
            || error.contains("HostCapacityTimeout")
        {
            Self::ServerFull
        } else if error.contains("Timeout")
            || error.contains("SyncLifetime")
            || error.contains("BulkStalled")
        {
            Self::Timeout
        } else if error.contains("Image") || error.contains("image") {
            Self::ImageUnavailable
        } else if error.contains("UnsupportedVersion")
            || error.contains("ProtocolViolation")
            || error.contains("WrongPhase")
            || error.contains("MalformedBaseline")
        {
            Self::ProtocolMismatch
        } else {
            Self::ConnectionFailed
        }
    }
    fn start(error: RuntimeStartError) -> Self {
        match error {
            RuntimeStartError::AlreadyActive => Self::AlreadyActive,
            RuntimeStartError::DefinitionUnavailable | RuntimeStartError::InvalidWorld => {
                Self::PuzzleUnavailable
            }
            RuntimeStartError::ImageHashMismatch => Self::ImageUnavailable,
            RuntimeStartError::Transport(_) => Self::ConnectionFailed,
        }
    }
}

pub(crate) struct ConnectionDraft {
    pub address: String,
    // No serialization, Debug, or Clone; dropping/replacing a draft wipes its buffer.
    pub password: Zeroizing<String>,
}
impl ConnectionDraft {
    fn new(address: &str) -> Self {
        Self {
            address: address.into(),
            password: Zeroizing::new(String::new()),
        }
    }
    pub fn valid(&self, host: bool) -> bool {
        parse_address(&self.address, host).is_ok()
            && (MIN_PASSWORD_BYTES..=MAX_PASSWORD_BYTES).contains(&self.password.len())
    }
    fn take(&mut self, host: bool) -> Result<(SocketAddr, SessionPassword), UiError> {
        let address = parse_address(&self.address, host)?;
        let password = SessionPassword::new(std::mem::take(&mut *self.password))
            .map_err(|_| UiError::Password)?;
        Ok((address, password))
    }
    pub fn clear_password(&mut self) {
        self.password = Zeroizing::new(String::new());
    }
}

fn parse_address(value: &str, host: bool) -> Result<SocketAddr, UiError> {
    let address: SocketAddr = value.trim().parse().map_err(|_| UiError::Address)?;
    if address.port() == 0
        || address.ip().is_multicast()
        || (!host && address.ip().is_unspecified())
    {
        return Err(UiError::Address);
    }
    Ok(address)
}

struct PendingHost {
    address: SocketAddr,
    password: SessionPassword,
}
enum Action {
    Join(JoinOptions),
    PrepareHost(PendingHost),
    Cancel,
}

#[derive(Resource)]
pub(crate) struct MultiplayerUi {
    pub screen: MenuScreen,
    pub host_setup: bool,
    pub host_settings_tab: bool,
    pub host: ConnectionDraft,
    pub join: ConnectionDraft,
    pub selected_save: Option<(SaveId, String)>,
    pub submitted: bool,
    pub error: Option<UiError>,
    connecting: bool,
    owns_session: bool,
    pending_host: Option<PendingHost>,
    action: Option<Action>,
}
impl Default for MultiplayerUi {
    fn default() -> Self {
        Self {
            screen: MenuScreen::Title,
            host_setup: false,
            host_settings_tab: true,
            host: ConnectionDraft::new("0.0.0.0:27015"),
            join: ConnectionDraft::new("127.0.0.1:27015"),
            selected_save: None,
            submitted: false,
            error: None,
            connecting: false,
            owns_session: false,
            pending_host: None,
            action: None,
        }
    }
}
impl MultiplayerUi {
    pub fn navigate(&mut self, screen: MenuScreen) {
        self.host.clear_password();
        self.join.clear_password();
        self.error = None;
        self.selected_save = None;
        self.submitted = false;
        self.screen = screen;
    }
    pub fn connection_screen(&self, status: &NetworkStatus) -> bool {
        self.connecting
            || (status.role == Some(RuntimeRole::Client) && status.phase != RuntimePhase::Ready)
    }
    pub fn submit_host(&mut self) {
        if self.submitted {
            return;
        }
        match self.host.take(true) {
            Ok((address, password)) => {
                self.submitted = true;
                self.connecting = true;
                self.owns_session = true;
                self.error = None;
                self.action = Some(Action::PrepareHost(PendingHost { address, password }));
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn submit_join(&mut self, profile: &PlayerSettingsState) {
        if self.submitted {
            return;
        }
        match self.join.take(false) {
            Ok((address, password)) => {
                self.submitted = true;
                self.connecting = true;
                self.owns_session = true;
                self.error = None;
                self.action = Some(Action::Join(JoinOptions {
                    address,
                    password,
                    display_name: profile.current.display_name.clone(),
                    cached_image: None,
                }));
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn cancel(&mut self) {
        self.host.clear_password();
        self.join.clear_password();
        self.pending_host = None;
        self.action = Some(Action::Cancel);
        self.submitted = true;
        self.screen = MenuScreen::Multiplayer;
    }
    pub fn return_to_title(&mut self) {
        self.cancel();
        self.screen = MenuScreen::Title;
    }
}

/// Runs after all screens have issued their one-shot actions, within the egui pass.
pub(crate) fn process_actions(world: &mut World) {
    let Some(action) = world.resource_mut::<MultiplayerUi>().action.take() else {
        return;
    };
    match action {
        Action::Cancel => {
            let screen = world.resource::<MultiplayerUi>().screen;
            *world.resource_mut::<MultiplayerUi>() = MultiplayerUi {
                screen,
                ..default()
            };
            puzzella_game::network::runtime::stop_session(world);
        }
        Action::Join(options) => {
            #[cfg(feature = "gns")]
            let result = puzzella_game::network::runtime::start_join(world, options);
            #[cfg(not(feature = "gns"))]
            let result: Result<(), RuntimeStartError> = {
                drop(options);
                Err(RuntimeStartError::InvalidWorld)
            };
            if let Err(error) = result {
                world.resource_mut::<MultiplayerUi>().error = Some(UiError::start(error));
            }
        }
        Action::PrepareHost(host) => {
            world.resource_mut::<MultiplayerUi>().pending_host = Some(host);
            world
                .resource_mut::<PersistenceState>()
                .retain_image_for_host = true;
            let selected = world
                .resource::<MultiplayerUi>()
                .selected_save
                .as_ref()
                .map(|s| s.0);
            if let Some(id) = selected {
                let limits = world.resource::<PuzzleImageLimits>().decode_limits(
                    &world
                        .resource::<puzzella_game::image_settings::ImageSettingsState>()
                        .current,
                );
                world.resource_scope(|world, service: Mut<PersistenceService>| {
                    service.load_for_host(
                        &mut world.resource_mut::<PersistenceState>(),
                        id,
                        limits,
                    );
                });
            } else {
                world
                    .resource_mut::<NextState<AppState>>()
                    .set(AppState::InGame);
            }
        }
    }
}

/// First runs before state transitions and gameplay input. Listen only after CPU
/// initialization and the existing GPU readiness barrier have both completed.
pub(crate) fn start_prepared_host(world: &mut World) {
    let menu_pending = matches!(
        world.get_resource::<NextState<AppState>>(),
        Some(NextState::Pending(AppState::Menu) | NextState::PendingIfNeq(AppState::Menu))
    );
    if menu_pending {
        world.resource_mut::<MultiplayerUi>().pending_host = None;
        return;
    }
    if world.resource::<MultiplayerUi>().pending_host.is_none() {
        return;
    }
    let app_state = *world.resource::<State<AppState>>().get();
    if app_state == AppState::Menu {
        let persistence = world.resource::<PersistenceState>();
        if !persistence.busy && persistence.error.is_some() {
            let mut ui = world.resource_mut::<MultiplayerUi>();
            ui.pending_host = None;
            ui.error = Some(UiError::PuzzleUnavailable);
        }
        return;
    }
    if !matches!(app_state, AppState::InGame | AppState::GameComplete) {
        return;
    }
    let progress = world.resource::<PieceGenerationProgress>();
    if progress.error.is_some() {
        world.resource_mut::<MultiplayerUi>().pending_host = None;
        world.resource_mut::<MultiplayerUi>().error = Some(UiError::PuzzleUnavailable);
        return;
    }
    if progress.is_generating
        || progress.receiver.is_some()
        || progress.generation_phase != GenerationPhase::Completed
    {
        return;
    }
    let store = world.resource::<PieceDataStore>();
    if !world
        .get_resource::<puzzella_game::render::RenderReady>()
        .is_some_and(|ready| ready.is_ready(store.epoch))
    {
        return;
    }
    let host = world
        .resource_mut::<MultiplayerUi>()
        .pending_host
        .take()
        .unwrap();
    let Some(image_hash) = world
        .get_resource::<OriginalPuzzleImage>()
        .filter(|image| image.encoded.is_some())
        .map(|image| image.hash)
    else {
        world.resource_mut::<MultiplayerUi>().error = Some(UiError::ImageUnavailable);
        return;
    };
    #[cfg(feature = "gns")]
    let result = {
        use puzzella_core::session::{SessionDefinition, SessionId};
        let options = puzzella_game::network::runtime::HostOptions {
            address: host.address,
            password: host.password,
            display_name: world
                .resource::<PlayerSettingsState>()
                .current
                .display_name
                .clone(),
            host: world.resource::<LocalPlayerId>().0,
            session: SessionDefinition {
                id: SessionId(rand::random()),
                image_hash,
            },
        };
        puzzella_game::network::runtime::start_host(world, options)
    };
    #[cfg(not(feature = "gns"))]
    let result: Result<SocketAddr, RuntimeStartError> = {
        let _ = (host.address, host.password, image_hash);
        Err(RuntimeStartError::InvalidWorld)
    };
    let mut ui = world.resource_mut::<MultiplayerUi>();
    match result {
        Ok(_) => {
            ui.connecting = false;
            ui.host_setup = false;
            ui.submitted = false;
        }
        Err(error) => ui.error = Some(UiError::start(error)),
    }
}

pub(crate) fn reset_on_menu(mut ui: ResMut<MultiplayerUi>, status: Res<NetworkStatus>) {
    ui.host.clear_password();
    ui.join.clear_password();
    ui.host_setup = false;
    ui.pending_host = None;
    if ui.owns_session
        && status.error.is_some()
        && matches!(
            status.phase,
            RuntimePhase::Failed | RuntimePhase::Disconnected
        )
    {
        ui.connecting = true;
    }
    if !ui.connecting {
        ui.submitted = false;
        ui.owns_session = false;
        ui.selected_save = None;
        if ui.screen != MenuScreen::Multiplayer {
            ui.screen = MenuScreen::Title;
        }
    }
}

pub(crate) fn paint_host_status(ui: &mut egui::Ui, status: &NetworkStatus, i18n: &Localization) {
    if status.role != Some(RuntimeRole::Host) {
        return;
    }
    let Some(address) = status.address else {
        return;
    };
    theme::hint(
        ui,
        i18n.format(
            "multiplayer-listening",
            &[("address", address.to_string().as_str().into())],
        ),
    );
    if address.ip().is_unspecified() {
        theme::hint(
            ui,
            i18n.format(
                "multiplayer-invite-port",
                &[("port", u32::from(address.port()).into())],
            ),
        );
    } else if address.ip().is_loopback() {
        theme::hint(ui, i18n.text("multiplayer-loopback"));
    } else {
        theme::hint(
            ui,
            i18n.format(
                "multiplayer-invite-address",
                &[("address", address.to_string().as_str().into())],
            ),
        );
    }
    theme::hint(ui, i18n.text("multiplayer-reachable-hint"));
}

pub(crate) fn paint_connection_fields(
    ui: &mut egui::Ui,
    draft: &mut ConnectionDraft,
    host: bool,
    profile: &PlayerSettingsState,
    i18n: &Localization,
) {
    theme::section(
        ui,
        "MP",
        i18n.text(if host {
            "multiplayer-host-settings"
        } else {
            "multiplayer-join"
        }),
    );
    let name = profile
        .current
        .display_name
        .as_ref()
        .map(|n| n.as_ref().to_owned())
        .unwrap_or_else(|| i18n.text("multiplayer-default-name"));
    theme::hint(
        ui,
        i18n.format("multiplayer-player-name", &[("name", name.as_str().into())]),
    );
    ui.label(i18n.text(if host {
        "multiplayer-bind-address"
    } else {
        "multiplayer-server-address"
    }));
    ui.add(egui::TextEdit::singleline(&mut draft.address).desired_width(f32::INFINITY));
    theme::hint(
        ui,
        i18n.text(if host {
            "multiplayer-bind-hint"
        } else {
            "multiplayer-address-hint"
        }),
    );
    if parse_address(&draft.address, host).is_err() {
        ui.colored_label(theme::DANGER, i18n.text("multiplayer-error-address"));
    }
    ui.label(i18n.text("multiplayer-password"));
    let mut password = egui::TextEdit::singleline(&mut *draft.password)
        .id(egui::Id::new(if host {
            "multiplayer-host-password"
        } else {
            "multiplayer-join-password"
        }))
        .password(true)
        .char_limit(MAX_PASSWORD_BYTES)
        .desired_width(f32::INFINITY)
        .show(ui);
    // egui records plain text in its undo state even for masked fields. Discard
    // that state each frame so closing/submitting leaves only the owned secret.
    password.state.clear_undoer();
    password.state.store(ui.ctx(), password.response.id);
    theme::hint(ui, i18n.text("multiplayer-password-hint"));
    if !draft.password.is_empty()
        && !(MIN_PASSWORD_BYTES..=MAX_PASSWORD_BYTES).contains(&draft.password.len())
    {
        ui.colored_label(theme::DANGER, i18n.text("multiplayer-error-password"));
    }
    if host {
        theme::hint(ui, i18n.text("multiplayer-reachable-hint"));
    }
}

pub(crate) fn paint_join(
    ui: &mut egui::Ui,
    state: &mut MultiplayerUi,
    profile: &PlayerSettingsState,
    i18n: &Localization,
) {
    paint_connection_fields(ui, &mut state.join, false, profile, i18n);
    theme::hint(ui, i18n.text("multiplayer-join-hint"));
    if let Some(error) = state.error {
        ui.colored_label(theme::DANGER, i18n.text(error.key()));
    }
    let valid = state.join.valid(false) && !state.submitted && cfg!(feature = "gns");
    ui.add_enabled_ui(valid, |ui| {
        if theme::button(
            ui,
            i18n.text("multiplayer-connect"),
            ui.available_width(),
            true,
        )
        .clicked()
        {
            state.submit_join(profile);
        }
    });
}

fn connection_text(status: &NetworkStatus) -> &'static str {
    match status.phase {
        RuntimePhase::Connecting => "multiplayer-connecting",
        RuntimePhase::Authenticating => "multiplayer-authenticating",
        RuntimePhase::Syncing(
            SyncPhase::ImageNegotiation
            | SyncPhase::AwaitingImageSlot
            | SyncPhase::ImageTransfer
            | SyncPhase::AwaitingImageReady,
        ) => "multiplayer-receiving-image",
        RuntimePhase::Syncing(_) => "multiplayer-syncing",
        RuntimePhase::Ready => "multiplayer-ready",
        _ => "multiplayer-connecting",
    }
}

pub(crate) fn draw_connection_ui(
    mut contexts: EguiContexts,
    i18n: Res<Localization>,
    status: Res<NetworkStatus>,
    mut state: ResMut<MultiplayerUi>,
) {
    if status.role == Some(RuntimeRole::Client) && status.phase == RuntimePhase::Ready {
        state.connecting = false;
        state.submitted = false;
        return;
    }
    if !state.connection_screen(&status) {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    theme::background(ctx);
    let screen = ctx.content_rect();
    let failed = matches!(
        status.phase,
        RuntimePhase::Failed | RuntimePhase::Disconnected
    ) && state.pending_host.is_none()
        && state.action.is_none();
    let error = state
        .error
        .or_else(|| failed.then(|| UiError::diagnostic(status.error.as_deref())));
    egui::Area::new("multiplayer_connection".into())
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            theme::frame().show(ui, |ui| {
                ui.set_width((screen.width() - 96.0).clamp(160.0, 480.0));
                egui::ScrollArea::vertical()
                    .max_height((screen.height() - 160.0).max(80.0))
                    .show(ui, |ui| {
                        theme::heading(ui, i18n.text("menu-multiplayer"));
                        if let Some(error) = error {
                            ui.colored_label(theme::DANGER, i18n.text(error.key()));
                        } else {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(i18n.text(
                                    if state.pending_host.is_some()
                                        || matches!(state.action, Some(Action::PrepareHost(_)))
                                    {
                                        "multiplayer-preparing-host"
                                    } else {
                                        connection_text(&status)
                                    },
                                ));
                            });
                        }
                        ui.add_space(12.0);
                        if theme::button(
                            ui,
                            i18n.text(if error.is_some() {
                                "common-back"
                            } else {
                                "common-cancel"
                            }),
                            ui.available_width(),
                            false,
                        )
                        .clicked()
                        {
                            state.cancel();
                        }
                    });
            });
        });
}

#[cfg(test)]
mod tests;
