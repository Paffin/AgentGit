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

#[test]
fn the_high_availability_overlay_configuration_runs_the_control_plane_on_postgres() {
    let cfg = load("deploy/kubernetes/overlays/ha/ag-server.toml");
    assert!(cfg.control.is_postgres(), "replicas must share a PostgreSQL control plane");
    assert_eq!(cfg.control.url_env.as_deref(), Some("AG_PG_URL"));
    assert!(cfg.server.drain_secs > 0, "a replica drains before it exits");
    // The drain has to fit into terminationGracePeriodSeconds of the overlay's Deployment patch.
    let patch = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/kubernetes/overlays/ha/deployment-patch.yaml"),
    )
    .unwrap();
    let grace: u64 = patch
        .lines()
        .find_map(|l| l.trim().strip_prefix("terminationGracePeriodSeconds:"))
        .and_then(|v| v.trim().parse().ok())
        .expect("the patch sets terminationGracePeriodSeconds");
    assert!(
        cfg.server.drain_secs < grace,
        "drain_secs {} must be below the grace period {grace}",
        cfg.server.drain_secs
    );
    let storage = cfg.repos.storage.expect("a storage template");
    assert!(matches!(storage.meta, MetaConfig::Postgres { .. }));
    assert!(matches!(storage.objects, ObjectsConfig::S3 { .. }), "replicas share no local disk");
}
