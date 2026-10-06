//! Non-secret build identity shared by the UI and application diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT_SHA: &str = env!("JIGSALL_GIT_SHA");

pub fn version_label() -> String {
    let short = COMMIT_SHA.get(..7).unwrap_or(COMMIT_SHA);
    format!("Jigsall {VERSION} ({short})")
}
