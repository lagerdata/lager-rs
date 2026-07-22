//! Gateway bearer-token auth for boxes fronted by an authenticating
//! reverse proxy.
//!
//! A plain Lager box needs none of this: no token is attached and no code
//! path here runs unless a box answers with the gateway discovery header.
//! When a box *is* gated, its gateway rejects unauthenticated traffic with
//! 401 + `X-Gateway-Auth-Url: <url>`. This module implements the same
//! contract as the Lager CLI (`lager login` / `cli/gateway_auth.py`); the
//! normative spec is `docs/reference/gateway-auth-contract.md` in the Lager
//! monorepo:
//!
//! - Tokens come from (in order): [`LagerBoxBuilder::bearer_token`],
//!   the `LAGER_GATEWAY_TOKEN` environment variable, or the CLI's token
//!   store (`~/.lager_gateway_auth`, overridable via
//!   `LAGER_GATEWAY_AUTH_FILE`) written by `lager login <url>`.
//! - The box→auth-server mapping is learned from the discovery header and
//!   recorded back into the store, exactly like the CLI, so the very first
//!   denied request is retried with credentials within the same call.
//! - Short-lived access tokens are refreshed transparently against
//!   `POST <url>/api/auth/refresh`, replaying the cookies the auth server
//!   set at login and persisting any rotations.
//!
//! [`LagerBoxBuilder::bearer_token`]: crate::LagerBoxBuilder::bearer_token

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::error::Error;

/// Gateway discovery header carried on 401/403/503 denials.
pub(crate) const DISCOVERY_HEADER: &str = "x-gateway-auth-url";

/// Refresh the access token when it expires within this many seconds
/// (mirrors the CLI's `EXPIRY_MARGIN_SECONDS`).
const EXPIRY_MARGIN_SECONDS: f64 = 60.0;

/// Timeout for requests to the auth server (mirrors the CLI).
pub(crate) const AUTH_SERVER_TIMEOUT: Duration = Duration::from_secs(10);

fn store_path() -> PathBuf {
    if let Ok(path) = std::env::var(crate::GATEWAY_AUTH_FILE_ENV) {
        return PathBuf::from(path);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home).join(".lager_gateway_auth")
}

/// Load the CLI-compatible token store, `{}` when missing/corrupt.
fn load_store() -> Value {
    std::fs::read_to_string(store_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}))
}

/// Persist the store (best-effort; mode 0600 like the CLI).
fn save_store(store: &Value) {
    let path = store_path();
    let Ok(text) = serde_json::to_string_pretty(store) else {
        return;
    };
    if std::fs::write(&path, text).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

// ---------------------------------------------------------------------------
// JWT expiry (unverified, like the CLI — the gateway verifies)
// ---------------------------------------------------------------------------

/// Decode unpadded base64url. Returns `None` on any invalid input.
fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let s = s.trim_end_matches('=').as_bytes();
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.chunks(4) {
        let mut acc: u32 = 0;
        for &c in chunk {
            acc = (acc << 6) | val(c)?;
        }
        let bits = chunk.len() * 6;
        acc <<= 24 - bits;
        out.push((acc >> 16) as u8);
        if chunk.len() > 2 {
            out.push((acc >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(acc as u8);
        }
    }
    Some(out)
}

/// Read `exp` from a JWT without verifying it. 0.0 when unreadable, so an
/// opaque (non-JWT) token is treated as expired and refresh is attempted;
/// if there is nothing to refresh with, the token is still sent as-is.
pub(crate) fn token_expires_at(token: &str) -> f64 {
    let Some(payload_b64) = token.split('.').nth(1) else {
        return 0.0;
    };
    let Some(bytes) = base64url_decode(payload_b64) else {
        return 0.0;
    };
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|payload| payload.get("exp")?.as_f64())
        .unwrap_or(0.0)
}

fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn token_is_fresh(token: &str) -> bool {
    token_expires_at(token) > now_epoch() + EXPIRY_MARGIN_SECONDS
}

// ---------------------------------------------------------------------------
// Cookie plumbing for the refresh call
// ---------------------------------------------------------------------------

/// Build a `Cookie:` header value from the store's cookie map.
fn cookie_header(cookies: &Map<String, Value>) -> String {
    cookies
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|v| format!("{k}={v}")))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Parse one `Set-Cookie:` header value into `(name, value)`.
fn parse_set_cookie(header: &str) -> Option<(String, String)> {
    let pair = header.split(';').next()?.trim();
    let (name, value) = pair.split_once('=')?;
    if name.is_empty() {
        return None;
    }
    Some((name.trim().to_string(), value.trim().to_string()))
}

/// The parts of a refresh response the store needs.
struct RefreshOutcome {
    access_token: String,
    set_cookies: Vec<(String, String)>,
}

fn apply_refresh(store: &mut Value, auth_url: &str, outcome: &RefreshOutcome) {
    let servers = store
        .as_object_mut()
        .map(|o| o.entry("authServers").or_insert_with(|| json!({})))
        .and_then(Value::as_object_mut);
    let Some(servers) = servers else { return };
    let entry = servers
        .entry(auth_url.to_string())
        .or_insert_with(|| json!({}));
    if let Some(entry) = entry.as_object_mut() {
        entry.insert("accessToken".into(), json!(outcome.access_token));
        // Replay-safe: merge rotated cookies over the existing ones.
        let cookies = entry
            .entry("cookies")
            .or_insert_with(|| json!({}));
        if let Some(cookies) = cookies.as_object_mut() {
            for (name, value) in &outcome.set_cookies {
                cookies.insert(name.clone(), json!(value));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Denial classification (mirrors the CLI's handle_gateway_denial)
// ---------------------------------------------------------------------------

/// Turn a gateway denial into an actionable [`Error`]. Only called for
/// responses carrying the discovery header, so plain boxes (and ordinary
/// application 401/403s) are never affected.
pub(crate) fn denial_error(status: u16, box_host: &str, auth_url: &str, sent_token: bool) -> Error {
    match status {
        401 => {
            let message = if sent_token {
                format!("your session was rejected by box {box_host}")
            } else {
                format!("box {box_host} requires sign-in")
            };
            Error::AuthRequired {
                box_host: box_host.to_string(),
                auth_url: auth_url.to_string(),
                message,
            }
        }
        403 => Error::Box {
            status,
            message: format!(
                "you are not authorized to use box {box_host} \
                 (your account has no access grant for it; ask your admin)"
            ),
        },
        // 503 from the gateway: the box could not verify authorization.
        _ => Error::Box {
            status,
            message: format!(
                "box {box_host} could not verify authorization: \
                 its auth server is unreachable. Try again shortly"
            ),
        },
    }
}

/// Should this `(status, discovery-header)` pair be handled as a gateway
/// denial?
pub(crate) fn is_denial(status: u16) -> bool {
    matches!(status, 401 | 403 | 503)
}

// ---------------------------------------------------------------------------
// Per-client auth state
// ---------------------------------------------------------------------------

/// Bearer-token state for one client. Cheap when the box is not gated:
/// a store lookup at construction, then a mutex-guarded `Option` check per
/// request.
pub(crate) struct GatewayAuth {
    /// Hostname of the box (the store's `boxes` key).
    box_host: String,
    /// Token pinned by the builder or `LAGER_GATEWAY_TOKEN`; always
    /// attached verbatim and never refreshed.
    static_token: Option<String>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Auth server for this box, from the store or a discovery header.
    auth_url: Option<String>,
    /// Access token resolved from the store (None until first needed).
    token: Option<String>,
}

impl GatewayAuth {
    /// Build auth state for a box base URL (e.g. `http://host:9000`).
    pub(crate) fn new(base_url: &str, builder_token: Option<String>) -> Self {
        let box_host = host_of(base_url).to_string();
        let static_token = builder_token
            .or_else(|| std::env::var(crate::GATEWAY_TOKEN_ENV).ok())
            .filter(|t| !t.trim().is_empty());
        // A box already known (from a previous run / the CLI) to be gated
        // gets its token attached proactively, like the CLI does.
        let auth_url = load_store()
            .get("boxes")
            .and_then(|b| b.get(&box_host))
            .and_then(Value::as_str)
            .map(str::to_string);
        GatewayAuth {
            box_host,
            static_token,
            state: Mutex::new(State {
                auth_url,
                token: None,
            }),
        }
    }

    pub(crate) fn box_host(&self) -> &str {
        &self.box_host
    }

    /// Record the box→auth-server mapping learned from a discovery header
    /// (memory + best-effort store write, like the CLI).
    pub(crate) fn learn_auth_server(&self, auth_url: &str) {
        let mut state = self.state.lock().unwrap();
        if state.auth_url.as_deref() == Some(auth_url) {
            return;
        }
        state.auth_url = Some(auth_url.to_string());
        drop(state);
        let mut store = load_store();
        if let Some(root) = store.as_object_mut() {
            let boxes = root.entry("boxes").or_insert_with(|| json!({}));
            if let Some(boxes) = boxes.as_object_mut() {
                if boxes.get(&self.box_host).and_then(Value::as_str) != Some(auth_url) {
                    boxes.insert(self.box_host.clone(), json!(auth_url));
                    save_store(&store);
                }
            }
        }
    }

    /// The auth server currently associated with this box, if known.
    pub(crate) fn auth_url(&self) -> Option<String> {
        self.state.lock().unwrap().auth_url.clone()
    }

    /// Token to attach without doing any network I/O: the static token, or
    /// a cached store token that is still fresh. Clears a stale cache.
    pub(crate) fn cached_token(&self) -> Option<String> {
        if let Some(token) = &self.static_token {
            return Some(token.clone());
        }
        let mut state = self.state.lock().unwrap();
        match &state.token {
            Some(token) if token_is_fresh(token) => Some(token.clone()),
            Some(_) => {
                state.token = None;
                None
            }
            None => None,
        }
    }

    /// Whether store-based resolution should run (no static token, and the
    /// box is known to be gated).
    pub(crate) fn wants_store_token(&self) -> bool {
        self.static_token.is_none() && self.state.lock().unwrap().auth_url.is_some()
    }

    /// True when tokens are pinned (builder/env) and must not be replaced
    /// by store resolution.
    pub(crate) fn has_static_token(&self) -> bool {
        self.static_token.is_some()
    }

    /// Cache a token resolved from the store.
    pub(crate) fn cache_token(&self, token: &str) {
        if self.static_token.is_none() {
            self.state.lock().unwrap().token = Some(token.to_string());
        }
    }

    /// The stored entry for an auth server: `(accessToken, cookies)`.
    fn store_entry(&self, auth_url: &str) -> (Option<String>, Map<String, Value>) {
        let store = load_store();
        let entry = store
            .get("authServers")
            .and_then(|s| s.get(auth_url));
        let token = entry
            .and_then(|e| e.get("accessToken"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let cookies = entry
            .and_then(|e| e.get("cookies"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        (token, cookies)
    }
}

fn host_of(base_url: &str) -> &str {
    let rest = base_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(base_url);
    let rest = rest.split('/').next().unwrap_or(rest);
    rest.rsplit_once(':').map(|(host, _)| host).unwrap_or(rest)
}

// ---------------------------------------------------------------------------
// Token resolution (store lookup + transparent refresh) per transport
// ---------------------------------------------------------------------------

/// Decide whether the stored token can be used directly, and with what
/// cookies a refresh should be attempted otherwise. `avoid` is a token that
/// was just rejected by the gateway and must not be returned again.
fn usable_or_refresh(
    stored: Option<String>,
    avoid: Option<&str>,
) -> (Option<String>, bool) {
    match stored {
        Some(token) if Some(token.as_str()) != avoid && token_is_fresh(&token) => {
            (Some(token), false)
        }
        _ => (None, true),
    }
}

#[cfg(feature = "blocking")]
impl GatewayAuth {
    /// Resolve a usable access token for `auth_url` from the store,
    /// refreshing over HTTP when needed (blocking transport). `avoid` is a
    /// token the gateway just rejected.
    pub(crate) fn resolve_token_blocking(
        &self,
        auth_url: &str,
        avoid: Option<&str>,
    ) -> Option<String> {
        let (stored, cookies) = self.store_entry(auth_url);
        let (usable, needs_refresh) = usable_or_refresh(stored, avoid);
        if let Some(token) = usable {
            self.cache_token(&token);
            return Some(token);
        }
        if !needs_refresh || cookies.is_empty() {
            return None;
        }
        let outcome = refresh_blocking(auth_url, &cookies)?;
        let mut store = load_store();
        apply_refresh(&mut store, auth_url, &outcome);
        save_store(&store);
        self.cache_token(&outcome.access_token);
        Some(outcome.access_token)
    }
}

#[cfg(feature = "blocking")]
fn refresh_blocking(auth_url: &str, cookies: &Map<String, Value>) -> Option<RefreshOutcome> {
    let req = ureq::post(&format!("{auth_url}/api/auth/refresh"))
        .timeout(AUTH_SERVER_TIMEOUT)
        .set("Cookie", &cookie_header(cookies));
    let resp = match req.call() {
        Ok(resp) => resp,
        Err(_) => return None,
    };
    let set_cookies = resp
        .all("set-cookie")
        .into_iter()
        .filter_map(parse_set_cookie)
        .collect();
    let body: Value = resp.into_json().ok()?;
    let access_token = body.get("accessToken")?.as_str()?.to_string();
    Some(RefreshOutcome {
        access_token,
        set_cookies,
    })
}

#[cfg(feature = "async")]
impl GatewayAuth {
    /// Async twin of [`GatewayAuth::resolve_token_blocking`].
    pub(crate) async fn resolve_token_async(
        &self,
        auth_url: &str,
        avoid: Option<&str>,
    ) -> Option<String> {
        let (stored, cookies) = self.store_entry(auth_url);
        let (usable, needs_refresh) = usable_or_refresh(stored, avoid);
        if let Some(token) = usable {
            self.cache_token(&token);
            return Some(token);
        }
        if !needs_refresh || cookies.is_empty() {
            return None;
        }
        let outcome = refresh_async(auth_url, &cookies).await?;
        let mut store = load_store();
        apply_refresh(&mut store, auth_url, &outcome);
        save_store(&store);
        self.cache_token(&outcome.access_token);
        Some(outcome.access_token)
    }
}

#[cfg(feature = "async")]
async fn refresh_async(auth_url: &str, cookies: &Map<String, Value>) -> Option<RefreshOutcome> {
    let resp = reqwest::Client::new()
        .post(format!("{auth_url}/api/auth/refresh"))
        .timeout(AUTH_SERVER_TIMEOUT)
        .header("Cookie", cookie_header(cookies))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let set_cookies = resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(parse_set_cookie)
        .collect();
    let body: Value = resp.json().await.ok()?;
    let access_token = body.get("accessToken")?.as_str()?.to_string();
    Some(RefreshOutcome {
        access_token,
        set_cookies,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Unsigned JWT with the given `exp` (the gateway verifies, we only
    /// read the expiry).
    pub(crate) fn fake_jwt(exp: f64) -> String {
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
        let header = b64url(br#"{"alg":"none"}"#);
        let payload = b64url(format!(r#"{{"exp":{exp}}}"#).as_bytes());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn jwt_expiry_roundtrip() {
        let exp = now_epoch() + 3600.0;
        let token = fake_jwt(exp);
        assert!((token_expires_at(&token) - exp).abs() < 1.0);
        assert!(token_is_fresh(&token));
        assert!(!token_is_fresh(&fake_jwt(now_epoch() - 10.0)));
        // Opaque tokens read as expired (refresh attempted, then sent as-is
        // if there is nothing to refresh with).
        assert_eq!(token_expires_at("not-a-jwt"), 0.0);
    }

    #[test]
    fn base64url_decodes_unpadded() {
        assert_eq!(base64url_decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(base64url_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64url_decode("").unwrap(), b"");
        assert!(base64url_decode("a").is_none());
        assert!(base64url_decode("!!!!").is_none());
    }

    #[test]
    fn cookie_header_and_set_cookie() {
        let mut cookies = Map::new();
        cookies.insert("refreshToken".into(), json!("abc"));
        assert_eq!(cookie_header(&cookies), "refreshToken=abc");
        assert_eq!(
            parse_set_cookie("refreshToken=xyz; Path=/; HttpOnly").unwrap(),
            ("refreshToken".into(), "xyz".into())
        );
        assert!(parse_set_cookie("nonsense").is_none());
    }

    #[test]
    fn host_extraction() {
        assert_eq!(host_of("http://192.168.1.42:9000"), "192.168.1.42");
        assert_eq!(host_of("http://box.tailnet.ts.net:9000"), "box.tailnet.ts.net");
        assert_eq!(host_of("box:9000"), "box");
    }

    #[test]
    fn refresh_merges_rotated_cookies_into_store() {
        let mut store = json!({
            "authServers": {
                "https://auth": {"accessToken": "old", "cookies": {"a": "1", "b": "2"}}
            }
        });
        apply_refresh(
            &mut store,
            "https://auth",
            &RefreshOutcome {
                access_token: "new".into(),
                set_cookies: vec![("b".into(), "rotated".into())],
            },
        );
        let entry = &store["authServers"]["https://auth"];
        assert_eq!(entry["accessToken"], "new");
        assert_eq!(entry["cookies"]["a"], "1");
        assert_eq!(entry["cookies"]["b"], "rotated");
    }

    #[test]
    fn denial_messages() {
        let e = denial_error(401, "box1", "https://auth", false);
        assert!(matches!(e, Error::AuthRequired { .. }));
        assert!(e.to_string().contains("lager login https://auth"));
        let e = denial_error(403, "box1", "https://auth", true);
        assert!(matches!(e, Error::Box { status: 403, .. }));
        let e = denial_error(503, "box1", "https://auth", true);
        assert!(matches!(e, Error::Box { status: 503, .. }));
    }
}
