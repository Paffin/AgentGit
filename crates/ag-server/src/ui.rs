//! The Web UI, served by the server itself.
//!
//! The UI is a static single-page app (`apps/web`). There are three ways the server can have it, chosen by
//! configuration and by how the binary was built:
//!
//! * **embedded** (default): the build is compiled into the binary (cargo feature `ui`, see `build.rs` and
//!   `scripts/build_ui.sh`), so one file is the whole product;
//! * **a directory** (`[ui] dir`, or the deprecated `server.web_dir`): a copy of `apps/web/dist` read from disk
//!   on each request, for operators who customise or hot-swap the UI;
//! * **nothing** (`[ui] enabled = false`): only the API answers.
//!
//! A binary built without the UI build serves a page that says how to get it instead of a bare 404.
//!
//! The UI is the router's *fallback*: it only ever sees requests no API route matched, so it cannot shadow
//! `/v1`, gRPC, `/metrics`, `/healthz`, `/readyz` or `/openapi.json`. Those prefixes stay reserved even when
//! they match nothing, and answer with the API's error envelope. Every other path that is not a file is the
//! app's own route (a bookmarked URL works after a reload).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::app::App;
use crate::config::{ConfigError, ServerConfig};

// ---- configuration --------------------------------------------------------------------------------------

/// `[ui]`
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UiSection {
    /// Serve the Web UI. On by default; `false` leaves only the API (every other path is a 404 envelope).
    pub enabled: bool,
    /// Serve the UI from this directory (a copy of `apps/web/dist`) instead of the build embedded in the binary.
    pub dir: Option<PathBuf>,
    /// Browser sign-in through an OpenID Connect provider (authorization code with PKCE). Each entry names a
    /// provider that is also listed under `[[auth.oidc]]`: that is what makes the server accept its tokens.
    pub login: Vec<UiLogin>,
}

impl Default for UiSection {
    fn default() -> Self {
        Self { enabled: true, dir: None, login: Vec::new() }
    }
}

/// `[[ui.login]]`: one identity provider the Web UI offers a "Sign in with …" button for.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiLogin {
    /// The provider's issuer; must equal the `issuer` of an `[[auth.oidc]]` entry.
    pub issuer: String,
    /// The OAuth client registered for the Web UI: a public client whose redirect URI is
    /// `<origin of the UI>/auth/callback`.
    pub client_id: String,
    /// Text of the sign-in button (default: the issuer's host name).
    #[serde(default)]
    pub name: Option<String>,
    /// Scopes to request; `openid` is required.
    #[serde(default = "default_scopes")]
    pub scopes: Vec<String>,
    /// Authorization endpoint. Absent: the browser reads it from the provider's discovery document.
    #[serde(default)]
    pub authorization_endpoint: Option<String>,
    /// Token endpoint. Absent: discovery. Set it when it lives on another origin than the issuer, so that the
    /// UI's Content-Security-Policy can allow the browser to reach it.
    #[serde(default)]
    pub token_endpoint: Option<String>,
    /// Where to send the browser to end the provider's session on sign-out (RP-initiated logout). Absent: discovery.
    #[serde(default)]
    pub end_session_endpoint: Option<String>,
    /// Which token the UI presents to this server: the `id_token` (default; its audience is the client id, so the
    /// matching `[[auth.oidc]]` entry must have `audience = client_id`) or the `access_token`.
    #[serde(default)]
    pub token: LoginToken,
}

impl UiLogin {
    /// A provider with the defaults: scopes `openid profile`, endpoints from discovery, the ID token as credential.
    pub fn new(issuer: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into(),
            client_id: client_id.into(),
            name: None,
            scopes: default_scopes(),
            authorization_endpoint: None,
            token_endpoint: None,
            end_session_endpoint: None,
            token: LoginToken::default(),
        }
    }
}

/// Which token of the authorization-code response is the bearer credential.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginToken {
    /// The OpenID Connect ID token.
    #[default]
    IdToken,
    /// The OAuth access token (must be a JWT this server can validate).
    AccessToken,
}

fn default_scopes() -> Vec<String> {
    vec!["openid".into(), "profile".into()]
}

impl ServerConfig {
    /// The directory the UI is served from instead of the embedded build, if configured (`ui.dir`, or the
    /// deprecated `server.web_dir`).
    pub fn ui_dir(&self) -> Option<&Path> {
        self.ui.dir.as_deref().or(self.server.web_dir.as_deref())
    }
}

/// Checks the `[ui]` section (called by `ServerConfig::validate`).
pub fn validate(cfg: &ServerConfig) -> Result<(), ConfigError> {
    let bad = |m: String| Err(ConfigError::Invalid(m));
    if cfg.ui.dir.is_some() && cfg.server.web_dir.is_some() {
        return bad("set only one of ui.dir and server.web_dir (the deprecated spelling)".into());
    }
    if cfg.ui.enabled
        && let Some(dir) = cfg.ui_dir()
        && !dir.join("index.html").is_file()
    {
        return bad(format!(
            "the UI directory {} has no index.html: point ui.dir at a build of the Web UI (apps/web/dist), \
             or remove it to use the UI embedded in the binary",
            dir.display()
        ));
    }
    for l in &cfg.ui.login {
        if l.issuer.is_empty() || l.client_id.is_empty() {
            return bad("every [[ui.login]] needs issuer and client_id".into());
        }
        if !l.scopes.iter().any(|s| s == "openid") {
            return bad(format!("[[ui.login]] for {}: scopes must include \"openid\"", l.issuer));
        }
        for (field, url) in [
            ("issuer", Some(&l.issuer)),
            ("authorization_endpoint", l.authorization_endpoint.as_ref()),
            ("token_endpoint", l.token_endpoint.as_ref()),
            ("end_session_endpoint", l.end_session_endpoint.as_ref()),
        ] {
            if let Some(u) = url {
                check_login_url(field, u)?;
            }
        }
        let Some(provider) = cfg.auth.oidc.iter().find(|p| same_issuer(&p.issuer, &l.issuer)) else {
            return bad(format!(
                "[[ui.login]] issuer {} is not listed under [[auth.oidc]]: the server could not validate its tokens",
                l.issuer
            ));
        };
        if l.token == LoginToken::IdToken && provider.audience != l.client_id {
            return bad(format!(
                "[[ui.login]] for {}: the UI presents the ID token, whose audience is the client id \"{}\", but the \
                 [[auth.oidc]] entry expects audience \"{}\" (make them equal, or set token = \"access_token\")",
                l.issuer, l.client_id, provider.audience
            ));
        }
    }
    Ok(())
}

fn same_issuer(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// The `scheme://host[:port]` part of an absolute URL.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().filter(|a| !a.is_empty())?;
    Some(format!("{scheme}://{authority}"))
}

/// Login URLs must be https; plain http is accepted for a loopback provider only (local development, tests).
fn check_login_url(field: &str, url: &str) -> Result<(), ConfigError> {
    let bad = |m: String| Err(ConfigError::Invalid(format!("[[ui.login]] {field}: {m}")));
    let Some(origin) = origin_of(url) else {
        return bad(format!("\"{url}\" is not an absolute URL"));
    };
    let (scheme, authority) = origin.split_once("://").unwrap_or(("", ""));
    if authority.contains('@') {
        return bad(format!("\"{url}\" must not contain credentials"));
    }
    let host = match authority.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => authority,
    };
    match scheme {
        "https" => Ok(()),
        "http" if matches!(host, "localhost" | "127.0.0.1" | "[::1]") => Ok(()),
        "http" => bad(format!("\"{url}\" must use https (http is accepted for localhost only)")),
        _ => bad(format!("\"{url}\" must be an https URL")),
    }
}

// ---- GET /v1/auth/config --------------------------------------------------------------------------------

/// Request of `GET /v1/auth/config` (it takes no parameters).
#[derive(Clone, Debug, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AuthConfigRequest {}

/// How a browser can sign in to this server. Public: it needs no credentials.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AuthConfig {
    /// `token` when the only way in is a pasted credential, `oidc` when at least one identity provider is offered
    /// for redirect sign-in (pasting a credential keeps working in both modes).
    pub mode: String,
    /// The providers to offer, in configuration order (`oidc` mode only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<AuthProvider>,
}

/// One identity provider for redirect sign-in (OpenID Connect authorization code with PKCE).
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AuthProvider {
    /// Text for the sign-in button.
    pub name: String,
    /// Issuer; discovery is at `<issuer>/.well-known/openid-configuration`.
    pub issuer: String,
    /// The OAuth client id of the Web UI.
    pub client_id: String,
    /// Scopes to request.
    pub scopes: Vec<String>,
    /// Authorization endpoint, when configured (otherwise read it from discovery).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_endpoint: Option<String>,
    /// Token endpoint, when configured (otherwise read it from discovery).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_endpoint: Option<String>,
    /// Logout endpoint, when configured (otherwise read it from discovery).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_session_endpoint: Option<String>,
    /// Which token to present to this server: `id_token` or `access_token`.
    pub token: LoginToken,
}

/// What `GET /v1/auth/config` answers for this configuration.
pub fn auth_config(cfg: &ServerConfig) -> AuthConfig {
    let providers: Vec<AuthProvider> = cfg
        .ui
        .login
        .iter()
        .map(|l| AuthProvider {
            name: l.name.clone().unwrap_or_else(|| {
                origin_of(&l.issuer)
                    .and_then(|o| o.split_once("://").map(|(_, h)| h.to_string()))
                    .unwrap_or_else(|| l.issuer.clone())
            }),
            issuer: l.issuer.clone(),
            client_id: l.client_id.clone(),
            scopes: l.scopes.clone(),
            authorization_endpoint: l.authorization_endpoint.clone(),
            token_endpoint: l.token_endpoint.clone(),
            end_session_endpoint: l.end_session_endpoint.clone(),
            token: l.token,
        })
        .collect();
    AuthConfig { mode: if providers.is_empty() { "token" } else { "oidc" }.into(), providers }
}

// ---- assets ---------------------------------------------------------------------------------------------

/// One embedded file with its pre-compressed variants (generated by `build.rs`).
pub struct Asset {
    /// Path below the UI root, `/`-separated.
    pub path: &'static str,
    /// The file.
    pub data: &'static [u8],
    /// Brotli-compressed `data`, if the build produced it.
    pub br: Option<&'static [u8]>,
    /// Gzip-compressed `data`, if the build produced it.
    pub gz: Option<&'static [u8]>,
}

static EMBEDDED: &[Asset] = include!(concat!(env!("OUT_DIR"), "/ui_assets.rs"));

/// Whether this binary contains a build of the Web UI.
pub fn embedded() -> bool {
    EMBEDDED.iter().any(|a| a.path == "index.html")
}

/// Where the UI comes from, given the configuration and the binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `ui.enabled = false`.
    Disabled,
    /// The build compiled into the binary.
    Embedded,
    /// A directory on disk.
    Dir(PathBuf),
    /// Enabled, but this binary has no UI and no directory is configured.
    NotBuilt,
}

/// Decides the mode.
pub fn mode(cfg: &ServerConfig) -> Mode {
    if !cfg.ui.enabled {
        Mode::Disabled
    } else if let Some(dir) = cfg.ui_dir() {
        Mode::Dir(dir.to_path_buf())
    } else if embedded() {
        Mode::Embedded
    } else {
        Mode::NotBuilt
    }
}

struct Prepared {
    asset: &'static Asset,
    etag: String,
}

enum Source {
    Embedded(HashMap<&'static str, Prepared>),
    Dir(PathBuf),
    NotBuilt,
    Disabled,
}

/// A file ready to be sent, in the encoding the client accepts.
struct Loaded {
    data: Bytes,
    /// `Content-Encoding` value, if compressed.
    encoding: Option<&'static str>,
    /// The `ETag` header value: quotes included, with a `W/` prefix for a weak tag.
    etag: String,
}

/// The UI as a request handler.
pub struct Ui {
    source: Source,
    csp: HeaderValue,
}

impl Ui {
    /// Builds the handler for a configuration (the embedded build, or a directory, as [`mode`] says).
    pub fn new(cfg: &ServerConfig) -> Self {
        let csp = csp(cfg);
        match mode(cfg) {
            Mode::Disabled => Self { source: Source::Disabled, csp },
            Mode::Dir(dir) => Self { source: Source::Dir(dir), csp },
            Mode::NotBuilt => Self { source: Source::NotBuilt, csp },
            Mode::Embedded => Self::from_assets(EMBEDDED, csp),
        }
    }

    fn from_assets(assets: &'static [Asset], csp: HeaderValue) -> Self {
        let map = assets
            .iter()
            .map(|asset| {
                let digest = Sha256::digest(asset.data);
                (asset.path, Prepared { asset, etag: hex::encode(&digest[..8]) })
            })
            .collect();
        Self { source: Source::Embedded(map), csp }
    }

    /// Answers one request that no API route matched.
    pub async fn handle(&self, req: Request) -> Response {
        let (parts, _) = req.into_parts();
        let path = parts.uri.path();
        if is_reserved(path) || matches!(self.source, Source::Disabled) {
            return crate::server::not_found(&parts.headers, &parts.method, path);
        }
        if parts.method != Method::GET && parts.method != Method::HEAD {
            let mut resp = StatusCode::METHOD_NOT_ALLOWED.into_response();
            resp.headers_mut().insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
            return resp;
        }
        if matches!(self.source, Source::NotBuilt) {
            return if accepts_html(&parts.headers) {
                not_built_page(&parts.method)
            } else {
                crate::server::not_found(&parts.headers, &parts.method, path)
            };
        }

        let accept = Accepts::parse(&parts.headers);
        let rel = path.trim_start_matches('/');
        let rel = if rel.is_empty() { "index.html" } else { rel };
        if is_plain_path(rel)
            && !rel.ends_with(".br")
            && !rel.ends_with(".gz")
            && let Some(found) = self.load(rel, accept).await
        {
            return self.file(&parts, rel, found);
        }
        // Not a file. A missing asset is a 404 (answering a script request with the page would hide the real
        // problem and cache the wrong thing); anything else is a route of the app, which renders its own pages.
        if looks_like_asset(rel) {
            return crate::server::not_found(&parts.headers, &parts.method, path);
        }
        match self.load("index.html", accept).await {
            Some(found) => self.file(&parts, "index.html", found),
            None => crate::server::not_found(&parts.headers, &parts.method, path),
        }
    }

    async fn load(&self, rel: &str, accept: Accepts) -> Option<Loaded> {
        match &self.source {
            Source::Embedded(map) => {
                let p = map.get(rel)?;
                let a = p.asset;
                Some(if let (true, Some(br)) = (accept.br, a.br) {
                    Loaded { data: Bytes::from_static(br), encoding: Some("br"), etag: format!("\"{}-br\"", p.etag) }
                } else if let (true, Some(gz)) = (accept.gzip, a.gz) {
                    Loaded { data: Bytes::from_static(gz), encoding: Some("gzip"), etag: format!("\"{}-gz\"", p.etag) }
                } else {
                    Loaded { data: Bytes::from_static(a.data), encoding: None, etag: format!("\"{}\"", p.etag) }
                })
            }
            Source::Dir(dir) => load_from_dir(dir, rel, accept).await,
            Source::NotBuilt | Source::Disabled => None,
        }
    }

    fn file(&self, parts: &axum::http::request::Parts, rel: &str, f: Loaded) -> Response {
        let etag = f.etag;
        let not_modified = parts
            .headers
            .get_all(header::IF_NONE_MATCH)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .any(|candidate| candidate == "*" || candidate.trim_start_matches("W/") == etag.trim_start_matches("W/"));
        let len = f.data.len();
        let mut resp = if not_modified {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::NOT_MODIFIED;
            r
        } else {
            let body = if parts.method == Method::HEAD { Body::empty() } else { Body::from(f.data) };
            let mut r = Response::new(body);
            r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(rel)));
            r.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(len));
            r
        };
        let h = resp.headers_mut();
        if let Ok(v) = HeaderValue::from_str(&etag) {
            h.insert(header::ETAG, v);
        }
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache_control(rel)));
        h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        if let (Some(enc), false) = (f.encoding, not_modified) {
            h.insert(header::CONTENT_ENCODING, HeaderValue::from_static(enc));
        }
        self.security_headers(h);
        resp
    }

    fn security_headers(&self, h: &mut HeaderMap) {
        h.insert(header::CONTENT_SECURITY_POLICY, self.csp.clone());
        h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
        h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
        h.insert(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=(), usb=()"),
        );
    }
}

async fn load_from_dir(dir: &Path, rel: &str, accept: Accepts) -> Option<Loaded> {
    let base = dir.join(rel);
    // The original must exist; a stray `.br` without it is not a file of the build.
    tokio::fs::metadata(&base).await.ok().filter(|m| m.is_file())?;
    let mut candidates: Vec<(Option<&'static str>, &'static str)> = Vec::new();
    if accept.br {
        candidates.push((Some("br"), ".br"));
    }
    if accept.gzip {
        candidates.push((Some("gzip"), ".gz"));
    }
    candidates.push((None, ""));
    for (encoding, suffix) in candidates {
        let mut path = base.clone().into_os_string();
        path.push(suffix);
        let path = PathBuf::from(path);
        let Ok(m) = tokio::fs::metadata(&path).await else { continue };
        if !m.is_file() {
            continue;
        }
        let Ok(data) = tokio::fs::read(&path).await else { continue };
        let mtime =
            m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        let tag = encoding.map_or("", |e| if e == "br" { "-br" } else { "-gz" });
        // Weak: derived from the file's size and modification time, not from its bytes.
        let etag = format!("W/\"{:x}-{mtime:x}{tag}\"", m.len());
        return Some(Loaded { data: Bytes::from(data), encoding, etag });
    }
    None
}

/// Installs the UI as the router's fallback (and the JSON 404 when there is none).
pub fn install(router: Router, app: &App) -> Router {
    let ui = Arc::new(Ui::new(&app.cfg));
    match mode(&app.cfg) {
        Mode::Embedded => tracing::info!(files = EMBEDDED.len(), "serving the Web UI embedded in this binary"),
        Mode::Dir(dir) => tracing::info!(dir = %dir.display(), "serving the Web UI from a directory"),
        Mode::NotBuilt => tracing::warn!(
            "this binary has no Web UI (build it with scripts/build_ui.sh, or set [ui] dir); the browser sees an explanation"
        ),
        Mode::Disabled => tracing::info!("the Web UI is disabled ([ui] enabled = false)"),
    }
    router.fallback(move |req: Request| {
        let ui = ui.clone();
        async move { ui.handle(req).await }
    })
}

// ---- small pure helpers ---------------------------------------------------------------------------------

/// Paths that belong to the API, gRPC and operations and are never the UI's, even when nothing matches them.
fn is_reserved(path: &str) -> bool {
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    matches!(first, "v1" | "metrics" | "healthz" | "readyz" | "openapi.json") || first.starts_with("agwp.")
}

/// A path made only of characters that can name a file of the build, with no empty, `.` or `..` segment. Anything
/// else is never looked up on disk (percent-escapes are not decoded, so `%2e%2e` is just a name nobody has).
fn is_plain_path(rel: &str) -> bool {
    !rel.is_empty()
        && rel.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '~' | '@' | '+'))
        })
}

/// Whether a request that matched no file is a request for a missing *asset* (as opposed to an app route).
/// Repository names may contain dots, so everything under `/repos/` is the app's.
fn looks_like_asset(rel: &str) -> bool {
    if rel.starts_with("repos/") {
        return false;
    }
    if rel.starts_with("assets/") {
        return true;
    }
    let last = rel.rsplit('/').next().unwrap_or("");
    last.rsplit_once('.').is_some_and(|(_, ext)| {
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "js" | "mjs"
                | "css"
                | "map"
                | "json"
                | "svg"
                | "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "ico"
                | "woff"
                | "woff2"
                | "ttf"
                | "txt"
                | "xml"
                | "webmanifest"
                | "wasm"
                | "br"
                | "gz"
        )
    })
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref() {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("txt") => "text/plain; charset=utf-8",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Vite names build outputs `assets/<name>-<8 hash characters>.<ext>`: those never change under the same name,
/// so they are cached for a year. Everything else (the page itself above all) is revalidated on each use.
fn cache_control(rel: &str) -> &'static str {
    if is_hashed_asset(rel) { "public, max-age=31536000, immutable" } else { "no-cache" }
}

fn is_hashed_asset(rel: &str) -> bool {
    let Some(name) = rel.strip_prefix("assets/") else { return false };
    let name = name.rsplit('/').next().unwrap_or(name);
    let stem = name.split('.').next().unwrap_or("");
    let n = stem.len();
    n > 9
        && stem.is_char_boundary(n - 9)
        && stem.as_bytes()[n - 9] == b'-'
        && stem.as_bytes()[n - 8..].iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn accepts_html(h: &HeaderMap) -> bool {
    h.get(header::ACCEPT).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("text/html"))
}

/// Which pre-compressed encodings the client takes.
#[derive(Clone, Copy, Default)]
struct Accepts {
    br: bool,
    gzip: bool,
}

impl Accepts {
    fn parse(h: &HeaderMap) -> Self {
        let (mut br, mut gzip, mut star) = (None, None, None);
        for v in h.get_all(header::ACCEPT_ENCODING).iter().filter_map(|v| v.to_str().ok()) {
            for item in v.split(',') {
                let mut parts = item.split(';');
                let name = parts.next().unwrap_or("").trim().to_ascii_lowercase();
                let q = parts
                    .find_map(|p| p.trim().strip_prefix("q=").and_then(|q| q.trim().parse::<f32>().ok()))
                    .unwrap_or(1.0);
                let ok = Some(q > 0.0);
                match name.as_str() {
                    "br" => br = ok,
                    "gzip" | "x-gzip" => gzip = ok,
                    "*" => star = ok,
                    _ => {}
                }
            }
        }
        Self { br: br.or(star).unwrap_or(false), gzip: gzip.or(star).unwrap_or(false) }
    }
}

/// The Content-Security-Policy of every UI response: scripts and styles only from the server itself. The browser
/// may talk to the server and, when redirect sign-in is configured, to the identity providers' origins.
fn csp(cfg: &ServerConfig) -> HeaderValue {
    let mut connect = vec!["'self'".to_string()];
    for l in &cfg.ui.login {
        for url in [Some(&l.issuer), l.token_endpoint.as_ref()].into_iter().flatten() {
            if let Some(o) = origin_of(url)
                && !connect.contains(&o)
            {
                connect.push(o);
            }
        }
    }
    let policy = format!(
        "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; \
         connect-src {}; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'",
        connect.join(" ")
    );
    HeaderValue::from_str(&policy).unwrap_or_else(|_| HeaderValue::from_static("default-src 'self'"))
}

const NOT_BUILT_PAGE: &str = include_str!("ui_not_built.html");

fn not_built_page(method: &Method) -> Response {
    let body = if method == Method::HEAD { Body::empty() } else { Body::from(NOT_BUILT_PAGE) };
    let mut resp = Response::new(body);
    *resp.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // The page styles itself inline; it loads nothing and runs nothing.
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashed_assets_are_recognised_by_vite_naming() {
        assert!(is_hashed_asset("assets/index-D004i6Pa.js"));
        assert!(is_hashed_asset("assets/Card-9Sd5_lV6.js"));
        assert!(is_hashed_asset("assets/Card-9Sd5-lV6.css"));
        assert!(!is_hashed_asset("assets/logo.svg"));
        assert!(!is_hashed_asset("index.html"));
        assert!(!is_hashed_asset("favicon.svg"));
        assert!(!is_hashed_asset("other/index-D004i6Pa.js"));
        assert!(!is_hashed_asset("assets/index-short.js"));
    }

    #[test]
    fn only_plain_names_are_ever_looked_up() {
        assert!(is_plain_path("assets/index-D004i6Pa.js"));
        assert!(is_plain_path("favicon.svg"));
        for bad in ["", "../etc/passwd", "a/../b", "a//b", "a/./b", "%2e%2e/x", "a\\b", "a b", "a:b", "assets/", "/x"] {
            assert!(!is_plain_path(bad), "{bad:?} must not be looked up");
        }
    }

    #[test]
    fn app_routes_are_not_assets_even_when_they_contain_dots() {
        assert!(!looks_like_asset("repos/acme/app.js"));
        assert!(!looks_like_asset("repos/acme/site.com/files"));
        assert!(!looks_like_asset("signin"));
        assert!(!looks_like_asset("auth/callback"));
        assert!(looks_like_asset("assets/gone.js"));
        assert!(looks_like_asset("assets/anything"));
        assert!(looks_like_asset("robots.txt"));
        assert!(looks_like_asset("static/x.css"));
    }

    #[test]
    fn accept_encoding_is_parsed_with_quality_values() {
        let parse = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::ACCEPT_ENCODING, HeaderValue::from_str(v).unwrap());
            Accepts::parse(&h)
        };
        let a = parse("gzip, deflate, br");
        assert!(a.br && a.gzip);
        let a = parse("gzip;q=0, br;q=0.5");
        assert!(a.br && !a.gzip);
        let a = parse("*;q=0");
        assert!(!a.br && !a.gzip);
        let a = parse("identity");
        assert!(!a.br && !a.gzip);
        assert!(!Accepts::parse(&HeaderMap::new()).br);
    }

    #[test]
    fn the_policy_allows_identity_providers_only_when_login_is_configured() {
        let mut cfg = ServerConfig::default();
        let plain = csp(&cfg).to_str().unwrap().to_string();
        assert!(plain.contains("connect-src 'self';"), "{plain}");
        assert!(plain.contains("script-src 'self';"));
        assert!(!plain.contains("unsafe-inline") && !plain.contains("unsafe-eval"));
        cfg.ui.login.push(UiLogin {
            token_endpoint: Some("https://tokens.example.net/oauth/token".into()),
            ..UiLogin::new("https://idp.example.com/realms/acme", "web")
        });
        let with = csp(&cfg).to_str().unwrap().to_string();
        assert!(with.contains("connect-src 'self' https://idp.example.com https://tokens.example.net;"), "{with}");
    }

    #[test]
    fn login_urls_must_be_https_except_on_loopback() {
        assert!(check_login_url("issuer", "https://idp.example.com/realms/a").is_ok());
        assert!(check_login_url("issuer", "http://127.0.0.1:9000").is_ok());
        assert!(check_login_url("issuer", "http://localhost").is_ok());
        assert!(check_login_url("issuer", "http://idp.example.com").is_err());
        assert!(check_login_url("issuer", "ftp://idp.example.com").is_err());
        assert!(check_login_url("issuer", "idp.example.com").is_err());
        assert!(check_login_url("issuer", "https://user:pw@idp.example.com").is_err());
    }

    fn get(path: &str) -> Request {
        Request::builder().uri(path).header("accept-encoding", "br, gzip").body(Body::empty()).unwrap()
    }

    async fn body_of(resp: Response) -> Vec<u8> {
        axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()
    }

    fn embedded_ui() -> Ui {
        static ASSETS: &[Asset] = &[
            Asset {
                path: "index.html",
                data: b"<!doctype html><div id=app></div>",
                br: Some(b"BR-INDEX"),
                gz: Some(b"GZ-INDEX"),
            },
            Asset { path: "assets/index-D004i6Pa.js", data: b"console.log(1)", br: None, gz: Some(b"GZ-JS") },
            Asset { path: "favicon.svg", data: b"<svg/>", br: None, gz: None },
        ];
        Ui::from_assets(ASSETS, csp(&ServerConfig::default()))
    }

    #[tokio::test]
    async fn embedded_files_are_served_with_types_cache_rules_and_the_policy() {
        let ui = embedded_ui();
        let r = ui.handle(get("/assets/index-D004i6Pa.js")).await;
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()[header::CONTENT_TYPE], "text/javascript; charset=utf-8");
        assert_eq!(r.headers()[header::CACHE_CONTROL], "public, max-age=31536000, immutable");
        assert_eq!(r.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(r.headers()[header::VARY], "Accept-Encoding");
        assert!(r.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().contains("script-src 'self'"));
        assert_eq!(r.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(body_of(r).await, b"GZ-JS");

        let r = ui.handle(get("/favicon.svg")).await;
        assert_eq!(r.headers()[header::CONTENT_TYPE], "image/svg+xml");
        assert_eq!(r.headers()[header::CACHE_CONTROL], "no-cache");
        assert!(r.headers().get(header::CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn the_best_accepted_encoding_wins_and_identity_is_the_fallback() {
        let ui = embedded_ui();
        let r = ui.handle(get("/")).await;
        assert_eq!(r.headers()[header::CONTENT_ENCODING], "br");
        assert_eq!(body_of(r).await, b"BR-INDEX");
        let gzip_only = Request::builder().uri("/").header("accept-encoding", "gzip").body(Body::empty()).unwrap();
        assert_eq!(body_of(ui.handle(gzip_only).await).await, b"GZ-INDEX");
        let bare = Request::builder().uri("/").body(Body::empty()).unwrap();
        let r = ui.handle(bare).await;
        assert!(r.headers().get(header::CONTENT_ENCODING).is_none());
        assert_eq!(body_of(r).await, b"<!doctype html><div id=app></div>");
    }

    #[tokio::test]
    async fn app_routes_fall_back_to_the_page_and_missing_assets_do_not() {
        let ui = embedded_ui();
        for path in [
            "/",
            "/signin",
            "/repos/acme/app.js",
            "/repos/acme/webapp/proposals/prop_1",
            "/auth/callback",
            "/some%20thing",
        ] {
            let r = ui.handle(get(path)).await;
            assert_eq!(r.status(), 200, "{path}");
            assert_eq!(r.headers()[header::CONTENT_TYPE], "text/html; charset=utf-8", "{path}");
            assert_eq!(r.headers()[header::CACHE_CONTROL], "no-cache", "{path}");
        }
        for path in ["/assets/gone-AAAAAAAA.js", "/robots.txt", "/x.css"] {
            let r = ui.handle(get(path)).await;
            assert_eq!(r.status(), 404, "{path}");
            assert_eq!(r.headers()[header::CONTENT_TYPE], "application/json", "{path} gets the API envelope");
        }
    }

    #[tokio::test]
    async fn reserved_prefixes_are_never_the_page() {
        let ui = embedded_ui();
        for path in
            ["/v1/nothing", "/v1", "/agwp.v1.Nothing/Call", "/metrics", "/healthz/x", "/readyz", "/openapi.json"]
        {
            let r = ui.handle(get(path)).await;
            assert_eq!(r.status(), 404, "{path}");
            assert_eq!(r.headers()[header::CONTENT_TYPE], "application/json", "{path}");
        }
    }

    #[tokio::test]
    async fn conditional_requests_and_head_and_other_methods() {
        let ui = embedded_ui();
        let first = ui.handle(get("/favicon.svg")).await;
        let etag = first.headers()[header::ETAG].to_str().unwrap().to_string();
        let again = Request::builder().uri("/favicon.svg").header("if-none-match", &etag).body(Body::empty()).unwrap();
        let r = ui.handle(again).await;
        assert_eq!(r.status(), 304);
        assert_eq!(r.headers()[header::ETAG].to_str().unwrap(), etag);
        assert!(body_of(r).await.is_empty());
        let stale =
            Request::builder().uri("/favicon.svg").header("if-none-match", "\"nope\"").body(Body::empty()).unwrap();
        assert_eq!(ui.handle(stale).await.status(), 200);

        let head = Request::builder().method("HEAD").uri("/favicon.svg").body(Body::empty()).unwrap();
        let r = ui.handle(head).await;
        assert_eq!(r.status(), 200);
        assert_eq!(r.headers()[header::CONTENT_LENGTH], "6");
        assert!(body_of(r).await.is_empty());

        let post = Request::builder().method("POST").uri("/favicon.svg").body(Body::empty()).unwrap();
        let r = ui.handle(post).await;
        assert_eq!(r.status(), 405);
        assert_eq!(r.headers()[header::ALLOW], "GET, HEAD");
    }

    #[tokio::test]
    async fn without_a_build_a_browser_gets_an_explanation_and_a_client_gets_the_envelope() {
        let ui = Ui { source: Source::NotBuilt, csp: csp(&ServerConfig::default()) };
        let page = Request::builder()
            .uri("/")
            .header("accept", "text/html,application/xhtml+xml")
            .body(Body::empty())
            .unwrap();
        let r = ui.handle(page).await;
        assert_eq!(r.status(), 503);
        assert!(r.headers()[header::CONTENT_SECURITY_POLICY].to_str().unwrap().contains("default-src 'none'"));
        let text = String::from_utf8(body_of(r).await).unwrap();
        assert!(text.contains("scripts/build_ui.sh") && text.contains("[ui]"), "{text}");
        let api = Request::builder().uri("/").header("accept", "application/json").body(Body::empty()).unwrap();
        let r = ui.handle(api).await;
        assert_eq!(r.status(), 404);
        assert_eq!(r.headers()[header::CONTENT_TYPE], "application/json");
    }

    #[test]
    fn the_configuration_is_checked() {
        let mut cfg = ServerConfig::default();
        cfg.validate().unwrap();
        cfg.ui.dir = Some(PathBuf::from("/definitely/not/here"));
        let e = cfg.validate().unwrap_err().to_string();
        assert!(e.contains("no index.html"), "{e}");
        cfg.ui.enabled = false;
        cfg.validate().unwrap();

        let mut cfg = ServerConfig::default();
        cfg.ui.login.push(UiLogin::new("https://idp.example.com", "web"));
        let e = cfg.validate().unwrap_err().to_string();
        assert!(e.contains("not listed under [[auth.oidc]]"), "{e}");
        assert_eq!(auth_config(&cfg).providers[0].name, "idp.example.com");
        assert_eq!(auth_config(&ServerConfig::default()).mode, "token");
    }
}
