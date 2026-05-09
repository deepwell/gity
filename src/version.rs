//! Application version: Meson `project(..., version:)` when built via Meson (`build-release.sh`);
//! plain `cargo build` falls back to `CARGO_PKG_VERSION`.

pub fn app_version() -> &'static str {
    env!("GITY_APP_VERSION")
}
