use bevy::log::tracing_subscriber::{self, EnvFilter};

pub(crate) fn init() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(format!("info,{}", bevy::log::DEFAULT_FILTER)));
        // Several GPU fixtures share one process. Respect an existing subscriber.
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}
