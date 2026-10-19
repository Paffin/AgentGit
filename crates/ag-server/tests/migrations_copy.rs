//! The crate builds from its own copy of the control-database migrations (a crate cannot reach outside its directory when it is packaged for
//! crates.io). The top-level `migrations/control` stays the source of truth; `python3 scripts/sync_crate_files.py` refreshes the
//! copy. In a checkout this fails when they differ; in a packaged crate there is no top-level directory and it passes.

use std::fs;
use std::path::Path;

fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
                out.push((rel, fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn the_control_migrations_copy_matches_the_top_level_ones() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = manifest.join("../../migrations/control");
    if source.is_dir() {
        assert_eq!(
            tree(&source),
            tree(&manifest.join("migrations/control")),
            "migrations/control differs from the top-level migrations/control: run python3 scripts/sync_crate_files.py"
        );
    }
}
