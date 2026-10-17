//! `ag completions SHELL` and `ag man`: the scripts parse in the shells that are installed, bash completes
//! real commands, the committed copies in `docs/reference/` are current, and the man pages are well formed.
//!
//! A shell that is not installed is skipped, unless `AG_REQUIRE_SHELLS=1` (CI sets it) turns that into a failure.
#![allow(missing_docs)]

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use common::Ag;

const SHELLS: &[(&str, &str)] =
    &[("bash", "ag.bash"), ("zsh", "_ag"), ("fish", "ag.fish"), ("powershell", "_ag.ps1"), ("elvish", "ag.elv")];

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn required() -> bool {
    std::env::var("AG_REQUIRE_SHELLS").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// True if `tool` can be used; false (with a note) if the test should be skipped.
fn available(tool: &str) -> bool {
    if have(tool) {
        return true;
    }
    assert!(!required(), "{tool} is required (AG_REQUIRE_SHELLS is set) but is not installed");
    eprintln!("skipping: {tool} is not installed");
    false
}

fn script(a: &Ag, shell: &str) -> String {
    a.ag(&["completions", shell]).ok().stdout()
}

#[test]
fn every_shell_gets_a_script_that_knows_the_commands() {
    let a = Ag::bare();
    for (shell, _) in SHELLS {
        let s = script(&a, shell);
        assert!(s.len() > 1000, "{shell}: suspiciously short");
        for word in ["snapshot", "integrate", "doctor", "completions", "json"] {
            assert!(s.contains(word), "{shell}: the script does not mention '{word}'");
        }
    }
    a.ag(&["completions", "tcsh"]).code_is(2);
}

#[test]
fn the_scripts_parse_in_the_shells_that_are_installed() {
    let a = Ag::bare();
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, shell: &str| {
        let p = dir.path().join(name);
        fs::write(&p, script(&a, shell)).unwrap();
        p
    };
    if available("bash") {
        let p = write("ag.bash", "bash");
        let o = Command::new("bash").arg("-n").arg(&p).output().unwrap();
        assert!(o.status.success(), "bash -n: {}", String::from_utf8_lossy(&o.stderr));
    }
    if available("zsh") {
        let p = write("_ag", "zsh");
        let o = Command::new("zsh").arg("-n").arg(&p).output().unwrap();
        assert!(o.status.success(), "zsh -n: {}", String::from_utf8_lossy(&o.stderr));
    }
    if available("fish") {
        let p = write("ag.fish", "fish");
        let o = Command::new("fish").arg("--no-execute").arg(&p).output().unwrap();
        assert!(o.status.success(), "fish --no-execute: {}", String::from_utf8_lossy(&o.stderr));
    }
    if have("pwsh") {
        let p = write("_ag.ps1", "powershell");
        let cmd = format!(
            "$tokens = $null; $errors = $null; [System.Management.Automation.Language.Parser]::ParseFile('{}', [ref]$tokens, [ref]$errors) | Out-Null; if ($errors.Count -gt 0) {{ $errors | ForEach-Object {{ $_.Message }}; exit 1 }}",
            p.display()
        );
        let o = Command::new("pwsh").args(["-NoProfile", "-NonInteractive", "-Command", &cmd]).output().unwrap();
        assert!(
            o.status.success(),
            "pwsh parse: {}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

#[test]
fn bash_completes_commands_and_option_values() {
    if !available("bash") {
        return;
    }
    let a = Ag::bare();
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("ag.bash");
    fs::write(&p, script(&a, "bash")).unwrap();
    let complete = |words: &[&str]| -> Vec<String> {
        let quoted: Vec<String> = words.iter().map(|w| format!("'{w}'")).collect();
        let prog = format!(
            "source '{}'; COMP_WORDS=({}); COMP_CWORD={}; COMP_LINE=\"{}\"; COMP_POINT=${{#COMP_LINE}}; _ag ag \"${{COMP_WORDS[COMP_CWORD]}}\" '{}'; printf '%s\\n' \"${{COMPREPLY[@]}}\"",
            p.display(),
            quoted.join(" "),
            words.len() - 1,
            words.join(" "),
            words[words.len() - 2]
        );
        let o = Command::new("bash").arg("-c").arg(prog).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect()
    };
    let r = complete(&["ag", "sta"]);
    assert_eq!(r, vec!["status".to_string()], "{r:?}");
    let r = complete(&["ag", "stream", "cr"]);
    assert_eq!(r, vec!["create".to_string()], "{r:?}");
    let r = complete(&["ag", "completions", ""]);
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        assert!(r.contains(&shell.to_string()), "{shell} missing from {r:?}");
    }
    let r = complete(&["ag", "--color", ""]);
    assert!(r.contains(&"always".to_string()) && r.contains(&"never".to_string()), "{r:?}");
    let r = complete(&["ag", "do"]);
    assert_eq!(r, vec!["doctor".to_string()]);
}

#[test]
fn the_copies_shipped_in_docs_reference_are_current() {
    let a = Ag::bare();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference/completions");
    if !dir.is_dir() {
        eprintln!("skipping: {} is not part of this checkout", dir.display());
        return;
    }
    for (shell, file) in SHELLS {
        let shipped = fs::read_to_string(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert!(
            shipped == script(&a, shell),
            "docs/reference/completions/{file} is out of date: run python3 scripts/gen_cli_docs.py"
        );
    }
}

#[test]
fn man_pages_cover_every_command_and_are_well_formed() {
    let a = Ag::bare();
    let top = a.ag(&["man"]).ok().stdout();
    assert!(top.starts_with(".ie \\n(.g .ds Aq") || top.contains(".TH ag 1"), "{}", &top[..top.len().min(200)]);
    assert!(top.contains("snapshot") && top.contains("integrate"));

    let dir = tempfile::tempdir().unwrap();
    let r = a.ag(&["man", "--dir", dir.path().to_str().unwrap()]).ok();
    assert!(r.stderr().contains("man page(s)"), "{}", r.stderr());
    let names: Vec<String> =
        fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    for expected in
        ["ag.1", "ag-integrate.1", "ag-stream-create.1", "ag-doctor.1", "ag-config-set.1", "ag-remote-integrate.1"]
    {
        assert!(names.iter().any(|n| n == expected), "{expected} missing from {names:?}");
    }
    let page = fs::read_to_string(dir.path().join("ag-integrate.1")).unwrap();
    assert!(page.contains("fencing"), "options are documented");

    // The committed copies are current (the generator writes the same files).
    let shipped = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference/man");
    if shipped.is_dir() {
        for n in &names {
            let want = fs::read_to_string(dir.path().join(n)).unwrap();
            let have = fs::read_to_string(shipped.join(n)).unwrap_or_else(|e| panic!("docs/reference/man/{n}: {e}"));
            assert!(want == have, "docs/reference/man/{n} is out of date: run python3 scripts/gen_cli_docs.py");
        }
    }
}
