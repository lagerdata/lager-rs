//! Debug tunnels through a box's gateway (`LagerBox::debug_tunnel`) and the
//! `bearer_token` escape hatch.
//!
//! A fake gateway -- a real socket server on 127.0.0.1 -- speaks the
//! CONNECT handshake from §10 of the gateway-auth contract and then echoes,
//! so bytes really cross the tunnel. The client reaches it through
//! `debug_service_url`, which is where the crate sends the CONNECT.
//!
//! These tests set the process-global `LAGER_GATEWAY_AUTH_FILE` /
//! `LAGER_GATEWAY_TOKEN` variables, so everything serializes on
//! [`env_lock`].

#![cfg(feature = "blocking")]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::Duration;
use std::time::Instant;

use httpmock::prelude::*;
use lager::{Error, LagerBox};
use serde_json::{json, Value};

const AUTH_URL: &str = "https://auth.example.com";

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    match LOCK.get_or_init(|| Mutex::new(())).lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct EnvVar {
    key: &'static str,
    previous: Option<String>,
}

impl EnvVar {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        EnvVar { key, previous }
    }

    fn unset(key: &'static str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::remove_var(key);
        EnvVar { key, previous }
    }
}

impl Drop for EnvVar {
    fn drop(&mut self) {
        match &self.previous {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

/// An isolated token store, and no pinned token from the environment.
struct Env {
    path: std::path::PathBuf,
    _store: EnvVar,
    _token: EnvVar,
}

impl Env {
    fn with(contents: &Value) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lager-rs-debug-tunnel-{}-{:?}.json",
            std::process::id(),
            thread::current().id(),
        ));
        std::fs::write(&path, serde_json::to_string(contents).unwrap()).unwrap();
        Env {
            _store: EnvVar::set("LAGER_GATEWAY_AUTH_FILE", path.to_str().unwrap()),
            _token: EnvVar::unset("LAGER_GATEWAY_TOKEN"),
            path,
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn fake_jwt(tag: &str, exp_offset_secs: i64) -> String {
    fn b64url(data: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let mut acc = 0u32;
            for (i, &b) in chunk.iter().enumerate() {
                acc |= (b as u32) << (16 - 8 * i);
            }
            for i in 0..=chunk.len() {
                out.push(ALPHABET[((acc >> (18 - 6 * i)) & 0x3f) as usize] as char);
            }
        }
        out
    }
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        + exp_offset_secs;
    let header = b64url(br#"{"alg":"none"}"#);
    let payload = b64url(format!(r#"{{"exp":{exp},"tag":"{tag}"}}"#).as_bytes());
    format!("{header}.{payload}.sig")
}

// ---------------------------------------------------------------------------
// Fake gateway
// ---------------------------------------------------------------------------

/// One CONNECT the gateway received: request line and headers (lowercased).
#[derive(Clone, Debug)]
struct Seen {
    line: String,
    headers: HashMap<String, String>,
}

impl Seen {
    fn authorization(&self) -> Option<&str> {
        self.headers.get("authorization").map(String::as_str)
    }
}

type Answer = dyn Fn(&Seen) -> Vec<u8> + Send + Sync;

/// A CONNECT-speaking gateway. `answer` returns the response bytes; when
/// they start with a 200 the connection turns into an echo, after the
/// optional `greeting`, which goes out in the same write as the head.
struct Gateway {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Gateway {
    fn start(answer: impl Fn(&Seen) -> Vec<u8> + Send + Sync + 'static) -> Self {
        Self::start_with_greeting(answer, b"")
    }

    fn start_with_greeting(
        answer: impl Fn(&Seen) -> Vec<u8> + Send + Sync + 'static,
        greeting: &'static [u8],
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let answer: Arc<Answer> = Arc::new(answer);
        let seen_in = seen.clone();
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { return };
                let (seen, answer) = (seen_in.clone(), answer.clone());
                thread::spawn(move || serve(conn, &seen, &*answer, greeting));
            }
        });
        Gateway { port, seen }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn serve(mut conn: TcpStream, seen: &Mutex<Vec<Seen>>, answer: &Answer, greeting: &[u8]) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match conn.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => return,
        }
    }
    let text = String::from_utf8_lossy(&head).to_string();
    let mut lines = text.split("\r\n");
    let line = lines.next().unwrap_or_default().to_string();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let request = Seen { line, headers };
    seen.lock().unwrap().push(request.clone());
    let reply = answer(&request);
    if !reply.starts_with(b"HTTP/1.1 200") {
        let _ = conn.write_all(&reply);
        return;
    }
    let mut first = reply;
    first.extend_from_slice(greeting);
    if conn.write_all(&first).is_err() {
        return;
    }
    let mut buf = [0u8; 65536];
    loop {
        match conn.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if conn.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
}

fn established() -> Vec<u8> {
    b"HTTP/1.1 200 Connection Established\r\n\r\n".to_vec()
}

fn reply(status: u16, reason: &str, headers: &[(&str, &str)], body: &str) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status} {reason}\r\n");
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    out.into_bytes()
}

fn denial(status: u16) -> Vec<u8> {
    reply(
        status,
        "Denied",
        &[
            ("X-Gateway-Auth-Url", AUTH_URL),
            ("Content-Type", "application/json"),
        ],
        "{}",
    )
}

/// A client whose box is 127.0.0.1 and whose debug service is `gw`.
fn client(gw: &Gateway) -> LagerBox {
    LagerBox::builder("127.0.0.1:9")
        .debug_service_url(gw.url())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

fn read_exactly(stream: &mut TcpStream, n: usize) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf).unwrap();
    buf
}

fn has_authorization(req: &HttpMockRequest) -> bool {
    req.headers.as_ref().is_some_and(|headers| {
        headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
    })
}

fn gated_store(token: &str) -> Value {
    json!({
        "boxes": { "127.0.0.1": AUTH_URL },
        "authServers": { AUTH_URL: { "accessToken": token, "cookies": {} } }
    })
}

// ---------------------------------------------------------------------------
// The tunnel
// ---------------------------------------------------------------------------

#[test]
fn request_is_a_bare_port_connect() {
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let gw = Gateway::start(|_| established());
    let stream = client(&gw).debug_tunnel(2332).unwrap();
    drop(stream);
    let seen = gw.seen();
    assert_eq!(seen[0].line, "CONNECT 2332 HTTP/1.1");
    assert_eq!(seen[0].headers["host"], "127.0.0.1:2332");
    // A box nobody knows to be gated gets no token.
    assert_eq!(seen[0].authorization(), None);
}

#[test]
fn a_large_payload_crosses_both_ways() {
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let gw = Gateway::start(|_| established());
    let mut stream = client(&gw).debug_tunnel(2331).unwrap();
    assert!(stream.nodelay().unwrap());
    assert_eq!(stream.read_timeout().unwrap(), None);

    let payload: Vec<u8> = (0..4 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let mut writer = stream.try_clone().unwrap();
    let sent = payload.clone();
    let send = thread::spawn(move || writer.write_all(&sent).unwrap());
    let got = read_exactly(&mut stream, payload.len());
    send.join().unwrap();
    assert!(got == payload);
}

#[test]
fn bytes_after_the_head_are_left_for_the_caller() {
    // A GDB server can speak first; its bytes arrive in the same segment as
    // the 200 and must not be swallowed by the head reader.
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let gw = Gateway::start_with_greeting(|_| established(), b"+$OK#9a");
    let mut stream = client(&gw).debug_tunnel(2331).unwrap();
    assert_eq!(read_exactly(&mut stream, 7), b"+$OK#9a");
}

#[test]
fn a_pinned_token_rides_on_the_connect() {
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let gw = Gateway::start(|_| established());
    let lager = LagerBox::builder("127.0.0.1:9")
        .debug_service_url(gw.url())
        .bearer_token("ci-token")
        .build()
        .unwrap();
    drop(lager.debug_tunnel(2331).unwrap());
    assert_eq!(gw.seen()[0].authorization(), Some("Bearer ci-token"));
}

#[test]
fn an_env_pinned_token_rides_on_the_connect() {
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let _token = EnvVar::set("LAGER_GATEWAY_TOKEN", "env-token");
    let gw = Gateway::start(|_| established());
    drop(client(&gw).debug_tunnel(2331).unwrap());
    assert_eq!(gw.seen()[0].authorization(), Some("Bearer env-token"));
}

#[test]
fn a_refreshed_token_is_used() {
    let _guard = env_lock();
    let auth = MockServer::start();
    let auth_url = auth.url("");
    let fresh = fake_jwt("fresh", 3600);
    let refresh = auth.mock(|when, then| {
        when.method(POST).path("/api/auth/refresh");
        then.status(200)
            .json_body(json!({ "accessToken": fresh.clone() }));
    });
    // A known-gated box whose stored token has expired.
    let _env = Env::with(&json!({
        "boxes": { "127.0.0.1": auth_url },
        "authServers": { auth_url.clone(): {
            "accessToken": fake_jwt("stale", -60), "cookies": { "refresh": "r1" } } }
    }));
    let gw = Gateway::start(|_| established());
    drop(client(&gw).debug_tunnel(2331).unwrap());
    refresh.assert();
    assert_eq!(
        gw.seen()[0].authorization().map(str::to_string),
        Some(format!("Bearer {fresh}"))
    );
}

#[test]
fn a_known_gated_box_gets_its_stored_token() {
    let _guard = env_lock();
    let token = fake_jwt("stored", 3600);
    let _env = Env::with(&gated_store(&token));
    let gw = Gateway::start(|_| established());
    drop(client(&gw).debug_tunnel(2331).unwrap());
    assert_eq!(
        gw.seen()[0].authorization().map(str::to_string),
        Some(format!("Bearer {token}"))
    );
}

#[test]
fn first_contact_401_retries_once_with_the_cli_session() {
    let _guard = env_lock();
    let token = fake_jwt("session", 3600);
    // `lager login` happened; this box was never contacted, so the mapping
    // is learned from the gateway's 401.
    let _env = Env::with(&json!({
        "authServers": { AUTH_URL: { "accessToken": token, "cookies": {} } }
    }));
    let gw = Gateway::start(|seen| {
        if seen.authorization().is_some() {
            established()
        } else {
            denial(401)
        }
    });
    drop(client(&gw).debug_tunnel(2331).unwrap());
    let seen = gw.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].authorization(), None);
    assert_eq!(
        seen[1].authorization().map(str::to_string),
        Some(format!("Bearer {token}"))
    );
}

#[test]
fn a_rejected_pinned_token_is_not_retried() {
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let _token = EnvVar::set("LAGER_GATEWAY_TOKEN", "revoked");
    let gw = Gateway::start(|_| denial(401));
    let err = client(&gw).debug_tunnel(2331).unwrap_err();
    assert!(matches!(err, Error::AuthRequired { .. }), "{err}");
    assert_eq!(gw.seen().len(), 1);
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[test]
fn a_401_without_a_session_asks_for_sign_in() {
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let gw = Gateway::start(|_| denial(401));
    let err = client(&gw).debug_tunnel(2331).unwrap_err();
    match &err {
        Error::AuthRequired { auth_url, .. } => assert_eq!(auth_url, AUTH_URL),
        other => panic!("expected AuthRequired, got {other}"),
    }
    assert!(err.to_string().contains(&format!("lager login {AUTH_URL}")));
}

#[test]
fn a_403_with_the_discovery_header_is_no_access() {
    let _guard = env_lock();
    let _env = Env::with(&gated_store(&fake_jwt("t", 3600)));
    let gw = Gateway::start(|_| denial(403));
    let err = client(&gw).debug_tunnel(2331).unwrap_err();
    assert!(matches!(err, Error::Box { status: 403, .. }), "{err}");
    assert!(
        err.to_string().contains("not authorized to use box"),
        "{err}"
    );
}

#[test]
fn a_503_is_the_auth_server_being_down() {
    let _guard = env_lock();
    let _env = Env::with(&gated_store(&fake_jwt("t", 3600)));
    let gw = Gateway::start(|_| denial(503));
    let err = client(&gw).debug_tunnel(2331).unwrap_err();
    assert!(matches!(err, Error::Box { status: 503, .. }), "{err}");
    assert!(
        err.to_string().contains("auth server is unreachable"),
        "{err}"
    );
}

#[test]
fn a_403_without_the_header_is_a_port_outside_the_debug_ranges() {
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let gw = Gateway::start(|_| {
        reply(
            403,
            "Forbidden",
            &[("Content-Type", "text/plain")],
            "port 22 is not tunnelable",
        )
    });
    let err = client(&gw).debug_tunnel(22).unwrap_err();
    assert!(matches!(err, Error::Box { status: 403, .. }), "{err}");
    let text = err.to_string();
    assert!(text.contains("does not tunnel port 22"), "{text}");
    assert!(text.contains("port 22 is not tunnelable"), "{text}");
    // Only a 502 is retried.
    assert_eq!(gw.seen().len(), 1);
}

/// A gateway that answers 502 `failures` times, then opens the tunnel.
fn starting_server(failures: usize) -> Gateway {
    let calls = AtomicUsize::new(0);
    Gateway::start(move |_| {
        if calls.fetch_add(1, Ordering::SeqCst) < failures {
            reply(502, "Bad Gateway", &[("Content-Type", "text/plain")], "")
        } else {
            established()
        }
    })
}

#[test]
fn a_502_while_the_server_starts_is_waited_out() {
    // The harness's sequence: start the server, dial its port at once.
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let gw = starting_server(2);
    let started = Instant::now();
    let mut stream = client(&gw).debug_tunnel(2332).unwrap();
    stream.write_all(b"ping").unwrap();
    assert_eq!(read_exactly(&mut stream, 4), b"ping");
    assert_eq!(gw.seen().len(), 3);
    // Two retries, about 250 ms apart.
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(450), "{waited:?}");
    assert!(waited < Duration::from_secs(3), "{waited:?}");
}

#[test]
fn a_502_that_persists_fails_after_about_five_seconds() {
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let gw = Gateway::start(|_| reply(502, "Bad Gateway", &[("Content-Type", "text/plain")], ""));
    let started = Instant::now();
    let err = client(&gw).debug_tunnel(2331).unwrap_err();
    let waited = started.elapsed();
    assert!(matches!(err, Error::Box { status: 502, .. }), "{err}");
    assert!(
        err.to_string()
            .contains("nothing is listening on port 2331"),
        "{err}"
    );
    assert!(waited >= Duration::from_millis(4500), "{waited:?}");
    assert!(waited < Duration::from_secs(8), "{waited:?}");
    // Every ~250 ms: well over a handful of attempts, far from a hot loop.
    let attempts = gw.seen().len();
    assert!((10..=25).contains(&attempts), "{attempts} attempts");
}

#[test]
fn a_denial_is_not_retried() {
    let _guard = env_lock();
    let _env = Env::with(&gated_store(&fake_jwt("t", 3600)));
    let gw = Gateway::start(|_| denial(503));
    let err = client(&gw).debug_tunnel(2331).unwrap_err();
    assert!(matches!(err, Error::Box { status: 503, .. }), "{err}");
    assert_eq!(gw.seen().len(), 1);
}

#[test]
fn a_pinned_token_behind_an_old_gateway_says_the_gateway_needs_updating() {
    // A pinned token never meets a denial, so the client never learns the
    // box's auth server. The token on the CONNECT is still proof enough
    // that a gateway is in front: a plain box's published port would have
    // answered the direct connect.
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let _token = EnvVar::set("LAGER_GATEWAY_TOKEN", "ci-token");
    let gw = Gateway::start(|_| reply(501, "Unsupported method", &[], ""));
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let err = client(&gw).debug_tunnel(closed).unwrap_err();
    assert!(matches!(err, Error::Box { status: 501, .. }), "{err}");
    assert!(
        err.to_string()
            .contains("does not support debug tunnels yet"),
        "{err}"
    );
    assert_eq!(gw.seen().len(), 1);
}

#[test]
fn an_old_gateway_on_a_gated_box_needs_updating() {
    let _guard = env_lock();
    let _env = Env::with(&gated_store(&fake_jwt("t", 3600)));
    // Forwarded to the box's own debug service, which answers 501.
    let gw = Gateway::start(|_| reply(501, "Unsupported method", &[], ""));
    // A port nothing listens on locally, so the direct attempt fails too.
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let err = client(&gw).debug_tunnel(closed).unwrap_err();
    assert!(matches!(err, Error::Box { status: 501, .. }), "{err}");
    assert!(
        err.to_string()
            .contains("does not support debug tunnels yet"),
        "{err}"
    );
}

#[test]
fn a_plain_box_is_reached_directly() {
    // A plain box's debug service answers CONNECT with 501, and its debug
    // ports are published, so the stream goes straight to the port.
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let gw = Gateway::start(|_| reply(501, "Unsupported method", &[], ""));
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = target.local_addr().unwrap().port();
    let echo = thread::spawn(move || {
        let (mut conn, _) = target.accept().unwrap();
        let mut buf = [0u8; 5];
        conn.read_exact(&mut buf).unwrap();
        conn.write_all(&buf).unwrap();
    });
    let mut stream = client(&gw).debug_tunnel(port).unwrap();
    stream.write_all(b"hello").unwrap();
    assert_eq!(read_exactly(&mut stream, 5), b"hello");
    echo.join().unwrap();
    assert_eq!(gw.seen().len(), 1);
}

#[test]
fn a_peer_that_is_not_http_counts_as_no_tunnel() {
    let _guard = env_lock();
    let _env = Env::with(&gated_store(&fake_jwt("t", 3600)));
    let gw = Gateway::start(|_| b"SSH-2.0-OpenSSH_9.6\r\n\r\n".to_vec());
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let err = client(&gw).debug_tunnel(closed).unwrap_err();
    assert!(
        err.to_string()
            .contains("does not support debug tunnels yet"),
        "{err}"
    );
}

#[test]
fn an_unreachable_debug_service_is_a_connection_error() {
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let lager = LagerBox::builder("127.0.0.1:9")
        .debug_service_url(format!("http://127.0.0.1:{closed}"))
        .build()
        .unwrap();
    let err = lager.debug_tunnel(2331).unwrap_err();
    assert!(matches!(err, Error::Connection(_)), "{err}");
}

// ---------------------------------------------------------------------------
// bearer_token
// ---------------------------------------------------------------------------

#[test]
fn bearer_token_is_none_for_a_plain_box() {
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let server = MockServer::start();
    let health = server.mock(|when, then| {
        when.method(GET).path("/health");
        then.status(200).json_body(json!({ "status": "healthy" }));
    });
    let lager = LagerBox::connect(server.address().to_string()).unwrap();
    assert_eq!(lager.bearer_token().unwrap(), None);
    health.assert();
}

#[test]
fn bearer_token_returns_a_pinned_token_without_a_request() {
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let lager = LagerBox::builder("127.0.0.1:9")
        .bearer_token("ci-token")
        .build()
        .unwrap();
    assert_eq!(lager.bearer_token().unwrap().as_deref(), Some("ci-token"));
}

#[test]
fn bearer_token_returns_the_stored_session_for_a_known_gated_box() {
    let _guard = env_lock();
    let token = fake_jwt("stored", 3600);
    let _env = Env::with(&gated_store(&token));
    let lager = LagerBox::connect("127.0.0.1:9").unwrap();
    assert_eq!(lager.bearer_token().unwrap(), Some(token));
}

#[test]
fn bearer_token_discovers_a_gated_box_on_first_contact() {
    let _guard = env_lock();
    let token = fake_jwt("session", 3600);
    let _env = Env::with(&json!({
        "authServers": { AUTH_URL: { "accessToken": token, "cookies": {} } }
    }));
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/health")
            .matches(|req| !has_authorization(req));
        then.status(401).header("X-Gateway-Auth-Url", AUTH_URL);
    });
    server.mock(|when, then| {
        when.method(GET)
            .path("/health")
            .header_exists("authorization");
        then.status(200).json_body(json!({ "status": "healthy" }));
    });
    let lager = LagerBox::connect(server.address().to_string()).unwrap();
    assert_eq!(lager.bearer_token().unwrap(), Some(token));
}

#[test]
fn bearer_token_on_a_gated_box_without_a_session_asks_for_sign_in() {
    let _guard = env_lock();
    let _env = Env::with(&json!({}));
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/health");
        then.status(401).header("X-Gateway-Auth-Url", AUTH_URL);
    });
    let lager = LagerBox::connect(server.address().to_string()).unwrap();
    let err = lager.bearer_token().unwrap_err();
    assert!(matches!(err, Error::AuthRequired { .. }), "{err}");
}

// ---------------------------------------------------------------------------
// Async client
// ---------------------------------------------------------------------------

// The env lock is held across awaits on purpose: it serializes the
// process-global variables for the whole test, and each test runs alone.
#[cfg(feature = "async")]
#[allow(clippy::await_holding_lock)]
mod async_client {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn async_client(gw: &Gateway) -> lager::AsyncLagerBox {
        lager::AsyncLagerBox::builder("127.0.0.1:9")
            .debug_service_url(gw.url())
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn splices_with_a_greeting_and_a_token() {
        let _guard = env_lock();
        let _env = Env::with(&json!({}));
        let _token = EnvVar::set("LAGER_GATEWAY_TOKEN", "async-token");
        let gw = Gateway::start_with_greeting(|_| established(), b"+$OK#9a");
        let mut stream = async_client(&gw).debug_tunnel(2331).await.unwrap();
        let mut greeting = [0u8; 7];
        stream.read_exact(&mut greeting).await.unwrap();
        assert_eq!(&greeting, b"+$OK#9a");
        let payload: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();
        let (mut rd, mut wr) = stream.into_split();
        let sent = payload.clone();
        let send = tokio::spawn(async move { wr.write_all(&sent).await.unwrap() });
        let mut got = vec![0u8; payload.len()];
        rd.read_exact(&mut got).await.unwrap();
        send.await.unwrap();
        assert!(got == payload);
        assert_eq!(gw.seen()[0].authorization(), Some("Bearer async-token"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn waits_out_a_502_while_the_server_starts() {
        let _guard = env_lock();
        let _env = Env::with(&json!({}));
        let gw = starting_server(2);
        let mut stream = async_client(&gw).debug_tunnel(2332).await.unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut echo = [0u8; 4];
        stream.read_exact(&mut echo).await.unwrap();
        assert_eq!(&echo, b"ping");
        assert_eq!(gw.seen().len(), 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_pinned_token_behind_an_old_gateway_says_so() {
        let _guard = env_lock();
        let _env = Env::with(&json!({}));
        let _token = EnvVar::set("LAGER_GATEWAY_TOKEN", "ci-token");
        let gw = Gateway::start(|_| reply(501, "Unsupported method", &[], ""));
        let closed = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let err = async_client(&gw).debug_tunnel(closed).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("does not support debug tunnels yet"),
            "{err}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bearer_token_returns_a_pinned_token() {
        let _guard = env_lock();
        let _env = Env::with(&json!({}));
        let _token = EnvVar::set("LAGER_GATEWAY_TOKEN", "async-token");
        let lager = lager::AsyncLagerBox::connect("127.0.0.1:9").unwrap();
        assert_eq!(
            lager.bearer_token().await.unwrap().as_deref(),
            Some("async-token")
        );
    }
}
