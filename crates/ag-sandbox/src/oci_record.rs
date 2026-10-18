//! What the container runtime itself says about a container it ran (`inspect`) and about the daemon that
//! ran it (`info`), compared with what was asked for.
//!
//! The OCI sandbox passes flags to the runtime. Flags are requests; this module reads the runtime's own
//! *record* of the container back (`HostConfig`, `Config`, `Mounts`, `State`) and decides, property by
//! property, whether the request was honoured. A property the runtime recorded differently is a failure
//! (the check must not run, or be signed, under weaker protection than promised); a property the record does
//! not contain is *unread* and is never claimed. The kernel's enforcement of the recorded settings is
//! exercised by `tests/oci_real.rs` against real Docker and Podman in CI, not on every run.
//!
//! Everything here is a pure function over `serde_json::Value`, so it is unit tested against the outputs of
//! real runtimes (see the fixtures in the tests below).

use serde_json::Value;

/// What a daemon says about itself (`<runtime> info --format '{{json .}}'`). `None`: not reported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DaemonFacts {
    /// A seccomp profile is applied to containers by default.
    pub seccomp: Option<bool>,
    /// Containers run in a user namespace (rootless mode, or `userns-remap`): container root is not host root.
    pub user_ns: Option<bool>,
    /// The runtime itself runs without root privileges.
    pub rootless: Option<bool>,
}

/// Reads Docker's (`SecurityOptions`) or Podman's (`host.security`) description of the daemon.
pub(crate) fn daemon_facts(info: &Value) -> DaemonFacts {
    if let Some(options) = info.get("SecurityOptions").and_then(Value::as_array) {
        let options: Vec<&str> = options.iter().filter_map(Value::as_str).collect();
        let rootless = options.contains(&"name=rootless");
        return DaemonFacts {
            seccomp: Some(options.iter().any(|o| o.starts_with("name=seccomp") && !o.contains("profile=unconfined"))),
            user_ns: Some(rootless || options.iter().any(|o| o.starts_with("name=userns"))),
            rootless: Some(rootless),
        };
    }
    if let Some(sec) = info.pointer("/host/security") {
        let rootless = sec.get("rootless").and_then(Value::as_bool);
        return DaemonFacts {
            seccomp: sec.get("seccompEnabled").and_then(Value::as_bool),
            user_ns: rootless,
            rootless,
        };
    }
    DaemonFacts::default()
}

/// What the sandbox asked the runtime for, in the terms `inspect` reports it.
pub(crate) struct Requested<'a> {
    pub user: &'a str,
    pub network_none: bool,
    pub pids: u64,
    pub memory_bytes: u64,
    pub cpus: Option<f64>,
    /// `(name, soft, hard)`.
    pub ulimits: Vec<(&'static str, u64, u64)>,
    /// `(destination, read_write)` of every bind mount.
    pub binds: Vec<(String, bool)>,
}

/// The verdict on one property of the container record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Check {
    /// The runtime recorded exactly what was asked.
    Holds,
    /// The record does not say (the field is missing or has an unexpected shape).
    Unread,
    /// The runtime recorded something else: the container is not what was promised.
    Violated(String),
}

impl Check {
    pub(crate) fn holds(&self) -> bool {
        matches!(self, Check::Holds)
    }
}

/// The result of comparing a container record with the request.
#[derive(Clone, Debug)]
pub(crate) struct Observed {
    pub read_only_root: Check,
    pub network: Check,
    pub no_new_privs: Check,
    pub capabilities_dropped: Check,
    pub unprivileged: Check,
    pub user: Check,
    pub private_namespaces: Check,
    pub mounts: Check,
    pub rlimits: Check,
    pub pids: Check,
    pub memory: Check,
    /// Not a protection: swap accounting is missing on many hosts and the runtime then drops the limit.
    pub swap: Check,
    /// Not a protection either: the CPU quota shapes sharing, it does not bound a hostile command.
    pub cpus: Check,
}

impl Observed {
    fn all(&self) -> [(&'static str, &Check); 13] {
        [
            ("read-only root file system", &self.read_only_root),
            ("network mode", &self.network),
            ("no-new-privileges", &self.no_new_privs),
            ("dropped capabilities", &self.capabilities_dropped),
            ("unprivileged container", &self.unprivileged),
            ("container user", &self.user),
            ("private pid/ipc/uts namespaces", &self.private_namespaces),
            ("mounts", &self.mounts),
            ("ulimits", &self.rlimits),
            ("pids limit", &self.pids),
            ("memory limit", &self.memory),
            ("memory+swap limit", &self.swap),
            ("cpu quota", &self.cpus),
        ]
    }

    /// The properties the runtime recorded differently from the request. The swap and CPU settings are
    /// excluded: they are reported in the notes, they do not make the container weaker than promised in a
    /// way that matters to a hostile command.
    pub(crate) fn violations(&self) -> Vec<String> {
        self.all()
            .into_iter()
            .filter(|(what, _)| !matches!(*what, "memory+swap limit" | "cpu quota"))
            .filter_map(|(what, c)| match c {
                Check::Violated(why) => Some(format!("{what}: {why}")),
                _ => None,
            })
            .collect()
    }

    /// Everything that is not a plain `Holds`, for the report's notes.
    pub(crate) fn caveats(&self) -> Vec<String> {
        self.all()
            .into_iter()
            .filter_map(|(what, c)| match c {
                Check::Holds => None,
                Check::Unread => Some(format!("{what}: not reported by the runtime")),
                Check::Violated(why) => Some(format!("{what}: {why}")),
            })
            .collect()
    }

    /// Every protection was read back and matches.
    pub(crate) fn all_protections_hold(&self) -> bool {
        self.all()
            .into_iter()
            .filter(|(what, _)| !matches!(*what, "memory+swap limit" | "cpu quota"))
            .all(|(_, c)| c.holds())
    }
}

/// What `inspect` says happened to the container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct State {
    /// `created`, `running`, `exited`, ...
    pub status: String,
    pub exit_code: Option<i64>,
    pub oom_killed: bool,
    /// The runtime's error text (a command that could not be started).
    pub error: String,
}

/// The first element of `inspect`'s output (an array with one object).
pub(crate) fn record(inspect: &Value) -> Option<&Value> {
    match inspect {
        Value::Array(items) => items.first(),
        Value::Object(_) => Some(inspect),
        _ => None,
    }
}

pub(crate) fn state(rec: &Value) -> Option<State> {
    let s = rec.get("State")?;
    Some(State {
        status: s.get("Status").and_then(Value::as_str).unwrap_or("").to_string(),
        exit_code: s.get("ExitCode").and_then(Value::as_i64),
        oom_killed: s.get("OOMKilled").and_then(Value::as_bool).unwrap_or(false),
        error: s.get("Error").and_then(Value::as_str).unwrap_or("").to_string(),
    })
}

fn compare<T: PartialEq + std::fmt::Debug>(read: Option<T>, want: T) -> Check {
    match read {
        None => Check::Unread,
        Some(v) if v == want => Check::Holds,
        Some(v) => Check::Violated(format!("recorded {v:?}, requested {want:?}")),
    }
}

fn strings(v: Option<&Value>) -> Option<Vec<&str>> {
    match v? {
        Value::Null => Some(Vec::new()),
        Value::Array(items) => items.iter().map(Value::as_str).collect(),
        _ => None,
    }
}

/// Compare the runtime's record of a container with what was requested.
pub(crate) fn observe(rec: &Value, want: &Requested<'_>) -> Observed {
    let host = rec.get("HostConfig");
    let hc = |key: &str| host.and_then(|h| h.get(key));

    let read_only_root = compare(hc("ReadonlyRootfs").and_then(Value::as_bool), true);
    let network = compare(hc("NetworkMode").and_then(Value::as_str), if want.network_none { "none" } else { "host" });

    let no_new_privs = match strings(hc("SecurityOpt")) {
        None => Check::Unread,
        Some(opts) => {
            if opts
                .iter()
                .any(|o| matches!(*o, "no-new-privileges" | "no-new-privileges=true" | "no-new-privileges:true"))
            {
                Check::Holds
            } else {
                Check::Violated(format!("security options {opts:?} do not include no-new-privileges"))
            }
        }
    };

    let capabilities_dropped = match (strings(hc("CapDrop")), strings(hc("CapAdd"))) {
        (Some(drop), Some(add)) => {
            if !drop.iter().any(|c| c.eq_ignore_ascii_case("all")) {
                Check::Violated(format!("capabilities dropped: {drop:?}, requested ALL"))
            } else if !add.is_empty() {
                Check::Violated(format!("capabilities added back: {add:?}"))
            } else {
                Check::Holds
            }
        }
        _ => Check::Unread,
    };

    let unprivileged = {
        let privileged = hc("Privileged").and_then(Value::as_bool);
        // Docker records devices as objects, Podman too; an empty array (or null) is "none".
        let devices = matches!(hc("Devices"), Some(Value::Array(d)) if !d.is_empty());
        if privileged == Some(true) {
            Check::Violated("the container is privileged".into())
        } else if devices {
            Check::Violated("host devices are attached".into())
        } else if privileged.is_none() {
            Check::Unread
        } else {
            Check::Holds
        }
    };

    let user = compare(rec.pointer("/Config/User").and_then(Value::as_str), want.user);

    let private_namespaces = {
        let mode = |key: &str, allowed: &[&str]| match hc(key).and_then(Value::as_str) {
            None => Check::Unread,
            Some(m) if allowed.contains(&m) => Check::Holds,
            Some(m) => Check::Violated(format!("{key} is {m:?}")),
        };
        let parts = [
            mode("PidMode", &["", "private"]),
            mode("IpcMode", &["", "private", "shareable"]),
            mode("UTSMode", &["", "private"]),
        ];
        parts
            .iter()
            .find(|c| matches!(c, Check::Violated(_)))
            .or_else(|| parts.iter().find(|c| matches!(c, Check::Unread)))
            .cloned()
            .unwrap_or(Check::Holds)
    };

    let mounts = match rec.get("Mounts") {
        Some(Value::Array(list)) => {
            let mut problem = None;
            let mut seen = Vec::new();
            for m in list {
                let dest = m.get("Destination").and_then(Value::as_str).unwrap_or("");
                let rw = m.get("RW").and_then(Value::as_bool);
                let kind = m.get("Type").and_then(Value::as_str).unwrap_or("");
                match want.binds.iter().find(|(d, _)| d == dest) {
                    Some((_, want_rw)) => {
                        seen.push(dest.to_string());
                        if rw.is_some_and(|rw| rw != *want_rw) {
                            problem =
                                Some(format!("{dest} is {} but was requested {}", mode(rw), mode(Some(*want_rw))));
                        }
                    }
                    // Anonymous volumes come from the image's own VOLUME declarations, not from the host.
                    None if kind == "volume" => {}
                    None => problem = Some(format!("unexpected {kind:?} mount at {dest:?}")),
                }
            }
            for (dest, _) in &want.binds {
                if !seen.contains(dest) {
                    problem = Some(format!("the requested mount {dest} is missing"));
                }
            }
            problem.map_or(Check::Holds, Check::Violated)
        }
        // A container with no mounts at all reports null (or an empty array).
        Some(Value::Null) if want.binds.is_empty() => Check::Holds,
        Some(Value::Null) => Check::Violated("no mounts are recorded, the working tree is not mounted".into()),
        None if want.binds.is_empty() => Check::Holds,
        _ => Check::Unread,
    };

    let rlimits = match hc("Ulimits") {
        Some(Value::Array(list)) => {
            let mut result = Check::Holds;
            for (name, soft, hard) in &want.ulimits {
                let found = list.iter().find(|u| u.get("Name").and_then(Value::as_str) == Some(name));
                let read_soft = found.and_then(|u| u.get("Soft")).and_then(Value::as_i64);
                let read_hard = found.and_then(|u| u.get("Hard")).and_then(Value::as_i64);
                let want = (i64::try_from(*soft).unwrap_or(i64::MAX), i64::try_from(*hard).unwrap_or(i64::MAX));
                if (read_soft, read_hard) != (Some(want.0), Some(want.1)) {
                    result = Check::Violated(format!(
                        "ulimit {name} is {read_soft:?}:{read_hard:?}, requested {}:{}",
                        want.0, want.1
                    ));
                    break;
                }
            }
            result
        }
        _ => Check::Unread,
    };

    let pids = compare(hc("PidsLimit").and_then(Value::as_i64), i64::try_from(want.pids).unwrap_or(i64::MAX));
    let memory = compare(hc("Memory").and_then(Value::as_i64), i64::try_from(want.memory_bytes).unwrap_or(i64::MAX));
    let swap = compare(hc("MemorySwap").and_then(Value::as_i64), i64::try_from(want.memory_bytes).unwrap_or(i64::MAX));
    let cpus = match want.cpus {
        None => Check::Holds,
        Some(c) => compare(hc("NanoCpus").and_then(Value::as_i64), (c * 1e9).round() as i64),
    };

    Observed {
        read_only_root,
        network,
        no_new_privs,
        capabilities_dropped,
        unprivileged,
        user,
        private_namespaces,
        mounts,
        rlimits,
        pids,
        memory,
        swap,
        cpus,
    }
}

fn mode(rw: Option<bool>) -> &'static str {
    match rw {
        Some(true) => "read-write",
        Some(false) => "read-only",
        None => "of unknown mode",
    }
}

/// The numeric user id of a `--user` value (`uid`, `uid:gid`); `None` for a name that only the image resolves.
pub(crate) fn numeric_uid(user: &str) -> Option<u32> {
    user.split(':').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn requested() -> Requested<'static> {
        Requested {
            user: "65534:65534",
            network_none: true,
            pids: 32,
            memory_bytes: 64 << 20,
            cpus: Some(1.0),
            ulimits: vec![("nofile", 1024, 1024), ("core", 0, 0)],
            binds: vec![("/work".into(), true), ("/usr".into(), false)],
        }
    }

    /// The shape Docker 29 records for a container started with the sandbox's flags (captured with
    /// `docker inspect` against a real daemon; unrelated fields removed).
    fn docker_record() -> Value {
        json!([{
            "State": {"Status": "exited", "ExitCode": 137, "OOMKilled": true, "Error": ""},
            "Config": {"User": "65534:65534"},
            "HostConfig": {
                "ReadonlyRootfs": true, "NetworkMode": "none", "PidsLimit": 32,
                "Memory": 67108864, "MemorySwap": 67108864, "NanoCpus": 1000000000,
                "CapDrop": ["ALL"], "CapAdd": null, "SecurityOpt": ["no-new-privileges"],
                "Privileged": false, "Devices": [],
                "PidMode": "", "IpcMode": "private", "UTSMode": "",
                "Ulimits": [{"Hard": 0, "Name": "core", "Soft": 0}, {"Hard": 1024, "Name": "nofile", "Soft": 1024}]
            },
            "Mounts": [
                {"Type": "bind", "Source": "/srv/tree", "Destination": "/work", "Mode": "", "RW": true},
                {"Type": "bind", "Source": "/usr", "Destination": "/usr", "Mode": "ro", "RW": false}
            ]
        }])
    }

    #[test]
    fn a_record_that_matches_the_request_holds_everywhere() {
        let doc = docker_record();
        let rec = record(&doc).unwrap();
        let o = observe(rec, &requested());
        assert!(o.violations().is_empty(), "{:?}", o.violations());
        assert!(o.all_protections_hold(), "{:?}", o.caveats());
        assert!(o.caveats().is_empty());
        let s = state(rec).unwrap();
        assert_eq!((s.status.as_str(), s.exit_code, s.oom_killed), ("exited", Some(137), true));
    }

    #[test]
    fn every_protection_is_checked_against_the_record() {
        fn broken(edit: impl FnOnce(&mut Value)) -> Vec<String> {
            let mut doc = docker_record();
            edit(&mut doc[0]);
            observe(record(&doc).unwrap(), &requested()).violations()
        }
        assert!(broken(|r| r["HostConfig"]["ReadonlyRootfs"] = json!(false))[0].contains("read-only root"));
        assert!(broken(|r| r["HostConfig"]["NetworkMode"] = json!("bridge"))[0].contains("network mode"));
        assert!(broken(|r| r["HostConfig"]["SecurityOpt"] = json!([]))[0].contains("no-new-privileges"));
        assert!(broken(|r| r["HostConfig"]["CapDrop"] = json!(["NET_RAW"]))[0].contains("capabilities"));
        assert!(broken(|r| r["HostConfig"]["CapAdd"] = json!(["SYS_ADMIN"]))[0].contains("added back"));
        assert!(broken(|r| r["HostConfig"]["Privileged"] = json!(true))[0].contains("privileged"));
        assert!(broken(|r| r["HostConfig"]["Devices"] = json!([{"PathOnHost": "/dev/kvm"}]))[0].contains("devices"));
        assert!(broken(|r| r["Config"]["User"] = json!("0:0"))[0].contains("container user"));
        assert!(broken(|r| r["HostConfig"]["PidMode"] = json!("host"))[0].contains("PidMode"));
        assert!(broken(|r| r["HostConfig"]["IpcMode"] = json!("host"))[0].contains("IpcMode"));
        assert!(broken(|r| r["HostConfig"]["PidsLimit"] = json!(0))[0].contains("pids limit"));
        assert!(broken(|r| r["HostConfig"]["Memory"] = json!(0))[0].contains("memory limit"));
        assert!(broken(|r| r["HostConfig"]["Ulimits"] = json!([]))[0].contains("ulimit"));
        assert!(
            broken(|r| r["Mounts"].as_array_mut().unwrap().push(
                json!({"Type": "bind", "Source": "/var/run/docker.sock", "Destination": "/var/run/docker.sock", "RW": true})
            ))[0]
                .contains("unexpected")
        );
        assert!(broken(|r| r["Mounts"][1]["RW"] = json!(true))[0].contains("read-only"));
        assert!(broken(|r| r["Mounts"] = json!([]))[0].contains("missing"));
    }

    #[test]
    fn swap_and_cpu_shaping_are_caveats_not_failures() {
        let mut doc = docker_record();
        doc[0]["HostConfig"]["MemorySwap"] = json!(-1);
        doc[0]["HostConfig"]["NanoCpus"] = json!(0);
        let o = observe(record(&doc).unwrap(), &requested());
        assert!(o.violations().is_empty());
        assert!(o.all_protections_hold());
        assert_eq!(o.caveats().len(), 2, "{:?}", o.caveats());
    }

    #[test]
    fn what_the_record_leaves_out_is_unread_never_assumed() {
        let doc = json!([{"State": {"Status": "exited", "ExitCode": 0}, "HostConfig": {}}]);
        let o = observe(record(&doc).unwrap(), &requested());
        assert!(o.violations().is_empty());
        assert!(!o.all_protections_hold());
        assert_eq!(o.read_only_root, Check::Unread);
        assert_eq!(o.pids, Check::Unread);
        assert_eq!(o.mounts, Check::Unread, "a working tree that is not recorded is not claimed to be mounted");
    }

    #[test]
    fn images_may_declare_anonymous_volumes() {
        let mut doc = docker_record();
        doc[0]["Mounts"]
            .as_array_mut()
            .unwrap()
            .push(json!({"Type": "volume", "Name": "abc", "Destination": "/var/lib/data", "RW": true}));
        assert_eq!(observe(record(&doc).unwrap(), &requested()).mounts, Check::Holds);
    }

    #[test]
    fn daemons_describe_themselves_in_two_dialects() {
        let docker = json!({"SecurityOptions": ["name=seccomp,profile=builtin", "name=cgroupns"]});
        assert_eq!(
            daemon_facts(&docker),
            DaemonFacts { seccomp: Some(true), user_ns: Some(false), rootless: Some(false) }
        );
        let rootless = json!({"SecurityOptions": ["name=seccomp,profile=builtin", "name=rootless", "name=cgroupns"]});
        assert_eq!(daemon_facts(&rootless).user_ns, Some(true));
        assert_eq!(daemon_facts(&rootless).rootless, Some(true));
        let unconfined = json!({"SecurityOptions": ["name=seccomp,profile=unconfined"]});
        assert_eq!(daemon_facts(&unconfined).seccomp, Some(false));
        let none = json!({"SecurityOptions": []});
        assert_eq!(daemon_facts(&none).seccomp, Some(false));
        let podman = json!({"host": {"security": {"rootless": true, "seccompEnabled": true}}});
        assert_eq!(
            daemon_facts(&podman),
            DaemonFacts { seccomp: Some(true), user_ns: Some(true), rootless: Some(true) }
        );
        assert_eq!(daemon_facts(&json!({"something": "else"})), DaemonFacts::default());
    }

    #[test]
    fn users_have_a_numeric_uid_or_none() {
        assert_eq!(numeric_uid("65534:65534"), Some(65534));
        assert_eq!(numeric_uid("0"), Some(0));
        assert_eq!(numeric_uid("nobody"), None);
        assert_eq!(numeric_uid("nobody:nogroup"), None);
    }
}
