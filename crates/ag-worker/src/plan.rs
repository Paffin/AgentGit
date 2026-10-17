//! From a job's `RunSpec` to a [`SandboxSpec`].
//!
//! The environment of the command is deliberately small and **constant for a given check**: the sandbox's
//! environment digest covers it, and a digest that changed with every tree or proposal could not be pinned
//! by a policy's `required_environment_digest`. Per-run values (the tree id, the proposal) are therefore not
//! passed to the command; it finds the tree in its working directory.

use std::path::Path;

use ag_protocol::v1;
use ag_sandbox::{Limits, Network, SandboxSpec};

use crate::profile::{Profile, Refusal};

/// `PATH` in the sandbox, unless the policy sets one.
pub const DEFAULT_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Why a job cannot be run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unrunnable {
    /// The job itself is malformed. Retrying anywhere is pointless.
    Invalid(String),
    /// Well-formed, but this worker will not run it (too large a request, network, image). Another worker
    /// may.
    Refused(Refusal),
}

fn invalid(m: impl Into<String>) -> Unrunnable {
    Unrunnable::Invalid(m.into())
}

fn refuse(m: impl Into<String>) -> Unrunnable {
    Unrunnable::Refused(Refusal(m.into()))
}

/// The sandbox specification for `task`, to run in `workdir`.
pub fn sandbox_spec(profile: &Profile, task: &v1::ValidationTask, workdir: &Path) -> Result<SandboxSpec, Unrunnable> {
    let run = task.run.as_ref().ok_or_else(|| invalid("the job carries no run specification"))?;
    if run.argv.is_empty() || run.argv[0].is_empty() {
        return Err(invalid("run.argv is empty"));
    }
    if run.argv.iter().any(|a| a.contains('\0')) {
        return Err(invalid("run.argv contains a NUL byte"));
    }
    let c = &profile.ceilings;
    let d = &profile.defaults;

    let timeout = if run.timeout_secs == 0 { c.timeout_secs } else { run.timeout_secs };
    let memory_mb = if run.memory_mb == 0 { d.memory_mb } else { run.memory_mb };
    let max_pids = if run.max_pids == 0 { d.max_pids } else { run.max_pids };
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get() as u64);
    let cpu_secs = if run.cpu_secs > 0 {
        u64::from(run.cpu_secs)
    } else if d.cpu_secs > 0 {
        u64::from(d.cpu_secs)
    } else {
        (u64::from(timeout) * cpus).min(u64::from(c.cpu_secs))
    };

    for (what, asked, ceiling) in [
        ("timeout_secs", u64::from(timeout), u64::from(c.timeout_secs)),
        ("memory_mb", u64::from(memory_mb), u64::from(c.memory_mb)),
        ("cpu_secs", cpu_secs, u64::from(c.cpu_secs)),
        ("max_pids", u64::from(max_pids), u64::from(c.max_pids)),
    ] {
        if asked > ceiling {
            return Err(refuse(format!("the policy asks for {what} = {asked}; this worker allows at most {ceiling}")));
        }
    }
    if run.network && !profile.allow_network {
        return Err(refuse("the policy asks for network access; this worker does not allow it"));
    }
    let image = (!run.image.is_empty()).then_some(run.image.as_str());
    if image.is_some() && !profile.backend.supports_images() {
        return Err(refuse(format!(
            "the policy asks for image '{}'; this worker cannot run container images",
            run.image
        )));
    }

    let mut spec = SandboxSpec::new(run.argv.iter().cloned(), workdir);
    spec.env.insert("PATH".into(), DEFAULT_PATH.into());
    spec.env.insert("HOME".into(), "/tmp".into());
    spec.env.insert("TMPDIR".into(), "/tmp".into());
    spec.env.insert("LANG".into(), "C.UTF-8".into());
    spec.env.insert("CI".into(), "true".into());
    spec.env.insert("AG_CHECK".into(), task.check.clone());
    // The policy's variables win: they are the administrators' intent, and the digest records them.
    for (k, v) in &run.env {
        spec.env.insert(k.clone(), v.clone());
    }
    spec.ro_mounts = profile.ro_mounts.clone();
    spec.rw_tmp_bytes = profile.tmp_mb << 20;
    spec.network = if run.network { Network::Host } else { Network::None };
    spec.limits = Limits {
        wall_ms: u64::from(timeout) * 1000,
        cpu_s: cpu_secs,
        memory_bytes: u64::from(memory_mb) << 20,
        pids: u64::from(max_pids),
        output_bytes: profile.output_bytes,
        ..Limits::default()
    };
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Backend;

    fn profile() -> Profile {
        let mut p = Profile::new(Backend::Namespace { host_ids: (65534, 65534) });
        p.ro_mounts.clear();
        p
    }

    fn task(run: v1::RunSpec) -> v1::ValidationTask {
        v1::ValidationTask { check: "tests".into(), run: Some(run), ..Default::default() }
    }

    fn run(argv: &[&str]) -> v1::RunSpec {
        v1::RunSpec { argv: argv.iter().map(|s| s.to_string()).collect(), timeout_secs: 60, ..Default::default() }
    }

    #[test]
    fn limits_come_from_the_policy_then_the_worker_defaults() {
        let p = profile();
        let mut r = run(&["cargo", "test"]);
        r.memory_mb = 512;
        r.max_pids = 64;
        let s = sandbox_spec(&p, &task(r), Path::new("/w")).unwrap();
        assert_eq!(s.argv, ["cargo", "test"]);
        assert_eq!(s.limits.wall_ms, 60_000);
        assert_eq!(s.limits.memory_bytes, 512 << 20);
        assert_eq!(s.limits.pids, 64);
        assert_eq!(s.network, Network::None);

        let s = sandbox_spec(&p, &task(run(&["true"])), Path::new("/w")).unwrap();
        assert_eq!(s.limits.memory_bytes, u64::from(p.defaults.memory_mb) << 20);
        assert_eq!(s.limits.pids, u64::from(p.defaults.max_pids));
        assert!(s.limits.cpu_s >= 60, "a multi-threaded check gets more CPU seconds than wall seconds");
    }

    #[test]
    fn the_environment_is_small_constant_and_policy_wins() {
        let p = profile();
        let mut r = run(&["true"]);
        r.env.insert("RUST_LOG".into(), "debug".into());
        r.env.insert("PATH".into(), "/opt/tools/bin".into());
        let a = sandbox_spec(&p, &task(r.clone()), Path::new("/w1")).unwrap();
        let b = sandbox_spec(&p, &task(r), Path::new("/w2")).unwrap();
        assert_eq!(a.env, b.env, "nothing in the environment depends on the run");
        assert_eq!(a.env["PATH"], "/opt/tools/bin");
        assert_eq!(a.env["AG_CHECK"], "tests");
        assert_eq!(a.env["RUST_LOG"], "debug");
        assert!(!a.env.keys().any(|k| k.contains("TOKEN") || k.contains("KEY")), "{:?}", a.env.keys());
    }

    #[test]
    fn requests_above_the_ceiling_are_refused_not_trimmed() {
        let mut p = profile();
        p.ceilings.timeout_secs = 100;
        let mut r = run(&["true"]);
        r.timeout_secs = 101;
        let e = sandbox_spec(&p, &task(r), Path::new("/w")).unwrap_err();
        assert!(matches!(&e, Unrunnable::Refused(m) if m.0.contains("timeout_secs")), "{e:?}");
        let mut r = run(&["true"]);
        r.memory_mb = p.ceilings.memory_mb + 1;
        assert!(matches!(sandbox_spec(&p, &task(r), Path::new("/w")), Err(Unrunnable::Refused(_))));
    }

    #[test]
    fn network_and_images_need_the_workers_consent() {
        let mut p = profile();
        let mut r = run(&["true"]);
        r.network = true;
        assert!(matches!(sandbox_spec(&p, &task(r.clone()), Path::new("/w")), Err(Unrunnable::Refused(_))));
        p.allow_network = true;
        assert_eq!(sandbox_spec(&p, &task(r), Path::new("/w")).unwrap().network, Network::Host);

        let mut r = run(&["true"]);
        r.image = "registry/x@sha256:aa".into();
        assert!(matches!(sandbox_spec(&p, &task(r), Path::new("/w")), Err(Unrunnable::Refused(_))));
    }

    #[test]
    fn malformed_jobs_are_invalid_everywhere() {
        let p = profile();
        let none = v1::ValidationTask::default();
        assert!(matches!(sandbox_spec(&p, &none, Path::new("/w")), Err(Unrunnable::Invalid(_))));
        assert!(matches!(sandbox_spec(&p, &task(run(&[])), Path::new("/w")), Err(Unrunnable::Invalid(_))));
        assert!(matches!(sandbox_spec(&p, &task(run(&["a\0b"])), Path::new("/w")), Err(Unrunnable::Invalid(_))));
    }
}
