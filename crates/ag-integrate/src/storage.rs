//! Where a repository keeps its metadata and its objects.
//!
//! The `storage` section of `.ag/config.json` is optional and additive (the
//! config `format` stays 1). Without it a repository is what it has always
//! been: a SQLite file and a directory of objects under `.ag/`.
//!
//! ```json
//! {
//!   "format": 1, "namespace": "acme/app", "default_stream": "main",
//!   "storage": {
//!     "meta":    { "kind": "postgres", "url_env": "AG_PG_URL", "schema": "ag_acme_app" },
//!     "objects": { "kind": "s3", "endpoint": "https://s3.eu-west-1.amazonaws.com",
//!                  "region": "eu-west-1", "bucket": "acme-agentgit", "prefix": "acme/app" }
//!   }
//! }
//! ```
//!
//! **Secrets are never stored.** The file names the environment variables that
//! hold the PostgreSQL URL (with its password) and the S3 credentials; a literal
//! `url` is accepted only when it carries no password.

use std::path::PathBuf;
use std::sync::Arc;

use ag_storage::meta::{PgOptions, url_has_password, validate_schema_name};
use ag_storage::{Clock, FileLock, FsObjectStore, Meta, ObjectLock, ObjectStore, PostgresAdvisoryLock};
use ag_types::Limits;
use serde::{Deserialize, Serialize};

use crate::layout::Layout;
use crate::{Error, Result};

/// Environment variable that holds the PostgreSQL URL unless `url_env` says otherwise.
pub const DEFAULT_PG_URL_ENV: &str = "AG_PG_URL";
/// Default variable names for the S3 credentials.
pub const DEFAULT_S3_ACCESS_KEY_ID_ENV: &str = "AWS_ACCESS_KEY_ID";
/// See [`DEFAULT_S3_ACCESS_KEY_ID_ENV`].
pub const DEFAULT_S3_SECRET_ACCESS_KEY_ENV: &str = "AWS_SECRET_ACCESS_KEY";
/// See [`DEFAULT_S3_ACCESS_KEY_ID_ENV`].
pub const DEFAULT_S3_SESSION_TOKEN_ENV: &str = "AWS_SESSION_TOKEN";

/// The `storage` section of the repository configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    /// Where the metadata lives.
    #[serde(default)]
    pub meta: MetaConfig,
    /// Where the objects live.
    #[serde(default)]
    pub objects: ObjectsConfig,
}

/// Metadata backend.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MetaConfig {
    /// `.ag/meta.db`, SQLite in WAL mode (the default).
    #[default]
    Sqlite,
    /// One schema of a PostgreSQL database.
    Postgres {
        /// Name of the environment variable that holds the connection URL
        /// (`postgres://user:password@host/db?sslmode=verify-full`).
        /// Default `AG_PG_URL`. Preferred: the URL usually contains a password.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url_env: Option<String>,
        /// A literal connection URL, accepted only if it contains no password
        /// (for example `host=/var/run/postgresql dbname=ag`, authenticated by the OS user).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// The schema that holds this repository (lowercase letters, digits, `_`).
        schema: String,
        /// Maximum pooled connections (default 8).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pool_size: Option<usize>,
    },
}

/// Object backend.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObjectsConfig {
    /// `.ag/store/objects`, files on the local disk (the default).
    #[default]
    Fs,
    /// An S3-compatible bucket.
    S3 {
        /// Service endpoint, e.g. `https://s3.eu-west-1.amazonaws.com` or `http://minio:9000`.
        endpoint: String,
        /// Signing region (default `us-east-1`).
        #[serde(default = "default_region")]
        region: String,
        /// Bucket name; it must already exist.
        bucket: String,
        /// Key prefix; default: the repository namespace. Repositories sharing a bucket need different prefixes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix: Option<String>,
        /// Path-style addressing (`host/bucket/key`). Default: yes, except for Amazon's endpoints.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path_style: Option<bool>,
        /// Variable holding the access key id (default `AWS_ACCESS_KEY_ID`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_key_id_env: Option<String>,
        /// Variable holding the secret access key (default `AWS_SECRET_ACCESS_KEY`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret_access_key_env: Option<String>,
        /// Variable holding the session token, if any (default `AWS_SESSION_TOKEN`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_token_env: Option<String>,
        /// PEM file with the CA certificate(s) of a private endpoint (not a secret).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ca_bundle: Option<String>,
        /// How writers and the garbage collector exclude each other (default `auto`).
        #[serde(default)]
        lock: ObjectLockKind,
    },
}

fn default_region() -> String {
    "us-east-1".into()
}

/// The lock that keeps garbage collection from deleting an object a writer just stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectLockKind {
    /// `postgres_advisory` with PostgreSQL metadata, otherwise `file`.
    #[default]
    Auto,
    /// One process only (tests, embedding). **Not safe** with several `ag` processes.
    InProcess,
    /// An `flock` on `.ag/objects.lock`: every process on this host that opens the repository.
    File,
    /// PostgreSQL session advisory locks: every process that can reach the database,
    /// on any host. Requires PostgreSQL metadata.
    PostgresAdvisory,
}

impl StorageConfig {
    /// True if this is the default layout (SQLite file + local objects).
    pub fn is_default(&self) -> bool {
        *self == StorageConfig::default()
    }

    /// Checks the configuration without touching any service: unusable names, a
    /// password in a literal URL, a lock kind that does not fit the metadata backend.
    pub fn validate(&self) -> Result<()> {
        if let MetaConfig::Postgres { url, schema, pool_size, .. } = &self.meta {
            validate_schema_name(schema).map_err(|e| Error::Config(format!("storage.meta.schema: {e}")))?;
            if let Some(u) = url
                && url_has_password(u)
            {
                return Err(Error::Config(
                    "storage.meta.url contains a password (or is not a valid connection string); secrets are never \
                     written to .ag/config.json: put the URL in an environment variable and name it with url_env"
                        .into(),
                ));
            }
            if *pool_size == Some(0) {
                return Err(Error::Config("storage.meta.pool_size must be at least 1".into()));
            }
        }
        if let ObjectsConfig::S3 { endpoint, bucket, prefix, lock, .. } = &self.objects {
            if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
                return Err(Error::Config("storage.objects.endpoint must start with http:// or https://".into()));
            }
            if endpoint.contains('@') {
                return Err(Error::Config(
                    "storage.objects.endpoint must not contain credentials; name the environment variables with \
                     access_key_id_env / secret_access_key_env"
                        .into(),
                ));
            }
            if bucket.is_empty() {
                return Err(Error::Config("storage.objects.bucket is empty".into()));
            }
            if let Some(p) = prefix
                && (p.contains("//") || p.chars().any(char::is_control))
            {
                return Err(Error::Config(format!("storage.objects.prefix '{p}' is not usable")));
            }
            if *lock == ObjectLockKind::PostgresAdvisory && !matches!(self.meta, MetaConfig::Postgres { .. }) {
                return Err(Error::Config(
                    "storage.objects.lock = postgres_advisory needs PostgreSQL metadata (storage.meta.kind = postgres)"
                        .into(),
                ));
            }
        }
        Ok(())
    }
}

fn env_value(var: &str, what: &str, key: &str) -> Result<String> {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(Error::Config(format!(
            "the environment variable {var} is not set; it must hold {what} (named by {key} in .ag/config.json)"
        ))),
    }
}

/// How to reach the PostgreSQL schema (resolved from the environment at open time).
#[derive(Clone)]
pub(crate) struct PgTarget {
    pub url: String,
    pub schema: String,
}

/// Opens the metadata store named by `storage`.
///
/// With `create`, a PostgreSQL schema that does not exist yet is created (`ag init`);
/// without it, a missing or empty schema is an error (opening an existing repository).
pub(crate) fn open_meta(
    storage: &StorageConfig,
    layout: &Layout,
    clock: Arc<dyn Clock>,
    limits: &Limits,
    create: bool,
) -> Result<(Meta, Option<PgTarget>)> {
    match &storage.meta {
        MetaConfig::Sqlite => Ok((Meta::open_with(&layout.db_path(), clock, limits.clone())?, None)),
        MetaConfig::Postgres { url_env, url, schema, pool_size } => {
            let url = match url {
                Some(u) => u.clone(),
                None => {
                    let var = url_env.as_deref().unwrap_or(DEFAULT_PG_URL_ENV);
                    env_value(var, "the PostgreSQL connection URL", "storage.meta.url_env")?
                }
            };
            let mut opts = PgOptions { create_schema: create, ..PgOptions::default() };
            if let Some(n) = pool_size {
                opts.pool_size = *n;
            }
            let meta = Meta::open_postgres_with(&url, schema, clock, limits.clone(), &opts)?;
            Ok((meta, Some(PgTarget { url, schema: schema.clone() })))
        }
    }
}

/// Opens the object store named by `storage`.
pub(crate) fn open_objects(
    storage: &StorageConfig,
    layout: &Layout,
    namespace: &str,
    limits: &Limits,
    pg: Option<&PgTarget>,
) -> Result<Arc<dyn ObjectStore>> {
    match &storage.objects {
        ObjectsConfig::Fs => Ok(Arc::new(FsObjectStore::open(layout.objects_dir(), limits.clone())?)),
        ObjectsConfig::S3 { .. } => open_s3(storage, layout, namespace, limits, pg),
    }
}

#[cfg(not(feature = "s3"))]
fn open_s3(_: &StorageConfig, _: &Layout, _: &str, _: &Limits, _: Option<&PgTarget>) -> Result<Arc<dyn ObjectStore>> {
    Err(Error::Config("this build of AgentGit has no S3 support (compiled without the `s3` feature)".into()))
}

#[cfg(feature = "s3")]
fn open_s3(
    storage: &StorageConfig,
    layout: &Layout,
    namespace: &str,
    limits: &Limits,
    pg: Option<&PgTarget>,
) -> Result<Arc<dyn ObjectStore>> {
    use ag_storage::{Credentials, S3Config, S3ObjectStore};

    let ObjectsConfig::S3 {
        endpoint,
        region,
        bucket,
        prefix,
        path_style,
        access_key_id_env,
        secret_access_key_env,
        session_token_env,
        ca_bundle,
        lock,
    } = &storage.objects
    else {
        unreachable!("checked by the caller");
    };
    let id_var = access_key_id_env.as_deref().unwrap_or(DEFAULT_S3_ACCESS_KEY_ID_ENV);
    let secret_var = secret_access_key_env.as_deref().unwrap_or(DEFAULT_S3_SECRET_ACCESS_KEY_ENV);
    let token_var = session_token_env.as_deref().unwrap_or(DEFAULT_S3_SESSION_TOKEN_ENV);
    let mut credentials = Credentials::new(
        env_value(id_var, "the S3 access key id", "storage.objects.access_key_id_env")?,
        env_value(secret_var, "the S3 secret access key", "storage.objects.secret_access_key_env")?,
    );
    if let Ok(t) = std::env::var(token_var)
        && !t.trim().is_empty()
    {
        credentials = credentials.with_session_token(t);
    }
    let prefix = prefix.clone().unwrap_or_else(|| namespace.to_string());
    let mut cfg = S3Config::new(endpoint.clone(), region.clone(), bucket.clone(), credentials);
    cfg.prefix = prefix.clone();
    cfg.limits = limits.clone();
    if let Some(ps) = path_style {
        cfg.path_style = *ps;
    }
    cfg.ca_bundle = ca_bundle.as_ref().map(PathBuf::from);

    let lock: Arc<dyn ObjectLock> = match (lock, pg) {
        (ObjectLockKind::Auto | ObjectLockKind::PostgresAdvisory, Some(target)) => {
            Arc::new(PostgresAdvisoryLock::connect(&target.url, &format!("{}/{bucket}/{prefix}", target.schema))?)
        }
        (ObjectLockKind::PostgresAdvisory, None) => {
            return Err(Error::Config("storage.objects.lock = postgres_advisory needs PostgreSQL metadata".into()));
        }
        (ObjectLockKind::Auto | ObjectLockKind::File, _) => {
            Arc::new(FileLock::new(layout.ag_dir().join("objects.lock")))
        }
        (ObjectLockKind::InProcess, _) => Arc::new(ag_storage::InProcessLock::new()),
    };
    Ok(Arc::new(S3ObjectStore::open(cfg, lock)?))
}

const META_SENTINEL: &str = "This repository keeps its metadata in PostgreSQL (see config.json, storage.meta).\n\
This path is occupied on purpose: an older `ag`, which does not know about\n\
the storage section, would otherwise create a fresh, empty local database and\n\
silently work on the wrong repository. Upgrade `ag` instead.\n";

const STORE_SENTINEL: &str = "This repository keeps its objects in an S3-compatible bucket (see config.json, storage.objects).\n\
This path is occupied on purpose: an older `ag`, which does not know about\n\
the storage section, would otherwise write objects to a local directory that\n\
the rest of the repository does not know about. Upgrade `ag` instead.\n";

/// Occupies the default local paths of a backend that is not used, so that an
/// `ag` which predates the `storage` section fails loudly instead of quietly
/// operating on an empty local repository.
pub(crate) fn place_guards(layout: &Layout, storage: &StorageConfig) -> std::io::Result<()> {
    if matches!(storage.meta, MetaConfig::Postgres { .. }) {
        let db = layout.db_path();
        if !db.exists() {
            std::fs::create_dir_all(&db)?;
            std::fs::write(db.join("README"), META_SENTINEL)?;
        }
    }
    if matches!(storage.objects, ObjectsConfig::S3 { .. }) {
        let store = layout.objects_dir();
        if !store.exists() {
            std::fs::write(&store, STORE_SENTINEL)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> serde_json::Result<StorageConfig> {
        serde_json::from_str(json)
    }

    #[test]
    fn the_default_is_sqlite_and_local_files() {
        let c = parse("{}").unwrap();
        assert!(c.is_default());
        assert_eq!(c.meta, MetaConfig::Sqlite);
        assert_eq!(c.objects, ObjectsConfig::Fs);
        c.validate().unwrap();
        assert_eq!(serde_json::to_string(&c).unwrap(), r#"{"meta":{"kind":"sqlite"},"objects":{"kind":"fs"}}"#);
    }

    #[test]
    fn postgres_and_s3_sections_round_trip() {
        let json = r#"{
            "meta": {"kind": "postgres", "url_env": "MY_PG", "schema": "ag_acme"},
            "objects": {"kind": "s3", "endpoint": "http://minio:9000", "bucket": "b", "path_style": true,
                        "secret_access_key_env": "MINIO_SECRET", "lock": "postgres_advisory"}
        }"#;
        let c = parse(json).unwrap();
        c.validate().unwrap();
        let MetaConfig::Postgres { url_env, schema, url, pool_size } = &c.meta else { panic!() };
        assert_eq!((url_env.as_deref(), schema.as_str(), url, pool_size), (Some("MY_PG"), "ag_acme", &None, &None));
        let ObjectsConfig::S3 { region, lock, prefix, .. } = &c.objects else { panic!() };
        assert_eq!((region.as_str(), *lock, prefix), ("us-east-1", ObjectLockKind::PostgresAdvisory, &None));
        assert_eq!(parse(&serde_json::to_string(&c).unwrap()).unwrap(), c);
    }

    #[test]
    fn typos_and_unknown_keys_are_errors_not_silently_ignored() {
        assert!(parse(r#"{"meta": {"kind": "postgress", "schema": "x"}}"#).is_err());
        assert!(parse(r#"{"meta": {"kind": "postgres", "schema": "x", "urll_env": "A"}}"#).is_err());
        assert!(parse(r#"{"metadata": {}}"#).is_err());
        assert!(parse(r#"{"objects": {"kind": "s3", "endpoint": "http://h"}}"#).is_err(), "bucket is required");
        assert!(parse(r#"{"meta": {"kind": "postgres"}}"#).is_err(), "schema is required");
    }

    #[test]
    fn secrets_are_refused_in_the_configuration() {
        let with_pw = StorageConfig {
            meta: MetaConfig::Postgres {
                url_env: None,
                url: Some("postgres://u:hunter2@db/ag".into()),
                schema: "ag".into(),
                pool_size: None,
            },
            objects: ObjectsConfig::Fs,
        };
        let e = with_pw.validate().unwrap_err().to_string();
        assert!(e.contains("password") && e.contains("url_env") && !e.contains("hunter2"), "{e}");

        let ok = StorageConfig {
            meta: MetaConfig::Postgres {
                url_env: None,
                url: Some("host=/var/run/postgresql dbname=ag".into()),
                schema: "ag".into(),
                pool_size: None,
            },
            objects: ObjectsConfig::Fs,
        };
        ok.validate().unwrap();

        let bad_endpoint = StorageConfig {
            meta: MetaConfig::Sqlite,
            objects: ObjectsConfig::S3 {
                endpoint: "https://key:secret@s3.example.com".into(),
                region: "r".into(),
                bucket: "b".into(),
                prefix: None,
                path_style: None,
                access_key_id_env: None,
                secret_access_key_env: None,
                session_token_env: None,
                ca_bundle: None,
                lock: ObjectLockKind::Auto,
            },
        };
        assert!(bad_endpoint.validate().unwrap_err().to_string().contains("credentials"));
    }

    #[test]
    fn invalid_combinations_are_refused() {
        let bad_schema = StorageConfig {
            meta: MetaConfig::Postgres { url_env: None, url: None, schema: "Bad-Name".into(), pool_size: None },
            objects: ObjectsConfig::Fs,
        };
        assert!(bad_schema.validate().unwrap_err().to_string().contains("schema"));
        let lock_without_pg = StorageConfig {
            meta: MetaConfig::Sqlite,
            objects: ObjectsConfig::S3 {
                endpoint: "http://h".into(),
                region: "r".into(),
                bucket: "b".into(),
                prefix: None,
                path_style: None,
                access_key_id_env: None,
                secret_access_key_env: None,
                session_token_env: None,
                ca_bundle: None,
                lock: ObjectLockKind::PostgresAdvisory,
            },
        };
        assert!(lock_without_pg.validate().unwrap_err().to_string().contains("PostgreSQL metadata"));
    }

    #[test]
    fn missing_environment_variables_are_named_but_their_values_are_not_needed() {
        let e = env_value("AG_SURELY_UNSET_VARIABLE", "the PostgreSQL connection URL", "storage.meta.url_env")
            .unwrap_err()
            .to_string();
        assert!(e.contains("AG_SURELY_UNSET_VARIABLE") && e.contains("storage.meta.url_env"), "{e}");
    }
}
