//! Embeds the built Web UI.
//!
//! With the default `ui` feature this script looks for the UI build (`apps/web/dist`, or the directory in the
//! environment variable `AG_UI_DIST`) and generates a table of its files, each with its pre-compressed variants
//! (`x.br`, `x.gz`, written by `npm run build`), as `include_bytes!` entries. When there is no build the table is
//! empty: the server still compiles, and serves a page that says how to build the UI. See `scripts/build_ui.sh`.
//!
//! One caveat of Cargo: a missing directory cannot be watched (Cargo would re-run this script on every build), so
//! when there is no build yet, building the UI does not by itself trigger a rebuild of the server. `scripts/build_ui.sh`
//! touches this file for that reason; touch it yourself if you built the UI by hand.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=AG_UI_DIST");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("ui_assets.rs");

    let mut code = String::from("&[\n");
    if env::var_os("CARGO_FEATURE_UI").is_some() {
        let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
        let dist = env::var_os("AG_UI_DIST").map(PathBuf::from).unwrap_or_else(|| manifest.join("../../apps/web/dist"));
        if dist.join("index.html").is_file() {
            println!("cargo:rerun-if-changed={}", dist.display());
            let mut files = BTreeMap::new();
            walk(&dist, &dist, &mut files);
            for (rel, entry) in &files {
                if rel.ends_with(".br") || rel.ends_with(".gz") {
                    // Pre-compressed variants are attached to their original below.
                    if files.contains_key(rel.trim_end_matches(".br").trim_end_matches(".gz")) {
                        continue;
                    }
                }
                let _ = write!(code, "    Asset {{ path: {rel:?}, data: include_bytes!({:?})", entry.path);
                for (field, ext) in [("br", "br"), ("gz", "gz")] {
                    let sibling = format!("{rel}.{ext}");
                    match files.get(&sibling) {
                        Some(v) => {
                            let _ = write!(code, ", {field}: Some(include_bytes!({:?}))", v.path);
                        }
                        None => {
                            let _ = write!(code, ", {field}: None");
                        }
                    }
                }
                code.push_str(" },\n");
            }
        } else if env::var_os("AG_UI_DIST").is_some() {
            println!("cargo:warning=AG_UI_DIST={} has no index.html; the UI is not embedded", dist.display());
        }
    }
    code.push_str("]\n");
    fs::write(&out, code).expect("write ui_assets.rs");
}

struct Entry {
    path: String,
}

/// Collects every regular file below `root`, keyed by its `/`-separated path relative to `root`.
fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Entry>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            walk(root, &path, out);
        } else if kind.is_file() {
            let rel = path.strip_prefix(root).expect("below root");
            let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            let abs = path.canonicalize().unwrap_or(path);
            out.insert(rel.join("/"), Entry { path: abs.to_string_lossy().into_owned() });
        }
    }
}
