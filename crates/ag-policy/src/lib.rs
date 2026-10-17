//! Policy: what must be true before a change may be integrated.
//!
//! The policy engine is a **pure function** from a [`Policy`] document and a
//! set of [`Facts`] to an [`Evaluation`]. It performs no I/O, so the
//! integration engine can evaluate it *inside* the transaction that moves a
//! stream head, immediately after reading the facts it depends on.
//!
//! The four states of a change are deliberately independent:
//!
//! * **mergeable** — the merge engine can produce an unambiguous tree (not
//!   decided here);
//! * **validated** — trusted, signed [`Attestation`]s exist for *that exact
//!   tree*, policy version and environment;
//! * **approved** — enough eligible reviewers approved *that exact head*;
//! * **integrated** — the stream was atomically updated (not decided here).
//!
//! Trust in a validation comes from the *identity and key of the issuer*,
//! listed explicitly in `trusted_issuers`, never from the mere presence of
//! a signature: an agent signing its own result proves nothing.

mod attestation;
mod document;
mod eval;
mod matcher;

pub use attestation::{Attestation, AttestationError, CheckResult};
pub use document::{
    AgentAutonomy, ApprovalRules, Policy, PolicyError, ProtectedPath, RequiredValidation, TrustedIssuer,
};
pub use eval::{
    ApprovalFact, ChangedPath, Decision, Evaluation, Facts, Unmet, evaluate,
};
pub use matcher::{IdentityMatcher, PathMatcher};
