#![no_main]
//! Policy evaluation on generated policies and facts: the gate in front of every integration.
//!
//! Invariants: never panics (attestation timestamps and the clock are arbitrary `i64`s); deterministic; the
//! verdict agrees with its list of unmet requirements; and the central security properties hold whatever
//! else is in the facts: a required check without a trusted, correctly signed `pass` for this exact tree and
//! policy version is never `Allow`, approvals are never conjured from nothing, and the file limit is a limit.

use ag_crypto::KeyPair;
use ag_policy::{
    AgentAutonomy, ApprovalFact, ApprovalRules, Attestation, ChangedPath, CheckResult, Decision, Facts, Policy,
    ProtectedPath, RequiredValidation, TrustedIssuer, evaluate,
};
use ag_types::{ChangesetId, Identity, ObjectId, Timestamp, TreeId};
use arbitrary::{Result as ArbResult, Unstructured};
use libfuzzer_sys::fuzz_target;

const IDS: &[&str] = &[
    "user:alice",
    "user:bob",
    "agent:bot",
    "agent:bot#r1",
    "agent:bot#r2",
    "service:ci",
    "service:evil",
    "system:root",
];
const PATTERNS: &[&str] =
    &["user:*", "agent:*", "agent:bot*", "service:ci", "*", "user:alice", "system:*", "agent:bot#*"];
const GLOBS: &[&str] =
    &["src/**", "docs/**", "*.md", "**/*.rs", "Cargo.toml", "src/x.rs", ".github/**", "{a,b}/**", "**"];
const PATHS: &[&str] =
    &["src/x.rs", "src/y/z.rs", "docs/a.md", "README.md", "Cargo.toml", ".github/ci.yml", "a/b", "c"];
const CHECKS: &[&str] = &["tests", "lint"];

fn ident(u: &mut Unstructured<'_>) -> ArbResult<Identity> {
    Ok(Identity::new(*u.choose(IDS)?).unwrap())
}

fn patterns(u: &mut Unstructured<'_>) -> ArbResult<Vec<String>> {
    let n = u.int_in_range(0..=3)?;
    (0..n).map(|_| Ok(u.choose(PATTERNS)?.to_string())).collect()
}

fn small(u: &mut Unstructured<'_>) -> ArbResult<u32> {
    Ok(if u.ratio(1, 8)? { u.arbitrary()? } else { u.int_in_range(0..=3)? })
}

fn cid(b: u8) -> ChangesetId {
    ChangesetId::new(ObjectId::from_bytes([b; 32]))
}

fn tid(b: u8) -> TreeId {
    TreeId::new(ObjectId::from_bytes([b; 32]))
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let _ = run(&mut u);
});

fn run(u: &mut Unstructured<'_>) -> ArbResult<()> {
    let trusted = KeyPair::from_seed([1; 32]);
    let stranger = KeyPair::from_seed([2; 32]);
    let ci = Identity::new("service:ci").unwrap();

    let mut policy = Policy::default();
    policy.integrators = patterns(u)?;
    policy.admins = patterns(u)?;
    policy.allowed_authors = patterns(u)?;
    if u.arbitrary()? {
        policy.trusted_issuers.push(TrustedIssuer { identity: ci.clone(), public_key: trusted.public() });
    }
    for check in CHECKS {
        if u.arbitrary()? {
            policy.required_validations.push(RequiredValidation {
                check: check.to_string(),
                issuers: if u.ratio(1, 4)? { vec![ci.clone()] } else { vec![] },
                environment_digest: u.ratio(1, 4)?.then(|| "env1".to_string()),
                max_age_secs: if u.ratio(1, 3)? { Some(u.arbitrary()?) } else { None },
                run: None,
            });
        }
    }
    policy.approvals = ApprovalRules {
        min: small(u)?,
        min_for_agents: small(u)?,
        approvers: patterns(u)?,
        forbid_self_approval: u.arbitrary()?,
    };
    for _ in 0..u.int_in_range(0..=2)? {
        policy.protected_paths.push(ProtectedPath {
            glob: u.choose(GLOBS)?.to_string(),
            min_approvals: small(u)?.max(1),
            approvers: patterns(u)?,
        });
    }
    policy.agent_autonomy = AgentAutonomy {
        auto_integrate: u.arbitrary()?,
        allowed_paths: (0..u.int_in_range(0..=2)?)
            .map(|_| Ok(u.choose(GLOBS)?.to_string()))
            .collect::<ArbResult<_>>()?,
        max_files: small(u)?,
    };
    policy.require_provenance_for_agents = u.arbitrary()?;
    policy.max_changed_files = small(u)?;
    // Only evaluate policies the engine would have accepted.
    let Ok(policy) = Policy::parse(&policy.to_json()) else { return Ok(()) };

    let candidate = tid(7);
    let policy_version = 3u64;
    let mut attestations = Vec::new();
    for i in 0..u.int_in_range(0..=4)? {
        let key = if u.ratio(3, 4)? { &trusted } else { &stranger };
        let mut att = Attestation::sign(
            key,
            format!("att{i}"),
            if u.ratio(4, 5)? { candidate } else { tid(9) },
            u.choose(CHECKS)?.to_string(),
            if u.arbitrary()? { CheckResult::Pass } else { CheckResult::Fail },
            if u.ratio(4, 5)? { policy_version } else { 4 },
            if u.arbitrary()? { "env1".into() } else { "env2".into() },
            if u.ratio(4, 5)? { ci.clone() } else { ident(u)? },
            Timestamp::from_millis(if u.ratio(1, 2)? { u.arbitrary()? } else { u.int_in_range(-1000..=1000)? }),
        )
        .unwrap();
        if u.ratio(1, 4)? {
            att.check.push('x'); // breaks the signature
        }
        attestations.push(att);
    }

    let head = cid(1);
    let approvals: Vec<ApprovalFact> = (0..u.int_in_range(0..=4)?)
        .map(|_| {
            Ok(ApprovalFact {
                reviewer: ident(u)?,
                head: if u.ratio(4, 5)? { head } else { cid(2) },
                approve: u.arbitrary()?,
            })
        })
        .collect::<ArbResult<_>>()?;
    let changed: Vec<ChangedPath> = (0..u.int_in_range(0..=5)?)
        .map(|_| Ok(ChangedPath { path: u.choose(PATHS)?.to_string() }))
        .collect::<ArbResult<_>>()?;
    let (actor, author) = (ident(u)?, ident(u)?);
    let facts = Facts {
        actor: &actor,
        author: &author,
        proposal_head: head,
        candidate_tree: candidate,
        policy_version,
        changed: &changed,
        attestations: &attestations,
        approvals: &approvals,
        has_provenance: u.arbitrary()?,
        now: Timestamp::from_millis(if u.ratio(1, 2)? { u.arbitrary()? } else { 0 }),
    };

    let e = evaluate(&policy, &facts);
    let again = evaluate(&policy, &facts);
    assert_eq!(serde_json::to_string(&e).unwrap(), serde_json::to_string(&again).unwrap(), "must be deterministic");
    assert_eq!(e.is_allowed(), e.unmet.is_empty(), "the verdict must agree with the unmet list");
    assert_eq!(e.decision == Decision::Allow, e.is_allowed());

    if e.is_allowed() {
        // Every required check has a trusted, validly signed pass for exactly this tree and version.
        for v in &policy.required_validations {
            let ok = attestations.iter().any(|a| {
                a.check == v.check
                    && a.result == CheckResult::Pass
                    && a.tree_id == candidate
                    && a.policy_version == policy_version
                    && a.public_key == trusted.public()
                    && a.issuer == ci
                    && a.verify_signature().is_ok()
                    && policy.trusted_issuers.iter().any(|t| t.identity == a.issuer && t.public_key == a.public_key)
            });
            assert!(ok, "allowed without a trusted pass for '{}'", v.check);
        }
        if policy.max_changed_files > 0 {
            assert!(changed.len() <= policy.max_changed_files as usize, "file limit exceeded yet allowed");
        }
        let approving = approvals.iter().filter(|a| a.approve && a.head == head).count() as u32;
        if policy.approvals.min > 0 {
            assert!(approving >= policy.approvals.min, "allowed with fewer approvals than the minimum");
        }
        if policy.require_provenance_for_agents && author.kind() == ag_types::IdentityKind::Agent {
            assert!(facts.has_provenance, "an agent's change was allowed without provenance");
        }
    }
    Ok(())
}
