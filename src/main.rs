#![cfg_attr(windows, windows_subsystem = "windows")]

use bevy::prelude::*;
use jigsall_game::{asset_reader::DirectFileAssetPlugin, GamePlugin, WindowIconPlugin};
use jigsall_ui::GameUiPlugin;

#[cfg(windows)]
mod windows_console;

fn main() {
    #[cfg(windows)]
    windows_console::attach_parent_console();

    let mut app = App::new();
    #[cfg(feature = "rendezvous")]
    configure_internet(&mut app);
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        close_when_requested: false,
        primary_window: Some(Window {
            title: "Jigsall".into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins(WindowIconPlugin)
    .add_plugins(GameUiPlugin)
    .add_plugins(DirectFileAssetPlugin)
    .add_plugins(GamePlugin)
    .run();
}

/// Deployment/operator input, outside the ordinary player UI. Production accepts
/// validated WSS only; loopback WS is injected explicitly by integration tests.
#[cfg(feature = "rendezvous")]
fn configure_internet(app: &mut App) {
    use jigsall_game::network::{
        gns::{rendezvous::EndpointUrl, IceConfig},
        runtime::RendezvousRuntimeConfig,
    };
    let endpoint = match internet_setting(
        std::env::var("JIGSALL_RENDEZVOUS_WSS_URL"),
        option_env!("BUILTIN_JIGSALL_RENDEZVOUS_WSS_URL").unwrap_or(""),
    ) {
        Ok(endpoint) if endpoint.is_empty() => return,
        Ok(endpoint) => endpoint,
        Err(error) => {
            eprintln!(
                "Invalid JIGSALL_RENDEZVOUS_WSS_URL: {error}; Internet multiplayer is disabled."
            );
            return;
        }
    };
    let endpoint = match EndpointUrl::production(&endpoint) {
        Ok(endpoint) => endpoint,
        Err(_) => {
            eprintln!("Invalid JIGSALL_RENDEZVOUS_WSS_URL; Internet multiplayer is disabled.");
            return;
        }
    };
    let allow_public_candidates = match internet_setting(
        std::env::var("JIGSALL_ICE_ALLOW_PUBLIC_CANDIDATES"),
        option_env!("BUILTIN_JIGSALL_ICE_ALLOW_PUBLIC_CANDIDATES").unwrap_or("false"),
    )
    .as_deref()
    {
        Ok("true") => true,
        Ok("false") => false,
        _ => {
            eprintln!("Invalid JIGSALL_ICE_ALLOW_PUBLIC_CANDIDATES; expected true or false. Internet multiplayer is disabled.");
            return;
        }
    };
    let stun_servers = match internet_setting(
        std::env::var("JIGSALL_ICE_STUN_SERVERS"),
        option_env!("BUILTIN_JIGSALL_ICE_STUN_SERVERS").unwrap_or(""),
    ) {
        Ok(servers) => servers,
        Err(error) => {
            eprintln!(
                "Invalid JIGSALL_ICE_STUN_SERVERS: {error}; Internet multiplayer is disabled."
            );
            return;
        }
    }
    .split(',')
    .map(str::trim)
    .filter(|entry| !entry.is_empty())
    .map(str::to_owned)
    .collect();
    app.insert_resource(RendezvousRuntimeConfig {
        endpoint,
        ice: IceConfig {
            allow_public_candidates,
            stun_servers,
        },
    });
}

#[cfg(feature = "rendezvous")]
fn internet_setting(
    override_value: Result<String, std::env::VarError>,
    builtin: &str,
) -> Result<String, std::env::VarError> {
    match override_value {
        Err(std::env::VarError::NotPresent) => Ok(builtin.to_owned()),
        value => value,
    }
}

#[cfg(all(test, feature = "rendezvous"))]
mod tests {
    use super::internet_setting;
    use std::env::VarError;

    #[test]
    fn internet_environment_overrides_embedded_defaults() {
        assert_eq!(
            internet_setting(Err(VarError::NotPresent), "default").unwrap(),
            "default"
        );
        assert_eq!(
            internet_setting(Ok("override".into()), "default").unwrap(),
            "override"
        );
        assert_eq!(internet_setting(Ok(String::new()), "default").unwrap(), "");
        assert!(internet_setting(Err(VarError::NotUnicode("invalid".into())), "default").is_err());
    }
}
