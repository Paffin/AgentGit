//! Connection strings: `postgres://…` URLs and `key=value` strings, plus the
//! two TLS options libpq has and `tokio-postgres` does not (`sslmode=verify-*`
//! and `sslrootcert`).

use std::path::PathBuf;
use std::str::FromStr;

use super::super::MetaError;

/// How the connection is protected, following libpq's `sslmode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsMode {
    /// Never use TLS.
    Disable,
    /// Use TLS if the server offers it, without verifying the certificate.
    Prefer,
    /// Require TLS, without verifying the certificate (libpq `require`).
    Require,
    /// Require TLS and verify the certificate chain *and* the host name.
    /// (`verify-ca` is accepted and treated the same: the host name is checked too.)
    VerifyFull,
}

/// A parsed connection target.
#[derive(Clone, Debug)]
pub struct Target {
    /// The driver configuration (without the TLS-only options).
    pub config: postgres::Config,
    /// TLS behaviour.
    pub tls: TlsMode,
    /// A PEM file with the CA certificate(s) to trust instead of the built-in roots.
    pub root_cert: Option<PathBuf>,
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn bad(msg: impl Into<String>) -> MetaError {
    MetaError::Invalid(format!("invalid PostgreSQL connection string: {}", msg.into()))
}

/// Parses `url` (never echoed in errors: it may contain a password).
pub fn parse_target(url: &str) -> Result<Target, MetaError> {
    let url = url.trim();
    let mut sslmode: Option<String> = None;
    let mut sslrootcert: Option<String> = None;

    // Pull the TLS-only options out of the string and hand the rest to the driver.
    let rest = if url.contains("://") {
        match url.split_once('?') {
            None => url.to_string(),
            Some((base, query)) => {
                let mut kept = Vec::new();
                for pair in query.split('&').filter(|p| !p.is_empty()) {
                    let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                    match k {
                        "sslmode" => sslmode = Some(percent_decode(v)),
                        "sslrootcert" => sslrootcert = Some(percent_decode(v)),
                        _ => kept.push(pair),
                    }
                }
                if kept.is_empty() { base.to_string() } else { format!("{base}?{}", kept.join("&")) }
            }
        }
    } else {
        let mut kept = Vec::new();
        for tok in url.split_whitespace() {
            match tok.split_once('=') {
                Some(("sslmode", v)) => sslmode = Some(v.trim_matches('\'').to_string()),
                Some(("sslrootcert", v)) => sslrootcert = Some(v.trim_matches('\'').to_string()),
                _ => kept.push(tok),
            }
        }
        kept.join(" ")
    };

    let tls = match sslmode.as_deref() {
        None | Some("prefer") => TlsMode::Prefer,
        Some("disable") => TlsMode::Disable,
        Some("allow") => {
            return Err(bad("sslmode=allow is not supported; use disable, prefer, require or verify-full"));
        }
        Some("require") => TlsMode::Require,
        Some("verify-ca" | "verify-full") => TlsMode::VerifyFull,
        Some(other) => return Err(bad(format!("unknown sslmode '{other}'"))),
    };
    let mut config = postgres::Config::from_str(&rest).map_err(|e| bad(e.to_string().lines().next().unwrap_or("")))?;
    config.ssl_mode(match tls {
        TlsMode::Disable => postgres::config::SslMode::Disable,
        TlsMode::Prefer => postgres::config::SslMode::Prefer,
        TlsMode::Require | TlsMode::VerifyFull => postgres::config::SslMode::Require,
    });
    if config.get_hosts().is_empty() {
        return Err(bad("no host given"));
    }
    Ok(Target { config, tls, root_cert: sslrootcert.map(PathBuf::from) })
}

/// True if the connection string embeds a password (or cannot be parsed, which
/// is treated the same way: do not store it anywhere).
pub fn url_has_password(url: &str) -> bool {
    match parse_target(url) {
        Ok(t) => t.config.get_password().is_some(),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_keyword_strings_parse() {
        let t = parse_target(
            "postgres://u:pw@db.example:5433/ag?sslmode=verify-full&sslrootcert=%2Fetc%2Fca.pem&application_name=x",
        )
        .unwrap();
        assert_eq!(t.tls, TlsMode::VerifyFull);
        assert_eq!(t.root_cert.as_deref(), Some(std::path::Path::new("/etc/ca.pem")));
        assert_eq!(t.config.get_user(), Some("u"));
        assert_eq!(t.config.get_dbname(), Some("ag"));
        assert_eq!(t.config.get_application_name(), Some("x"));
        assert_eq!(t.config.get_ports(), &[5433]);

        let t = parse_target("host=localhost user=postgres sslmode=disable").unwrap();
        assert_eq!(t.tls, TlsMode::Disable);
        let t = parse_target("postgresql://localhost/db").unwrap();
        assert_eq!(t.tls, TlsMode::Prefer, "libpq's default");
    }

    #[test]
    fn bad_strings_are_rejected_without_echoing_secrets() {
        for s in ["", "postgres://", "postgres://u:secret@h/db?sslmode=bogus", "postgres://u:secret@h/db?sslmode=allow"]
        {
            let e = parse_target(s).unwrap_err().to_string();
            assert!(!e.contains("secret"), "{e}");
        }
    }

    #[test]
    fn passwords_are_detected() {
        assert!(url_has_password("postgres://u:pw@h/db"));
        assert!(url_has_password("postgres://u@h/db?password=pw"));
        assert!(url_has_password("host=h user=u password=pw"));
        assert!(!url_has_password("postgres://u@h/db"));
        assert!(!url_has_password("host=h user=u"));
    }
}
