//! Memory-only menu drafts and the handoff to the existing network runtime.
use crate::{localization::Localization, theme};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use jigsall_game::{
    network::{
        address::{AddressResolution, ResolutionError, ServerAddress},
        auth::{SessionPassword, MAX_PASSWORD_CHARS},
        runtime::{
            HostOptions, HostStartRequest, JoinOptions, NetworkFailureKind, NetworkStatus,
            RendezvousControlStatus, RuntimeConnectionMethod, RuntimePhase, RuntimeRole,
            RuntimeStartError,
        },
        syncing::SyncPhase,
    },
    persistence::{runtime::*, SaveId},
    player_settings::PlayerSettingsState,
    resources::*,
};
use std::{net::SocketAddr, time::Instant};
use zeroize::{Zeroize, Zeroizing};

const PASSWORD_DRAFT_CAPACITY: usize = MAX_PASSWORD_CHARS * 4;

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
    RoomCode,
    RoomNotFound,
    InternetUnavailable,
    Resolution,
    Password,
    WrongPassword,
    Timeout,
    ServerFull,
    ConnectionFailed,
    ConnectionLost,
    ProtocolMismatch,
    ImageUnavailable,
    PuzzleUnavailable,
    AlreadyActive,
}
impl UiError {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::RoomCode => "multiplayer-error-room-code",
            Self::RoomNotFound => "multiplayer-error-room-not-found",
            Self::InternetUnavailable => "multiplayer-internet-unavailable",
            Self::Address => "multiplayer-error-address",
            Self::Resolution => "multiplayer-error-resolution",
            Self::Password => "multiplayer-error-password",
            Self::WrongPassword => "multiplayer-error-wrong-password",
            Self::Timeout => "multiplayer-error-timeout",
            Self::ServerFull => "multiplayer-error-full",
            Self::ConnectionFailed => "multiplayer-error-connection",
            Self::ConnectionLost => "multiplayer-error-connection-lost",
            Self::ProtocolMismatch => "multiplayer-error-protocol",
            Self::ImageUnavailable => "multiplayer-error-image",
            Self::PuzzleUnavailable => "multiplayer-error-puzzle",
            Self::AlreadyActive => "multiplayer-error-active",
        }
    }
    fn key_for_method(self, method: RuntimeConnectionMethod) -> &'static str {
        match (self, method) {
            (Self::Timeout, RuntimeConnectionMethod::Internet) => "multiplayer-error-room-timeout",
            (Self::ConnectionFailed, RuntimeConnectionMethod::Internet) => {
                "multiplayer-error-room-connection"
            }
            _ => self.key(),
        }
    }
    fn failure(kind: NetworkFailureKind) -> Self {
        match kind {
            NetworkFailureKind::Authentication => Self::WrongPassword,
            NetworkFailureKind::Timeout => Self::Timeout,
            NetworkFailureKind::Capacity => Self::ServerFull,
            NetworkFailureKind::Protocol => Self::ProtocolMismatch,
            NetworkFailureKind::Image => Self::ImageUnavailable,
            NetworkFailureKind::RoomNotFound => Self::RoomNotFound,
            NetworkFailureKind::Connection => Self::ConnectionFailed,
            NetworkFailureKind::ConnectionLost => Self::ConnectionLost,
        }
    }
    fn start(error: RuntimeStartError) -> Self {
        match error {
            RuntimeStartError::AlreadyActive => Self::AlreadyActive,
            RuntimeStartError::DefinitionUnavailable | RuntimeStartError::InvalidWorld => {
                Self::PuzzleUnavailable
            }
            RuntimeStartError::ImageHashMismatch => Self::ImageUnavailable,
            RuntimeStartError::InternetUnavailable => Self::InternetUnavailable,
            #[cfg(feature = "rendezvous")]
            RuntimeStartError::Rendezvous(error) => {
                Self::failure(NetworkFailureKind::rendezvous(&error))
            }
            RuntimeStartError::Transport(error) => {
                Self::failure(NetworkFailureKind::transport(&error))
            }
        }
    }
}

pub(crate) struct ConnectionDraft {
    pub address: String,
    pub room_code: String,
    pub method: RuntimeConnectionMethod,
    player_name_draft: Option<String>,
    // No serialization, Debug, or Clone; dropping/replacing a draft wipes its buffer.
    pub password: Zeroizing<String>,
    invitation: Zeroizing<String>,
}
impl ConnectionDraft {
    fn new(address: &str) -> Self {
        Self {
            address: address.into(),
            room_code: String::new(),
            method: RuntimeConnectionMethod::DirectIp,
            player_name_draft: None,
            password: Zeroizing::new(String::with_capacity(PASSWORD_DRAFT_CAPACITY)),
            invitation: Zeroizing::new(String::with_capacity(PASSWORD_DRAFT_CAPACITY + 512)),
        }
    }
    pub fn valid(&self, host: bool) -> bool {
        (match self.method {
            RuntimeConnectionMethod::DirectIp => valid_address(&self.address, host),
            RuntimeConnectionMethod::Internet => host || valid_room_code(&self.room_code),
        }) && ((host && self.password.is_empty())
            || SessionPassword::validate(&self.password).is_ok())
    }
    fn take_password(&mut self) -> Result<SessionPassword, UiError> {
        SessionPassword::new(std::mem::replace(
            &mut *self.password,
            String::with_capacity(PASSWORD_DRAFT_CAPACITY),
        ))
        .map_err(|_| UiError::Password)
    }
    pub fn clear_password(&mut self) {
        self.invitation.zeroize();
        if self.password.capacity() == PASSWORD_DRAFT_CAPACITY {
            self.password.zeroize();
        } else {
            // Release oversized raw drafts on navigation/settings changes.
            self.password = Zeroizing::new(String::with_capacity(PASSWORD_DRAFT_CAPACITY));
        }
    }
    fn import_invitation(&mut self) -> bool {
        let mut parts = self.invitation.splitn(4, '|');
        if parts.next() != Some("jigsall-invite-v1") {
            return false;
        }
        let method = match parts.next() {
            Some("internet") => RuntimeConnectionMethod::Internet,
            Some("direct") => RuntimeConnectionMethod::DirectIp,
            _ => return false,
        };
        let Some(target) = parts.next() else {
            return false;
        };
        let Some(secret) = parts.next() else {
            return false;
        };
        if SessionPassword::validate(secret).is_err()
            || !(match method {
                RuntimeConnectionMethod::Internet => valid_room_code(target),
                RuntimeConnectionMethod::DirectIp => valid_address(target, false),
            })
        {
            return false;
        }
        let target = target.to_owned();
        let secret = Zeroizing::new(secret.to_owned());
        self.clear_password();
        self.password.push_str(&secret);
        self.method = method;
        match method {
            RuntimeConnectionMethod::Internet => self.room_code = target.to_ascii_uppercase(),
            RuntimeConnectionMethod::DirectIp => self.address = target,
        }
        true
    }
}

/// Preserve full raw/IME input while wiping any allocation retired during growth.
struct PasswordBuffer<'a>(&'a mut Zeroizing<String>);
impl egui::TextBuffer for PasswordBuffer<'_> {
    fn is_mutable(&self) -> bool {
        true
    }
    fn as_str(&self) -> &str {
        self.0.as_str()
    }
    fn insert_text(&mut self, text: &str, char_index: egui::text::CharIndex) -> usize {
        let required_capacity = self.0.len() + text.len();
        if required_capacity > self.0.capacity() {
            // Decomposed and invalid drafts can exceed the normalized limit.
            // Never reserve on the live String: it would free unwiped storage.
            let capacity = required_capacity.max(self.0.capacity().saturating_mul(2));
            let mut replacement = Zeroizing::new(String::with_capacity(capacity));
            replacement.push_str(self.0.as_str());
            std::mem::swap(self.0, &mut replacement);
            // replacement now owns the old allocation and zeroizes it on drop.
        }
        egui::TextBuffer::insert_text(&mut **self.0, text, char_index)
    }
    fn delete_char_range(&mut self, char_range: std::ops::Range<egui::text::CharIndex>) {
        egui::TextBuffer::delete_char_range(&mut **self.0, char_range);
    }
    fn clear(&mut self) {
        self.0.zeroize();
    }
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<PasswordBuffer<'static>>()
    }
}

fn valid_room_code(value: &str) -> bool {
    #[cfg(feature = "rendezvous")]
    {
        value
            .parse::<jigsall_game::network::runtime::RoomCode>()
            .is_ok()
    }
    #[cfg(not(feature = "rendezvous"))]
    {
        let _ = value;
        false
    }
}
fn parse_bind_address(value: &str) -> Result<SocketAddr, UiError> {
    let address: SocketAddr = value.trim().parse().map_err(|_| UiError::Address)?;
    if address.port() == 0 || address.ip().to_canonical().is_multicast() {
        return Err(UiError::Address);
    }
    Ok(address)
}

fn valid_address(value: &str, host: bool) -> bool {
    if host {
        parse_bind_address(value).is_ok()
    } else {
        value.parse::<ServerAddress>().is_ok()
    }
}

struct JoinRequest {
    address: ServerAddress,
    password: SessionPassword,
    display_name: Option<jigsall_core::PlayerDisplayName>,
}
struct PendingJoin {
    resolution: AddressResolution,
    password: SessionPassword,
    display_name: Option<jigsall_core::PlayerDisplayName>,
}

struct PendingDirectHost {
    address: SocketAddr,
    password: SessionPassword,
}
enum PendingHost {
    Direct(PendingDirectHost),
    #[cfg(feature = "rendezvous")]
    Internet(SessionPassword),
}
enum Action {
    Join(JoinRequest),
    #[cfg(feature = "rendezvous")]
    JoinInternet(jigsall_game::network::runtime::RendezvousJoinOptions),
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
    pub internet_available: bool,
    method_selected: bool,
    password_cleared: bool,
    invite_shown: bool,
    invite_open: bool,
    invite_secret: Option<Zeroizing<String>>,
    invite_address: String,
    disconnected_save_opened: bool,
    connecting: bool,
    owns_session: bool,
    pending_host: Option<PendingHost>,
    retry_host: Option<HostStartRequest>,
    prepared_host: bool,
    editing_host_retry: bool,
    retrying: bool,
    pending_join: Option<PendingJoin>,
    action: Option<Action>,
}
impl Default for MultiplayerUi {
    fn default() -> Self {
        Self {
            screen: MenuScreen::Title,
            host_setup: false,
            host_settings_tab: false,
            host: ConnectionDraft::new("0.0.0.0:43576"),
            join: ConnectionDraft::new(""),
            selected_save: None,
            submitted: false,
            error: None,
            internet_available: false,
            method_selected: false,
            password_cleared: false,
            invite_shown: false,
            invite_open: false,
            invite_secret: None,
            invite_address: String::new(),
            disconnected_save_opened: false,
            connecting: false,
            owns_session: false,
            pending_host: None,
            retry_host: None,
            prepared_host: false,
            editing_host_retry: false,
            retrying: false,
            pending_join: None,
            action: None,
        }
    }
}
impl MultiplayerUi {
    pub(crate) fn paint_invite_button(&mut self, ui: &mut egui::Ui, i18n: &Localization) {
        if self.invite_secret.is_some() && ui.button(i18n.text("multiplayer-invite-open")).clicked()
        {
            self.invite_open = true;
        }
    }
    pub fn configure_connection_methods(&mut self, available: bool) {
        self.internet_available = available && cfg!(feature = "rendezvous");
        let method = if !self.internet_available {
            RuntimeConnectionMethod::DirectIp
        } else if !self.method_selected {
            RuntimeConnectionMethod::Internet
        } else {
            self.host.method
        };
        self.set_connection_method(method);
    }
    fn set_connection_method(&mut self, method: RuntimeConnectionMethod) {
        if self.host.method != method || self.join.method != method {
            self.clear_passwords_for_settings();
            self.error = None;
            self.host.method = method;
            self.join.method = method;
        }
    }
    pub fn clear_passwords_for_settings(&mut self) {
        self.password_cleared |= !self.host.password.is_empty() || !self.join.password.is_empty();
        self.host.clear_password();
        self.join.clear_password();
    }
    pub fn paint_password_notice(&self, ui: &mut egui::Ui, i18n: &Localization) {
        if self.password_cleared && self.host.password.is_empty() && self.join.password.is_empty() {
            theme::hint(ui, i18n.text("multiplayer-password-cleared"));
        }
    }
    pub fn host_available(&self) -> bool {
        self.host.valid(true)
            && cfg!(feature = "gns")
            && (self.host.method == RuntimeConnectionMethod::DirectIp || self.internet_available)
    }
    pub fn navigate(&mut self, screen: MenuScreen) {
        self.invite_secret = None;
        self.invite_address.clear();
        self.host.clear_password();
        self.join.clear_password();
        self.host.player_name_draft = None;
        self.join.player_name_draft = None;
        self.error = None;
        self.password_cleared = false;
        self.selected_save = None;
        self.pending_join = None;
        self.action = None;
        self.connecting = false;
        self.submitted = false;
        self.screen = screen;
    }
    pub fn connection_screen(&self, status: &NetworkStatus) -> bool {
        self.connecting
            || (status.role == Some(RuntimeRole::Client) && status.phase != RuntimePhase::Ready)
            || (status.role == Some(RuntimeRole::Host) && status.phase == RuntimePhase::Connecting)
    }
    pub fn submit_host(&mut self) {
        if self.submitted {
            return;
        }
        if self.host.password.is_empty() {
            match SessionPassword::generate() {
                Ok(password) => self.host.password.push_str(&password.invitation_secret()),
                Err(_) => {
                    self.error = Some(UiError::Password);
                    return;
                }
            }
        }
        if self.host.method == RuntimeConnectionMethod::Internet
            && (!cfg!(feature = "rendezvous") || !self.internet_available)
        {
            self.error = Some(UiError::InternetUnavailable);
            return;
        }
        #[cfg(feature = "rendezvous")]
        if self.host.method == RuntimeConnectionMethod::Internet {
            if !self.internet_available {
                self.error = Some(UiError::InternetUnavailable);
                return;
            }
            match self.host.take_password() {
                Ok(password) => {
                    self.invite_secret = Some(password.invitation_secret());
                    self.submitted = true;
                    self.connecting = true;
                    self.owns_session = true;
                    self.error = None;
                    self.action = Some(Action::PrepareHost(PendingHost::Internet(password)));
                }
                Err(error) => self.error = Some(error),
            }
            return;
        }
        match parse_bind_address(&self.host.address).and_then(|address| {
            self.host
                .take_password()
                .map(|password| (address, password))
        }) {
            Ok((address, password)) => {
                self.invite_secret = Some(password.invitation_secret());
                self.submitted = true;
                self.connecting = true;
                self.owns_session = true;
                self.error = None;
                self.action = Some(Action::PrepareHost(PendingHost::Direct(
                    PendingDirectHost { address, password },
                )));
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn recover_prepared_host(&mut self, error: UiError) {
        self.prepared_host = true;
        self.editing_host_retry = true;
        self.connecting = true;
        self.submitted = false;
        self.retrying = false;
        self.host.clear_password();
        self.error = Some(error);
    }
    fn submit_join(&mut self, profile: &PlayerSettingsState) {
        if self.submitted {
            return;
        }
        if self.join.method == RuntimeConnectionMethod::Internet {
            if !cfg!(feature = "rendezvous") || !self.internet_available {
                self.error = Some(UiError::InternetUnavailable);
                return;
            }
            if !valid_room_code(&self.join.room_code) {
                self.error = Some(UiError::RoomCode);
                return;
            }
        }
        #[cfg(feature = "rendezvous")]
        if self.join.method == RuntimeConnectionMethod::Internet {
            if !self.internet_available {
                self.error = Some(UiError::InternetUnavailable);
                return;
            }
            let room = self
                .join
                .room_code
                .parse::<jigsall_game::network::runtime::RoomCode>()
                .map_err(|_| UiError::RoomCode);
            match room.and_then(|room_code| {
                self.join
                    .take_password()
                    .map(|password| (room_code, password))
            }) {
                Ok((room_code, password)) => {
                    self.join.room_code = room_code.to_string();
                    self.submitted = true;
                    self.connecting = true;
                    self.owns_session = true;
                    self.error = None;
                    self.action = Some(Action::JoinInternet(
                        jigsall_game::network::runtime::RendezvousJoinOptions {
                            display_name: profile.current.display_name.clone(),
                            room_code,
                            password,
                            cached_image: None,
                        },
                    ));
                }
                Err(error) => self.error = Some(error),
            }
            return;
        }
        match self
            .join
            .address
            .parse::<ServerAddress>()
            .map_err(|_| UiError::Address)
            .and_then(|address| {
                self.join
                    .take_password()
                    .map(|password| (address, password))
            }) {
            Ok((address, password)) => {
                self.submitted = true;
                self.connecting = true;
                self.owns_session = true;
                self.error = None;
                self.action = Some(Action::Join(JoinRequest {
                    address,
                    password,
                    display_name: profile.current.display_name.clone(),
                }));
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn cancel(&mut self) {
        self.invite_secret = None;
        self.invite_address.clear();
        self.host.clear_password();
        self.join.clear_password();
        self.pending_host = None;
        self.retry_host = None;
        self.prepared_host = false;
        self.retrying = false;
        self.editing_host_retry = false;
        self.pending_join = None;
        self.action = Some(Action::Cancel);
        self.submitted = true;
        self.screen = MenuScreen::Multiplayer;
    }
    pub fn return_to_title(&mut self) {
        self.cancel();
        self.screen = MenuScreen::Title;
    }
}

pub(crate) fn connection_screen_hidden(ui: Res<MultiplayerUi>, status: Res<NetworkStatus>) -> bool {
    !ui.connection_screen(&status)
}
pub(crate) fn disconnected_game_available(status: Res<NetworkStatus>) -> bool {
    status.has_disconnected_game()
}

pub(crate) fn sync_local_gameplay_block(
    ui: Res<MultiplayerUi>,
    status: Res<NetworkStatus>,
    settings: Res<crate::settings::SettingsDialog>,
    next: Res<NextState<AppState>>,
    mut blocked: ResMut<LocalGameplayBlocked>,
) {
    let value = settings.open
        || ui.connection_screen(&status)
        || matches!(
            *next,
            NextState::Pending(AppState::Menu) | NextState::PendingIfDifferent(AppState::Menu)
        );
    if blocked.0 != value {
        blocked.0 = value;
    }
}

/// Runs after all screens have issued their one-shot actions, within the egui pass.
pub(crate) fn process_actions(world: &mut World) {
    let Some(action) = world.resource_mut::<MultiplayerUi>().action.take() else {
        finish_address_resolution(world, Instant::now());
        return;
    };
    match action {
        Action::Cancel => {
            let mut ui = world.resource_mut::<MultiplayerUi>();
            let screen = ui.screen;
            let host_address = std::mem::take(&mut ui.host.address);
            let join_address = std::mem::take(&mut ui.join.address);
            let room_code = std::mem::take(&mut ui.join.room_code);
            let host_method = ui.host.method;
            let join_method = ui.join.method;
            let internet_available = ui.internet_available;
            let method_selected = ui.method_selected;
            *world.resource_mut::<MultiplayerUi>() = MultiplayerUi {
                screen,
                ..default()
            };
            let mut ui = world.resource_mut::<MultiplayerUi>();
            ui.host.address = host_address;
            ui.join.address = join_address;
            ui.join.room_code = room_code;
            ui.host.method = host_method;
            ui.join.method = join_method;
            ui.internet_available = internet_available;
            ui.method_selected = method_selected;
            jigsall_game::network::runtime::stop_session(world);
        }
        #[cfg(feature = "rendezvous")]
        Action::JoinInternet(options) => {
            if let Err(error) =
                jigsall_game::network::runtime::start_rendezvous_join(world, options)
            {
                world.resource_mut::<MultiplayerUi>().error = Some(UiError::start(error));
            }
        }
        Action::Join(request) => {
            if let Some(address) = request.address.socket_addr() {
                start_resolved_join(world, address, request.password, request.display_name);
            } else {
                match request.address.resolve() {
                    Ok(resolution) => {
                        world.resource_mut::<MultiplayerUi>().pending_join = Some(PendingJoin {
                            resolution,
                            password: request.password,
                            display_name: request.display_name,
                        });
                    }
                    Err(error) => resolution_failed(world, error),
                }
            }
        }
        Action::PrepareHost(host) => {
            world.resource_mut::<MultiplayerUi>().pending_host = Some(host);
            // Retry the current generated/loaded puzzle without reentering InGame
            // or loading selected_save again.
            if world.resource::<MultiplayerUi>().prepared_host {
                return;
            }
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
                        .resource::<jigsall_game::image_settings::ImageSettingsState>()
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

fn finish_address_resolution(world: &mut World, now: Instant) {
    let mut ui = world.resource_mut::<MultiplayerUi>();
    let Some(result) = ui
        .pending_join
        .as_mut()
        .and_then(|join| join.resolution.poll(now))
    else {
        return;
    };
    let join = ui.pending_join.take().unwrap();
    match result {
        Ok(address) => start_resolved_join(world, address, join.password, join.display_name),
        Err(error) => resolution_failed(world, error),
    }
}

fn resolution_failed(world: &mut World, error: ResolutionError) {
    warn!("Multiplayer address resolution failed: {error:?}");
    world.resource_mut::<MultiplayerUi>().error = Some(UiError::Resolution);
}

fn start_resolved_join(
    world: &mut World,
    address: SocketAddr,
    password: SessionPassword,
    display_name: Option<jigsall_core::PlayerDisplayName>,
) {
    let options = JoinOptions {
        address,
        password,
        display_name,
        cached_image: None,
    };
    #[cfg(feature = "gns")]
    let result = jigsall_game::network::runtime::start_join(world, options);
    #[cfg(not(feature = "gns"))]
    let result: Result<(), RuntimeStartError> = {
        drop(options);
        Err(RuntimeStartError::InvalidWorld)
    };
    if let Err(error) = result {
        world.resource_mut::<MultiplayerUi>().error = Some(UiError::start(error));
    }
}

/// First runs before state transitions and gameplay input. Listen only after CPU
/// initialization and the existing GPU readiness barrier have both completed.
pub(crate) fn start_prepared_host(world: &mut World) {
    let menu_pending = matches!(
        world.get_resource::<NextState<AppState>>(),
        Some(NextState::Pending(AppState::Menu) | NextState::PendingIfDifferent(AppState::Menu))
    );
    if menu_pending {
        let mut ui = world.resource_mut::<MultiplayerUi>();
        ui.pending_host = None;
        ui.retry_host = None;
        return;
    }
    if world.resource::<MultiplayerUi>().pending_host.is_none()
        && !world.resource::<MultiplayerUi>().retrying
    {
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
        let mut ui = world.resource_mut::<MultiplayerUi>();
        ui.pending_host = None;
        ui.retry_host = None;
        ui.retrying = false;
        ui.error = Some(UiError::PuzzleUnavailable);
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
        .get_resource::<jigsall_game::render::RenderReady>()
        .is_some_and(|ready| ready.is_ready(store.epoch))
    {
        return;
    }
    let Some(image_hash) = world
        .get_resource::<OriginalPuzzleImage>()
        .filter(|image| image.encoded.is_some())
        .map(|image| image.hash)
    else {
        let mut ui = world.resource_mut::<MultiplayerUi>();
        ui.pending_host = None;
        ui.retry_host = None;
        ui.retrying = false;
        ui.error = Some(UiError::ImageUnavailable);
        return;
    };
    use jigsall_core::session::{SessionDefinition, SessionId};
    #[allow(unused_mut)]
    let mut request = if world.resource::<MultiplayerUi>().retrying {
        world
            .resource_mut::<MultiplayerUi>()
            .retry_host
            .take()
            .unwrap()
    } else {
        let pending = world
            .resource_mut::<MultiplayerUi>()
            .pending_host
            .take()
            .unwrap();
        let session = SessionDefinition {
            id: SessionId(rand::random()),
            image_hash,
        };
        let host = world.resource::<LocalPlayerId>().0;
        let display_name = world
            .resource::<PlayerSettingsState>()
            .current
            .display_name
            .clone();
        #[cfg(feature = "rendezvous")]
        let direct = match pending {
            PendingHost::Direct(direct) => direct,
            #[cfg(feature = "rendezvous")]
            PendingHost::Internet(password) => {
                let result = jigsall_game::network::runtime::start_rendezvous_host(
                    world,
                    jigsall_game::network::runtime::RendezvousHostOptions {
                        display_name,
                        session,
                        host,
                        password,
                    },
                );
                let mut ui = world.resource_mut::<MultiplayerUi>();
                ui.retrying = false;
                match result {
                    Ok(()) => {
                        ui.host_setup = false;
                    } // RoomCreated ends connection UI.
                    Err(error) => ui.recover_prepared_host(UiError::start(error)),
                }
                return;
            }
        };
        #[cfg(not(feature = "rendezvous"))]
        let PendingHost::Direct(direct) = pending;
        HostStartRequest::new(HostOptions {
            address: direct.address,
            password: direct.password,
            display_name,
            host,
            session,
        })
    };
    #[cfg(feature = "gns")]
    let result = request.start(world);
    #[cfg(not(feature = "gns"))]
    let result: Result<SocketAddr, RuntimeStartError> = { Err(RuntimeStartError::InvalidWorld) };
    let mut ui = world.resource_mut::<MultiplayerUi>();
    ui.retrying = false;
    match result {
        Ok(_) => {
            ui.connecting = false;
            ui.host_setup = false;
            ui.submitted = false;
            ui.prepared_host = false;
        }
        Err(error) => {
            if matches!(error, RuntimeStartError::Transport(_)) {
                ui.retry_host = Some(request);
            }
            ui.error = Some(UiError::start(error));
        }
    }
}

pub(crate) fn reset_on_menu(mut ui: ResMut<MultiplayerUi>, status: Res<NetworkStatus>) {
    ui.host.clear_password();
    ui.join.clear_password();
    ui.host_setup = false;
    ui.invite_open = false;
    ui.invite_shown = false;
    ui.pending_host = None;
    ui.retry_host = None;
    ui.prepared_host = false;
    ui.editing_host_retry = false;
    ui.retrying = false;
    if ui.pending_join.take().is_some() {
        ui.connecting = false;
    }
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
        ui.invite_secret = None;
        ui.invite_address.clear();
        ui.selected_save = None;
        if !matches!(ui.screen, MenuScreen::Multiplayer | MenuScreen::Join) {
            ui.screen = MenuScreen::Title;
        }
    }
}

pub(crate) fn host_status_key(status: &NetworkStatus) -> Option<&'static str> {
    if status.role != Some(RuntimeRole::Host) {
        return None;
    }
    Some(if status.phase != RuntimePhase::Hosting {
        "multiplayer-host-not-open"
    } else if status.connection_method == Some(RuntimeConnectionMethod::Internet)
        && (status.rendezvous_control != Some(RendezvousControlStatus::Available)
            || status.room_code.is_none())
    {
        "multiplayer-host-not-accepting"
    } else {
        "multiplayer-hosting"
    })
}

pub(crate) fn paint_host_status(ui: &mut egui::Ui, status: &NetworkStatus, i18n: &Localization) {
    if status.role != Some(RuntimeRole::Host) {
        return;
    }
    if let Some(key) = host_status_key(status) {
        ui.label(i18n.text(key));
    }
    if status.connection_method == Some(RuntimeConnectionMethod::Internet) {
        paint_control_warning(ui, status, i18n);
        if status.room_code.is_some()
            && status.rendezvous_control == Some(RendezvousControlStatus::Available)
        {
            theme::hint(ui, i18n.text("multiplayer-invite-room-steps"));
            theme::hint(ui, i18n.text("multiplayer-invite-password"));
        }
        return;
    }
    let Some(address) = status.address else {
        return;
    };
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
    theme::hint(ui, i18n.text("multiplayer-invite-direct-steps"));
    theme::hint(ui, i18n.text("multiplayer-invite-password"));
    paint_connection_help(ui, true, Some(address), i18n);
}

pub(crate) fn paint_connection_fields(
    ui: &mut egui::Ui,
    draft: &mut ConnectionDraft,
    host: bool,
    profile: &mut PlayerSettingsState,
    i18n: &Localization,
) {
    if !host {
        ui.label(i18n.text("multiplayer-paste-invite"));
        let mut field = egui::TextEdit::singleline(&mut PasswordBuffer(&mut draft.invitation))
            .id(egui::Id::new("multiplayer-invitation"))
            .password(true)
            .desired_width(f32::INFINITY)
            .show(ui);
        field.state.clear_undoer();
        field.state.store(ui.ctx(), field.response.id);
        draft.import_invitation();
    }
    theme::section(
        ui,
        i18n.text(if host {
            "multiplayer-host-settings"
        } else {
            "multiplayer-join"
        }),
    );
    crate::settings::paint_player_settings(
        ui,
        &mut draft.player_name_draft,
        profile,
        i18n,
        &i18n.text(if host {
            "multiplayer-your-player-name"
        } else {
            "settings-player-name"
        }),
    );
    match draft.method {
        RuntimeConnectionMethod::DirectIp => {
            ui.label(i18n.text(if host {
                "multiplayer-bind-address"
            } else {
                "multiplayer-server-address"
            }));
            ui.add(
                egui::TextEdit::singleline(&mut draft.address)
                    .hint_text(if host {
                        "0.0.0.0:43576"
                    } else {
                        "example.com:43576"
                    })
                    .desired_width(f32::INFINITY),
            );
            theme::hint(
                ui,
                i18n.text(if host {
                    "multiplayer-bind-hint"
                } else {
                    "multiplayer-address-hint"
                }),
            );
        }
        RuntimeConnectionMethod::Internet if host => {
            theme::hint(ui, i18n.text("multiplayer-room-code-hint"))
        }
        RuntimeConnectionMethod::Internet => {
            ui.label(i18n.text("multiplayer-room-code"));
            ui.add(
                egui::TextEdit::singleline(&mut draft.room_code)
                    .id_salt("multiplayer-room-code")
                    .desired_width(f32::INFINITY),
            );
            draft.room_code.make_ascii_uppercase();
        }
    }
    ui.label(i18n.text("multiplayer-password"));
    if host {
        theme::hint(ui, i18n.text("multiplayer-auto-password"));
        if ui
            .button(i18n.text("multiplayer-generate-password"))
            .clicked()
        {
            if let Ok(password) = SessionPassword::generate() {
                draft.clear_password();
                draft.password.push_str(&password.invitation_secret());
            }
        }
    }
    // Keep the full draft, including decomposed input and IME composition. A raw
    // char_limit would truncate passwords before the shared NFC length check.
    let mut password = egui::TextEdit::singleline(&mut PasswordBuffer(&mut draft.password))
        .id(egui::Id::new(if host {
            "multiplayer-host-password"
        } else {
            "multiplayer-join-password"
        }))
        .password(true)
        .desired_width(f32::INFINITY)
        .show(ui);
    // egui records plain text in its undo state even for masked fields. Discard
    // that state each frame so closing/submitting leaves only the owned secret.
    password.state.clear_undoer();
    password.state.store(ui.ctx(), password.response.id);
    theme::hint(
        ui,
        i18n.text(if host {
            "multiplayer-password-hint"
        } else {
            "multiplayer-join-password-hint"
        }),
    );
    paint_invalid_fields(ui, draft, host, i18n);
    if draft.method == RuntimeConnectionMethod::DirectIp {
        paint_connection_help(ui, host, None, i18n);
    }
}

/// Show errors for supplied values while leaving empty fields quiet.
fn paint_invalid_fields(
    ui: &mut egui::Ui,
    draft: &ConnectionDraft,
    host: bool,
    i18n: &Localization,
) {
    let target_error = match draft.method {
        RuntimeConnectionMethod::DirectIp
            if !draft.address.trim().is_empty() && !valid_address(&draft.address, host) =>
        {
            Some("multiplayer-error-address")
        }
        RuntimeConnectionMethod::Internet
            if !host
                && !draft.room_code.trim().is_empty()
                && !valid_room_code(&draft.room_code) =>
        {
            Some("multiplayer-error-room-code")
        }
        _ => None,
    };
    if let Some(key) = target_error {
        ui.colored_label(theme::DANGER, i18n.text(key));
    }
    if !draft.password.is_empty() && SessionPassword::validate(&draft.password).is_err() {
        ui.colored_label(theme::DANGER, i18n.text("multiplayer-error-password"));
    }
}

fn paint_connection_help(
    ui: &mut egui::Ui,
    host: bool,
    address: Option<SocketAddr>,
    i18n: &Localization,
) {
    egui::CollapsingHeader::new(i18n.text("multiplayer-network-details"))
        .id_salt(("connection_help", host))
        .show(ui, |ui| {
            if let Some(address) = address {
                theme::hint(
                    ui,
                    i18n.format(
                        "multiplayer-listening",
                        &[("address", address.to_string().as_str().into())],
                    ),
                );
            }
            theme::hint(ui, i18n.text("multiplayer-address-details"));
            if host {
                theme::hint(ui, i18n.text("multiplayer-bind-details"));
            }
            theme::hint(ui, i18n.text("multiplayer-reachable-hint"));
        });
}

pub(crate) fn paint_join(
    ui: &mut egui::Ui,
    state: &mut MultiplayerUi,
    profile: &mut PlayerSettingsState,
    i18n: &Localization,
) {
    paint_connection_fields(ui, &mut state.join, false, profile, i18n);
    if let Some(error) = state.error {
        ui.colored_label(
            theme::DANGER,
            i18n.text(error.key_for_method(state.join.method)),
        );
    }
    let valid = state.join.valid(false)
        && !state.submitted
        && cfg!(feature = "gns")
        && (state.join.method == RuntimeConnectionMethod::DirectIp || state.internet_available);
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
        RuntimePhase::Connecting
            if status.connection_method == Some(RuntimeConnectionMethod::Internet)
                && status.rendezvous_control == Some(RendezvousControlStatus::Available) =>
        {
            if status.role == Some(RuntimeRole::Host) {
                "multiplayer-creating-room"
            } else {
                "multiplayer-joining-room"
            }
        }
        RuntimePhase::Connecting => "multiplayer-connecting",
        RuntimePhase::Authenticating => "multiplayer-authenticating",
        RuntimePhase::Syncing(SyncPhase::ImageNegotiation) => "multiplayer-checking-image",
        RuntimePhase::Syncing(SyncPhase::AwaitingImageSlot) => "multiplayer-image-queued",
        RuntimePhase::Syncing(SyncPhase::ImageTransfer) => "multiplayer-receiving-image",
        RuntimePhase::Syncing(SyncPhase::AwaitingImageReady) => "multiplayer-preparing-image",
        RuntimePhase::Syncing(SyncPhase::AwaitingBaselineSlot) => "multiplayer-sync-queued",
        RuntimePhase::Syncing(SyncPhase::CatchingUp) => "multiplayer-catching-up",
        RuntimePhase::Syncing(SyncPhase::Finalizing) => "multiplayer-finalizing",
        RuntimePhase::Syncing(_) => "multiplayer-syncing",
        RuntimePhase::Ready => "multiplayer-ready",
        _ => "multiplayer-connecting",
    }
}

fn paint_host_retry(
    ui: &mut egui::Ui,
    state: &mut MultiplayerUi,
    profile: &mut PlayerSettingsState,
    i18n: &Localization,
) {
    if state.prepared_host && state.retry_host.is_none() {
        let available = state.internet_available;
        paint_method(ui, state, available, i18n);
        theme::hint(ui, i18n.text("multiplayer-retry-prepared"));
        state.paint_password_notice(ui, i18n);
        paint_connection_fields(ui, &mut state.host, true, profile, i18n);
        if let Some(error) = state.error {
            ui.colored_label(
                theme::DANGER,
                i18n.text(error.key_for_method(state.host.method)),
            );
        }
        let valid = state.host.valid(true)
            && !state.submitted
            && cfg!(feature = "gns")
            && (state.host.method == RuntimeConnectionMethod::DirectIp || available);
        ui.add_enabled_ui(valid, |ui| {
            if theme::button(
                ui,
                i18n.text("multiplayer-start-host"),
                ui.available_width(),
                true,
            )
            .clicked()
            {
                state.submit_host();
                if state.submitted {
                    state.editing_host_retry = false;
                }
            }
        });
        if theme::button(ui, i18n.text("common-cancel"), ui.available_width(), false).clicked() {
            state.cancel();
        }
        return;
    }
    ui.label(i18n.text("multiplayer-host-settings"));
    if let Some(name) = &profile.current.display_name {
        theme::hint(
            ui,
            i18n.format("multiplayer-player-name", &[("name", name.as_ref().into())]),
        );
    }
    theme::hint(ui, i18n.text("multiplayer-retry-prepared"));
    ui.label(i18n.text("multiplayer-bind-address"));
    ui.add(egui::TextEdit::singleline(&mut state.host.address).desired_width(f32::INFINITY));
    let address = parse_bind_address(&state.host.address);
    if address.is_err() {
        ui.colored_label(theme::DANGER, i18n.text("multiplayer-error-address"));
    }
    theme::hint(ui, i18n.text("multiplayer-bind-hint"));
    paint_connection_help(ui, true, None, i18n);
    theme::hint(ui, i18n.text("multiplayer-retry-password"));
    ui.add_enabled_ui(address.is_ok() && !state.submitted, |ui| {
        if theme::button(
            ui,
            i18n.text("multiplayer-start-host"),
            ui.available_width(),
            true,
        )
        .clicked()
        {
            state
                .retry_host
                .as_mut()
                .unwrap()
                .set_address(address.unwrap());
            state.retrying = true;
            state.editing_host_retry = false;
            state.submitted = true;
        }
    });
    if theme::button(ui, i18n.text("common-cancel"), ui.available_width(), false).clicked() {
        state.cancel();
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_connection_ui(
    mut contexts: EguiContexts,
    i18n: Res<Localization>,
    status: Res<NetworkStatus>,
    mut state: ResMut<MultiplayerUi>,
    mut profile: ResMut<PlayerSettingsState>,
    mut saves: ResMut<crate::persistence::SaveDialogs>,
    mut persistence: ResMut<PersistenceState>,
    definition: Option<Res<jigsall_core::PuzzleDefinition>>,
    original: Option<Res<OriginalPuzzleImage>>,
    store: Res<PieceDataStore>,
    app_state: Option<Res<State<AppState>>>,
) {
    if (status.role == Some(RuntimeRole::Client) && status.phase == RuntimePhase::Ready)
        || (status.role == Some(RuntimeRole::Host) && status.phase == RuntimePhase::Hosting)
    {
        state.connecting = false;
        state.submitted = false;
        state.prepared_host = false;
        if status.role == Some(RuntimeRole::Host) && state.owns_session {
            if !state.invite_shown {
                state.invite_open = true;
                state.invite_shown = true;
            }
            if let Ok(ctx) = contexts.ctx_mut() {
                let state = &mut *state;
                paint_invite_panel(
                    ctx,
                    &mut state.invite_open,
                    &status,
                    &i18n,
                    state.invite_secret.as_deref().map(|s| s.as_str()),
                    &mut state.invite_address,
                );
            }
        }
        return;
    }
    if status.host_start_failed
        && state.owns_session
        && state.submitted
        && state.pending_host.is_none()
        && state.retry_host.is_none()
        && state.action.is_none()
    {
        state.recover_prepared_host(UiError::failure(
            status.failure.unwrap_or(NetworkFailureKind::Connection),
        ));
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
        && state.pending_join.is_none()
        && state.action.is_none();
    let error = state.error.or_else(|| {
        failed.then(|| UiError::failure(status.failure.unwrap_or(NetworkFailureKind::Connection)))
    });
    // The retry form needs a fresh layout/scroll extent instead of retaining
    // the compact error card's constrained height.
    let area_id = if state.editing_host_retry {
        "multiplayer_host_retry"
    } else {
        "multiplayer_connection"
    };
    egui::Area::new(area_id.into())
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            theme::frame().show(ui, |ui| {
                ui.set_width((screen.width() - 96.0).clamp(160.0, 480.0));
                // Save outcomes can grow the card beyond the Area's previous size.
                ui.set_max_height((screen.height() - 160.0).max(80.0));
                egui::ScrollArea::vertical()
                    .max_height((screen.height() - 160.0).max(80.0))
                    .show(ui, |ui| {
                        theme::heading(ui, i18n.text("menu-multiplayer"));
                        if state.editing_host_retry {
                            paint_host_retry(ui, &mut state, &mut profile, &i18n);
                            return;
                        }
                        if let Some(error) = error {
                            let method = status.connection_method.unwrap_or(
                                if state.screen == MenuScreen::Join {
                                    state.join.method
                                } else {
                                    state.host.method
                                },
                            );
                            ui.colored_label(
                                theme::DANGER,
                                i18n.text(error.key_for_method(method)),
                            );
                        } else {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(i18n.text(
                                    if state.pending_host.is_some()
                                        || matches!(state.action, Some(Action::PrepareHost(_)))
                                    {
                                        "multiplayer-preparing-host"
                                    } else if state.pending_join.is_some() {
                                        "multiplayer-resolving"
                                    } else {
                                        connection_text(&status)
                                    },
                                ));
                            });
                        }
                        ui.add_space(12.0);
                        if status.has_disconnected_game()
                            && app_state.as_ref().is_some_and(|state| {
                                matches!(state.get(), AppState::InGame | AppState::GameComplete)
                            })
                        {
                            theme::hint(ui, i18n.text("multiplayer-save-disconnected-hint"));
                            crate::persistence::status(ui, &persistence, &i18n);
                            ui.add_enabled_ui(
                                !persistence.busy
                                    && !persistence.title_dialog_open
                                    && definition.is_some()
                                    && original.is_some()
                                    && !store.is_empty(),
                                |ui| {
                                    if theme::button(
                                        ui,
                                        i18n.text("multiplayer-save-disconnected"),
                                        ui.available_width(),
                                        true,
                                    )
                                    .clicked()
                                    {
                                        saves.open_title(&mut persistence, &i18n);
                                        state.disconnected_save_opened = true;
                                    }
                                },
                            );
                            ui.add_space(8.0);
                        }
                        ui.add_enabled_ui(
                            !persistence.busy && !persistence.title_dialog_open,
                            |ui| {
                                if theme::button(
                                    ui,
                                    i18n.text(if status.has_disconnected_game() {
                                        if state.disconnected_save_opened
                                            && matches!(
                                                persistence.message,
                                                Some(PersistenceNotice::Saved)
                                            )
                                        {
                                            "multiplayer-return-after-save"
                                        } else {
                                            "multiplayer-discard-return"
                                        }
                                    } else if error.is_some() {
                                        "common-back"
                                    } else {
                                        "common-cancel"
                                    }),
                                    ui.available_width(),
                                    false,
                                )
                                .clicked()
                                {
                                    if error.is_some() && state.retry_host.is_some() {
                                        state.editing_host_retry = true;
                                        state.submitted = false;
                                        state.error = None;
                                    } else {
                                        let join_failed =
                                            error.is_some() && state.screen == MenuScreen::Join;
                                        state.cancel();
                                        if join_failed {
                                            state.screen = MenuScreen::Join;
                                        }
                                    }
                                }
                            },
                        );
                    });
            });
        });
}

#[cfg(test)]
mod tests;

pub(crate) fn paint_room_code(ui: &mut egui::Ui, status: &NetworkStatus, i18n: &Localization) {
    if let Some(code) = &status.room_code {
        ui.label(i18n.text("multiplayer-room-code"));
        ui.horizontal(|ui| {
            // Visibility is local to this UI and code, and is never saved to disk.
            let visible_id = ui.make_persistent_id(("room-code-visible", code));
            let mut visible = ui.data(|data| data.get_temp::<bool>(visible_id).unwrap_or(false));
            if ui
                .add(
                    egui::Button::new(i18n.text(if visible {
                        "multiplayer-hide-room-code"
                    } else {
                        "multiplayer-show-room-code"
                    }))
                    .sense(egui::Sense::CLICK),
                )
                .clicked()
            {
                visible = !visible;
                ui.data_mut(|data| data.insert_temp(visible_id, visible));
            }
            if visible {
                ui.label(egui::RichText::new(code).monospace().size(24.0).strong());
            }
            let copied_id = ui.make_persistent_id(("room-code-copied", code));
            let now = ui.input(|input| input.time);
            if ui
                .add(egui::Button::new(i18n.text("multiplayer-copy")).sense(egui::Sense::CLICK))
                .clicked()
            {
                ui.ctx().copy_text(code.clone());
                ui.data_mut(|data| data.insert_temp(copied_id, now));
            }
            if ui
                .data(|data| data.get_temp::<f64>(copied_id))
                .is_some_and(|time| now - time < 3.0)
            {
                ui.label(i18n.text("multiplayer-copied"));
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs(3));
            }
        });
    }
}
pub(crate) fn paint_control_warning(
    ui: &mut egui::Ui,
    status: &NetworkStatus,
    i18n: &Localization,
) {
    if status.rendezvous_control == Some(RendezvousControlStatus::Unavailable) {
        theme::hint(ui, i18n.text("multiplayer-control-unavailable"));
        theme::hint(ui, i18n.text("multiplayer-existing-game-continues"));
    }
}
pub(crate) fn paint_method(
    ui: &mut egui::Ui,
    state: &mut MultiplayerUi,
    available: bool,
    i18n: &Localization,
) {
    state.configure_connection_methods(available);
    let mut method = state.host.method;
    let mut selected = false;
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(state.internet_available, |ui| {
            selected |= ui
                .selectable_value(
                    &mut method,
                    RuntimeConnectionMethod::Internet,
                    i18n.text("multiplayer-internet"),
                )
                .clicked();
        });
        selected |= ui
            .selectable_value(
                &mut method,
                RuntimeConnectionMethod::DirectIp,
                i18n.text("multiplayer-direct-ip"),
            )
            .clicked();
    });
    if !state.internet_available {
        theme::hint(ui, i18n.text("multiplayer-internet-unavailable"));
    }
    if selected {
        state.method_selected = true;
        state.set_connection_method(method);
    }
    theme::hint(
        ui,
        i18n.text(if method == RuntimeConnectionMethod::Internet {
            "multiplayer-internet-hint"
        } else {
            "multiplayer-direct-ip-hint"
        }),
    );
}

fn paint_invite_panel(
    ctx: &egui::Context,
    open: &mut bool,
    status: &NetworkStatus,
    i18n: &Localization,
    secret: Option<&str>,
    address: &mut String,
) {
    let mut dismissed = false;
    egui::Window::new(i18n.text("multiplayer-invite-title"))
        .id("multiplayer_invite".into())
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(360.0_f32.min((ctx.content_rect().width() - 48.0).max(120.0)))
        .default_pos(ctx.content_rect().left_top() + egui::vec2(24.0, 80.0))
        .show(ctx, |ui| {
            paint_host_status(ui, status, i18n);
            if status.connection_method == Some(RuntimeConnectionMethod::Internet) {
                paint_room_code(ui, status, i18n);
            }
            if let Some(secret) = secret {
                let target = if status.connection_method == Some(RuntimeConnectionMethod::Internet)
                {
                    status.room_code.as_deref()
                } else {
                    if address.is_empty() {
                        if let Some(bound) = status.address.filter(|a| !a.ip().is_unspecified()) {
                            *address = bound.to_string();
                        }
                    }
                    ui.label(i18n.text("multiplayer-server-address"));
                    ui.text_edit_singleline(address);
                    valid_address(address, false).then_some(address.as_str())
                };
                if let Some(target) = target {
                    if ui.button(i18n.text("multiplayer-copy-invite")).clicked() {
                        let method = if status.connection_method
                            == Some(RuntimeConnectionMethod::Internet)
                        {
                            "internet"
                        } else {
                            "direct"
                        };
                        // Clipboard ownership is an explicit user action. No other
                        // secret copy enters status, logs, persistence or undo.
                        ui.ctx()
                            .copy_text(format!("jigsall-invite-v1|{method}|{target}|{secret}"));
                    }
                }
            }
            theme::hint(ui, i18n.text("multiplayer-invite-playing"));
            dismissed = ui.button(i18n.text("common-close")).clicked();
        });
    if dismissed {
        *open = false;
    }
}
