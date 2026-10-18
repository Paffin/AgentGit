#![no_main]
//! From a job as the server hands it to a worker to the command a sandbox would run. The input is a
//! protobuf `ValidationTask` (the untrusted part: it carries the policy's `RunSpec`); a worker turns it into
//! a `SandboxSpec` under its own profile, and the container backend turns that into runtime arguments.
//!
//! Invariants: nothing panics; a spec is only produced within the worker's ceilings and never grants the
//! network when the profile forbids it; the container arguments are `run <flags> --entrypoint argv[0] image
//! argv[1..]`, so nothing the policy wrote can become a runtime flag.

use std::path::Path;
use std::sync::OnceLock;

use ag_protocol::v1;
use ag_sandbox::{Network, OciSandbox};
use ag_worker::{Backend, Profile, Unrunnable, sandbox_spec};
use libfuzzer_sys::fuzz_target;
use prost::Message;

const IMAGE: &str = "registry.example/ci@sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn profiles() -> &'static [Profile] {
    static P: OnceLock<Vec<Profile>> = OnceLock::new();
    P.get_or_init(|| {
        let mut ns = Profile::new(Backend::namespace((65534, 65534)));
        ns.ro_mounts.clear();
        let mut net = ns.clone();
        net.allow_network = true;
        let mut oci = Profile::new(Backend::oci("docker", Some(IMAGE.into()), "65534:65534"));
        oci.ro_mounts.clear();
        let mut tight = ns.clone();
        tight.ceilings = ag_worker::Ceilings { timeout_secs: 60, memory_mb: 256, cpu_secs: 60, max_pids: 64 };
        vec![ns, net, oci, tight]
    })
}

fuzz_target!(|data: &[u8]| {
    let Ok(task) = v1::ValidationTask::decode(data) else { return };
    for profile in profiles() {
        match sandbox_spec(profile, &task, Path::new("/tmp")) {
            Ok(spec) => {
                let c = &profile.ceilings;
                assert!(spec.limits.wall_ms <= u64::from(c.timeout_secs) * 1000, "timeout over the ceiling");
                assert!(spec.limits.memory_bytes <= u64::from(c.memory_mb) << 20, "memory over the ceiling");
                assert!(spec.limits.pids <= u64::from(c.max_pids), "pids over the ceiling");
                assert!(spec.limits.cpu_s <= u64::from(c.cpu_secs), "cpu over the ceiling");
                if !profile.allow_network {
                    assert_eq!(spec.network, Network::None, "network granted to a worker that forbids it");
                }
                assert_eq!(spec.argv, task.run.as_ref().unwrap().argv, "the command is exactly the policy's argv");
                // The policy's variables win over the defaults, and nothing else is added.
                for (k, v) in &task.run.as_ref().unwrap().env {
                    assert_eq!(spec.env.get(k), Some(v));
                }
                if spec.validate().is_ok() {
                    if let Ok(oci) = OciSandbox::new("docker", IMAGE) {
                        if let Ok(args) = oci.run_args(&spec, "ag-fuzz", None) {
                            let at =
                                args.iter().position(|a| a == "--entrypoint").expect("--entrypoint is always passed");
                            assert_eq!(args[0], "run");
                            assert_eq!(args[at + 1], spec.argv[0]);
                            assert_eq!(args[at + 2], IMAGE);
                            assert_eq!(&args[at + 3..], &spec.argv[1..], "argv[1..] must follow the image verbatim");
                            assert_eq!(args[..at].iter().filter(|a| *a == "--entrypoint").count(), 0);
                        }
                    }
                }
            }
            Err(Unrunnable::Invalid(_) | Unrunnable::Refused(_)) => {}
        }
    }
});
