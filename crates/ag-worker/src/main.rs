//! `ag-worker` — claims validation jobs from an `ag-server`, runs them in a sandbox, signs attestations.
//!
//! The execution plane is Unix-only: the sandbox it exists to drive (`ag-sandbox`) is built on Linux
//! namespaces, seccomp and cgroups, or on a container runtime. On other platforms this binary still builds,
//! so a workspace-wide `cargo build` works everywhere, but every invocation says plainly that it is
//! unsupported and exits with status 2. See `docs/operations/windows.md`.

#[cfg(unix)]
mod app;

#[cfg(unix)]
fn main() {
    app::main();
}

#[cfg(not(unix))]
fn main() -> std::process::ExitCode {
    eprintln!("ag-worker: {}", ag_worker::UNSUPPORTED_PLATFORM);
    std::process::ExitCode::from(2)
}
