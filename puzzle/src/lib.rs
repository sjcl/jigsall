//! Seeded puzzle placement and procedural shape references; no game World or UI.
#[cfg(any(test, feature = "shape-analysis"))]
pub mod fingerprint;
pub mod grid;
pub mod placement;
pub mod procedural;
