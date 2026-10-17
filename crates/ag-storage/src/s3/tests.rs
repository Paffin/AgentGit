//! The S3 client against a scripted local HTTP server: wire format, signing,
//! retries, and refusing to treat failures as success. No real S3 needed.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ag_object::RawObject;
use ag_types::Timestamp;

use super::sigv4::{self, Credentials, SigningInput, sha256_hex};
use super::{S3Config, S3ObjectStore};
use crate::error::StoreError;
use crate::lock::{InProcessLock, ObjectLock};
use crate::object_store::{ObjectStatus, ObjectStore};

#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    fn path(&self) -> &str {
        self.target.split('?').next().unwrap()
    }
}

enum Reply {
    Status(u16, Vec<(&'static str, String)>, Vec<u8>),
    Drop,
}

fn ok(body: &str) -> Reply {
    Reply::Status(200, vec![], body.as_bytes().to_vec())
}

fn status(code: u16, body: &str) -> Reply {
    Reply::Status(code, vec![], body.as_bytes().to_vec())
}

const EMPTY_LISTING: &str =
    "<?xml version=\"1.0\"?><ListBucketResult><IsTruncated>false</IsTruncated></ListBucketResult>";

struct Fake {
    addr: String,
    log: Arc<Mutex<Vec<Recorded>>>,
    stop: Arc<AtomicBool>,
}

impl Fake {
    fn start(handler: impl Fn(usize, &Recorded) -> Reply + Send + Sync + 'static) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let (log, stop) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
        let handler = Arc::new(handler);
        let (l2, s2) = (log.clone(), stop.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if s2.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(conn) = conn else { continue };
                let (handler, log) = (handler.clone(), l2.clone());
                std::thread::spawn(move || serve(conn, &*handler, &log));
            }
        });
        Fake { addr, log, stop }
    }

    fn requests(&self) -> Vec<Recorded> {
        self.log.lock().unwrap().clone()
    }

    fn config(&self) -> S3Config {
        let mut cfg = S3Config::new(
            format!("http://{}", self.addr),
            "eu-test-1",
            "bkt",
            Credentials::new("AKIDEXAMPLE", "very-secret-key").with_session_token("session-tok"),
        );
        cfg.prefix = "repo/one".into();
        cfg.max_retries = 3;
        cfg.request_timeout = Duration::from_secs(10);
        cfg
    }

    fn store(&self) -> S3ObjectStore {
        S3ObjectStore::open(self.config(), Arc::new(InProcessLock::new())).unwrap()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(&self.addr);
    }
}

fn serve(mut conn: TcpStream, handler: &(dyn Fn(usize, &Recorded) -> Reply + Send + Sync), log: &Mutex<Vec<Recorded>>) {
    conn.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap().split(' ');
    let (method, target) = (first.next().unwrap().to_string(), first.next().unwrap().to_string());
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())))
        .collect();
    let len: usize = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    let rec = Recorded { method, target, headers, body };
    let n = {
        let mut l = log.lock().unwrap();
        l.push(rec.clone());
        l.len() - 1
    };
    match handler(n, &rec) {
        Reply::Drop => {} // closing the socket without an answer
        Reply::Status(code, headers, body) => {
            let mut out = format!("HTTP/1.1 {code} X\r\nconnection: close\r\ncontent-length: {}\r\n", body.len());
            for (k, v) in headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str("\r\n");
            let _ = conn.write_all(out.as_bytes());
            if rec.method != "HEAD" {
                let _ = conn.write_all(&body);
            }
        }
    }
}

fn blob(data: &[u8]) -> RawObject {
    RawObject::blob(data.to_vec())
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

#[test]
fn every_request_is_signed_consistently_with_what_is_on_the_wire() {
    let fake = Fake::start(|_, r| match r.method.as_str() {
        "GET" if r.target.contains("list-type=2") => ok(EMPTY_LISTING),
        "PUT" => ok(""),
        _ => status(404, "<Error><Code>NoSuchKey</Code></Error>"),
    });
    let store = fake.store();
    let obj = blob(b"payload that needs hashing");
    store.put(&obj).unwrap();
    assert!(store.get(&obj.id()).unwrap().is_none());

    let creds = fake.config().credentials;
    for r in fake.requests() {
        let auth = r.header("authorization").expect("signed");
        assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"), "{auth}");
        assert!(auth.contains("/eu-test-1/s3/aws4_request"), "{auth}");
        assert_eq!(r.header("x-amz-content-sha256"), Some(sha256_hex(&r.body).as_str()), "payload hash");
        assert_eq!(r.header("x-amz-security-token"), Some("session-tok"), "the session token is sent and signed");
        assert_eq!(r.header("host"), Some(fake.addr.as_str()));
        assert!(auth.contains("x-amz-security-token"), "{auth}");

        // Recompute the signature from the received request, the way a server would.
        let signed_names: Vec<&str> =
            auth.split("SignedHeaders=").nth(1).unwrap().split(',').next().unwrap().split(';').collect();
        let headers: Vec<(String, String)> =
            signed_names.iter().map(|n| (n.to_string(), r.header(n).unwrap().to_string())).collect();
        let query: Vec<(String, String)> = r
            .target
            .split_once('?')
            .map(|(_, q)| {
                q.split('&')
                    .map(|kv| {
                        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                        (percent_decode(k), percent_decode(v))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let date = r.header("x-amz-date").unwrap();
        let expected = sigv4::sign(
            &creds,
            &SigningInput {
                method: &r.method,
                canonical_uri: r.path(),
                query: &query,
                headers: &headers,
                payload_sha256: r.header("x-amz-content-sha256").unwrap(),
                region: "eu-test-1",
                amz_date: date,
            },
        );
        assert_eq!(auth, expected.authorization, "signature of {} {}", r.method, r.target);
    }
}

#[test]
fn keys_use_the_documented_layout_and_conditional_create() {
    let fake = Fake::start(|_, r| if r.method == "GET" { ok(EMPTY_LISTING) } else { ok("") });
    let store = fake.store();
    let obj = blob(b"layout");
    let id = store.put(&obj).unwrap();
    let hex = id.to_hex();
    let puts: Vec<_> = fake.requests().into_iter().filter(|r| r.method == "PUT").collect();
    assert_eq!(puts.len(), 1);
    assert_eq!(puts[0].path(), format!("/bkt/repo/one/objects/{}/{}", &hex[..2], &hex[2..]));
    assert_eq!(puts[0].header("if-none-match"), Some("*"), "created conditionally");
    assert_eq!(puts[0].body, obj.encode_envelope());
}

#[test]
fn transient_errors_are_retried_and_then_succeed() {
    let obj = blob(b"retry me");
    let envelope = obj.encode_envelope();
    let env2 = envelope.clone();
    let fake = Fake::start(move |n, r| match (r.method.as_str(), n) {
        ("GET", 0) => ok(EMPTY_LISTING), // open
        (_, 1) => status(503, "<Error><Code>SlowDown</Code><Message>slow</Message></Error>"),
        (_, 2) => Reply::Drop,
        (_, 3) => status(500, ""),
        _ => Reply::Status(200, vec![], env2.clone()),
    });
    let store = fake.store();
    assert_eq!(store.get(&obj.id()).unwrap().unwrap(), obj);
    assert_eq!(fake.requests().len(), 5, "open + 3 failures + the successful answer");
}

#[test]
fn giving_up_is_an_error_naming_the_last_status() {
    let fake =
        Fake::start(
            |n, _| if n == 0 { ok(EMPTY_LISTING) } else { status(503, "<Error><Code>SlowDown</Code></Error>") },
        );
    let store = fake.store();
    let err = store.get(&blob(b"x").id()).unwrap_err();
    assert!(
        matches!(&err, StoreError::Remote { status: Some(503), message } if message.contains("SlowDown") && message.contains("gave up after 4")),
        "{err}"
    );
    assert_eq!(fake.requests().len(), 1 + 4, "the first attempt plus max_retries");
}

#[test]
fn client_errors_are_never_retried_and_never_mistaken_for_absence() {
    let fake = Fake::start(|n, _| {
        if n == 0 {
            ok(EMPTY_LISTING)
        } else {
            status(403, "<Error><Code>AccessDenied</Code><Message>Access Denied</Message></Error>")
        }
    });
    let store = fake.store();
    let id = blob(b"x").id();
    let err = store.contains(&id).unwrap_err();
    assert!(
        matches!(&err, StoreError::Remote { status: Some(403), message } if message.contains("permission")),
        "{err}"
    );
    assert_eq!(fake.requests().len(), 2, "no retry for a 403");
    assert!(store.get(&id).is_err(), "403 is not 'not found'");
    assert!(store.put(&blob(b"y")).is_err(), "a refused write is not a success");
    assert!(store.delete_if_older_than(&id, Timestamp::from_millis(i64::MAX)).is_err());
}

#[test]
fn only_a_404_means_absent_and_only_where_absence_is_a_valid_answer() {
    let fake = Fake::start(|n, r| {
        if n == 0 {
            ok(EMPTY_LISTING)
        } else if r.method == "PUT" {
            status(404, "<Error><Code>NoSuchBucket</Code></Error>")
        } else {
            status(404, "<Error><Code>NoSuchKey</Code></Error>")
        }
    });
    let store = fake.store();
    let id = blob(b"x").id();
    assert!(store.get(&id).unwrap().is_none());
    assert!(!store.contains(&id).unwrap());
    assert_eq!(store.verify(&id).unwrap(), ObjectStatus::Missing);
    assert!(!store.delete(&id).unwrap());
    assert!(!store.delete_if_older_than(&id, Timestamp::from_millis(i64::MAX)).unwrap());
    let err = store.put(&blob(b"x")).unwrap_err();
    assert!(
        matches!(&err, StoreError::Remote { status: Some(404), message } if message.contains("NoSuchBucket") && message.contains("must exist")),
        "{err}"
    );
}

#[test]
fn a_copy_that_answers_200_with_an_error_document_is_an_error() {
    // The age refresh is a CopyObject, which S3 may answer "200 OK" with an <Error> body.
    let obj = blob(b"existing");
    let envelope = obj.encode_envelope();
    let fake = Fake::start(move |n, r| match (r.method.as_str(), n) {
        ("GET", 0) => ok(EMPTY_LISTING),
        ("PUT", _) if r.header("x-amz-copy-source").is_some() => {
            ok("<?xml version=\"1.0\"?><Error><Code>AccessDenied</Code><Message>no copy for you</Message></Error>")
        }
        ("PUT", _) => status(412, "<Error><Code>PreconditionFailed</Code></Error>"),
        _ => Reply::Status(200, vec![], envelope.clone()),
    });
    let store = fake.store();
    let err = store.put(&obj).unwrap_err();
    assert!(err.to_string().contains("AccessDenied"), "{err}");
}

#[test]
fn an_existing_healthy_object_is_refreshed_not_rewritten_and_a_damaged_one_is_overwritten() {
    let obj = blob(b"existing");
    let good = obj.encode_envelope();
    let damaged = Arc::new(AtomicBool::new(false));
    let d2 = damaged.clone();
    let fake = Fake::start(move |n, r| match (r.method.as_str(), n) {
        ("GET", 0) => ok(EMPTY_LISTING),
        ("PUT", _) if r.header("if-none-match").is_some() => {
            status(412, "<Error><Code>PreconditionFailed</Code></Error>")
        }
        ("PUT", _) => ok("<CopyObjectResult/>"),
        ("GET", _) if d2.load(Ordering::SeqCst) => Reply::Status(200, vec![], b"garbage".to_vec()),
        _ => Reply::Status(200, vec![], good.clone()),
    });
    let store = fake.store();

    store.put(&obj).unwrap();
    let reqs = fake.requests();
    let copy = reqs.iter().find(|r| r.header("x-amz-copy-source").is_some()).expect("refreshed by a server-side copy");
    assert_eq!(copy.header("x-amz-metadata-directive"), Some("REPLACE"), "REPLACE is what lets S3 copy onto itself");
    assert!(copy.header("x-amz-copy-source").unwrap().starts_with("/bkt/repo/one/objects/"));
    assert!(copy.body.is_empty(), "no data is transferred to refresh the age");
    assert_eq!(
        reqs.iter().filter(|r| r.method == "PUT" && !r.body.is_empty()).count(),
        1,
        "only the conditional create sent data"
    );

    damaged.store(true, Ordering::SeqCst);
    let before = fake.requests().len();
    store.put(&obj).unwrap();
    let after: Vec<_> = fake.requests().split_off(before);
    let heal = after.iter().rev().find(|r| r.method == "PUT").unwrap();
    assert!(heal.header("if-none-match").is_none() && heal.header("x-amz-copy-source").is_none());
    assert_eq!(heal.body, obj.encode_envelope(), "the damaged copy is replaced by good bytes");
}

#[test]
fn a_service_without_conditional_writes_falls_back_to_look_then_write() {
    let obj = blob(b"old minio");
    let fake = Fake::start(|n, r| match (r.method.as_str(), n) {
        ("GET", 0) => ok(EMPTY_LISTING),
        ("PUT", _) if r.header("if-none-match").is_some() => status(501, "<Error><Code>NotImplemented</Code></Error>"),
        ("HEAD", _) => status(404, ""),
        _ => ok(""),
    });
    fake.store().put(&obj).unwrap();
    let methods: Vec<String> = fake.requests().iter().skip(1).map(|r| r.method.clone()).collect();
    assert_eq!(methods, ["PUT", "HEAD", "PUT"]);
}

#[test]
fn listing_follows_continuation_tokens_and_skips_foreign_keys() {
    let ids: Vec<_> = (0..3).map(|i| blob(&[i]).id()).collect();
    let key = |id: &ag_types::ObjectId| {
        let h = id.to_hex();
        format!("repo/one/objects/{}/{}", &h[..2], &h[2..])
    };
    let page = |keys: &[String], next: Option<&str>| {
        let mut x = String::from("<ListBucketResult><Name>bkt</Name>");
        for k in keys {
            x.push_str(&format!(
                "<Contents><Key>{k}</Key><LastModified>2026-10-17T13:00:17.000Z</LastModified><ETag>\"e\"</ETag><Size>42</Size></Contents>"
            ));
        }
        match next {
            Some(t) => x.push_str(&format!(
                "<IsTruncated>true</IsTruncated><NextContinuationToken>{t}</NextContinuationToken>"
            )),
            None => x.push_str("<IsTruncated>false</IsTruncated>"),
        }
        x.push_str("</ListBucketResult>");
        x
    };
    let (p1, p2) = (
        page(&[key(&ids[0]), "repo/one/objects/README".into(), "repo/one/objects/zz/short".into()], Some("tok/1+2=")),
        page(&[key(&ids[1]), key(&ids[2])], None),
    );
    let fake = Fake::start(move |n, r| {
        if n == 0 {
            ok(EMPTY_LISTING)
        } else if r.target.contains("continuation-token=tok%2F1%2B2%3D") {
            ok(&p2)
        } else {
            ok(&p1)
        }
    });
    let store = fake.store();
    let listed = store.list().unwrap();
    let mut want = ids.clone();
    want.sort();
    assert_eq!(
        listed.iter().map(|i| i.id).collect::<Vec<_>>(),
        want,
        "three objects over two pages, foreign keys ignored"
    );
    assert!(listed.iter().all(|i| i.size == 42 && i.modified == Timestamp::from_millis(1_792_242_017_000)));
    let reqs = fake.requests();
    assert!(reqs[1].target.contains("prefix=repo%2Fone%2Fobjects%2F"), "{}", reqs[1].target);
}

#[test]
fn delete_if_older_than_looks_at_the_servers_last_modified_and_holds_the_lock() {
    let lm = "Fri, 24 May 2013 00:00:00 GMT"; // 1_369_353_600_000 ms
    let fake = Fake::start(move |n, r| match (r.method.as_str(), n) {
        ("GET", 0) => ok(EMPTY_LISTING),
        ("HEAD", _) => Reply::Status(200, vec![("last-modified", lm.into())], vec![]),
        _ => status(204, ""),
    });
    let store = fake.store();
    let id = blob(b"x").id();
    assert!(
        !store.delete_if_older_than(&id, Timestamp::from_millis(1_369_353_600_000)).unwrap(),
        "not older than itself"
    );
    assert!(!store.delete_if_older_than(&id, Timestamp::from_millis(1_000)).unwrap(), "younger than the cutoff");
    assert!(!fake.requests().iter().any(|r| r.method == "DELETE"));
    assert!(store.delete_if_older_than(&id, Timestamp::from_millis(1_369_353_600_001)).unwrap());
    assert_eq!(fake.requests().last().unwrap().method, "DELETE");
}

#[test]
fn addressing_styles_and_endpoints() {
    use super::http::Client;
    let creds = Credentials::new("a", "b");
    let path =
        Client::new(&S3Config::new("https://s3.example.com:9000/gateway/", "r", "my-bucket", creds.clone())).unwrap();
    assert_eq!(
        path.address(Some("a b/c")),
        ("s3.example.com:9000".to_string(), "/gateway/my-bucket/a%20b/c".to_string())
    );
    assert_eq!(path.address(None).1, "/gateway/my-bucket");
    let mut cfg = S3Config::new("https://s3.eu-west-1.amazonaws.com", "eu-west-1", "my-bucket", creds.clone());
    assert!(!cfg.path_style, "Amazon defaults to virtual-hosted style");
    let virt = Client::new(&cfg).unwrap();
    assert_eq!(virt.address(Some("k/1")), ("my-bucket.s3.eu-west-1.amazonaws.com".to_string(), "/k/1".to_string()));
    assert_eq!(virt.address(None).1, "/");
    assert!(
        S3Config::new("http://127.0.0.1:9000", "r", "b", creds.clone()).path_style,
        "custom endpoints default to path style"
    );
    // misconfigurations are reported, not guessed around
    for bad in ["", "ftp://x", "127.0.0.1:9000", "http://user:pw@host", "http://"] {
        assert!(Client::new(&S3Config::new(bad, "r", "b", creds.clone())).is_err(), "{bad}");
    }
    assert!(Client::new(&S3Config::new("http://h", "r", "a/b", creds.clone())).is_err());
    cfg.endpoint = "http://127.0.0.1:9000".into();
    assert!(Client::new(&cfg).is_err(), "virtual-hosted style needs a DNS name");
}

#[test]
fn credentials_and_signatures_do_not_leak_into_messages() {
    let fake = Fake::start(|n, _| {
        if n == 0 { ok(EMPTY_LISTING) } else { status(403, "<Error><Code>SignatureDoesNotMatch</Code></Error>") }
    });
    let store = fake.store();
    let err = store.get(&blob(b"x").id()).unwrap_err().to_string();
    for secret in ["very-secret-key", "session-tok"] {
        assert!(!err.contains(secret), "{err}");
        assert!(!format!("{:?}", fake.config()).contains(secret));
    }
    assert!(err.contains("access key"), "the hint tells the operator what to check: {err}");
    // a bad prefix is refused up front
    let mut cfg = fake.config();
    cfg.prefix = "a//b".into();
    assert!(S3ObjectStore::open(cfg, Arc::new(InProcessLock::new())).is_err());
}

#[test]
fn the_lock_is_held_for_the_whole_of_put_and_delete() {
    // A lock that records when guards are taken and released: put holds a shared
    // guard around its requests, delete_if_older_than an exclusive one.
    struct Spy(Arc<Mutex<Vec<String>>>);
    struct Held(Arc<Mutex<Vec<String>>>, &'static str);
    impl Drop for Held {
        fn drop(&mut self) {
            self.0.lock().unwrap().push(self.1.into());
        }
    }
    impl ObjectLock for Spy {
        fn shared(&self) -> Result<crate::lock::LockGuard, StoreError> {
            self.0.lock().unwrap().push("shared+".into());
            Ok(Box::new(Held(self.0.clone(), "shared-")))
        }
        fn exclusive(&self) -> Result<crate::lock::LockGuard, StoreError> {
            self.0.lock().unwrap().push("exclusive+".into());
            Ok(Box::new(Held(self.0.clone(), "exclusive-")))
        }
    }

    let fake = Fake::start(|n, r| match (r.method.as_str(), n) {
        ("GET", 0) => ok(EMPTY_LISTING),
        ("HEAD", _) => Reply::Status(200, vec![("last-modified", "Fri, 24 May 2013 00:00:00 GMT".into())], vec![]),
        _ => ok(""),
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let store = S3ObjectStore::open(fake.config(), Arc::new(Spy(events.clone()))).unwrap();
    store.put(&blob(b"x")).unwrap();
    store.delete_if_older_than(&blob(b"x").id(), Timestamp::from_millis(i64::MAX)).unwrap();
    assert_eq!(*events.lock().unwrap(), ["shared+", "shared-", "exclusive+", "exclusive-"]);
}
