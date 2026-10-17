//! AWS Signature Version 4, as used by S3 (and by every S3-compatible service).
//!
//! Hand-rolled on top of SHA-256 so that the store does not need an SDK. The
//! unit tests check it against the worked examples published in the AWS
//! documentation ("Signature Calculations for the Authorization Header").

use sha2::{Digest, Sha256};

use crate::error::StoreError;

/// Static credentials. Never written to a repository's configuration: the
/// configuration only names the environment variables that hold them.
#[derive(Clone)]
pub struct Credentials {
    /// Access key id.
    pub access_key_id: String,
    /// Secret access key.
    pub secret_access_key: String,
    /// Session token of temporary credentials.
    pub session_token: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .field("session_token", &self.session_token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Credentials {
    /// Credentials from explicit values.
    pub fn new(access_key_id: impl Into<String>, secret_access_key: impl Into<String>) -> Self {
        Self { access_key_id: access_key_id.into(), secret_access_key: secret_access_key.into(), session_token: None }
    }

    /// Adds a session token.
    pub fn with_session_token(mut self, token: impl Into<String>) -> Self {
        self.session_token = Some(token.into());
        self
    }

    /// The standard variables: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and
    /// optionally `AWS_SESSION_TOKEN`.
    pub fn from_env() -> Result<Self, StoreError> {
        Self::from_env_vars("AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", Some("AWS_SESSION_TOKEN"))
    }

    /// Credentials read from the named environment variables. The token variable
    /// is optional: unset or empty means "no session token".
    pub fn from_env_vars(id_var: &str, secret_var: &str, token_var: Option<&str>) -> Result<Self, StoreError> {
        let need = |var: &str| match std::env::var(var) {
            Ok(v) if !v.trim().is_empty() => Ok(v),
            _ => Err(StoreError::Remote {
                status: None,
                message: format!("the environment variable {var} (S3 credentials) is not set"),
            }),
        };
        let mut c = Self::new(need(id_var)?, need(secret_var)?);
        if let Some(var) = token_var
            && let Ok(t) = std::env::var(var)
            && !t.trim().is_empty()
        {
            c.session_token = Some(t);
        }
        Ok(c)
    }
}

/// SHA-256 as lowercase hex.
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// HMAC-SHA-256 (RFC 2104).
pub(crate) fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Percent-encodes everything except RFC 3986 unreserved characters
/// (`A-Z a-z 0-9 - . _ ~`), and `/` when `keep_slash`.
pub(crate) fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Days since 1970-01-01 → (year, month, day) in the proleptic Gregorian calendar.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// (year, month, day) → days since 1970-01-01.
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `YYYYMMDDTHHMMSSZ` for a Unix time in seconds.
pub(crate) fn amz_date(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs = unix_secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// Everything that goes into a signature.
pub(crate) struct SigningInput<'a> {
    /// `GET`, `PUT`, …
    pub method: &'a str,
    /// The request path, already percent-encoded (S3 does not encode twice).
    pub canonical_uri: &'a str,
    /// Query parameters, not yet encoded.
    pub query: &'a [(String, String)],
    /// Headers to sign, any case. Must include `host` and every `x-amz-*` header.
    pub headers: &'a [(String, String)],
    /// Hex SHA-256 of the body (also sent as `x-amz-content-sha256`).
    pub payload_sha256: &'a str,
    /// Region, e.g. `us-east-1`.
    pub region: &'a str,
    /// `YYYYMMDDTHHMMSSZ`.
    pub amz_date: &'a str,
}

/// The result of signing, with the intermediate values for testing.
pub(crate) struct Signed {
    /// The `Authorization` header value.
    pub authorization: String,
    #[cfg(test)]
    pub canonical_request: String,
    #[cfg(test)]
    pub string_to_sign: String,
}

/// Signs a request (AWS4-HMAC-SHA256, service `s3`).
pub(crate) fn sign(creds: &Credentials, input: &SigningInput<'_>) -> Signed {
    let mut headers: Vec<(String, String)> = input
        .headers
        .iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v.split_whitespace().collect::<Vec<_>>().join(" ")))
        .collect();
    headers.sort();
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_headers = headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");

    let mut query: Vec<(String, String)> =
        input.query.iter().map(|(k, v)| (uri_encode(k, false), uri_encode(v, false))).collect();
    query.sort();
    let canonical_query = query.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");

    let canonical_request = format!(
        "{}\n{}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{}",
        input.method, input.canonical_uri, input.payload_sha256
    );
    let date = &input.amz_date[..8];
    let scope = format!("{date}/{}/s3/aws4_request", input.region);
    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{}\n{scope}\n{}", input.amz_date, sha256_hex(canonical_request.as_bytes()));

    let k_date = hmac_sha256(format!("AWS4{}", creds.secret_access_key).as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, input.region.as_bytes());
    let k_service = hmac_sha256(&k_region, b"s3");
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex::encode(hmac_sha256(&k_signing, string_to_sign.as_bytes()));

    Signed {
        authorization: format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope},SignedHeaders={signed_headers},Signature={signature}",
            creds.access_key_id
        ),
        #[cfg(test)]
        canonical_request,
        #[cfg(test)]
        string_to_sign,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aws_example_credentials() -> Credentials {
        Credentials::new("AKIAIOSFODNN7EXAMPLE", "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY")
    }

    const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    /// The four examples of "Examples: Signature Calculations in AWS Signature
    /// Version 4" for Amazon S3 (2013-05-24, bucket `examplebucket`).
    #[test]
    fn aws_documentation_vectors_get_object_with_range() {
        let headers = pairs(&[
            ("Host", "examplebucket.s3.amazonaws.com"),
            ("Range", "bytes=0-9"),
            ("x-amz-content-sha256", EMPTY_SHA),
            ("x-amz-date", "20130524T000000Z"),
        ]);
        let s = sign(
            &aws_example_credentials(),
            &SigningInput {
                method: "GET",
                canonical_uri: "/test.txt",
                query: &[],
                headers: &headers,
                payload_sha256: EMPTY_SHA,
                region: "us-east-1",
                amz_date: "20130524T000000Z",
            },
        );
        assert_eq!(
            s.canonical_request,
            "GET\n/test.txt\n\nhost:examplebucket.s3.amazonaws.com\nrange:bytes=0-9\nx-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nx-amz-date:20130524T000000Z\n\nhost;range;x-amz-content-sha256;x-amz-date\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            s.string_to_sign,
            "AWS4-HMAC-SHA256\n20130524T000000Z\n20130524/us-east-1/s3/aws4_request\n7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972"
        );
        assert_eq!(
            s.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,SignedHeaders=host;range;x-amz-content-sha256;x-amz-date,Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn aws_documentation_vectors_put_object() {
        let body = b"Welcome to Amazon S3.";
        let body_sha = sha256_hex(body);
        assert_eq!(body_sha, "44ce7dd67c959e0d3524ffac1771dfbba87d2b6b4b4e99e42034a8b803f8b072");
        let headers = pairs(&[
            ("Date", "Fri, 24 May 2013 00:00:00 GMT"),
            ("Host", "examplebucket.s3.amazonaws.com"),
            ("x-amz-date", "20130524T000000Z"),
            ("x-amz-storage-class", "REDUCED_REDUNDANCY"),
            ("x-amz-content-sha256", &body_sha),
        ]);
        let s = sign(
            &aws_example_credentials(),
            &SigningInput {
                method: "PUT",
                canonical_uri: &format!("/{}", uri_encode("test$file.text", true)),
                query: &[],
                headers: &headers,
                payload_sha256: &body_sha,
                region: "us-east-1",
                amz_date: "20130524T000000Z",
            },
        );
        assert_eq!(
            s.string_to_sign.lines().last().unwrap(),
            "9e0e90d9c76de8fa5b200d8c849cd5b8dc7a3be3951ddb7f6a76b4158342019d"
        );
        assert!(s.authorization.ends_with(
            "SignedHeaders=date;host;x-amz-content-sha256;x-amz-date;x-amz-storage-class,Signature=98ad721746da40c64f1a55b78f14c238d841ea1380cd77a1b5971af0ece108bd"
        ));
    }

    #[test]
    fn aws_documentation_vectors_query_strings() {
        let headers = pairs(&[
            ("host", "examplebucket.s3.amazonaws.com"),
            ("x-amz-content-sha256", EMPTY_SHA),
            ("x-amz-date", "20130524T000000Z"),
        ]);
        let sign_with = |query: &[(String, String)]| {
            sign(
                &aws_example_credentials(),
                &SigningInput {
                    method: "GET",
                    canonical_uri: "/",
                    query,
                    headers: &headers,
                    payload_sha256: EMPTY_SHA,
                    region: "us-east-1",
                    amz_date: "20130524T000000Z",
                },
            )
        };
        // GET Bucket Lifecycle: a sub-resource with an empty value
        let lifecycle = sign_with(&pairs(&[("lifecycle", "")]));
        assert!(lifecycle.canonical_request.starts_with("GET\n/\nlifecycle=\n"));
        assert!(
            lifecycle
                .authorization
                .ends_with("Signature=fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543")
        );
        // GET Bucket (List Objects): parameters are sorted, whatever order they are given in
        let list = sign_with(&pairs(&[("prefix", "J"), ("max-keys", "2")]));
        assert!(list.canonical_request.starts_with("GET\n/\nmax-keys=2&prefix=J\n"));
        assert!(
            list.authorization.ends_with("Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7")
        );
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        // test case 2
        assert_eq!(
            hex::encode(hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // test case 6: a key longer than the block size is hashed first
        assert_eq!(
            hex::encode(hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
        // test case 1
        assert_eq!(
            hex::encode(hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn uri_encoding_follows_rfc_3986() {
        assert_eq!(uri_encode("test$file.text", true), "test%24file.text");
        assert_eq!(uri_encode("a b/c+d~e-f_g.h", true), "a%20b/c%2Bd~e-f_g.h");
        assert_eq!(uri_encode("a/b", false), "a%2Fb");
        assert_eq!(uri_encode("é", false), "%C3%A9");
        assert_eq!(uri_encode("", false), "");
    }

    #[test]
    fn dates_are_formatted_in_utc() {
        assert_eq!(amz_date(0), "19700101T000000Z");
        assert_eq!(amz_date(1_369_353_600), "20130524T000000Z");
        assert_eq!(amz_date(1_709_164_799), "20240228T235959Z");
        assert_eq!(amz_date(1_709_164_800), "20240229T000000Z", "leap day");
        assert_eq!(amz_date(951_782_400), "20000229T000000Z");
        assert_eq!(amz_date(-1), "19691231T235959Z");
        for (y, m, d) in [(1970, 1, 1), (2000, 2, 29), (2024, 12, 31), (2100, 3, 1), (1969, 7, 20)] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
    }

    #[test]
    fn credentials_never_show_their_secrets() {
        let c = aws_example_credentials().with_session_token("tok");
        let shown = format!("{c:?}");
        assert!(shown.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!shown.contains("wJalr") && !shown.contains("tok\""), "{shown}");
    }
}
