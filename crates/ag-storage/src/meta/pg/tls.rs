//! TLS for PostgreSQL connections (rustls with the `ring` provider).
//!
//! Without the `tls` feature only `sslmode=disable` and the opportunistic
//! `prefer` (which then stays unencrypted) work; `require` and `verify-full`
//! are refused rather than silently downgraded.

use super::super::MetaError;
use super::url::{Target, TlsMode};

/// How to wrap a freshly opened socket.
#[derive(Clone)]
pub enum Connector {
    /// No TLS implementation (or `sslmode=disable`).
    Plain,
    /// rustls.
    #[cfg(feature = "tls")]
    Rustls(tokio_postgres_rustls::MakeRustlsConnect),
}

#[cfg(feature = "tls")]
mod imp {
    use std::path::Path;
    use std::sync::Arc;

    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};

    use super::*;

    /// `sslmode=require`/`prefer`: encrypts, authenticates nobody (as libpq does).
    #[derive(Debug)]
    struct NoVerify(WebPkiSupportedAlgorithms);

    impl ServerCertVerifier for NoVerify {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, rustls::Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.0.supported_schemes()
        }
    }

    fn roots_from_pem(path: &Path) -> Result<RootCertStore, MetaError> {
        let mut roots = RootCertStore::empty();
        let certs = CertificateDer::pem_file_iter(path)
            .map_err(|e| MetaError::Invalid(format!("cannot read sslrootcert {}: {e}", path.display())))?;
        for cert in certs {
            let cert = cert.map_err(|e| MetaError::Invalid(format!("bad certificate in {}: {e}", path.display())))?;
            roots
                .add(cert)
                .map_err(|e| MetaError::Invalid(format!("unusable certificate in {}: {e}", path.display())))?;
        }
        if roots.is_empty() {
            return Err(MetaError::Invalid(format!("{} contains no certificates", path.display())));
        }
        Ok(roots)
    }

    pub fn connector(target: &Target) -> Result<Connector, MetaError> {
        if target.tls == TlsMode::Disable {
            return Ok(Connector::Plain);
        }
        let provider: Arc<CryptoProvider> = Arc::new(rustls::crypto::ring::default_provider());
        let builder = ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| MetaError::Invalid(format!("TLS setup failed: {e}")))?;
        let config = match target.tls {
            TlsMode::Prefer | TlsMode::Require => builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerify(provider.signature_verification_algorithms)))
                .with_no_client_auth(),
            TlsMode::VerifyFull => {
                let roots = match &target.root_cert {
                    Some(path) => roots_from_pem(path)?,
                    None => RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() },
                };
                builder.with_root_certificates(roots).with_no_client_auth()
            }
            TlsMode::Disable => unreachable!("handled above"),
        };
        Ok(Connector::Rustls(tokio_postgres_rustls::MakeRustlsConnect::new(config)))
    }
}

#[cfg(feature = "tls")]
pub fn connector(target: &Target) -> Result<Connector, MetaError> {
    imp::connector(target)
}

/// Without TLS support: plain connections only.
#[cfg(not(feature = "tls"))]
pub fn connector(target: &Target) -> Result<Connector, MetaError> {
    match target.tls {
        TlsMode::Disable | TlsMode::Prefer => Ok(Connector::Plain),
        TlsMode::Require | TlsMode::VerifyFull => Err(MetaError::Invalid(
            "this build of ag-storage has no TLS support (feature `tls`), but the connection string asks for it".into(),
        )),
    }
}
