//! How the blocking client's HTTP layer turns transport outcomes into
//! [`lager::Error`]s: timeouts, per-call timeout budgets, stalled bodies,
//! large bodies, and error statuses on the debug-service stream path.
//!
//! These pin the client's observable behavior independent of the HTTP
//! library underneath it. Cases already covered elsewhere are not repeated
//! here: connection refused (`mock_api.rs`), box error statuses on the
//! command path (`mock_api.rs`), the admin download timeout
//! (`mock_admin_files.rs`), and the HTTP/1.0 chunked RTT stream
//! (`rtt_stream.rs`).

#![cfg(feature = "blocking")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::Duration;

use httpmock::prelude::*;
use lager::{Error, LagerBox};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The gateway-auth tests below set process-global environment variables.
fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    match LOCK.get_or_init(|| Mutex::new(())).lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// One canned answer from [`serve_raw`]: bytes to write, then optionally
/// hold the connection open (without writing more) for `hold`.
struct Reply {
    bytes: Vec<u8>,
    hold: Option<Duration>,
}

impl Reply {
    fn send(bytes: impl Into<Vec<u8>>) -> Self {
        Reply {
            bytes: bytes.into(),
            hold: None,
        }
    }

    fn then_stall(bytes: impl Into<Vec<u8>>, hold: Duration) -> Self {
        Reply {
            bytes: bytes.into(),
            hold: Some(hold),
        }
    }
}

/// A raw HTTP server on 127.0.0.1 that answers one connection per reply,
/// in order, with exactly the given bytes. Returns the base URL and the
/// request heads it received (lower-cased), for header assertions.
fn serve_raw(replies: Vec<Reply>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let heads = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&heads);
    thread::spawn(move || {
        for reply in replies {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                }
                head.push_str(&line.to_ascii_lowercase());
            }
            let mut body = vec![0u8; content_length];
            let _ = reader.read_exact(&mut body);
            seen.lock().unwrap().push(head);

            let mut stream = stream;
            let _ = stream.write_all(&reply.bytes);
            let _ = stream.flush();
            if let Some(hold) = reply.hold {
                thread::sleep(hold);
            }
        }
    });
    (format!("http://{addr}"), heads)
}

/// An httpmock box that knows one debug net, for the RTT stream path.
fn nets_with_debug(nets: &MockServer) {
    nets.mock(|when, then| {
        when.method(GET).path("/nets/list");
        then.status(200).json_body(json!([{
            "name": "debug1",
            "role": "debug",
            "pin": "nrf5340_xxaa_app",
            "instrument": "jlink1",
            "address": "jlink://000440012345"
        }]));
    });
}

fn rtt_client(nets: &MockServer, debug_url: String) -> LagerBox {
    nets_with_debug(nets);
    LagerBox::builder(nets.address().to_string())
        .debug_service_url(debug_url)
        .build()
        .unwrap()
}

/// An HTTP/1.0 answer with a JSON body, the way the debug service sends
/// errors.
fn http10_json(status_line: &str, body: &Value) -> Vec<u8> {
    let body = body.to_string();
    format!(
        "HTTP/1.0 {status_line}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn adc_ok() -> Value {
    json!({"success": true, "action": "read", "message": "1.5 V", "value": 1.5})
}

// ---------------------------------------------------------------------------
// Timeouts on the command path
// ---------------------------------------------------------------------------

#[test]
fn no_answer_within_the_default_timeout_is_a_timeout() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/net/command");
        then.status(200)
            .delay(Duration::from_millis(1500))
            .json_body(adc_ok());
    });
    let lager = LagerBox::builder(server.address().to_string())
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let err = lager.adc("adc1").read().unwrap_err();
    assert!(matches!(err, Error::Timeout(_)), "{err:?}");
}

#[test]
fn a_json_body_that_stalls_after_the_headers_is_a_timeout() {
    // The headers arrive in time, the JSON body never finishes: a timeout,
    // as on the async client, not a decode failure.
    let (url, _) = serve_raw(vec![Reply::then_stall(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/json\r\n\
         Content-Length: 100\r\n\r\n{\"success\": tr",
        Duration::from_secs(3),
    )]);
    let lager = LagerBox::builder(url)
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap();
    let err = lager.adc("adc1").read().unwrap_err();
    assert!(matches!(err, Error::Timeout(_)), "{err:?}");
}

#[cfg(feature = "admin")]
#[test]
fn a_raw_body_that_stalls_after_the_headers_is_a_connection_error() {
    // Same stall on a raw-bytes call (`download_file` with a budget): the
    // body read fails as `Connection`.
    let (url, _) = serve_raw(vec![Reply::then_stall(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/octet-stream\r\n\
         Content-Length: 100\r\n\r\n\x7fELF",
        Duration::from_secs(3),
    )]);
    let lager = LagerBox::connect(url).unwrap();
    let err = lager
        .admin()
        .download_file_with_timeout("/tmp/lager-output/x", Some(Duration::from_millis(300)))
        .unwrap_err();
    assert!(matches!(err, Error::Connection(_)), "{err:?}");
    assert!(
        err.to_string().contains("reading the response failed"),
        "{err}"
    );
}

#[test]
fn a_per_call_budget_overrides_a_shorter_default() {
    // `nets_state` widens its own budget to 15s; a 200ms client default
    // must not cut it off.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/nets/state");
        then.status(200)
            .delay(Duration::from_millis(700))
            .json_body(json!([]));
    });
    let lager = LagerBox::builder(server.address().to_string())
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    assert!(lager.nets_state().unwrap().is_empty());
}

#[cfg(feature = "admin")]
#[test]
fn an_unbounded_call_ignores_the_default_timeout() {
    // `download_file` without a timeout is unbounded.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/download-file");
        then.status(200)
            .delay(Duration::from_millis(700))
            .body(b"\x7fELF");
    });
    let lager = LagerBox::builder(server.address().to_string())
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let bytes = lager.admin().download_file("/tmp/lager-output/x").unwrap();
    assert_eq!(bytes, b"\x7fELF");
}

// ---------------------------------------------------------------------------
// Body size
// ---------------------------------------------------------------------------

#[cfg(feature = "admin")]
#[test]
fn a_large_download_is_read_in_full() {
    // Larger than the 10 MiB some HTTP clients cap a buffered body at.
    let size = 12 * 1024 * 1024 + 7;
    let payload: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/download-file");
        then.status(200).body(payload.clone());
    });
    let lager = LagerBox::connect(server.address().to_string()).unwrap();
    let bytes = lager
        .admin()
        .download_file("/tmp/lager-output/big")
        .unwrap();
    assert_eq!(bytes.len(), size);
    assert!(bytes == payload, "content differs");
}

// ---------------------------------------------------------------------------
// The debug-service stream path (RTT)
// ---------------------------------------------------------------------------

#[test]
fn an_rtt_error_status_carries_the_service_message() {
    let nets = MockServer::start();
    let (url, _) = serve_raw(vec![Reply::send(http10_json(
        "500 Internal Server Error",
        &json!({"error": "J-Link probe not found"}),
    ))]);
    match rtt_client(&nets, url).debug("debug1").rtt() {
        Err(Error::Box { status, message }) => {
            assert_eq!(status, 500);
            assert_eq!(message, "J-Link probe not found");
        }
        other => panic!("expected Error::Box, got {:?}", other.err()),
    }
}

#[test]
fn a_busy_probe_on_rtt_is_probe_busy() {
    let nets = MockServer::start();
    let (url, _) = serve_raw(vec![Reply::send(http10_json(
        "503 Service Unavailable",
        &json!({"error": "probe busy: another session holds it"}),
    ))]);
    match rtt_client(&nets, url).debug("debug1").rtt() {
        Err(Error::ProbeBusy { message }) => assert!(message.contains("busy")),
        other => panic!("expected Error::ProbeBusy, got {:?}", other.err()),
    }
}

#[test]
fn an_unreachable_debug_service_on_rtt_is_a_connection_error() {
    let nets = MockServer::start();
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    match rtt_client(&nets, format!("http://127.0.0.1:{closed}"))
        .debug("debug1")
        .rtt()
    {
        Err(Error::Connection(_)) => {}
        other => panic!("expected Error::Connection, got {:?}", other.err()),
    }
}

/// Points the CLI session store at a temp file for the test's duration and
/// clears any pinned token.
struct AuthEnv {
    path: std::path::PathBuf,
    previous: Vec<(&'static str, Option<String>)>,
}

impl AuthEnv {
    fn with(store: &Value) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lager-rs-http-transport-{}-{:?}.json",
            std::process::id(),
            thread::current().id(),
        ));
        std::fs::write(&path, store.to_string()).unwrap();
        let keys = ["LAGER_GATEWAY_AUTH_FILE", "LAGER_GATEWAY_TOKEN"];
        let previous = keys.iter().map(|k| (*k, std::env::var(k).ok())).collect();
        std::env::set_var("LAGER_GATEWAY_AUTH_FILE", &path);
        std::env::remove_var("LAGER_GATEWAY_TOKEN");
        AuthEnv { path, previous }
    }
}

impl Drop for AuthEnv {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn gateway_401(auth_url: &str) -> Vec<u8> {
    format!(
        "HTTP/1.0 401 Unauthorized\r\n\
         X-Gateway-Auth-Url: {auth_url}\r\n\
         Content-Length: 0\r\n\r\n"
    )
    .into_bytes()
}

#[test]
fn a_gateway_denial_on_rtt_without_a_session_is_auth_required() {
    let _guard = env_lock();
    let _env = AuthEnv::with(&json!({}));
    let auth_url = "https://auth.example.com";
    let nets = MockServer::start();
    let (url, _) = serve_raw(vec![Reply::send(gateway_401(auth_url))]);
    match rtt_client(&nets, url).debug("debug1").rtt() {
        Err(Error::AuthRequired { auth_url: got, .. }) => assert_eq!(got, auth_url),
        other => panic!("expected Error::AuthRequired, got {:?}", other.err()),
    }
}

#[test]
fn a_gateway_denial_on_rtt_retries_once_with_the_cli_session() {
    let _guard = env_lock();
    let auth_url = "https://auth.example.com";
    // Unsigned JWT expiring far in the future; only `exp` is read.
    let token = "eyJhbGciOiJub25lIn0.eyJleHAiOjQxMDI0NDQ4MDB9.sig";
    let _env = AuthEnv::with(&json!({
        "authServers": { auth_url: { "accessToken": token, "cookies": {} } }
    }));
    let nets = MockServer::start();
    let (url, heads) = serve_raw(vec![
        Reply::send(gateway_401(auth_url)),
        Reply::send(
            "HTTP/1.0 200 OK\r\n\
             Content-Type: application/octet-stream\r\n\r\n\
             hello from rtt\n",
        ),
    ]);
    let mut out = String::new();
    rtt_client(&nets, url)
        .debug("debug1")
        .rtt()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    assert_eq!(out, "hello from rtt\n");
    let heads = heads.lock().unwrap();
    assert_eq!(heads.len(), 2, "expected exactly one retry");
    assert!(!heads[0].contains("authorization:"), "{}", heads[0]);
    assert!(
        heads[1].contains(&format!(
            "authorization: bearer {}",
            token.to_ascii_lowercase()
        )),
        "{}",
        heads[1]
    );
}
