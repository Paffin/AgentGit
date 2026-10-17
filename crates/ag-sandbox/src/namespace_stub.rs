//! The namespace sandbox exists only on Linux; elsewhere it reports itself unavailable.

use crate::Sandbox;
use crate::detect::Availability;
use crate::error::SandboxError;
use crate::outcome::{IsolationLevel, Outcome};
use crate::spec::SandboxSpec;

/// The rootless namespace sandbox. On this platform it is never available.
#[derive(Clone, Copy, Debug, Default)]
pub struct NamespaceSandbox {
    _private: (),
}

impl NamespaceSandbox {
    /// A namespace sandbox. Constructing one never fails; it is simply unavailable here.
    pub fn new() -> Self {
        NamespaceSandbox { _private: () }
    }

    /// Accepted so that callers compile on every platform; the sandbox is unavailable here, so the ids are
    /// never used.
    #[must_use]
    pub fn with_host_ids(self, _uid: u32, _gid: u32) -> Self {
        self
    }

    /// Always `Unavailable` off Linux.
    pub fn availability(&self) -> Availability {
        Availability::Unavailable("namespaces are a Linux feature".into())
    }
}

impl Sandbox for NamespaceSandbox {
    fn name(&self) -> &'static str {
        "namespace"
    }

    fn isolation(&self) -> IsolationLevel {
        IsolationLevel::Namespaces
    }

    fn run(&self, spec: &SandboxSpec) -> Result<Outcome, SandboxError> {
        spec.validate()?;
        Err(SandboxError::Unavailable { sandbox: "namespace", reason: "namespaces are a Linux feature".into() })
    }
}
