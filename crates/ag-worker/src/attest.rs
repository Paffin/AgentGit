//! Turning what the sandbox reports into a signed attestation and a job result.
//!
//! The rule for the verdict: **a check passes only if its command exited with status 0.** Everything else —
//! a non-zero exit, a signal, a timeout, a memory, CPU, process or output limit — is `fail`: the check did
//! not pass under the limits the policy gave it. Things that are *not* the check's fault (the sandbox could
//! not start, the command does not exist on this worker, the tree could not be fetched) never become a
//! signed `fail`; they fail the *job*, so that another worker can try and nobody's merge is blocked by a
//! broken worker.

use ag_crypto::KeyPair;
use ag_policy::{Attestation, CheckResult};
use ag_protocol::v1;
use ag_sandbox::{ExitStatus, Outcome};
use ag_types::{Identity, Timestamp, TreeId};
use serde_json::{Value, json};

/// `pass` iff the command exited normally with status 0.
pub fn verdict(status: &ExitStatus) -> CheckResult {
    if status.success() { CheckResult::Pass } else { CheckResult::Fail }
}

/// A short human sentence for the job result.
pub fn describe(status: &ExitStatus) -> String {
    match status {
        ExitStatus::Exited(0) => "exited with status 0".into(),
        ExitStatus::Exited(n) => format!("exited with status {n}"),
        ExitStatus::Signaled(s) => format!("was killed by signal {s}"),
        ExitStatus::TimedOut => "ran out of wall-clock time".into(),
        ExitStatus::OutputLimit => "wrote more output than allowed".into(),
        ExitStatus::MemoryLimit => "used more memory than allowed".into(),
        ExitStatus::CpuLimit => "used more CPU time than allowed".into(),
        ExitStatus::PidsLimit => "started more processes than allowed".into(),
        other => format!("ended abnormally ({other:?})"),
    }
}

/// The attestation id for the `attempt`-th claim of `job_id`. Deterministic, so that delivering the same
/// completion twice (a lost response) names the same attestation.
pub fn attestation_id(job_id: &str, attempt: u32) -> String {
    format!("att_{job_id}_{attempt}")
}

/// Signs the verdict for `tree` and `check` under `policy_version`.
#[allow(clippy::too_many_arguments)]
pub fn sign(
    key: &KeyPair,
    issuer: &Identity,
    id: String,
    tree: TreeId,
    check: &str,
    policy_version: u64,
    environment_digest: &str,
    result: CheckResult,
) -> Result<Attestation, ag_policy::AttestationError> {
    Attestation::sign(
        key,
        id,
        tree,
        check.to_string(),
        result,
        policy_version,
        environment_digest.to_string(),
        issuer.clone(),
        Timestamp::now(),
    )
}

/// The wire form.
pub fn to_wire(a: &Attestation) -> v1::Attestation {
    v1::Attestation {
        id: a.id.clone(),
        tree_id: a.tree_id.to_string(),
        check: a.check.clone(),
        result: a.result.as_str().to_string(),
        policy_version: a.policy_version,
        environment_digest: a.environment_digest.clone(),
        issuer: a.issuer.to_string(),
        issued_at_ms: a.issued_at.as_millis(),
        public_key: a.public_key.to_hex(),
        signature: a.signature.to_hex(),
    }
}

/// The last `max` bytes of `bytes`, as text (invalid UTF-8 replaced), starting on a character boundary.
pub fn tail(bytes: &[u8], max: usize) -> String {
    let start = bytes.len().saturating_sub(max);
    let s = String::from_utf8_lossy(&bytes[start..]).into_owned();
    // If we cut in the middle of a multi-byte character the first char is U+FFFD; drop it.
    if start > 0 { s.trim_start_matches('\u{FFFD}').to_string() } else { s }
}

/// The free-form `result_json` stored with the job: what happened, in enough detail to debug a failed check
/// without access to the worker. It is **not signed** and carries no authority; the attestation does.
pub fn result_json(outcome: &Outcome, sandbox: &str, worker: &str, tail_bytes: usize) -> Value {
    let iso = &outcome.isolation;
    json!({
        "status": outcome.status,
        "summary": describe(&outcome.status),
        "wall_ms": outcome.wall_ms,
        "cpu_ms": outcome.cpu_ms,
        "max_rss_bytes": outcome.max_rss_bytes,
        "sandbox": sandbox,
        "isolation_verified": iso.verified,
        "memory_enforcement": iso.memory,
        "task_enforcement": iso.tasks,
        "environment_digest": outcome.environment_digest,
        "notes": iso.notes,
        "worker": worker,
        "stdout": {"tail": tail(&outcome.stdout, tail_bytes), "truncated": outcome.stdout_truncated, "captured_bytes": outcome.stdout.len()},
        "stderr": {"tail": tail(&outcome.stderr, tail_bytes), "truncated": outcome.stderr_truncated, "captured_bytes": outcome.stderr.len()},
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_clean_exit_passes() {
        assert_eq!(verdict(&ExitStatus::Exited(0)), CheckResult::Pass);
        for s in [
            ExitStatus::Exited(1),
            ExitStatus::Exited(-1),
            ExitStatus::Signaled(9),
            ExitStatus::TimedOut,
            ExitStatus::OutputLimit,
            ExitStatus::MemoryLimit,
            ExitStatus::CpuLimit,
            ExitStatus::PidsLimit,
        ] {
            assert_eq!(verdict(&s), CheckResult::Fail, "{s:?}");
            assert!(!describe(&s).is_empty());
        }
    }

    #[test]
    fn tails_cut_on_character_boundaries() {
        assert_eq!(tail(b"hello", 100), "hello");
        assert_eq!(tail(b"hello", 3), "llo");
        let s = "aé€b".as_bytes(); // 1 + 2 + 3 + 1 bytes
        assert_eq!(tail(s, 5), "€b");
        assert_eq!(tail(s, 2), "b");
        assert_eq!(tail(&[0xff, 0xfe, b'x'], 10), "\u{FFFD}\u{FFFD}x");
    }

    #[test]
    fn the_attestation_id_is_stable_per_claim() {
        assert_eq!(attestation_id("job_1", 2), attestation_id("job_1", 2));
        assert_ne!(attestation_id("job_1", 2), attestation_id("job_1", 3));
    }

    #[test]
    fn the_signed_attestation_verifies_and_roundtrips_to_the_wire() {
        let key = KeyPair::generate().unwrap();
        let issuer = Identity::new("service:ci-1").unwrap();
        let tree: TreeId = "ab".repeat(32).parse().unwrap();
        let a = sign(&key, &issuer, "att_1".into(), tree, "tests", 7, "sha256:00", CheckResult::Pass).unwrap();
        a.verify_signature().unwrap();
        let w = to_wire(&a);
        assert_eq!((w.result.as_str(), w.policy_version, w.check.as_str()), ("pass", 7, "tests"));
        assert_eq!(w.public_key, key.public().to_hex());
        assert_eq!(w.issuer, "service:ci-1");
    }
}
