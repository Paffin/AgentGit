//! Behaviour that must be identical on every backend: the places where the
//! SQLite and PostgreSQL defaults differ (ordering, LIKE, aborted
//! transactions, NUL bytes).

use ag_changeset::{Changeset, Op};
use ag_object::hash_blob;
use ag_types::{FileType, Identity, Limits, Namespace, RepoPath, Timestamp};
use serde_json::json;

use super::meta;
use crate::meta::{MetaError, MetaKind};

#[test]
fn the_backend_under_test_is_the_one_the_environment_selects() {
    let m = meta();
    let want_pg = crate::testing::postgres_url().is_some();
    assert_eq!(m.kind() == MetaKind::Postgres, want_pg, "tests must really run on the selected backend");
    assert_eq!(m.schema_name().is_some(), want_pg);
}

#[test]
fn text_ordering_and_like_escaping_agree_across_backends() {
    // Byte-wise ordering (not locale), exact range scans, and LIKE metacharacters
    // in user data: the places where SQLite and PostgreSQL defaults differ.
    let m = meta();
    let c = Changeset::build(
        Namespace::new("t/t").unwrap(),
        vec![],
        Identity::new("user:t").unwrap(),
        Timestamp::from_millis(0),
        "paths",
        ["B", "a", "a b", "a/b", "a_b", "a%b", "aXb", "é", "Z/z"]
            .iter()
            .map(|p| Op::Put {
                path: RepoPath::new(*p).unwrap(),
                content: hash_blob(p.as_bytes()),
                file_type: FileType::Regular,
                executable: false,
            })
            .collect(),
        &Limits::default(),
    )
    .unwrap();
    let id = c.id();
    m.write(|tx| tx.register_changeset(&id, &c)).unwrap();
    let paths: Vec<String> = m.read(|tx| tx.changeset_paths(&id)).unwrap().into_iter().map(|(p, _)| p).collect();
    assert_eq!(paths, ["B", "Z/z", "a", "a b", "a%b", "a/b", "aXb", "a_b", "é"], "byte order");

    m.read(|tx| {
        assert!(tx.touches_path(&id, "a")?, "a directory touches its entries");
        assert!(tx.touches_path(&id, "Z")?);
        assert!(tx.touches_path(&id, "z")?, "ASCII case is ignored, as SQLite always did");
        assert!(!tx.touches_path(&id, "a_")?, "'_' is not a wildcard");
        assert!(!tx.touches_path(&id, "%")?, "'%' is not a wildcard");
        assert!(!tx.touches_path(&id, "aX")?);
        Ok(())
    })
    .unwrap();

    // audit search treats % and _ literally and ignores ASCII case
    m.write(|tx| {
        tx.audit_append("user:Alice", "x", "100%_done", &json!({}))?;
        tx.audit_append("user:bob", "x", "other", &json!({}))
    })
    .unwrap();
    assert_eq!(m.read(|tx| tx.audit_list(Some("100%_"), 10)).unwrap().len(), 1);
    assert_eq!(m.read(|tx| tx.audit_list(Some("ALICE"), 10)).unwrap().len(), 1);
    assert_eq!(m.read(|tx| tx.audit_list(None, 10)).unwrap().len(), 2);
    assert_eq!(m.read(|tx| tx.audit_list(Some("\\"), 10)).unwrap().len(), 0, "a backslash is literal too");
}

#[test]
fn huge_values_work_and_nul_bytes_are_refused_cleanly_on_postgres() {
    let m = meta();
    let big = "x".repeat(2_000_000);
    m.write(|tx| tx.meta_set("big", &big)).unwrap();
    assert_eq!(m.read(|tx| tx.meta_get("big")).unwrap().unwrap().len(), big.len());
    if m.is_postgres() {
        let r = m.write(|tx| tx.meta_set("nul", "a\0b"));
        assert!(matches!(r, Err(MetaError::Invalid(_))), "{r:?}");
        assert!(m.read(|tx| tx.meta_get("nul")).unwrap().is_none());
    }
}

#[test]
fn a_swallowed_statement_error_does_not_commit_a_half_transaction() {
    // PostgreSQL aborts a transaction after any failed statement; SQLite does not.
    // On PostgreSQL the write must fail rather than silently commit (or silently
    // lose) part of the work.
    let m = meta();
    let r = m.write(|tx| {
        tx.meta_set("before", "1")?;
        let _ = tx.raw_execute("INSERT INTO no_such_table VALUES (1)", &[]); // error ignored
        tx.meta_set("after", "2")
    });
    if m.is_postgres() {
        assert!(r.is_err(), "the ignored error must not turn into a commit");
        assert!(m.read(|tx| tx.meta_get("before")).unwrap().is_none());
    } else {
        r.unwrap(); // SQLite keeps going; the failed statement simply did nothing
    }
}

#[test]
fn a_single_statement_read_refuses_a_second_statement() {
    let m = meta();
    m.write(|tx| tx.meta_set("a", "1")).unwrap();
    assert_eq!(m.read_single(|tx| tx.meta_get("a")).unwrap().as_deref(), Some("1"));
    let two = m.read_single(|tx| {
        tx.meta_get("a")?;
        tx.meta_get("a")
    });
    if m.is_postgres() {
        assert!(matches!(two, Err(MetaError::Invalid(_))), "{two:?}");
    } else {
        two.unwrap(); // SQLite runs it as an ordinary read
    }
}
