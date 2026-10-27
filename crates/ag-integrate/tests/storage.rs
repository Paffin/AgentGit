//! The `storage` section of `.ag/config.json`: how a repository is pointed at
//! PostgreSQL and S3, and that nothing secret ends up in the file.
//!
//! The tests that need a server skip without `AG_TEST_POSTGRES_URL` /
//! `AG_TEST_S3_ENDPOINT` (and fail with `AG_REQUIRE_COMPONENTS=postgres,s3`).
#![allow(missing_docs)]

mod common;

use std::fs;

use ag_integrate::{Error, InitOptions, MetaConfig, ObjectsConfig, Repository, StorageConfig};
use ag_storage::testing::{TempSchema, postgres_url};
use ag_types::{Namespace, StreamName};
use common::*;

fn opts(storage: Option<StorageConfig>) -> InitOptions {
    InitOptions {
        namespace: Namespace::new("acme/app").unwrap(),
        default_stream: StreamName::new("main").unwrap(),
        actor: id("user:alice"),
        storage,
    }
}

fn postgres_section(schema: &str) -> StorageConfig {
    StorageConfig {
        meta: MetaConfig::Postgres {
            url_env: Some("AG_TEST_POSTGRES_URL".into()),
            url: None,
            schema: schema.into(),
            pool_size: Some(4),
        },
        objects: ObjectsConfig::Fs,
    }
}

#[test]
fn a_default_repository_has_no_storage_section_and_old_configs_still_open() {
    let dir = tempdir();
    // `Some(default)` is the same as `None`: no redundant section is written
    let repo = Repository::init(dir.path(), &opts(Some(StorageConfig::default()))).unwrap();
    let json = fs::read_to_string(dir.path().join(".ag/config.json")).unwrap();
    assert!(!json.contains("storage"), "{json}");
    assert!(dir.path().join(".ag/meta.db").is_file() && dir.path().join(".ag/store/objects").is_dir());
    drop(repo);

    // a configuration written by an earlier release (format 1, no storage key)
    let old = tempdir();
    fs::create_dir_all(old.path().join(".ag")).unwrap();
    fs::write(old.path().join(".ag/config.json"), r#"{"format":1,"namespace":"acme/old","default_stream":"main"}"#)
        .unwrap();
    let repo = Repository::open(old.path()).unwrap();
    assert!(repo.config().storage.is_none());
    assert_eq!(repo.config().format, 1);
    assert_eq!(repo.meta().kind(), ag_storage::meta::MetaKind::Sqlite);
}

#[test]
fn secrets_are_refused_before_anything_is_created() {
    let dir = tempdir();
    let mut s = postgres_section("ag_x");
    let MetaConfig::Postgres { url, url_env, .. } = &mut s.meta else { unreachable!() };
    *url_env = None;
    *url = Some("postgres://alice:hunter2@db.example/ag".into());
    let err = Repository::init(dir.path(), &opts(Some(s))).err().expect("refused");
    let msg = err.to_string();
    assert!(matches!(err, Error::Config(_)), "{msg}");
    assert!(msg.contains("password") && msg.contains("url_env") && !msg.contains("hunter2"), "{msg}");
    assert!(!dir.path().join(".ag").exists(), "nothing was created");

    let bad_schema = postgres_section("Not A Schema");
    assert!(matches!(Repository::init(dir.path(), &opts(Some(bad_schema))), Err(Error::Config(_))));
}

#[test]
fn a_missing_environment_variable_is_named_in_the_error() {
    let dir = tempdir();
    fs::create_dir_all(dir.path().join(".ag")).unwrap();
    fs::write(
        dir.path().join(".ag/config.json"),
        r#"{"format":1,"namespace":"acme/app","default_stream":"main",
            "storage":{"meta":{"kind":"postgres","url_env":"AG_SURELY_UNSET_URL","schema":"ag_x"}}}"#,
    )
    .unwrap();
    let err = Repository::open(dir.path()).err().expect("refused").to_string();
    assert!(err.contains("AG_SURELY_UNSET_URL") && err.contains("url_env"), "{err}");

    fs::write(
        dir.path().join(".ag/config.json"),
        r#"{"format":1,"namespace":"acme/app","default_stream":"main",
            "storage":{"objects":{"kind":"s3","endpoint":"http://127.0.0.1:1","bucket":"b",
                                  "access_key_id_env":"AG_SURELY_UNSET_KEY"}}}"#,
    )
    .unwrap();
    let err = Repository::open(dir.path()).err().expect("refused").to_string();
    assert!(err.contains("AG_SURELY_UNSET_KEY") && err.contains("access_key_id_env"), "{err}");
}

#[test]
fn a_typo_in_the_storage_section_is_an_error_not_a_silent_default() {
    let dir = tempdir();
    fs::create_dir_all(dir.path().join(".ag")).unwrap();
    fs::write(
        dir.path().join(".ag/config.json"),
        r#"{"format":1,"namespace":"acme/app","default_stream":"main",
            "storage":{"meta":{"kind":"postgres","schema":"ag_x","url_envv":"AG_PG_URL"}}}"#,
    )
    .unwrap();
    let err = Repository::open(dir.path()).err().expect("refused").to_string();
    assert!(err.contains("url_envv"), "{err}");
}

#[test]
fn a_postgres_repository_is_created_reopened_and_cannot_be_initialized_twice() {
    let Some(url) = ag_testkit::require_here(
        "postgres",
        "AG_TEST_POSTGRES_URL is not set (use scripts/pg-test.sh)",
        postgres_url(),
    ) else {
        return;
    };
    let schema = TempSchema::new(&url);
    let dir = tempdir();
    let repo = Repository::init(dir.path(), &opts(Some(postgres_section(schema.name())))).unwrap();
    assert_eq!(repo.meta().schema_name(), Some(schema.name()));

    // the configuration names the variable, not the URL; the format is unchanged
    let json = fs::read_to_string(dir.path().join(".ag/config.json")).unwrap();
    assert!(json.contains("\"format\": 1") && json.contains("AG_TEST_POSTGRES_URL"), "{json}");
    assert!(!json.contains("postgres://"), "{json}");
    // a pre-storage `ag` that opens this directory fails loudly instead of creating an empty local database
    assert!(dir.path().join(".ag/meta.db").is_dir(), "the SQLite path is occupied on purpose");

    // work, then reopen: same state
    let c = snap_in(&repo, None, &[("a", "1")], "user:alice");
    let p = repo.propose("main", &c, "t", "", &id("user:alice"), None).unwrap();
    repo.integrate(&ag_integrate::IntegrateRequest {
        proposal_id: p.id,
        actor: id("user:alice"),
        idempotency_key: None,
        fencing_tokens: vec![],
    })
    .unwrap();
    let again = Repository::open(dir.path()).unwrap();
    assert_eq!(again.resolve_rev("main").unwrap(), c);
    assert_eq!(repo.reopen().unwrap().resolve_rev("main").unwrap(), c);
    assert!(again.fsck().unwrap().is_ok());
    assert!(again.config().storage.is_some());

    // another directory pointed at the same schema must not initialize on top of it
    let other = tempdir();
    let err = Repository::init(other.path(), &opts(Some(postgres_section(schema.name())))).err().expect("refused");
    assert!(matches!(err, Error::Conflict(_)) && err.to_string().contains("already holds a repository"), "{err}");
    assert!(!other.path().join(".ag/config.json").exists());

    // while a different schema is a different, independent repository
    let second = TempSchema::new(&url);
    let third = tempdir();
    let r2 = Repository::init(third.path(), &opts(Some(postgres_section(second.name())))).unwrap();
    assert!(r2.resolve_rev("main").is_err(), "empty: schemas isolate repositories");
    drop(r2);

    // A schema that vanished (dropped, or the URL points at another database) must not quietly
    // become a new, empty repository when opened.
    ag_storage::meta::drop_schema(&url, second.name()).unwrap();
    let err = Repository::open(third.path()).err().expect("refused").to_string();
    assert!(err.contains("holds no repository") && err.contains(second.name()), "{err}");
    let m = ag_storage::Meta::open_postgres(
        &url,
        second.name(),
        std::sync::Arc::new(ag_storage::SystemClock),
        Default::default(),
    );
    assert!(m.is_ok(), "(creating it explicitly, as init does, still works)");
    drop((schema, second));
}

#[cfg(feature = "s3")]
#[test]
fn an_s3_repository_keeps_no_objects_locally_and_survives_reopen() {
    if ag_testkit::require_here(
        "s3",
        "AG_TEST_S3_ENDPOINT is not set (use scripts/s3-test.sh)",
        ag_storage::testing::s3_endpoint(),
    )
    .is_none()
    {
        return;
    }
    let dir = tempdir();
    let mut storage = storage_for_tests(&dir).unwrap();
    storage.meta = MetaConfig::Sqlite;
    let repo = Repository::init(dir.path(), &opts(Some(storage))).unwrap();
    assert!(objects_are_s3(&repo));
    assert!(dir.path().join(".ag/store").is_file(), "the local object path is occupied on purpose");
    let json = fs::read_to_string(dir.path().join(".ag/config.json")).unwrap();
    assert!(json.contains("AG_TEST_S3_SECRET_ACCESS_KEY") && !json.contains("\"secret_access_key\""), "{json}");

    let blob = repo.put_blob(b"in the bucket".to_vec()).unwrap();
    let again = Repository::open(dir.path()).unwrap();
    assert_eq!(again.blob(&blob).unwrap(), b"in the bucket");
    assert_eq!(again.objects().durability().name, "s3-compatible");
}
