use serde::{Deserialize, Serialize};

use crate::Sandbox;
use crate::namespace::NamespaceSandbox;
use crate::process::ProcessSandbox;

/// Whether a sandbox can run on this host.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    /// It can run (for [`crate::OciSandbox`]: the runtime executable exists; its daemon and image are not probed).
    Available,
    /// It cannot, with the reason and, where one is known, how to fix it.
    Unavailable(String),
}

impl Availability {
    /// `true` for [`Availability::Available`].
    pub fn is_available(&self) -> bool {
        matches!(self, Availability::Available)
    }
}

/// Probe which sandboxes can run on this host, strongest first: `"oci"`, `"namespace"`, `"process"`.
///
/// * `"namespace"` is determined by doing the complete namespace setup once (result cached for the process):
///   unprivileged user namespaces disabled by a sysctl or an LSM, a container runtime's seccomp profile
///   forbidding `clone`/`mount`/`pivot_root`, a missing `/proc`, a non-Linux host and an unsupported CPU
///   architecture all show up as `Unavailable` with the reason.
/// * `"oci"` only looks for a `docker` or `podman` executable on `PATH`. Whether its daemon answers, and
///   whether an image is present, is deliberately not probed (either can hang); build an
///   [`crate::OciSandbox`] explicitly with the image you want.
/// * `"process"` is always available on Unix; it is limits only.
pub fn detect() -> Vec<(&'static str, Availability)> {
    let oci = ["docker", "podman"]
        .iter()
        .find_map(|rt| {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|d| d.join(rt))
                .find(|p| p.is_file())
        })
        .map(|_| Availability::Available)
        .unwrap_or_else(|| Availability::Unavailable("neither `docker` nor `podman` was found on PATH".into()));
    vec![("oci", oci), ("namespace", NamespaceSandbox::new().availability()), ("process", Availability::Available)]
}

/// The strongest sandbox that needs no further configuration: the namespace sandbox if this host can run it,
/// otherwise the limits-only process sandbox.
///
/// Check [`Sandbox::isolation`] before trusting the result with hostile code: on a host without
/// unprivileged namespaces this quietly returns [`crate::IsolationLevel::None`]. The OCI sandbox is never
/// returned because it needs an image; construct it yourself and prefer it for untrusted multi-tenant work.
pub fn best_available() -> Box<dyn Sandbox> {
    let ns = NamespaceSandbox::new();
    if ns.availability().is_available() { Box::new(ns) } else { Box::new(ProcessSandbox::new()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_lists_every_sandbox_and_the_process_fallback_is_always_there() {
        let d = detect();
        let names: Vec<_> = d.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["oci", "namespace", "process"]);
        assert!(d[2].1.is_available());
    }

    #[test]
    fn best_available_never_returns_nothing_and_agrees_with_detect() {
        let best = best_available();
        let ns_ok = detect().iter().any(|(n, a)| *n == "namespace" && a.is_available());
        assert_eq!(best.name(), if ns_ok { "namespace" } else { "process" });
    }
}
