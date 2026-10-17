//! The Web UI as a table of embedded files.
//!
//! `build.rs` turns the UI build (`apps/web/dist`, made by `scripts/build_ui.sh`) into [`FILES`]: every file with
//! its pre-compressed brotli/gzip variants, as `include_bytes!` entries. Without a build the table is empty and
//! [`embedded`] is false; the crate still compiles, and `ag-server` then serves a page that says how to build the UI.
//! It is a separate crate so that the embedded bytes recompile only when the UI changes, and so that
//! `ag-server`'s own build script stays the one it shares with the other binaries.

/// One embedded file with its pre-compressed variants.
pub struct Asset {
    /// Path below the UI root, `/`-separated.
    pub path: &'static str,
    /// The file.
    pub data: &'static [u8],
    /// Brotli-compressed `data`, if the build produced it.
    pub br: Option<&'static [u8]>,
    /// Gzip-compressed `data`, if the build produced it.
    pub gz: Option<&'static [u8]>,
}

/// Every file of the embedded UI build (empty when there was none at compile time).
pub static FILES: &[Asset] = include!(concat!(env!("OUT_DIR"), "/ui_assets.rs"));

/// Whether a UI build was embedded.
pub fn embedded() -> bool {
    FILES.iter().any(|a| a.path == "index.html")
}
