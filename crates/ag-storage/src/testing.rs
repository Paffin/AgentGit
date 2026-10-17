//! Test support, shared by this crate's tests and the engine's: which backends
//! are under test, and an isolated schema / key prefix per test.
//!
//! By default everything runs on SQLite and the filesystem or memory, with no
//! external service. Setting environment variables moves the *same* tests to
//! the other backends:
//!
//! | variable | effect |
//! |---|---|
//! | `AG_TEST_POSTGRES_URL` | metadata in PostgreSQL (a fresh schema per test, dropped afterwards) |
//! | `AG_TEST_S3_ENDPOINT` (+ `AG_TEST_S3_BUCKET`, `AG_TEST_S3_REGION`, `AG_TEST_S3_ACCESS_KEY_ID`, `AG_TEST_S3_SECRET_ACCESS_KEY`) | objects in an S3-compatible bucket (a fresh key prefix per test, emptied afterwards) |
//! | `AG_REQUIRE_BACKENDS=1` | a test that needs a backend whose variable is unset **fails** instead of silently running on the default |
//!
//! `scripts/pg-test.sh` and `scripts/s3-test.sh` start throwaway servers and
//! set these variables around any command.

use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;

use ag_types::Limits;

use crate::clock::Clock;
use crate::meta::{Meta, MetaError};

/// Connection URL of the PostgreSQL server to test against.
pub const ENV_POSTGRES_URL: &str = "AG_TEST_POSTGRES_URL";
/// Endpoint of the S3-compatible server to test against.
pub const ENV_S3_ENDPOINT: &str = "AG_TEST_S3_ENDPOINT";
/// Make missing backends an error instead of a silent fallback.
pub const ENV_REQUIRE_BACKENDS: &str = "AG_REQUIRE_BACKENDS";

fn truthy(v: &str) -> bool {
    matches!(v.trim(), "1" | "true" | "yes" | "on")
}

/// True if `AG_REQUIRE_BACKENDS` asks for failure when a backend is missing.
pub fn backends_required() -> bool {
    std::env::var(ENV_REQUIRE_BACKENDS).is_ok_and(|v| truthy(&v))
}

fn from_env(var: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => {
            assert!(
                !backends_required(),
                "{ENV_REQUIRE_BACKENDS} is set but {var} is not: the tests that need it would silently be skipped \
                 (run them through scripts/pg-test.sh / scripts/s3-test.sh)"
            );
            None
        }
    }
}

/// The PostgreSQL URL under test, if configured. With `AG_REQUIRE_BACKENDS` set
/// and no URL this panics, which fails the calling test.
pub fn postgres_url() -> Option<String> {
    from_env(ENV_POSTGRES_URL)
}

/// The S3 endpoint under test, if configured (same rule as [`postgres_url`]).
pub fn s3_endpoint() -> Option<String> {
    from_env(ENV_S3_ENDPOINT)
}

/// A random lowercase-hex token for unique names.
pub fn unique(prefix: &str) -> String {
    ag_crypto::random_id(prefix).expect("system randomness").to_lowercase()
}

/// A PostgreSQL schema that exists for the lifetime of the value.
pub struct TempSchema {
    url: String,
    name: String,
}

impl TempSchema {
    /// Reserves a unique schema name on the server behind `url` (the schema is
    /// created by the first [`Meta::open_postgres`]).
    pub fn new(url: &str) -> Self {
        Self { url: url.to_string(), name: unique("agtest") }
    }

    /// Schema name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Server URL.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for TempSchema {
    fn drop(&mut self) {
        // Best effort: a throwaway cluster is destroyed wholesale anyway.
        let _ = crate::meta::drop_schema(&self.url, &self.name);
    }
}

enum Backing {
    Memory,
    File(PathBuf),
    Postgres(TempSchema),
}

/// A [`Meta`] on whichever backend the environment selects, cleaned up on drop.
pub struct TestMeta {
    meta: Meta,
    backing: Backing,
    clock: Arc<dyn Clock>,
}

impl Deref for TestMeta {
    type Target = Meta;
    fn deref(&self) -> &Meta {
        &self.meta
    }
}

impl TestMeta {
    /// PostgreSQL if `AG_TEST_POSTGRES_URL` is set, otherwise a private
    /// in-memory SQLite database (which cannot be [`reopen`](Self::reopen)ed).
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        match postgres_url() {
            Some(url) => Self::postgres_at(&url, clock),
            None => Self {
                meta: Meta::open_in_memory(clock.clone()).expect("open in-memory SQLite"),
                backing: Backing::Memory,
                clock,
            },
        }
    }

    /// Like [`TestMeta::new`] but SQLite lives in a temporary file, so a second
    /// handle ([`TestMeta::reopen`]) can be opened: the stand-in for "another process".
    pub fn shared(clock: Arc<dyn Clock>) -> Self {
        match postgres_url() {
            Some(url) => Self::postgres_at(&url, clock),
            None => Self::sqlite_file(clock),
        }
    }

    /// SQLite in a temporary file, whatever the environment says.
    pub fn sqlite_file(clock: Arc<dyn Clock>) -> Self {
        let dir = std::env::temp_dir().join(unique("agtest-meta"));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let meta = Meta::open_with(&dir.join("meta.db"), clock.clone(), Limits::default()).expect("open SQLite");
        Self { meta, backing: Backing::File(dir), clock }
    }

    /// PostgreSQL, or `None` when no server is configured (and `AG_REQUIRE_BACKENDS` is off).
    pub fn postgres(clock: Arc<dyn Clock>) -> Option<Self> {
        postgres_url().map(|url| Self::postgres_at(&url, clock))
    }

    fn postgres_at(url: &str, clock: Arc<dyn Clock>) -> Self {
        let schema = TempSchema::new(url);
        let meta = Meta::open_postgres(url, schema.name(), clock.clone(), Limits::default())
            .unwrap_or_else(|e| panic!("cannot open PostgreSQL schema {}: {e}", schema.name()));
        Self { meta, backing: Backing::Postgres(schema), clock }
    }

    /// True if this handle talks to PostgreSQL.
    pub fn is_postgres(&self) -> bool {
        matches!(self.backing, Backing::Postgres(_))
    }

    /// A second, independent handle on the same database: its own connection(s),
    /// as another process would have.
    pub fn reopen(&self) -> Result<Meta, MetaError> {
        match &self.backing {
            Backing::Memory => {
                Err(MetaError::Invalid("an in-memory database cannot be reopened; use TestMeta::shared".into()))
            }
            Backing::File(dir) => Meta::open_with(&dir.join("meta.db"), self.clock.clone(), Limits::default()),
            Backing::Postgres(s) => Meta::open_postgres(s.url(), s.name(), self.clock.clone(), Limits::default()),
        }
    }

    /// The PostgreSQL URL and schema (`None` for SQLite).
    pub fn postgres_target(&self) -> Option<(&str, &str)> {
        match &self.backing {
            Backing::Postgres(s) => Some((s.url(), s.name())),
            _ => None,
        }
    }

    /// Path of the SQLite file (`None` for memory and PostgreSQL).
    pub fn sqlite_path(&self) -> Option<PathBuf> {
        match &self.backing {
            Backing::File(dir) => Some(dir.join("meta.db")),
            _ => None,
        }
    }
}

impl Drop for TestMeta {
    fn drop(&mut self) {
        if let Backing::File(dir) = &self.backing {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

// ---- objects -------------------------------------------------------------------

/// Settings of the S3 server under test (the `AG_TEST_S3_*` variables) with every
/// key under `prefix`. `None` when no server is configured (and `AG_REQUIRE_BACKENDS` is off).
#[cfg(feature = "s3")]
pub fn s3_config(prefix: &str) -> Option<crate::S3Config> {
    let endpoint = s3_endpoint()?;
    let env =
        |k: &str, default: &str| std::env::var(k).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string());
    let creds =
        crate::Credentials::new(env("AG_TEST_S3_ACCESS_KEY_ID", "test"), env("AG_TEST_S3_SECRET_ACCESS_KEY", "test"));
    let mut cfg = crate::S3Config::new(
        endpoint,
        env("AG_TEST_S3_REGION", "us-east-1"),
        env("AG_TEST_S3_BUCKET", "ag-test"),
        creds,
    );
    cfg.prefix = prefix.to_string();
    Some(cfg)
}

/// An [`ObjectStore`](crate::ObjectStore) on whichever backend the environment selects:
/// an S3 bucket prefix (emptied on drop) if `AG_TEST_S3_ENDPOINT` is set, otherwise memory.
pub struct TestObjects {
    store: Arc<dyn crate::ObjectStore>,
    #[cfg(feature = "s3")]
    s3: Option<Arc<crate::S3ObjectStore>>,
}

impl TestObjects {
    /// Memory (aging by `clock`) or a fresh S3 prefix (aging by the server's clock).
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        #[cfg(feature = "s3")]
        if let Some(cfg) = s3_config(&unique("agtest")) {
            let s3 = Arc::new(
                crate::S3ObjectStore::open(cfg, Arc::new(crate::InProcessLock::new())).expect("open the S3 test store"),
            );
            return Self { store: s3.clone(), s3: Some(s3) };
        }
        Self {
            store: Arc::new(crate::MemoryObjectStore::with_clock(clock)),
            #[cfg(feature = "s3")]
            s3: None,
        }
    }

    /// The store.
    pub fn store(&self) -> Arc<dyn crate::ObjectStore> {
        self.store.clone()
    }

    /// True if the store is S3 (object ages then come from the server, not from the test clock).
    pub fn is_s3(&self) -> bool {
        #[cfg(feature = "s3")]
        return self.s3.is_some();
        #[cfg(not(feature = "s3"))]
        false
    }
}

impl Drop for TestObjects {
    fn drop(&mut self) {
        #[cfg(feature = "s3")]
        if let Some(s3) = &self.s3 {
            let _ = s3.purge_for_test();
        }
    }
}
