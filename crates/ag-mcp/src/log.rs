//! Logging to stderr. Standard output belongs to the protocol and carries nothing else.
//!
//! Log lines never contain tool arguments or results, only names, durations and outcome codes.

use std::sync::atomic::{AtomicU8, Ordering};

static LEVEL: AtomicU8 = AtomicU8::new(2);

/// Parses `error`, `warn`, `info` or `debug`.
pub fn set_level(name: &str) -> Result<(), String> {
    let l = match name {
        "error" => 0,
        "warn" => 1,
        "info" => 2,
        "debug" => 3,
        other => return Err(format!("unknown log level `{other}` (error, warn, info, debug)")),
    };
    LEVEL.store(l, Ordering::Relaxed);
    Ok(())
}

fn emit(level: u8, tag: &str, msg: &str) {
    if level <= LEVEL.load(Ordering::Relaxed) {
        // A message is one line: never let text from outside add lines or escape sequences.
        let clean: String = msg.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        eprintln!("ag-mcp: {tag}: {clean}");
    }
}

/// An error.
pub fn error(msg: &str) {
    emit(0, "error", msg);
}

/// A warning.
pub fn warn(msg: &str) {
    emit(1, "warning", msg);
}

/// Normal operation.
pub fn info(msg: &str) {
    emit(2, "info", msg);
}

/// Detail for troubleshooting.
pub fn debug(msg: &str) {
    emit(3, "debug", msg);
}
