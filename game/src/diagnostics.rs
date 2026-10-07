//! Local build/GPU diagnostics. No network configuration or player data is recorded.
use bevy::{
    log::{
        tracing_subscriber::{filter::filter_fn, Layer},
        BoxedLayer,
    },
    prelude::*,
    render::renderer::RenderAdapterInfo,
};
use std::{
    io::Write,
    time::{SystemTime, UNIX_EPOCH},
};

/// Add a local file to Bevy's existing logger. Limit it to our explicit diagnostic
/// events so dependency debug logging cannot persist signaling or credentials.
pub fn file_layer(_app: &mut App) -> Option<BoxedLayer> {
    let open = || -> std::io::Result<std::fs::File> {
        let base = directories::BaseDirs::new()
            .ok_or_else(|| std::io::Error::other("user data directory unavailable"))?;
        let directory = base.data_local_dir().join("jigsall/logs");
        std::fs::create_dir_all(&directory)?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let path = directory.join(format!(
            "diagnostics-{timestamp}-{}.log",
            std::process::id()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        writeln!(
            file,
            "{}\ncommit={}\nOS={} architecture={}\nfeatures: gns={} rendezvous={}",
            crate::build_info::version_label(),
            crate::build_info::COMMIT_SHA,
            std::env::consts::OS,
            std::env::consts::ARCH,
            cfg!(feature = "gns"),
            cfg!(feature = "rendezvous")
        )?;
        Ok(file)
    };
    match open() {
        Ok(file) => Some(
            bevy::log::tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file)
                .with_filter(filter_fn(|metadata| {
                    metadata.target() == "jigsall_diagnostics"
                }))
                .boxed(),
        ),
        Err(error) => {
            eprintln!("Could not create local diagnostics log: {error}");
            None
        }
    }
}

pub fn log_adapter(adapter: Option<Res<RenderAdapterInfo>>) {
    info!(target: "jigsall_diagnostics", "{}; commit={}; OS={}; architecture={}; gns={}; rendezvous={}",
        crate::build_info::version_label(), crate::build_info::COMMIT_SHA,
        std::env::consts::OS, std::env::consts::ARCH,
        cfg!(feature = "gns"), cfg!(feature = "rendezvous"));
    match adapter {
        Some(adapter) => info!(target: "jigsall_diagnostics",
            "GPU={}; vendor={:#x}; device={:#x}; driver={}; driver_info={}; wgpu backend={:?}",
            adapter.name, adapter.vendor, adapter.device, adapter.driver,
            adapter.driver_info, adapter.backend),
        None => warn!(target: "jigsall_diagnostics", "GPU adapter unavailable"),
    }
}
