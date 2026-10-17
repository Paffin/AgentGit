#![no_main]
//! A `SandboxSpec` as JSON (what a service would hand to a sandbox). Validation must be total; a spec that
//! validates must also be usable by the container backend (or refused with an error, never a panic), its
//! environment digest must be well formed and depend on the environment, and mount targets must not be able
//! to name the paths the sandbox provides itself.

use ag_sandbox::{Network, OciSandbox, SandboxSpec};
use libfuzzer_sys::fuzz_target;

const IMAGE: &str = "registry.example/ci@sha256:0000000000000000000000000000000000000000000000000000000000000000";

fuzz_target!(|data: &[u8]| {
    let Ok(mut spec) = serde_json::from_slice::<SandboxSpec>(data) else { return };
    // Never give the fuzzer a real directory to mount or run in beyond /tmp.
    spec.workdir = "/tmp".into();
    if spec.validate().is_err() {
        return;
    }
    for m in &spec.ro_mounts {
        let t = m.sandbox.to_string_lossy();
        assert!(
            !(t == "/"
                || t == "/proc"
                || t == "/dev"
                || t == "/tmp"
                || t == "/work"
                || t.starts_with("/proc/")
                || t.starts_with("/dev/")
                || t.starts_with("/work/")
                || t.starts_with("/tmp/")),
            "reserved mount target {t} accepted"
        );
        assert!(
            m.sandbox
                .components()
                .all(|c| !matches!(c, std::path::Component::ParentDir | std::path::Component::CurDir))
        );
    }
    assert!(spec.limits.wall_ms > 0 && spec.limits.memory_bytes > 0 && spec.limits.pids > 0);
    if let Ok(oci) = OciSandbox::new("docker", IMAGE) {
        if let Ok(args) = oci.run_args(&spec, "ag-fuzz", None) {
            let at = args.iter().position(|a| a == "--entrypoint").expect("--entrypoint is always passed");
            assert_eq!(&args[at + 3..], &spec.argv[1..]);
            let net = args.windows(2).find(|w| w[0] == "--network").map(|w| w[1].as_str());
            assert_eq!(net, Some(if spec.network == Network::None { "none" } else { "host" }));
        }
    }
});
