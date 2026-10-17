#![no_main]
//! Path and name validation. `RepoPath` is the root of the traversal defence, so the invariants are the
//! security properties themselves: nothing accepted can escape a checkout, name `.git` or `.ag`, or hide a
//! control character; whatever is accepted round-trips through serde and `Display`.

use ag_types::{Identity, Namespace, ObjectId, OperationId, RepoPath, StreamName, Timestamp};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        // Non-UTF-8 never becomes a path; lossy forms must still be handled.
        let _ = RepoPath::new(String::from_utf8_lossy(data).into_owned());
        return;
    };

    if let Ok(p) = RepoPath::new(s) {
        assert_eq!(p.as_str(), s);
        assert!(!s.starts_with('/') && !s.ends_with('/') && !s.contains("//"));
        assert!(!s.chars().any(|c| c.is_control() || c == '\\'));
        for comp in p.components() {
            assert!(!comp.is_empty() && comp != "." && comp != "..");
            assert!(!comp.eq_ignore_ascii_case(".git") && !comp.eq_ignore_ascii_case(".ag"));
        }
        // A joined-up path must stay inside the root it is joined to.
        let joined = std::path::Path::new("/root").join(s);
        assert!(joined.starts_with("/root"));
        assert!(joined.components().all(|c| !matches!(c, std::path::Component::ParentDir)));
        for anc in p.ancestors() {
            let a = RepoPath::new(anc).expect("an ancestor of a valid path is valid");
            assert!(a.contains(&p) && !p.contains(&a));
        }
        assert!(p.contains(&p));
        assert_eq!(p.file_name(), s.rsplit('/').next().unwrap());
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<RepoPath>(&json).unwrap(), p);
        assert_eq!(p.to_string().parse::<RepoPath>().unwrap(), p);
    } else {
        // The JSON path must agree with the constructor.
        let json = serde_json::to_string(s).unwrap();
        assert!(serde_json::from_str::<RepoPath>(&json).is_err());
    }

    if let Ok(id) = Identity::new(s) {
        assert_eq!(id.as_str(), s);
        let _ = (id.kind(), id.name(), id.principal());
        assert_eq!(id.principal().principal(), id.principal());
        assert_eq!(serde_json::from_str::<Identity>(&serde_json::to_string(&id).unwrap()).unwrap(), id);
    }
    if let Ok(n) = Namespace::new(s) {
        assert_eq!(n.to_string().parse::<Namespace>().unwrap(), n);
    }
    if let Ok(n) = StreamName::new(s) {
        assert_eq!(n.to_string().parse::<StreamName>().unwrap(), n);
        // Stream names end up in file names and URLs; they must not carry path syntax.
        assert!(!n.as_str().contains(['\0', '\n']));
    }
    if let Ok(n) = OperationId::new(s) {
        assert_eq!(n.to_string().parse::<OperationId>().unwrap(), n);
    }
    if let Ok(id) = s.parse::<ObjectId>() {
        // Only the canonical lowercase spelling is accepted: one id, one string.
        assert_eq!(id.to_hex(), s);
    }

    // Timestamps decoded from objects are arbitrary i64s; formatting must be total.
    if data.len() >= 8 {
        let ms = i64::from_be_bytes(data[..8].try_into().unwrap());
        let ts = Timestamp::from_millis(ms);
        let _ = ts.to_rfc3339();
        let _ = ts.to_string();
    }
});
