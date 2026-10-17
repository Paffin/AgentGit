//! Turning a [`SandboxSpec`] into the C-string data the child needs, shared by the process-based sandboxes.

use std::ffi::CString;
use std::path::Path;

use crate::error::SandboxError;
use crate::spawn::CStrings;
use crate::spec::SandboxSpec;

/// `PATH` used to resolve `argv[0]` when the spec's environment does not set one.
const DEFAULT_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

pub(crate) fn cstring(s: &str) -> Result<CString, SandboxError> {
    CString::new(s).map_err(|_| SandboxError::invalid("string contains a NUL byte"))
}

pub(crate) fn path_cstring(p: &Path) -> Result<CString, SandboxError> {
    use std::os::unix::ffi::OsStrExt;
    CString::new(p.as_os_str().as_bytes()).map_err(|_| SandboxError::invalid("path contains a NUL byte"))
}

/// `argv`, `envp` and the ordered list of paths to try with `execve`.
///
/// `argv[0]` containing a `/` is used as is (relative to the working directory); otherwise it is searched
/// for on the spec's own `PATH` — never the host's.
pub(crate) fn exec_inputs(spec: &SandboxSpec) -> Result<(CStrings, CStrings, Vec<CString>), SandboxError> {
    let argv = spec.argv.iter().map(|a| cstring(a)).collect::<Result<Vec<_>, _>>()?;
    let envp = spec.env.iter().map(|(k, v)| cstring(&format!("{k}={v}"))).collect::<Result<Vec<_>, _>>()?;
    let program = &spec.argv[0];
    let paths = if program.contains('/') {
        vec![cstring(program)?]
    } else {
        let path = spec.env.get("PATH").map(String::as_str).unwrap_or(DEFAULT_PATH);
        path.split(':')
            .filter(|d| !d.is_empty())
            .map(|d| cstring(&format!("{}/{program}", d.trim_end_matches('/'))))
            .collect::<Result<Vec<_>, _>>()?
    };
    if paths.is_empty() {
        return Err(SandboxError::invalid("`PATH` in the environment is empty, so argv[0] cannot be resolved"));
    }
    Ok((CStrings::new(argv), CStrings::new(envp), paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv0_is_resolved_on_the_specs_own_path() {
        let dir = std::env::temp_dir();
        let spec = SandboxSpec::new(["tool", "x"], &dir).env("PATH", "/opt/bin::/usr/bin/");
        let (_, _, paths) = exec_inputs(&spec).unwrap();
        let got: Vec<_> = paths.iter().map(|p| p.to_str().unwrap().to_string()).collect();
        assert_eq!(got, ["/opt/bin/tool", "/usr/bin/tool"]);
        let spec = SandboxSpec::new(["tool"], &dir);
        let (_, _, paths) = exec_inputs(&spec).unwrap();
        assert_eq!(paths.len(), 3, "the default PATH is used, not the host's");
        let spec = SandboxSpec::new(["./run.sh"], &dir);
        assert_eq!(exec_inputs(&spec).unwrap().2[0].to_str().unwrap(), "./run.sh");
        assert!(exec_inputs(&SandboxSpec::new(["tool"], &dir).env("PATH", "")).is_err());
    }
}
