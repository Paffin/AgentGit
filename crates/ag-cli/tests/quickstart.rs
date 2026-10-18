//! Runs `examples/quickstart.sh` (the demo shown in the README) against the freshly built binary — and, on
//! Windows, its PowerShell twin `examples/quickstart.ps1`.
#![allow(missing_docs)]

use std::path::PathBuf;
use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// A line of the README transcript as a pattern: `…` and any run of six or more hexadecimal digits (an id, a hash,
/// possibly cut short with `…`) stand for "any text". `None` is a wildcard; `Some(s)` must appear as is.
fn pattern(line: &str) -> Vec<Option<String>> {
    let chars: Vec<char> = line.chars().collect();
    let (mut out, mut lit, mut i) = (Vec::new(), String::new(), 0);
    let flush = |lit: &mut String, out: &mut Vec<Option<String>>| {
        if !lit.is_empty() {
            out.push(Some(std::mem::take(lit)));
        }
    };
    while i < chars.len() {
        let start = i;
        while i < chars.len() && (chars[i].is_ascii_digit() || ('a'..='f').contains(&chars[i])) {
            i += 1;
        }
        let run = i - start;
        if i < chars.len() && chars[i] == '…' && run > 0 {
            i += 1; // "8f78…": a cut-short id
        } else if run < 6 {
            lit.extend(&chars[start..i]);
            if run == 0 {
                if chars[i] == '…' {
                    flush(&mut lit, &mut out);
                    out.push(None);
                } else {
                    lit.push(chars[i]);
                }
                i += 1;
            }
            continue;
        }
        flush(&mut lit, &mut out);
        out.push(None);
    }
    flush(&mut lit, &mut out);
    out.dedup_by(|a, b| a.is_none() && b.is_none());
    out
}

/// Does `text` match the whole of `pat`?
fn matches(pat: &[Option<String>], text: &str) -> bool {
    match pat.split_first() {
        None => text.is_empty(),
        Some((None, rest)) => (0..=text.len()).filter(|&i| text.is_char_boundary(i)).any(|i| matches(rest, &text[i..])),
        Some((Some(lit), rest)) => text.strip_prefix(lit.as_str()).is_some_and(|r| matches(rest, r)),
    }
}

/// `s` without ANSI colour sequences (the demo prints its headings in bold even when `NO_COLOR` is set).
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The output lines of the README's "See it work" transcript (not its `$ ...` command lines), without the trailing
/// `   # comment` that annotates some of them.
fn readme_transcript() -> Vec<String> {
    let readme = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../README.md")).unwrap();
    let section = readme.split("## See it work").nth(1).expect("README has a 'See it work' section");
    let block = section.split("```text").nth(1).expect("the section has a text block").split("```").next().unwrap();
    block
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('$'))
        .map(|l| match l.find("  #") {
            Some(i) => l[..i].trim_end().to_string(),
            None => l.trim_end().to_string(),
        })
        .collect()
}

#[test]
fn the_matcher_for_the_readme_transcript_means_what_it_says() {
    let p =
        pattern("check 'tests' PASSED on tree 315f74d45183 (policy v1); attestation att_0f1a… signed by service:ci");
    assert!(matches(
        &p,
        "check 'tests' PASSED on tree aaaaaaaaaaaa (policy v1); attestation att_9c2b4e1d77 signed by service:ci"
    ));
    assert!(!matches(
        &p,
        "check 'tests' FAILED on tree aaaaaaaaaaaa (policy v1); attestation att_9c2b signed by service:ci"
    ));
    assert!(!matches(
        &p,
        "check 'tests' PASSED on tree aaaaaaaaaaaa (policy v1); attestation att_9c2b signed by someone"
    ));
    let tail = pattern("  - actor not allowed to integrate: agents may not integrate unless … on …");
    assert!(matches(
        &tail,
        "  - actor not allowed to integrate: agents may not integrate unless agent_autonomy is on and more"
    ));
    assert!(!matches(&tail, "  - actor allowed to integrate: agents may not integrate unless x"));
    let exact = pattern("Blocked (policy v1); the stream was not changed:");
    assert!(matches(&exact, "Blocked (policy v1); the stream was not changed:"));
    assert!(!matches(&exact, "Blocked (policy v1); the stream was not changed: extra"));
    assert!(!matches(&exact, "xx Blocked (policy v1); the stream was not changed:"));
}

#[test]
fn the_readme_demo_script_passes() {
    // On Windows `bash` may be a stub (WSL) and `python3` the Store alias: the PowerShell script covers it.
    if cfg!(windows) {
        eprintln!("skipping on Windows: see the_powershell_demo_script_passes");
        return;
    }
    if !have("bash") || !have("python3") {
        eprintln!("skipping: bash and python3 are required");
        return;
    }
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/quickstart.sh");
    let out = Command::new("bash").arg(&script).env("AG", env!("CARGO_BIN_EXE_ag")).output().expect("run bash");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "quickstart failed\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}");
    assert!(stdout.contains("Demo finished"), "{stdout}");

    // The transcript in the README ("See it work") is trimmed from this demo: every line of output it shows must be
    // a line that the demo really prints (ids and text cut short with `…` are wildcards). The README is not edited
    // by hand-waving: when a message is reworded, this test says which README line went stale.
    let printed: Vec<String> =
        plain(&format!("{stdout}\n{stderr}")).lines().map(|l| l.trim_end().to_string()).collect();
    let transcript = readme_transcript();
    assert!(transcript.len() >= 8, "the README transcript lost its output lines: {transcript:?}");
    for line in &transcript {
        let pat = pattern(line);
        assert!(
            printed.iter().any(|p| matches(&pat, p)),
            "the README shows a line that examples/quickstart.sh does not print:\n    {line}\nthe demo printed:\n{}",
            printed.join("\n")
        );
    }
}

/// The same demo in PowerShell. PowerShell 7 (`pwsh`) if installed, else the Windows PowerShell 5.1 that ships with
/// every Windows; CI runs the script under both.
#[cfg(windows)]
#[test]
fn the_powershell_demo_script_passes() {
    let usable = |shell: &str| {
        Command::new(shell)
            .args(["-NoProfile", "-NonInteractive", "-Command", "exit 0"])
            .output()
            .is_ok_and(|o| o.status.success())
    };
    let shell = ["pwsh", "powershell"].into_iter().find(|s| usable(s)).expect("no PowerShell found");
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/quickstart.ps1");
    let out = Command::new(shell)
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .env("AG", env!("CARGO_BIN_EXE_ag"))
        .output()
        .expect("run PowerShell");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "quickstart.ps1 failed under {shell}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(stdout.contains("Demo finished"), "{stdout}");
}
