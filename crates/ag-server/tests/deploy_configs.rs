//! The configuration files shipped under deploy/ must load and validate with this very server.

use std::path::Path;

use ag_integrate::{MetaConfig, ObjectsConfig};
use ag_server::config::ServerConfig;

fn load(rel: &str) -> ServerConfig {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel);
    let cfg = ServerConfig::load(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
    cfg.validate().unwrap_or_else(|e| panic!("{rel}: {e}"));
    cfg
}

#[test]
fn the_container_image_configuration_is_valid() {
    let cfg = load("deploy/docker/ag-server.toml");
    assert_eq!(cfg.server.listen.port(), 8080);
    assert!(cfg.server.allow_insecure_remote, "the image relies on TLS termination in front of it");
    assert_eq!(cfg.telemetry.log_format, "json");
}

#[test]
fn the_kubernetes_configuration_is_valid() {
    let cfg = load("deploy/kubernetes/base/ag-server.toml");
    assert_eq!(cfg.server.data_dir, Path::new("/data"));
    assert_eq!(cfg.metrics.token_env, "AG_METRICS_TOKEN");
}

#[test]
fn the_compose_configuration_uses_postgres_and_s3() {
    let cfg = load("deploy/compose/ag-server.toml");
    let storage = cfg.repos.storage.expect("a storage template");
    assert!(matches!(storage.meta, MetaConfig::Postgres { .. }));
    match storage.objects {
        ObjectsConfig::S3 { endpoint, bucket, path_style, .. } => {
            assert_eq!(endpoint, "http://seaweedfs:8333");
            assert_eq!(bucket, "agentgit");
            assert_eq!(path_style, Some(true));
        }
        other => panic!("expected S3, got {other:?}"),
    }
}
