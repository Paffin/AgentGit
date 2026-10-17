use super::*;
use ag_object::hash_blob;
use proptest::prelude::*;

fn ns() -> Namespace {
    Namespace::new("acme/app").unwrap()
}

fn id(n: u8) -> ChangesetId {
    ChangesetId::new(ag_types::ObjectId::from_bytes([n; 32]))
}

fn put(path: &str, content: &[u8]) -> Op {
    Op::Put {
        path: RepoPath::new(path).unwrap(),
        content: hash_blob(content),
        file_type: FileType::Regular,
        executable: false,
    }
}

fn del(path: &str) -> Op {
    Op::Delete { path: RepoPath::new(path).unwrap() }
}

fn sample() -> Changeset {
    Changeset::build(
        ns(),
        vec![id(1), id(2)],
        Identity::new("agent:bot#run-1").unwrap(),
        Timestamp::from_millis(1_760_659_200_000),
        "add things",
        vec![put("b.txt", b"b"), del("a/old.txt"), put("a/new.txt", b"new")],
        &Limits::default(),
    )
    .unwrap()
}

#[test]
fn build_sorts_ops_canonically() {
    let cs = sample();
    let paths: Vec<_> = cs.ops.iter().map(|o| o.path().as_str().to_string()).collect();
    assert_eq!(paths, ["a/new.txt", "a/old.txt", "b.txt"]);
}

#[test]
fn encode_decode_roundtrip_and_stable_id() {
    let cs = sample();
    let bytes = cs.encode();
    let back = Changeset::decode(&bytes, &Limits::default()).unwrap();
    assert_eq!(back, cs);
    assert_eq!(back.encode(), bytes);
    assert_eq!(back.id(), cs.id());
    assert!(cs.is_merge());
    assert_eq!(cs.first_parent(), Some(id(1)));
}

#[test]
fn id_depends_on_every_field() {
    let base = sample();
    let mut variants = Vec::new();
    let mut c = base.clone();
    c.message.push('!');
    variants.push(c);
    let mut c = base.clone();
    c.timestamp = Timestamp::from_millis(1);
    variants.push(c);
    let mut c = base.clone();
    c.parents.reverse();
    variants.push(c);
    let mut c = base.clone();
    c.namespace = Namespace::new("acme/other").unwrap();
    variants.push(c);
    let mut c = base.clone();
    c.author = Identity::new("agent:bot#run-2").unwrap();
    variants.push(c);
    let mut c = base.clone();
    c.ops.pop();
    variants.push(c);
    for v in variants {
        assert_ne!(v.id(), base.id());
    }
}

#[test]
fn rejects_non_canonical_ops() {
    let mut cs = sample();
    cs.ops.swap(0, 1);
    assert!(matches!(cs.validate(&Limits::default()), Err(ChangesetError::OpsNotCanonical(_))));
    // duplicate path
    let mut cs = sample();
    cs.ops.push(put("b.txt", b"other"));
    assert!(matches!(cs.validate(&Limits::default()), Err(ChangesetError::OpsNotCanonical(_))));
}

#[test]
fn rejects_duplicate_parents_and_limits() {
    let mut cs = sample();
    cs.parents = vec![id(1), id(1)];
    assert!(matches!(cs.validate(&Limits::default()), Err(ChangesetError::DuplicateParent(_))));

    let tight = Limits { max_ops: 2, ..Limits::default() };
    assert_eq!(sample().validate(&tight), Err(ChangesetError::TooManyOps(3)));
    let tight = Limits { max_parents: 1, ..Limits::default() };
    assert_eq!(sample().validate(&tight), Err(ChangesetError::TooManyParents(2)));
    let tight = Limits { max_message_len: 3, ..Limits::default() };
    assert_eq!(sample().validate(&tight), Err(ChangesetError::MessageTooLong(10)));
}

#[test]
fn rejects_executable_symlink() {
    let op = Op::Put {
        path: RepoPath::new("link").unwrap(),
        content: hash_blob(b"target"),
        file_type: FileType::Symlink,
        executable: true,
    };
    let r = Changeset::build(
        ns(),
        vec![],
        Identity::new("user:a").unwrap(),
        Timestamp::from_millis(0),
        "",
        vec![op],
        &Limits::default(),
    );
    assert!(matches!(r, Err(ChangesetError::ExecutableSymlink(_))));
}

#[test]
fn decode_rejects_trailing_bytes_and_bad_version() {
    let mut bytes = sample().encode();
    bytes.push(0);
    assert!(matches!(
        Changeset::decode(&bytes, &Limits::default()),
        Err(ChangesetError::Canon(CanonError::TrailingBytes(1)))
    ));
    let mut bytes = sample().encode();
    bytes[1] = 9;
    assert_eq!(Changeset::decode(&bytes, &Limits::default()), Err(ChangesetError::UnsupportedVersion(9)));
}

#[test]
fn from_raw_checks_kind() {
    let raw = RawObject::blob(b"x".to_vec());
    assert_eq!(Changeset::from_raw(&raw, &Limits::default()), Err(ChangesetError::WrongKind));
    let cs = sample();
    assert_eq!(Changeset::from_raw(&cs.to_raw(), &Limits::default()).unwrap(), cs);
}

#[test]
fn decode_rejects_traversal_paths() {
    // Hand-craft a payload with a ".." path to make sure decode revalidates.
    let mut w = Writer::new();
    w.u16(FORMAT_VERSION).str("acme/app").varint(0).str("user:a").i64(0).str("m").varint(1);
    w.u8(TAG_DELETE).str("../etc/passwd");
    let bytes = w.finish();
    assert!(matches!(
        Changeset::decode(&bytes, &Limits::default()),
        Err(ChangesetError::InvalidField { field: "path", .. })
    ));
}

#[test]
fn blobs_lists_only_puts() {
    let cs = sample();
    assert_eq!(cs.blobs().count(), 2);
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = Changeset::decode(&bytes, &Limits::default());
    }

    #[test]
    fn roundtrip_random_changesets(
        msg in "\\PC{0,40}",
        ts in any::<i64>(),
        files in proptest::collection::btree_map("[a-z]{1,5}(/[a-z]{1,5}){0,2}", proptest::collection::vec(any::<u8>(), 0..8), 0..8),
    ) {
        // btree_map keys are unique; drop any key that is a directory prefix of another.
        let keys: Vec<_> = files.keys().cloned().collect();
        let ops: Vec<Op> = files.iter()
            .filter(|(k, _)| !keys.iter().any(|o| o != *k && o.starts_with(&format!("{k}/"))))
            .map(|(k, v)| put(k, v)).collect();
        let cs = Changeset::build(
            ns(), vec![id(7)], Identity::new("user:p").unwrap(),
            Timestamp::from_millis(ts), msg, ops, &Limits::default(),
        ).unwrap();
        let bytes = cs.encode();
        let back = Changeset::decode(&bytes, &Limits::default()).unwrap();
        prop_assert_eq!(&back, &cs);
        prop_assert_eq!(back.encode(), bytes);
    }
}
