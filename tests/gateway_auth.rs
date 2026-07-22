//! Gateway auth contract tests: a mock authenticating gateway in front of
//! the box (401/403 + `X-Gateway-Auth-Url`), plus a mock auth server for
//! the refresh flow. Mirrors the CLI contract in `cli/gateway_auth.py`.
//!
//! These tests set the process-global `LAGER_GATEWAY_AUTH_FILE` /
//! `LAGER_GATEWAY_TOKEN` environment variables, so everything that touches
//! them serializes on [`env_lock`].

#![cfg(feature = "blocking")]

use std::sync::{Mutex, MutexGuard, OnceLock};

use httpmock::prelude::*;
use lager::{Error, LagerBox};
use serde_json::{json, Value};

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    // A test failure poisons the mutex; later tests are still fine to run.
    match LOCK.get_or_init(|| Mutex::new(())).lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// RAII env-var override that restores the previous state on drop.
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
}

impl Drop for EnvVar {
    fn drop(&mut self) {
        match &self.previous {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

/// A store file for the test, in a unique temp path.
struct StoreFile {
    path: std::path::PathBuf,
    _env: EnvVar,
}

impl StoreFile {
    fn with(contents: &Value) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lager-rs-gateway-auth-{}-{:?}.json",
            std::process::id(),
            std::thread::current().id(),
        ));
        std::fs::write(&path, serde_json::to_string(contents).unwrap()).unwrap();
        let env = EnvVar::set("LAGER_GATEWAY_AUTH_FILE", path.to_str().unwrap());
        StoreFile { path, _env: env }
    }

    fn read(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(&self.path).unwrap()).unwrap()
    }
}

impl Drop for StoreFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Unsigned JWT with the given `exp` (only the expiry is read client-side;
/// signature verification is the gateway's job).
fn fake_jwt(tag: &str, exp_offset_secs: i64) -> String {
    fn b64url(data: &[u8]) -> String {
        const ALPHABET: &[u8] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
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

fn adc_ok() -> Value {
    json!({"success": true, "action": "read", "message": "1.5 V", "value": 1.5})
}

fn has_authorization(req: &httpmock::prelude::HttpMockRequest) -> bool {
    req.headers.as_ref().is_some_and(|headers| {
        headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
    })
}

// ---------------------------------------------------------------------------
// Plain (ungated) boxes: no auth header, ever
// ---------------------------------------------------------------------------

#[test]
fn plain_box_gets_no_authorization_header() {
    let _guard = env_lock();
    let store = StoreFile::with(&json!({}));
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/net/command")
            .matches(|req| !has_authorization(req));
        then.status(200).json_body(adc_ok());
    });
    let lager = LagerBox::connect(server.address().to_string()).unwrap();
    assert_eq!(lager.adc("adc1").read().unwrap(), 1.5);
    m.assert();
    // Nothing was learned or written for an ungated box.
    assert_eq!(store.read(), json!({}));
}

// ---------------------------------------------------------------------------
// Pinned tokens (builder / env)
// ---------------------------------------------------------------------------

#[test]
fn builder_bearer_token_is_attached() {
    let _guard = env_lock();
    let _store = StoreFile::with(&json!({}));
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/net/command")
            .header("Authorization", "Bearer my-token");
        then.status(200).json_body(adc_ok());
    });
    let lager = LagerBox::builder(server.address().to_string())
        .bearer_token("my-token")
        .build()
        .unwrap();
    assert_eq!(lager.adc("adc1").read().unwrap(), 1.5);
    m.assert();
}

#[test]
fn env_bearer_token_is_attached() {
    let _guard = env_lock();
    let _store = StoreFile::with(&json!({}));
    let _token = EnvVar::set("LAGER_GATEWAY_TOKEN", "env-token");
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/net/command")
            .header("Authorization", "Bearer env-token");
        then.status(200).json_body(adc_ok());
    });
    let lager = LagerBox::connect(server.address().to_string()).unwrap();
    assert_eq!(lager.adc("adc1").read().unwrap(), 1.5);
    m.assert();
}

// ---------------------------------------------------------------------------
// Gated box + CLI session store
// ---------------------------------------------------------------------------

#[test]
fn first_contact_discovers_gateway_and_retries_with_cli_session() {
    let _guard = env_lock();
    let auth_url = "https://auth.example.com";
    let token = fake_jwt("fresh", 3600);
    // `lager login` happened, but this box has never been contacted: no
    // boxes mapping yet — it must be learned from the discovery header.
    let store = StoreFile::with(&json!({
        "authServers": { auth_url: { "accessToken": token, "cookies": {} } }
    }));

    let server = MockServer::start();
    let denied = server.mock(|when, then| {
        when.method(POST)
            .path("/net/command")
            .matches(|req| !has_authorization(req));
        then.status(401)
            .header("X-Gateway-Auth-Url", auth_url)
            .json_body(json!({"error": "authorization required"}));
    });
    let allowed = server.mock(|when, then| {
        when.method(POST)
            .path("/net/command")
            .header("Authorization", format!("Bearer {token}"));
        then.status(200).json_body(adc_ok());
    });

    let lager = LagerBox::connect(server.address().to_string()).unwrap();
    assert_eq!(lager.adc("adc1").read().unwrap(), 1.5);
    denied.assert();
    allowed.assert();
    // The box→auth-server mapping was recorded, like the CLI does.
    assert_eq!(
        store.read()["boxes"][server.address().ip().to_string()],
        json!(auth_url)
    );

    // Follow-up requests attach the token proactively: no second 401.
    assert_eq!(lager.adc("adc1").read().unwrap(), 1.5);
    denied.assert_hits(1);
    allowed.assert_hits(2);
}

#[test]
fn known_gated_box_attaches_token_proactively() {
    let _guard = env_lock();
    let auth_url = "https://auth.example.com";
    let token = fake_jwt("fresh", 3600);
    let server = MockServer::start();
    // Store already links this box to the auth server (previous run / CLI).
    let _store = StoreFile::with(&json!({
        "boxes": { server.address().ip().to_string(): auth_url },
        "authServers": { auth_url: { "accessToken": token, "cookies": {} } }
    }));
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/net/command")
            .header("Authorization", format!("Bearer {token}"));
        then.status(200).json_body(adc_ok());
    });
    let lager = LagerBox::connect(server.address().to_string()).unwrap();
    assert_eq!(lager.adc("adc1").read().unwrap(), 1.5);
    m.assert_hits(1);
}

#[test]
fn expired_token_is_refreshed_and_rotated_cookies_persisted() {
    let _guard = env_lock();
    let expired = fake_jwt("expired", -3600);
    let fresh = fake_jwt("fresh", 3600);

    // Mock auth server implementing POST /api/auth/refresh.
    let auth_server = MockServer::start();
    let auth_url = auth_server.base_url();
    let refresh = auth_server.mock(|when, then| {
        when.method(POST)
            .path("/api/auth/refresh")
            .header("Cookie", "refreshToken=old-cookie");
        then.status(200)
            .header("Set-Cookie", "refreshToken=rotated-cookie; Path=/; HttpOnly")
            .json_body(json!({"accessToken": fresh}));
    });

    let box_server = MockServer::start();
    let store = StoreFile::with(&json!({
        "boxes": { box_server.address().ip().to_string(): auth_url },
        "authServers": {
            auth_url.clone(): {
                "accessToken": expired,
                "cookies": { "refreshToken": "old-cookie" }
            }
        }
    }));
    let m = box_server.mock(|when, then| {
        when.method(POST)
            .path("/net/command")
            .header("Authorization", format!("Bearer {fresh}"));
        then.status(200).json_body(adc_ok());
    });

    let lager = LagerBox::connect(box_server.address().to_string()).unwrap();
    assert_eq!(lager.adc("adc1").read().unwrap(), 1.5);
    refresh.assert();
    m.assert();

    // The refreshed token and the rotated cookie were written back.
    let entry = &store.read()["authServers"][&auth_url];
    assert_eq!(entry["accessToken"], json!(fresh));
    assert_eq!(entry["cookies"]["refreshToken"], json!("rotated-cookie"));
}

// ---------------------------------------------------------------------------
// Denials that cannot be resolved
// ---------------------------------------------------------------------------

#[test]
fn gated_box_without_login_is_auth_required() {
    let _guard = env_lock();
    let _store = StoreFile::with(&json!({}));
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/net/command");
        then.status(401)
            .header("X-Gateway-Auth-Url", "https://auth.example.com")
            .json_body(json!({"error": "authorization required"}));
    });
    let lager = LagerBox::connect(server.address().to_string()).unwrap();
    let err = lager.adc("adc1").read().unwrap_err();
    match &err {
        Error::AuthRequired { auth_url, .. } => {
            assert_eq!(auth_url, "https://auth.example.com");
        }
        other => panic!("expected AuthRequired, got {other:?}"),
    }
    // The message tells the user exactly how to fix it.
    assert!(err.to_string().contains("lager login https://auth.example.com"));
}

#[test]
fn rejected_pinned_token_is_auth_required_without_retry() {
    let _guard = env_lock();
    let _store = StoreFile::with(&json!({}));
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/net/command");
        then.status(401)
            .header("X-Gateway-Auth-Url", "https://auth.example.com")
            .json_body(json!({"error": "token revoked"}));
    });
    let lager = LagerBox::builder(server.address().to_string())
        .bearer_token("revoked-token")
        .build()
        .unwrap();
    let err = lager.adc("adc1").read().unwrap_err();
    assert!(matches!(err, Error::AuthRequired { .. }));
    assert!(err.to_string().contains("session was rejected"));
    // Pinned tokens are never replaced by store resolution: exactly one try.
    m.assert_hits(1);
}

#[test]
fn forbidden_maps_to_box_error_with_grant_hint() {
    let _guard = env_lock();
    let _store = StoreFile::with(&json!({}));
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/net/command");
        then.status(403)
            .header("X-Gateway-Auth-Url", "https://auth.example.com")
            .json_body(json!({"error": "no grant"}));
    });
    let lager = LagerBox::builder(server.address().to_string())
        .bearer_token("some-token")
        .build()
        .unwrap();
    let err = lager.adc("adc1").read().unwrap_err();
    match &err {
        Error::Box { status: 403, message } => {
            assert!(message.contains("not authorized"), "message: {message}");
        }
        other => panic!("expected Box error, got {other:?}"),
    }
}

#[test]
fn ordinary_application_401_is_not_treated_as_gateway() {
    let _guard = env_lock();
    let _store = StoreFile::with(&json!({}));
    let server = MockServer::start();
    // No discovery header: this is the box itself answering 401.
    let m = server.mock(|when, then| {
        when.method(POST).path("/net/command");
        then.status(401).json_body(json!({"error": "some box-side 401"}));
    });
    let lager = LagerBox::connect(server.address().to_string()).unwrap();
    let err = lager.adc("adc1").read().unwrap_err();
    match err {
        Error::Box { status: 401, message } => assert_eq!(message, "some box-side 401"),
        other => panic!("expected plain Box error, got {other:?}"),
    }
    m.assert_hits(1);
}

// ---------------------------------------------------------------------------
// Async client parity
// ---------------------------------------------------------------------------

#[cfg(feature = "async")]
mod async_parity {
    use super::*;
    use lager::AsyncLagerBox;

    #[test]
    fn async_first_contact_discovers_gateway_and_retries() {
        let _guard = env_lock();
        let auth_url = "https://auth.example.com";
        let token = fake_jwt("fresh", 3600);
        let server = MockServer::start();
        let _store = StoreFile::with(&json!({
            "authServers": { auth_url: { "accessToken": token, "cookies": {} } }
        }));
        let denied = server.mock(|when, then| {
            when.method(POST)
                .path("/net/command")
                .matches(|req| !has_authorization(req));
            then.status(401)
                .header("X-Gateway-Auth-Url", auth_url)
                .json_body(json!({"error": "authorization required"}));
        });
        let allowed = server.mock(|when, then| {
            when.method(POST)
                .path("/net/command")
                .header("Authorization", format!("Bearer {token}"));
            then.status(200).json_body(adc_ok());
        });

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
            assert_eq!(lager.adc("adc1").read().await.unwrap(), 1.5);
        });
        denied.assert();
        allowed.assert();
    }
}
