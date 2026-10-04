use bevy::prelude::*;
use puzzella_game::{asset_reader::DirectFileAssetPlugin, GamePlugin, WindowIconPlugin};
use puzzella_ui::GameUiPlugin;

fn main() {
    let mut app = App::new();
    #[cfg(feature = "rendezvous")]
    configure_internet(&mut app);
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        close_when_requested: false,
        primary_window: Some(Window {
            title: "Puzzella".into(),
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
    use puzzella_game::network::{
        gns::{rendezvous::EndpointUrl, IceConfig},
        runtime::RendezvousRuntimeConfig,
    };
    let Ok(endpoint) = std::env::var("PUZZELLA_RENDEZVOUS_WSS_URL") else {
        return;
    };
    let endpoint = match EndpointUrl::production(&endpoint) {
        Ok(endpoint) => endpoint,
        Err(_) => {
            eprintln!("Invalid PUZZELLA_RENDEZVOUS_WSS_URL; Internet multiplayer is disabled.");
            return;
        }
    };
    let allow_public_candidates = match std::env::var("PUZZELLA_ICE_ALLOW_PUBLIC_CANDIDATES")
        .as_deref()
    {
        Ok("true") => true,
        Ok("false") | Err(std::env::VarError::NotPresent) => false,
        _ => {
            eprintln!("Invalid PUZZELLA_ICE_ALLOW_PUBLIC_CANDIDATES; expected true or false. Internet multiplayer is disabled.");
            return;
        }
    };
    let stun_servers = std::env::var("PUZZELLA_ICE_STUN_SERVERS")
        .unwrap_or_default()
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
