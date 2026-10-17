//! `ag-sandbox-probe`: a test-support program that the integration tests run *inside* the sandboxes.
//!
//! Every sub-command tries one thing a hostile build step might try and reports the raw outcome as the
//! first line of stdout: `ok [details]` or `err <ERRNO_NAME>`. The tests then assert on what the kernel
//! said, from inside the sandbox, instead of trusting the sandbox's own report.
//!
//! This binary is never shipped or needed at run time. It deliberately uses raw system calls. On Linux it
//! has the full set of probes (`probe_linux.rs`); elsewhere only a few portable ones (`probe_portable.rs`).
#![allow(unsafe_code)]
#![allow(clippy::undocumented_unsafe_blocks)] // test-support program: raw syscalls with literal arguments

#[cfg(target_os = "linux")]
include!("probe_linux.rs");

#[cfg(not(target_os = "linux"))]
include!("probe_portable.rs");
