use sha2::{Digest, Sha256};

use crate::outcome::IsolationReport;
use crate::spec::{Network, SandboxSpec};

const DOMAIN: &[u8] = b"agentgit.sandbox.environment.v1\0";

/// Hash the *environment* a check ran in, for binding attestations to it (ADR-0001: a validation is bound
/// to the tree, the policy version **and the environment**).
///
/// The digest covers, in a fixed canonical encoding:
///
/// * the sandbox implementation's name and the host OS and architecture;
/// * the [`IsolationReport`]'s `verified` flag and every protection flag (namespaces, private and read-only
///   root, `no_new_privs`, capabilities, seccomp, rlimits, `host_root`), the cgroup that was used, the network
///   mode, the image and the runtime — so the same spec run with and without a seccomp filter, or with and
///   without a cgroup, has different digests. Not covered: the free-text `notes`, and `memory` and `tasks`
///   (*how* a limit was enforced: the limits themselves, and whether a cgroup held them, are covered);
/// * all [`crate::Limits`], the network mode and the size of `/tmp`;
/// * the environment variables of the command (names and values; keep secrets out of them);
/// * the read-only mounts, sorted by sandbox path, each by its [`crate::Mount::identity`] when it has one
///   and otherwise by the host path (a weaker statement: it names where the content came from, not what it
///   was);
/// * the container image reference for OCI runs (inside the report).
///
/// It deliberately does **not** cover `argv`, `stdin` or the working tree: those identify *what* was
/// checked (the check name and the tree id are signed separately); this digest identifies *where*.
///
/// It records **which** protections were in place, not how strong each one is: a release that changes the
/// seccomp deny-list or the masked `/proc` paths, or a different kernel, gives the same digest. A policy that
/// needs to notice that pins the worker's version separately.
///
/// Returns `sha256:<64 hex digits>`, the format `ag validate run` already uses for environment digests.
/// The encoding is versioned by its domain string; a change to it would change every digest, and every
/// `required_environment_digest` a policy has pinned, so it needs an ADR.
pub fn environment_digest(sandbox: &str, report: &IsolationReport, spec: &SandboxSpec) -> String {
    let mut h = Sha256::new();
    h.update(DOMAIN);
    let mut put = |tag: u8, bytes: &[u8]| {
        h.update([tag]);
        h.update((bytes.len() as u64).to_be_bytes());
        h.update(bytes);
    };
    put(b'S', sandbox.as_bytes());
    put(b'O', std::env::consts::OS.as_bytes());
    put(b'A', std::env::consts::ARCH.as_bytes());

    let flags = [
        report.verified,
        report.user_ns,
        report.mount_ns,
        report.pid_ns,
        report.net_ns,
        report.ipc_ns,
        report.uts_ns,
        report.private_root,
        report.read_only_root,
        report.no_new_privs,
        report.capabilities_dropped,
        report.seccomp,
        report.rlimits,
        report.host_root,
    ];
    put(b'R', &flags.map(u8::from));
    match report.cgroup {
        None => put(b'C', &[0]),
        Some(c) => put(b'C', &[1, u8::from(c.memory), u8::from(c.pids)]),
    }
    put(
        b'N',
        &[match report.network {
            Network::None => 0,
            Network::Host => 1,
        }],
    );
    put(b'I', report.image.as_deref().unwrap_or("").as_bytes());
    put(b'U', report.runtime.as_deref().unwrap_or("").as_bytes());

    let l = &spec.limits;
    for (tag, v) in [
        (b'w', l.wall_ms),
        (b'c', l.cpu_s),
        (b'm', l.memory_bytes),
        (b'p', l.pids),
        (b'f', l.open_files),
        (b's', l.file_size_bytes),
        (b'o', l.output_bytes),
        (b't', spec.rw_tmp_bytes),
    ] {
        put(tag, &v.to_be_bytes());
    }

    for (k, v) in &spec.env {
        put(b'k', k.as_bytes());
        put(b'v', v.as_bytes());
    }

    let mut mounts: Vec<_> = spec.ro_mounts.iter().collect();
    mounts.sort_by(|a, b| a.sandbox.cmp(&b.sandbox));
    for m in mounts {
        put(b'M', m.sandbox.to_string_lossy().as_bytes());
        match &m.identity {
            Some(id) => put(b'i', id.as_bytes()),
            None => put(b'h', m.host.to_string_lossy().as_bytes()),
        }
    }

    let sum = h.finalize();
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for b in sum {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Mount;

    fn base() -> (IsolationReport, SandboxSpec) {
        let r = IsolationReport { verified: true, user_ns: true, seccomp: true, ..Default::default() };
        let s = SandboxSpec::new(["true"], "/tmp")
            .env("A", "1")
            .ro_mount(Mount::new("/usr", "/usr").with_identity("usr-1"));
        (r, s)
    }

    #[test]
    fn the_digest_is_stable_and_well_formed() {
        let (r, s) = base();
        let d = environment_digest("namespace", &r, &s);
        assert_eq!(d, environment_digest("namespace", &r, &s));
        assert!(d.starts_with("sha256:") && d.len() == 7 + 64);
        assert!(d[7..].bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn every_ingredient_changes_the_digest() {
        let (r, s) = base();
        let d = environment_digest("namespace", &r, &s);
        let mut seen = std::collections::BTreeSet::from([d.clone()]);
        let mut check = |name: &str, other: String| {
            assert!(seen.insert(other), "{name} did not change the digest");
        };

        check("sandbox name", environment_digest("process", &r, &s));
        let mut r2 = r.clone();
        r2.seccomp = false;
        check("seccomp", environment_digest("namespace", &r2, &s));
        let mut r2 = r.clone();
        r2.cgroup = Some(Default::default());
        check("cgroup", environment_digest("namespace", &r2, &s));
        let mut r2 = r.clone();
        r2.image = Some("img@sha256:aa".into());
        check("image", environment_digest("namespace", &r2, &s));
        let mut r2 = r.clone();
        r2.network = Network::Host;
        check("network", environment_digest("namespace", &r2, &s));
        check("env", environment_digest("namespace", &r, &s.clone().env("B", "2")));
        check(
            "limit",
            environment_digest("namespace", &r, &{
                let mut s2 = s.clone();
                s2.limits.pids += 1;
                s2
            }),
        );
        check("tmp size", environment_digest("namespace", &r, &s.clone().rw_tmp_bytes(1 << 20)));
        check(
            "mount identity",
            environment_digest("namespace", &r, &{
                let mut s2 = s.clone();
                s2.ro_mounts[0].identity = Some("usr-2".into());
                s2
            }),
        );
        check(
            "mount path-only identity",
            environment_digest("namespace", &r, &{
                let mut s2 = s.clone();
                s2.ro_mounts[0].identity = None;
                s2
            }),
        );
    }

    #[test]
    fn things_that_do_not_describe_the_environment_do_not_change_it() {
        let (r, s) = base();
        let d = environment_digest("namespace", &r, &s);
        // the command, stdin and notes identify the check, not the environment
        let mut s2 = s.clone();
        s2.argv = vec!["other".into()];
        s2.stdin = b"input".to_vec();
        let mut r2 = r.clone();
        r2.notes.push("remark".into());
        assert_eq!(d, environment_digest("namespace", &r2, &s2));
        // the host location of an identified mount is irrelevant
        let mut s3 = s.clone();
        s3.ro_mounts[0].host = "/somewhere/else".into();
        assert_eq!(d, environment_digest("namespace", &r, &s3));
    }

    #[test]
    fn mount_order_does_not_matter() {
        let (r, s) = base();
        let a = s.clone().ro_mount(Mount::new("/bin", "/bin"));
        let mut b = s.clone();
        b.ro_mounts.insert(0, Mount::new("/bin", "/bin"));
        assert_eq!(environment_digest("n", &r, &a), environment_digest("n", &r, &b));
    }
}
