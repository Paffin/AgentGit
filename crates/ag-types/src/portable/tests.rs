use super::*;

fn p(s: &str) -> RepoPath {
    RepoPath::new(s).unwrap_or_else(|e| panic!("{s:?}: {e}"))
}

#[test]
fn every_spelling_of_the_reserved_directories_is_reserved() {
    for name in [
        ".git",
        ".GIT",
        ".Git",
        ".ag",
        ".AG",
        // NTFS ignores trailing dots and spaces
        ".git.",
        ".git ",
        ".git...",
        ".git. .",
        ".AG.",
        ".ag ",
        // alternate data streams
        ".git::$INDEX_ALLOCATION",
        ".GIT::$index_allocation",
        ".git:stream",
        ".ag::$INDEX_ALLOCATION",
        // 8.3 short names
        "git~1",
        "GIT~1",
        "git~12",
        "GIT~1.",
        "git~1 ",
        "git~1::$INDEX_ALLOCATION",
        "ag~1",
        "AG~2",
        // HFS+ ignores these code points when comparing names
        ".g\u{200c}it",
        ".\u{feff}git",
        ".a\u{200d}g",
        "\u{202e}.git",
        ".git\u{206a}",
    ] {
        assert!(is_reserved_component(name), "{name:?} must be reserved");
    }
}

#[test]
fn ordinary_dot_names_are_not_reserved() {
    for name in [
        ".gitignore",
        ".gitattributes",
        ".gitmodules",
        ".git-notes",
        ".gitx",
        ".agignore",
        ".ag-tmp-1",
        "git",
        "ag",
        "agent",
        "git~",
        "git~x",
        "git~1x",
        "ag~1.txt",
        "a.git",
        ".git.d",
        "x.git.",
        "digit",
        "",
    ] {
        assert!(!is_reserved_component(name), "{name:?} must not be reserved");
    }
}

#[test]
fn windows_device_names_are_hazards_with_or_without_extension() {
    for name in [
        "CON",
        "con",
        "Con.txt",
        "PRN",
        "AUX",
        "aux.c",
        "NUL",
        "nul.tar.gz",
        "COM1",
        "com9.log",
        "LPT3",
        "lpt0",
        "COM\u{b9}",
        "LPT\u{b2}",
        "com\u{b3}.x",
        "CONIN$",
        "conout$",
        "aux .c",
        "dir/aux/file",
        "a/b/NUL",
    ] {
        let h = path_hazard(&p(name), &Target::WINDOWS).unwrap_or_else(|| panic!("{name} must be a hazard"));
        assert!(matches!(h.kind, HazardKind::DeviceName(_)), "{name}: {h:?}");
    }
    for name in ["CONSOLE", "comm", "COM10", "COM", "LPT", "auxiliary", "xcon", "con-1", "nu", "a/COM1x", "console.log"]
    {
        assert_eq!(path_hazard(&p(name), &Target::WINDOWS), None, "{name}");
    }
}

#[test]
fn trailing_dots_spaces_and_forbidden_characters_are_hazards() {
    for name in [
        "a.",
        "a ",
        "dir./x",
        "dir/x. ",
        "...",
        "a:b",
        "C:foo",
        "file.txt:Zone.Identifier",
        "x/y:stream",
        "a<b",
        "a>b",
        "a\"b",
        "a|b",
        "a?b",
        "a*b",
    ] {
        assert!(path_hazard(&p(name), &Target::WINDOWS).is_some(), "{name}");
    }
    assert!(matches!(path_hazard(&p("a:b"), &Target::WINDOWS).unwrap().kind, HazardKind::InvalidChar(':')));
    assert!(matches!(path_hazard(&p("a."), &Target::WINDOWS).unwrap().kind, HazardKind::TrailingDotOrSpace));
    for name in [".a", "a.b", "a b", "docs/файл.txt", "a-b_c", "x/.hidden", "a=b", "a,b", "a;b", "a$b"] {
        assert_eq!(path_hazard(&p(name), &Target::WINDOWS), None, "{name}");
    }
}

#[test]
fn other_targets_accept_what_only_windows_cannot_hold() {
    for name in ["aux.txt", "a:b", "a.", "a ", "CON", "a?b"] {
        assert_eq!(path_hazard(&p(name), &Target::POSIX), None, "{name}");
        assert_eq!(path_hazard(&p(name), &Target::MACOS), None, "{name}");
    }
}

#[test]
fn hazard_messages_say_what_and_why() {
    let msg = |name: &str| path_hazard(&p(name), &Target::WINDOWS).unwrap().to_string();
    assert!(msg("src/aux.c").contains("device name AUX"), "{}", msg("src/aux.c"));
    assert!(msg("a:b").contains("alternate data stream"));
    assert!(msg("a.").contains("trailing dot"));
    assert!(msg("a?b").contains("does not allow"));
}

fn paths(names: &[&str]) -> Vec<RepoPath> {
    names.iter().map(|n| p(n)).collect()
}

fn spellings(c: &[Collision]) -> Vec<Vec<&str>> {
    c.iter().map(|c| c.spellings.iter().map(String::as_str).collect()).collect()
}

#[test]
fn names_differing_only_in_case_collide_on_case_insensitive_targets() {
    let tree =
        paths(&["README", "readme", "src/a.rs", "src/b.rs", "Docs/x", "docs/y", "Ünï/f", "ünï/g", "ok/a", "ok/b"]);
    for t in [Target::WINDOWS, Target::MACOS] {
        let c = find_collisions(&tree, &t);
        assert_eq!(spellings(&c), vec![vec!["Docs", "docs"], vec!["README", "readme"], vec!["Ünï", "ünï"]], "{t:?}");
    }
    assert!(find_collisions(&tree, &Target::POSIX).is_empty(), "a case-sensitive target keeps them apart");
}

#[test]
fn a_collision_is_reported_once_at_the_directory_not_for_every_file_below() {
    let c = find_collisions(&paths(&["Src/a.rs", "src/a.rs", "src/b/c", "Src/b/c"]), &Target::WINDOWS);
    assert_eq!(spellings(&c), vec![vec!["Src", "src"]]);
}

#[test]
fn a_file_and_a_directory_that_differ_in_case_collide() {
    let c = find_collisions(&paths(&["lib", "Lib/x"]), &Target::MACOS);
    assert_eq!(spellings(&c), vec![vec!["Lib", "lib"]]);
}

#[test]
fn the_result_does_not_depend_on_input_order() {
    let names = ["b/X", "a", "A", "b/x", "c/D/e", "c/d/E", "z"];
    let forward = find_collisions(&paths(&names), &Target::WINDOWS);
    let mut rev = names;
    rev.reverse();
    assert_eq!(find_collisions(&paths(&rev), &Target::WINDOWS), forward);
    assert_eq!(spellings(&forward), vec![vec!["A", "a"], vec!["b/X", "b/x"], vec!["c/D", "c/d"]]);
}

#[test]
fn unicode_case_pairs_are_folded_like_a_file_system_would() {
    // Greek sigma forms, the Kelvin sign and the long s all upper-case into letters that have a lower-case twin.
    let c = find_collisions(&paths(&["\u{212a}", "k", "\u{17f}", "s", "\u{3a3}", "\u{3c3}"]), &Target::WINDOWS);
    assert_eq!(c.len(), 3, "{c:?}");
    // `ß` has no one-to-one upper-case, like on NTFS: it is its own name.
    assert!(find_collisions(&paths(&["stra\u{df}e", "STRASSE", "strasse"]), &Target::WINDOWS).len() == 1);
}

#[test]
fn canonically_equivalent_spellings_collide_only_where_the_file_system_normalises() {
    let nfc = "caf\u{e9}";
    let nfd = "cafe\u{301}";
    let tree = paths(&[nfc, nfd]);
    assert_eq!(find_collisions(&tree, &Target::MACOS).len(), 1, "APFS/HFS+ treat them as one name");
    assert!(find_collisions(&tree, &Target::WINDOWS).is_empty(), "NTFS keeps them apart");
    assert!(find_collisions(&tree, &Target::POSIX).is_empty());
}

#[test]
fn a_case_sensitive_volume_on_a_normalising_system_still_merges_equivalent_spellings_only() {
    // macOS with a case-sensitive APFS volume: `A` and `a` are different, `é` and `é` are not.
    let t = Target { case_insensitive: false, ..Target::MACOS };
    assert!(find_collisions(&paths(&["README", "readme"]), &t).is_empty());
    assert_eq!(find_collisions(&paths(&["caf\u{e9}", "cafe\u{301}"]), &t).len(), 1);
}

#[test]
fn inspect_reports_hazards_and_collisions_separately() {
    let tree = paths(&["src/aux.c", "README", "readme", "ok", "NUL/x", "nul/y"]);
    let f = inspect(&tree, &Target::WINDOWS);
    assert_eq!(f.hazards.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(), vec!["NUL/x", "nul/y", "src/aux.c"]);
    assert_eq!(spellings(&f.collisions), vec![vec!["README", "readme"]], "hazard paths are not also collisions");
    assert_eq!(f.len(), 4);
    assert!(!f.is_clean());
    assert!(inspect(&tree, &Target::POSIX).is_clean());
    let lines = f.describe(2);
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[2], "... and 2 more");
    assert_eq!(f.describe(10).len(), 4);
}

#[test]
fn host_target_matches_the_platform() {
    let t = Target::host();
    assert_eq!(t.windows_names, cfg!(windows));
    assert_eq!(t.case_insensitive, cfg!(any(windows, target_os = "macos")));
    assert_eq!(t.normalization_insensitive, cfg!(target_os = "macos"));
}
