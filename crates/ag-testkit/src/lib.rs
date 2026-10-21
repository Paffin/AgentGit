//! The one way a test of this repository says "the component I need is not here".
//!
//! A test that talks to an external component (PostgreSQL, an S3 server, a container runtime, Keycloak, the
//! `ag-server` binary, a model, ...) must never pass by doing nothing. It asks [`require_component`] instead of
//! returning early:
//!
//! * the component is there: the test runs;
//! * the component is missing and `AG_REQUIRE_COMPONENTS` names it (or is `all`): the test **fails**;
//! * otherwise the test **skips** and leaves a machine-readable marker, which `scripts/check_skips.py` reads and
//!   compares with the reviewed list `tests/skip-allowlist.toml`.
//!
//! The marker is one line, written to the real stderr (the test harness swallows `eprintln!`) and, when
//! `AG_SKIP_REPORT` names a file, appended to it as one JSON object per line:
//!
//! ```text
//! AG-SKIP component=postgres test=the_pool_reconnects reason=AG_TEST_POSTGRES_URL is not set
//! {"marker":"AG-SKIP","component":"postgres","test":"the_pool_reconnects","reason":"...","lang":"rust"}
//! ```
//!
//! Component names are lower-case; `oci:docker` and `oci:podman` are refinements of `oci` (requiring `oci` requires
//! either). The vocabulary is listed in `docs/architecture/test-skips.md`.
//!
//! The older switches keep working as aliases: `AG_REQUIRE_BACKENDS` (postgres, s3), `AG_REQUIRE_SANDBOX`
//! (namespace-sandbox), `AG_REQUIRE_OCI` (oci), `AG_REQUIRE_CGROUP` (cgroup-v2), `AG_REQUIRE_SHELLS` (shell),
//! `AG_REQUIRE_DOC_SNIPPETS` (bash, python3, git).

use std::fmt::Display;
use std::io::Write as _;

/// The list of components whose absence is a failure: `postgres,s3`, or `all`.
pub const ENV_REQUIRE: &str = "AG_REQUIRE_COMPONENTS";
/// A file every skip is appended to (JSON lines), whatever the harness does with stderr.
pub const ENV_REPORT: &str = "AG_SKIP_REPORT";
/// The marker every skip line starts with.
pub const MARKER: &str = "AG-SKIP";

/// `(legacy variable, the components it stands for)`.
const LEGACY: &[(&str, &[&str])] = &[
    ("AG_REQUIRE_BACKENDS", &["postgres", "s3"]),
    ("AG_REQUIRE_SANDBOX", &["namespace-sandbox"]),
    ("AG_REQUIRE_OCI", &["oci"]),
    ("AG_REQUIRE_CGROUP", &["cgroup-v2"]),
    ("AG_REQUIRE_SHELLS", &["shell"]),
    ("AG_REQUIRE_DOC_SNIPPETS", &["bash", "python3", "git"]),
];

fn truthy(v: &str) -> bool {
    !matches!(v.trim(), "" | "0" | "false" | "no")
}

/// Does the `list` (`a,b` / `a b` / `all`) require `component`? `oci` and `oci:docker` cover each other.
pub fn list_requires(list: &str, component: &str) -> bool {
    let component = component.to_ascii_lowercase();
    list.split(|c: char| c == ',' || c.is_whitespace()).map(str::trim).filter(|t| !t.is_empty()).any(|t| {
        let t = t.to_ascii_lowercase();
        t == "all"
            || t == component
            || component.strip_prefix(&t).is_some_and(|rest| rest.starts_with(':'))
            || t.strip_prefix(&component).is_some_and(|rest| rest.starts_with(':'))
    })
}

fn required_with(get: &dyn Fn(&str) -> Option<String>, component: &str) -> bool {
    if get(ENV_REQUIRE).is_some_and(|l| list_requires(&l, component)) {
        return true;
    }
    LEGACY
        .iter()
        .any(|(var, comps)| get(var).is_some_and(|v| truthy(&v)) && comps.iter().any(|c| list_requires(c, component)))
}

/// Is the absence of `component` a failure in this process?
pub fn required(component: &str) -> bool {
    required_with(&|k| std::env::var(k).ok(), component)
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

/// The name of the running test: the harness names the thread of each test after it. With `--test-threads=1` the
/// tests run on the main thread and the name is lost, so it is then the test binary's.
pub fn current_test() -> String {
    let t = std::thread::current();
    match t.name() {
        Some(n) if n != "main" => n.to_string(),
        _ => std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "unknown".into()),
    }
}

/// The line [`skip`] writes to stderr.
pub fn marker_line(component: &str, test: &str, reason: &str) -> String {
    format!("{MARKER} component={} test={} reason={}", component, test, one_line(reason))
}

fn record(component: &str, test: &str, reason: &str) {
    // Written to the real stderr: the test harness would swallow `eprintln!` output of passing tests.
    let _ = writeln!(std::io::stderr(), "{}", marker_line(component, test, reason));
    if let Some(path) = std::env::var_os(ENV_REPORT).filter(|p| !p.is_empty()) {
        let line = format!(
            "{{\"marker\":\"{MARKER}\",\"component\":\"{}\",\"test\":\"{}\",\"reason\":\"{}\",\"lang\":\"rust\"}}\n",
            json_escape(component),
            json_escape(test),
            json_escape(&one_line(reason))
        );
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// Report that `test` cannot run here for lack of `component`: **panics** when the component is required
/// (`AG_REQUIRE_COMPONENTS`), otherwise leaves the marker and returns (the caller returns).
#[track_caller]
pub fn skip(component: &str, test: &str, reason: &str) {
    if required(component) {
        panic!("{component} is required ({ENV_REQUIRE}) but unavailable, so {test} cannot run: {reason}");
    }
    record(component, test, reason);
}

/// The value when the component is there; when it is not, a failure (required) or a skip with a marker
/// (`None`). Use as `let Some(x) = require_component("postgres", "my_test", probe()) else { return };`.
#[track_caller]
pub fn require_component<T, E: Display>(component: &str, test: &str, available: Result<T, E>) -> Option<T> {
    match available {
        Ok(v) => Some(v),
        Err(why) => {
            skip(component, test, &why.to_string());
            None
        }
    }
}

/// [`require_component`] for a probe that answers `Option`, with the reason given here.
#[track_caller]
pub fn require_some<T>(component: &str, test: &str, reason: &str, available: Option<T>) -> Option<T> {
    require_component(component, test, available.ok_or(reason))
}

/// [`require_some`] for the running test (see [`current_test`]): the form for a shared `fn url() -> Option<String>`
/// helper that does not know which test calls it.
#[track_caller]
pub fn require_here<T>(component: &str, reason: &str, available: Option<T>) -> Option<T> {
    require_some(component, &current_test(), reason, available)
}

/// The component name of a container runtime given as the command that starts it (`docker`,
/// `/usr/local/bin/podman-rootful`): `oci:docker`, `oci:podman`, or plain `oci` when it cannot be told.
pub fn oci_component(runtime: &str) -> String {
    let name = runtime.rsplit(['/', '\\']).next().unwrap_or(runtime).to_ascii_lowercase();
    if name.contains("podman") {
        "oci:podman".into()
    } else if name.contains("docker") {
        "oci:docker".into()
    } else {
        "oci".into()
    }
}

/// A test that does not apply under the configuration it runs in (a backend whose clock a test cannot move: the
/// same guarantee is tested elsewhere). Never a failure; the marker (component `n/a:<kind>`) still reaches the
/// report, so the list of such exclusions is reviewed like the list of skips.
pub fn not_applicable(kind: &str, test: &str, reason: &str) {
    record(&format!("n/a:{kind}"), test, reason);
}

/// A test that does not apply on this platform (the Windows harness drives bash, ...). Never a failure: the
/// platform is a fact, not a missing component. The marker (component `platform:<os>`) still reaches the report.
pub fn skip_platform(test: &str, reason: &str) {
    record(&format!("platform:{}", std::env::consts::OS), test, reason);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn a_list_names_components() {
        assert!(list_requires("postgres,s3", "s3"));
        assert!(list_requires(" postgres , s3 ", "postgres"));
        assert!(list_requires("postgres s3", "s3"));
        assert!(!list_requires("postgres,s3", "keycloak"));
        assert!(!list_requires("", "postgres"));
        assert!(!list_requires("postgres", "post"));
        assert!(!list_requires("postgres", "postgres-tls"));
        assert!(list_requires("ALL", "anything"));
        assert!(list_requires("Postgres", "POSTGRES"));
    }

    #[test]
    fn a_runtime_refines_the_generic_name() {
        assert!(list_requires("oci", "oci:docker"));
        assert!(list_requires("oci:docker", "oci"));
        assert!(list_requires("oci:docker", "oci:docker"));
        assert!(!list_requires("oci:docker", "oci:podman"));
        assert!(!list_requires("oci:docker", "ocip"));
    }

    #[test]
    fn nothing_is_required_by_default() {
        assert!(!required_with(&env(&[]), "postgres"));
        assert!(!required_with(&env(&[(ENV_REQUIRE, "")]), "postgres"));
    }

    #[test]
    fn the_older_switches_are_aliases() {
        let e = env(&[("AG_REQUIRE_BACKENDS", "1")]);
        assert!(required_with(&e, "postgres") && required_with(&e, "s3") && !required_with(&e, "oci"));
        assert!(!required_with(&env(&[("AG_REQUIRE_BACKENDS", "0")]), "s3"));
        assert!(required_with(&env(&[("AG_REQUIRE_OCI", "1")]), "oci:podman"));
        assert!(required_with(&env(&[("AG_REQUIRE_SANDBOX", "1")]), "namespace-sandbox"));
        assert!(required_with(&env(&[("AG_REQUIRE_CGROUP", "1")]), "cgroup-v2"));
        assert!(required_with(&env(&[("AG_REQUIRE_SHELLS", "1")]), "shell:zsh"));
        assert!(required_with(&env(&[("AG_REQUIRE_DOC_SNIPPETS", "1")]), "bash"));
    }

    #[test]
    fn the_marker_is_one_line() {
        assert_eq!(
            marker_line("postgres", "t", "AG_TEST_POSTGRES_URL\nis not   set"),
            "AG-SKIP component=postgres test=t reason=AG_TEST_POSTGRES_URL is not set"
        );
    }

    #[test]
    fn a_runtime_command_names_its_component() {
        assert_eq!(oci_component("docker"), "oci:docker");
        assert_eq!(oci_component("/usr/local/bin/podman-rootful"), "oci:podman");
        assert_eq!(oci_component("/opt/tools/nerdctl"), "oci");
    }

    #[test]
    fn json_is_escaped() {
        assert_eq!(json_escape("a\"b\\c\n"), "a\\\"b\\\\c\\u000a");
    }

    #[test]
    fn an_available_component_yields_its_value() {
        assert_eq!(require_component("postgres", "t", Ok::<_, String>(7)), Some(7));
        assert_eq!(require_some("postgres", "t", "no", Some(1)), Some(1));
    }
}
