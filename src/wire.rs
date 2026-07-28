//! Sans-io wire layer: request builders and response parsers for the Lager
//! box HTTP API on port 9000.
//!
//! Everything in this module is pure data-in/data-out. Both the blocking
//! client ([`crate::LagerBox`]) and the async client run the exact same
//! builders and parsers, so the two transports cannot drift apart.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::error::{Error, Result};

/// Default port of the box HTTP server (`box_http_server.py`).
pub const DEFAULT_PORT: u16 = 9000;

/// Port of the box debug service (`lager.debug.service`), published on the
/// box host. Debug nets talk here rather than to the port-9000 server.
pub const DEBUG_SERVICE_PORT: u16 = 8765;

/// Default HTTP timeout for quick net commands, matching the Lager CLI's
/// 10-second budget.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// HTTP method of a request. The box API only uses GET and POST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// HTTP GET.
    Get,
    /// HTTP POST with a JSON body.
    Post,
}

/// Client-side timeout policy for one request.
///
/// Mirrors the Lager CLI's budgets: quick commands get the 10s default,
/// while actions that block on the box for a caller-controlled duration
/// (watt/energy integration windows, `wait_for_level`) widen or drop the
/// client timeout so a healthy request is never aborted mid-measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timeout {
    /// Use the client's configured default (10s unless overridden).
    Default,
    /// Use this specific timeout.
    After(Duration),
    /// No client-side timeout at all (e.g. an unbounded `wait_for_level`).
    Unbounded,
}

/// One fully-described HTTP request to the box, transport-agnostic.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    /// HTTP method.
    pub method: Method,
    /// Path relative to the box base URL, e.g. `"/net/command"`.
    pub path: String,
    /// JSON body for POST requests.
    pub body: Option<Value>,
    /// Client-side timeout policy.
    pub timeout: Timeout,
}

/// A complete operation: the request to send plus the parser that turns the
/// box's response envelope into a typed value. Net handles build these; the
/// sync/async clients only execute them.
pub struct Op<T> {
    /// The request to send.
    pub req: HttpRequest,
    /// Parser applied to the successful response envelope.
    pub parse: fn(CommandResponse) -> Result<T>,
}

// ---------------------------------------------------------------------------
// Response envelope
// ---------------------------------------------------------------------------

/// The common response envelope every box command endpoint returns:
/// `{"success": bool, "action": ..., "message": ..., "value": ..., "state": ...}`.
#[derive(Debug, Clone, Deserialize)]
pub struct CommandResponse {
    /// Whether the box executed the command.
    #[serde(default)]
    pub success: bool,
    /// Echo of the action that ran.
    #[serde(default)]
    pub action: Option<String>,
    /// Human-readable result message (what the CLI prints).
    #[serde(default)]
    pub message: Option<String>,
    /// Structured result value, shape depends on the action.
    #[serde(default)]
    pub value: Option<Value>,
    /// Structured state object (supply/battery `state`, usb port state).
    #[serde(default)]
    pub state: Option<Value>,
    /// Error message when `success` is false.
    #[serde(default)]
    pub error: Option<String>,
    /// Any endpoint-specific extra fields (e.g. SPI's top-level `word_size`).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Interpret a raw `(status, body)` pair as the command envelope, applying
/// the box's error conventions:
///
/// - 200 + `success: true` is the only success shape.
/// - 501 means the box image predates the endpoint ([`Error::UnsupportedByBox`]).
/// - Everything else (including `success: false` with HTTP 200, which the box
///   uses for cross-role instrument conflicts) is [`Error::Box`].
pub fn parse_command(status: u16, body: Value) -> Result<CommandResponse> {
    let resp: CommandResponse = serde_json::from_value(body)
        .map_err(|e| Error::Decode(format!("invalid response envelope: {e}")))?;
    if status == 200 && resp.success {
        return Ok(resp);
    }
    let message = resp
        .error
        .or(resp.message)
        .unwrap_or_else(|| format!("HTTP {status}"));
    if status == 501 {
        return Err(Error::UnsupportedByBox { message });
    }
    Err(Error::Box { status, message })
}

// ---------------------------------------------------------------------------
// Generic request builders
// ---------------------------------------------------------------------------

/// Build a `POST /net/command` request (gpio, adc, dac, thermocouple,
/// watt-meter, eload, spi, i2c, energy-analyzer).
///
/// The `role` is sent as a hint; the box resolves the authoritative role from
/// its `saved_nets.json` and verifies the hint, so a typo'd net name or a
/// role mismatch fails loudly instead of driving the wrong instrument.
///
/// `role: None` omits the hint entirely and defers fully to the box's
/// resolution. Used for roles with saved-record aliases (router nets may be
/// saved as `"router"` or the legacy `"mikrotik"`; the box dispatches both
/// to the same handler, but a hint must match the saved string exactly).
pub fn net_command(
    netname: &str,
    role: impl Into<Option<&'static str>>,
    action: &str,
    params: Value,
    timeout: Timeout,
) -> HttpRequest {
    let mut body = json!({
        "netname": netname,
        "action": action,
        "params": params,
    });
    if let Some(role) = role.into() {
        body["role"] = json!(role);
    }
    HttpRequest {
        method: Method::Post,
        path: "/net/command".to_string(),
        body: Some(body),
        timeout,
    }
}

/// Build a `POST /supply/command` request.
pub fn supply_command(netname: &str, action: &str, params: Value) -> HttpRequest {
    HttpRequest {
        method: Method::Post,
        path: "/supply/command".to_string(),
        body: Some(json!({
            "netname": netname,
            "action": action,
            "params": params,
        })),
        timeout: Timeout::Default,
    }
}

/// Build a `POST /battery/command` request.
pub fn battery_command(netname: &str, action: &str, params: Value) -> HttpRequest {
    HttpRequest {
        method: Method::Post,
        path: "/battery/command".to_string(),
        body: Some(json!({
            "netname": netname,
            "action": action,
            "params": params,
        })),
        timeout: Timeout::Default,
    }
}

/// Build a `POST /usb/command` request (no `params` object on this endpoint).
pub fn usb_command(netname: &str, action: &str) -> HttpRequest {
    HttpRequest {
        method: Method::Post,
        path: "/usb/command".to_string(),
        body: Some(json!({
            "netname": netname,
            "action": action,
        })),
        timeout: Timeout::Default,
    }
}

/// Build a box-level command request (`POST /ble/command`, `/wifi/command`,
/// `/blufi/command`). These endpoints drive the box's own hardware (its
/// Bluetooth adapter or wlan interface) rather than a saved net, so the body
/// carries only `{action, params}` — no `netname`.
pub fn box_command(path: &str, action: &str, params: Value, timeout: Timeout) -> HttpRequest {
    HttpRequest {
        method: Method::Post,
        path: path.to_string(),
        body: Some(json!({
            "action": action,
            "params": params,
        })),
        timeout,
    }
}

/// Build a plain GET request (discovery/health endpoints).
pub fn get(path: &str) -> HttpRequest {
    HttpRequest {
        method: Method::Get,
        path: path.to_string(),
        body: None,
        timeout: Timeout::Default,
    }
}

/// Normalize a user-supplied host string into a base URL.
///
/// Accepts a bare host/IP (`"192.168.1.42"`, `"mybox.tailnet.ts.net"`), a
/// `host:port` pair, or a full URL. The scheme defaults to `http` and the
/// port to [`DEFAULT_PORT`] (9000).
pub(crate) fn base_url(host: &str) -> Result<String> {
    base_url_with_port(host, DEFAULT_PORT)
}

/// Like [`base_url`] but uses `default_port` when the host has no explicit
/// port. Used for the debug-service URL, which defaults to 8765.
pub(crate) fn base_url_with_port(host: &str, default_port: u16) -> Result<String> {
    let input = host.trim();
    let (scheme, rest) = match input.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        None => ("http", input),
    };
    let rest = rest.trim_end_matches('/');
    if scheme.is_empty() || rest.is_empty() {
        return Err(Error::Config(format!("invalid box host '{host}'")));
    }
    if rest.contains(':') {
        Ok(format!("{scheme}://{rest}"))
    } else {
        Ok(format!("{scheme}://{rest}:{default_port}"))
    }
}

/// Derive a sibling service base URL on the same host but a different port,
/// e.g. turn `http://192.168.1.42:9000` into `http://192.168.1.42:8765` for
/// the debug service. `base` is expected to be normalized by [`base_url`].
pub(crate) fn service_base(base: &str, port: u16) -> String {
    let (scheme, rest) = base.split_once("://").unwrap_or(("http", base));
    let host = rest.rsplit_once(':').map(|(h, _)| h).unwrap_or(rest);
    format!("{scheme}://{host}:{port}")
}

// ---------------------------------------------------------------------------
// Debug service (:8765) — separate protocol from the :9000 command envelope
// ---------------------------------------------------------------------------

/// Build a `POST /debug/<op>` request for the debug service.
pub fn debug_request(path: &str, body: Value, timeout: Timeout) -> HttpRequest {
    HttpRequest {
        method: Method::Post,
        path: path.to_string(),
        body: Some(body),
        timeout,
    }
}

/// Interpret a debug-service `(status, body)` pair. The debug service does
/// not use the `success` envelope: 200 is success, and errors arrive as
/// `{"error": msg, "status": "error"}` with a 4xx/5xx code.
pub fn parse_debug(status: u16, body: Value) -> Result<Value> {
    if status == 200 {
        return Ok(body);
    }
    let message = body
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| body.get("message").and_then(Value::as_str))
        .unwrap_or("debug service request failed")
        .to_string();
    Err(Error::Box { status, message })
}

/// GDB-server sub-object of a [`DebugConnection`].
#[derive(Debug, Clone, Deserialize)]
pub struct GdbServer {
    /// Server state, e.g. `"started"` or `"already_running"`.
    #[serde(default)]
    pub status: Option<String>,
    /// GDB protocol port.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub gdb_port: Option<i64>,
    /// SWO output port (J-Link only).
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub swo_port: Option<i64>,
    /// Telnet I/O port.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub telnet_port: Option<i64>,
    /// TCL/RPC port (OpenOCD only).
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub tcl_port: Option<i64>,
    /// RTT telnet port.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub rtt_telnet_port: Option<i64>,
    /// Server process id.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub pid: Option<i64>,
}

/// Result of a debug `connect`.
#[derive(Debug, Clone, Deserialize)]
pub struct DebugConnection {
    /// Connection status, e.g. `"connected"`.
    #[serde(default)]
    pub status: Option<String>,
    /// Resolved device/target type.
    #[serde(default)]
    pub device: Option<String>,
    /// Probe instrument name.
    #[serde(default)]
    pub probe: Option<String>,
    /// Probe serial.
    #[serde(default)]
    pub serial: Option<String>,
    /// Debug backend that started (`"jlink"` or `"openocd"`).
    #[serde(default)]
    pub backend: Option<String>,
    /// Human-readable message.
    #[serde(default)]
    pub message: Option<String>,
    /// Backend process id.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub pid: Option<i64>,
    /// GDB-server details, if a server was started.
    #[serde(default)]
    pub gdb_server: Option<GdbServer>,
}

/// Result of a debug `info` query.
#[derive(Debug, Clone, Deserialize)]
pub struct DebugInfo {
    /// Net name.
    #[serde(default)]
    pub net_name: Option<String>,
    /// Resolved device/target type.
    #[serde(default)]
    pub device: Option<String>,
    /// Target architecture.
    #[serde(default)]
    pub arch: Option<String>,
    /// Probe instrument name.
    #[serde(default)]
    pub probe: Option<String>,
    /// Probe serial.
    #[serde(default)]
    pub serial: Option<String>,
    /// Debug backend (`"jlink"` or `"openocd"`).
    #[serde(default)]
    pub backend: Option<String>,
    /// Whether a gdbserver/daemon is currently running for this probe.
    #[serde(default)]
    pub connected: bool,
}

/// Result of a debug `status` query.
#[derive(Debug, Clone, Deserialize)]
pub struct DebugStatus {
    /// Whether a gdbserver/daemon is currently running for this probe.
    #[serde(default)]
    pub connected: bool,
    /// Backend process id, when connected.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub pid: Option<i64>,
    /// Probe serial.
    #[serde(default)]
    pub serial: Option<String>,
    /// Debug backend (`"jlink"` or `"openocd"`).
    #[serde(default)]
    pub backend: Option<String>,
}

/// Extract the hex `data` field from a `/debug/memrd` response into bytes.
pub fn debug_memory_bytes(body: &Value) -> Result<Vec<u8>> {
    let hex = body
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Decode("memrd response missing 'data'".to_string()))?;
    decode_hex(hex)
}

/// Decode a hex string into bytes.
pub(crate) fn decode_hex(s: &str) -> Result<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return Err(Error::Decode("odd-length hex string".to_string()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| Error::Decode(format!("invalid hex byte '{}'", &s[i..i + 2])))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Lenient deserialization helpers
// ---------------------------------------------------------------------------
//
// Instrument drivers occasionally hand numbers back as strings (SCPI query
// results). These helpers accept a JSON number, a numeric string, or null.

pub(crate) fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

pub(crate) fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s
            .trim()
            .parse::<i64>()
            .ok()
            .or_else(|| s.trim().parse::<f64>().ok().map(|f| f as i64)),
        Value::Bool(b) => Some(i64::from(*b)),
        _ => None,
    }
}

pub(crate) fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|f| f != 0.0),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "on" | "1" | "yes" | "enabled" => Some(true),
            "false" | "off" | "0" | "no" | "disabled" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) mod lenient {
    //! `deserialize_with` adapters for fields that may arrive as numbers,
    //! numeric strings, or null.

    use serde::{Deserialize, Deserializer};
    use serde_json::Value;

    pub fn opt_f64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
        let v = Option::<Value>::deserialize(d)?;
        Ok(v.as_ref().and_then(super::as_f64))
    }

    pub fn opt_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
        let v = Option::<Value>::deserialize(d)?;
        Ok(v.as_ref().and_then(super::as_i64))
    }

    pub fn opt_bool<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
        let v = Option::<Value>::deserialize(d)?;
        Ok(v.as_ref().and_then(super::as_bool))
    }
}

// ---------------------------------------------------------------------------
// Typed extraction from the envelope
// ---------------------------------------------------------------------------

/// Extract the `value` field as an `f64` (accepting numeric strings).
pub fn value_f64(resp: CommandResponse) -> Result<f64> {
    resp.value
        .as_ref()
        .and_then(as_f64)
        .ok_or_else(|| Error::Decode(format!("expected numeric 'value', got {:?}", resp.value)))
}

/// Extract the `value` field as an `i64` (accepting numeric strings).
pub fn value_i64(resp: CommandResponse) -> Result<i64> {
    resp.value
        .as_ref()
        .and_then(as_i64)
        .ok_or_else(|| Error::Decode(format!("expected integer 'value', got {:?}", resp.value)))
}

/// Extract the `value` field as a list of integers (I2C bytes/addresses,
/// SPI words).
pub fn value_int_list(resp: CommandResponse) -> Result<Vec<u32>> {
    let arr = resp
        .value
        .as_ref()
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Decode(format!("expected list 'value', got {:?}", resp.value)))?;
    arr.iter()
        .map(|v| {
            as_i64(v)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| Error::Decode(format!("non-integer element in 'value': {v:?}")))
        })
        .collect()
}

/// Deserialize the `value` field into a typed struct.
pub fn value_as<T: DeserializeOwned>(resp: CommandResponse) -> Result<T> {
    let v = resp
        .value
        .ok_or_else(|| Error::Decode("response has no 'value' field".to_string()))?;
    serde_json::from_value(v).map_err(|e| Error::Decode(format!("invalid 'value' shape: {e}")))
}

/// Deserialize the `state` field into a typed struct.
pub fn state_as<T: DeserializeOwned>(resp: CommandResponse) -> Result<T> {
    let v = resp
        .state
        .ok_or_else(|| Error::Decode("response has no 'state' field".to_string()))?;
    serde_json::from_value(v).map_err(|e| Error::Decode(format!("invalid 'state' shape: {e}")))
}

/// Interpret a nets-list body into raw JSON values. `/nets/list` returns a
/// bare array; the older `/uart/nets/list` wraps it in `{"nets": [...]}`.
pub(crate) fn nets_list_values(body: Value) -> Vec<Value> {
    match body {
        Value::Array(a) => a,
        Value::Object(mut o) => match o.remove("nets") {
            Some(Value::Array(a)) => a,
            _ => vec![],
        },
        _ => vec![],
    }
}

/// Interpret a nets-list body into typed [`NetRecord`]s.
pub(crate) fn nets_from_body(body: Value) -> Result<Vec<NetRecord>> {
    let list = Value::Array(nets_list_values(body));
    serde_json::from_value(list).map_err(|e| Error::Decode(format!("invalid nets list: {e}")))
}

/// Discard the payload, keeping only success/failure.
pub fn unit(_resp: CommandResponse) -> Result<()> {
    Ok(())
}

/// Keep the whole envelope (for callers that want `message` etc.).
pub fn envelope(resp: CommandResponse) -> Result<CommandResponse> {
    Ok(resp)
}

// ---------------------------------------------------------------------------
// Shared response types
// ---------------------------------------------------------------------------

/// One saved-net record from `GET /nets/list` (the box's `saved_nets.json`).
#[derive(Debug, Clone, Deserialize)]
pub struct NetRecord {
    /// Net name, e.g. `"supply1"`.
    #[serde(default)]
    pub name: String,
    /// Role string, e.g. `"power-supply"`, `"gpio"`, `"adc"`.
    #[serde(default)]
    pub role: String,
    /// Instrument backing this net, e.g. `"Rigol DP832"`, `"LabJack T7"`.
    #[serde(default)]
    pub instrument: Option<String>,
    /// Pin / channel identifier (number or string depending on role).
    #[serde(default)]
    pub pin: Option<Value>,
    /// Channel (multi-channel instruments).
    #[serde(default)]
    pub channel: Option<Value>,
    /// VISA or device address.
    #[serde(default)]
    pub address: Option<String>,
    /// Role-specific saved parameters (SPI mode, UART baudrate, ...).
    #[serde(default)]
    pub params: Option<Map<String, Value>>,
    /// Everything else in the record (mappings, scope_points, ...).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Response of `GET /health`.
#[derive(Debug, Clone, Deserialize)]
pub struct Health {
    /// `"healthy"` when the box HTTP server is up.
    #[serde(default)]
    pub status: String,
    /// Service identifier.
    #[serde(default)]
    pub service: Option<String>,
    /// Service version string.
    #[serde(default)]
    pub version: Option<String>,
}

/// One net summary inside [`BoxStatus`].
#[derive(Debug, Clone, Deserialize)]
pub struct NetSummary {
    /// Net name.
    #[serde(default)]
    pub name: String,
    /// Net type name (e.g. `"PowerSupply"`, `"GPIO"`).
    #[serde(default, rename = "type")]
    pub net_type: String,
}

/// Capabilities advertised by the box in `GET /status`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BoxCapabilities {
    /// Whether the box serves `POST /net/command` (Tier-1 nets over HTTP).
    /// When false the box software predates this crate's contract.
    #[serde(default, rename = "netCommand")]
    pub net_command: bool,
    /// Roles served by `POST /net/command` on this box. Empty on box images
    /// that predate role advertising (which still serve the original Tier-1
    /// roles when `net_command` is true).
    #[serde(default, rename = "netCommandRoles")]
    pub net_command_roles: Vec<String>,
    /// Whether the box serves `POST /ble/command`.
    #[serde(default, rename = "bleCommand")]
    pub ble_command: bool,
    /// Whether the box serves `POST /wifi/command`.
    #[serde(default, rename = "wifiCommand")]
    pub wifi_command: bool,
    /// Whether the box serves `POST /blufi/command`.
    #[serde(default, rename = "blufiCommand")]
    pub blufi_command: bool,
    /// Whether the box serves the `/custom-devices/*` endpoints (the
    /// `lager nets assign` backend). Not used by this crate; surfaced for
    /// callers probing box features.
    #[serde(default, rename = "customDevices")]
    pub custom_devices: bool,
    /// Whether the box serves `/binaries/*` and `/download-file`. Not used
    /// by this crate; surfaced for callers probing box features.
    #[serde(default)]
    pub binaries: bool,
}

/// Response of `GET /status`.
#[derive(Debug, Clone, Deserialize)]
pub struct BoxStatus {
    /// Whether the box reports itself healthy.
    #[serde(default)]
    pub healthy: bool,
    /// Box software version.
    #[serde(default)]
    pub version: String,
    /// Configured nets (name + type only; use `nets()` for full records).
    #[serde(default)]
    pub nets: Vec<NetSummary>,
    /// Endpoint capabilities.
    #[serde(default)]
    pub capabilities: BoxCapabilities,
}

/// Structured power-supply state from the `state` action on
/// `POST /supply/command`. Fields are `None` when the individual read failed
/// (the box returns a full-shaped dict even on a degraded read; `error`
/// carries the reason).
#[derive(Debug, Clone, Deserialize)]
pub struct SupplyState {
    /// Net name echo.
    #[serde(default)]
    pub netname: Option<String>,
    /// Instrument channel driving this net.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub channel: Option<i64>,
    /// Transport-level error when the whole state gather failed.
    #[serde(default)]
    pub error: Option<String>,
    /// Measured output voltage (V).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub voltage: Option<f64>,
    /// Measured output current (A).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub current: Option<f64>,
    /// Measured output power (W).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub power: Option<f64>,
    /// Whether the output is enabled.
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub enabled: Option<bool>,
    /// Operating mode (e.g. `"CV"`, `"CC"`).
    #[serde(default)]
    pub mode: Option<String>,
    /// Voltage setpoint (V).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub voltage_set: Option<f64>,
    /// Current limit setpoint (A).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub current_set: Option<f64>,
    /// Hardware maximum voltage (V).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub voltage_max: Option<f64>,
    /// Hardware maximum current (A).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub current_max: Option<f64>,
    /// Over-current protection limit (A).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub ocp_limit: Option<f64>,
    /// Whether OCP has tripped.
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub ocp_tripped: Option<bool>,
    /// Over-voltage protection limit (V).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub ovp_limit: Option<f64>,
    /// Whether OVP has tripped.
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub ovp_tripped: Option<bool>,
}

/// Structured battery-simulator state from the `state` action on
/// `POST /battery/command`. Fields are `None` when the individual read
/// failed.
#[derive(Debug, Clone, Deserialize)]
pub struct BatteryState {
    /// Net name echo.
    #[serde(default)]
    pub netname: Option<String>,
    /// Instrument channel driving this net.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub channel: Option<i64>,
    /// Transport-level error when the whole state gather failed.
    #[serde(default)]
    pub error: Option<String>,
    /// Simulated terminal voltage (V).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub terminal_voltage: Option<f64>,
    /// Measured current (A).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub current: Option<f64>,
    /// Equivalent series resistance (ohm).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub esr: Option<f64>,
    /// State of charge (%).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub soc: Option<f64>,
    /// Open-circuit voltage (V).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub voc: Option<f64>,
    /// Whether the simulator output is enabled.
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub enabled: Option<bool>,
    /// Simulation mode (`"static"` / `"dynamic"`).
    #[serde(default)]
    pub mode: Option<String>,
    /// Battery model / part number.
    #[serde(default)]
    pub model: Option<String>,
    /// Battery capacity (Ah).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub capacity: Option<f64>,
    /// Current limit (A).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub current_limit: Option<f64>,
    /// Over-current protection limit (A).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub ocp_limit: Option<f64>,
    /// Over-voltage protection limit (V).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub ovp_limit: Option<f64>,
    /// Voltage considered "full" (V).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub volt_full: Option<f64>,
    /// Voltage considered "empty" (V).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub volt_empty: Option<f64>,
    /// Whether OCP has tripped.
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub ocp_tripped: Option<bool>,
    /// Whether OVP has tripped.
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub ovp_tripped: Option<bool>,
}

/// Structured e-load state from the `state` action on `POST /net/command`.
#[derive(Debug, Clone, Deserialize)]
pub struct EloadState {
    /// Active mode (`"cc"`, `"cv"`, `"cr"`, `"cp"`).
    #[serde(default)]
    pub mode: Option<String>,
    /// Whether the load input is enabled.
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub input_enabled: Option<bool>,
    /// Measured voltage (V).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub measured_voltage: Option<f64>,
    /// Measured current (A).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub measured_current: Option<f64>,
    /// Measured power (W).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub measured_power: Option<f64>,
    /// Any extra driver-specific fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Combined current/voltage/power reading from a watt-meter `all` action.
#[derive(Debug, Clone, Deserialize)]
pub struct WattReading {
    /// Mean current over the window (A).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub current: Option<f64>,
    /// Mean voltage over the window (V).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub voltage: Option<f64>,
    /// Mean power over the window (W).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub power: Option<f64>,
    /// Measurement window (s).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub duration_s: Option<f64>,
}

/// Integrated energy reading from an energy-analyzer `read_energy` action.
#[derive(Debug, Clone, Deserialize)]
pub struct EnergyReading {
    /// Integrated energy (J).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub energy_j: Option<f64>,
    /// Integrated charge (C).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub charge_c: Option<f64>,
    /// Actual integration window (s).
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub duration_s: Option<f64>,
    /// Any extra analyzer-specific fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Statistical summary of one signal inside [`EnergyStats`].
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StatSummary {
    /// Mean value.
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub mean: Option<f64>,
    /// Minimum value.
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub min: Option<f64>,
    /// Maximum value.
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub max: Option<f64>,
    /// Standard deviation.
    #[serde(default, deserialize_with = "lenient::opt_f64")]
    pub std: Option<f64>,
    /// Any extra analyzer-specific fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Statistics from an energy-analyzer `read_stats` action.
#[derive(Debug, Clone, Deserialize)]
pub struct EnergyStats {
    /// Current statistics (A).
    #[serde(default)]
    pub current: Option<StatSummary>,
    /// Voltage statistics (V).
    #[serde(default)]
    pub voltage: Option<StatSummary>,
    /// Power statistics (W).
    #[serde(default)]
    pub power: Option<StatSummary>,
    /// Any extra analyzer-specific fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

// ---------------------------------------------------------------------------
// Arm / webcam / router / BLE / WiFi / BluFi response types
// ---------------------------------------------------------------------------

/// Cartesian position of a robot arm's end effector (mm), from the `value:
/// [x, y, z]` list the arm actions return.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArmPosition {
    /// X coordinate (mm).
    pub x: f64,
    /// Y coordinate (mm).
    pub y: f64,
    /// Z coordinate (mm).
    pub z: f64,
}

/// Parse an arm response's `value: [x, y, z]` into an [`ArmPosition`].
pub fn value_arm_position(resp: CommandResponse) -> Result<ArmPosition> {
    let arr = resp
        .value
        .as_ref()
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Decode(format!("expected [x, y, z] 'value', got {:?}", resp.value)))?;
    match arr.as_slice() {
        [x, y, z] => match (as_f64(x), as_f64(y), as_f64(z)) {
            (Some(x), Some(y), Some(z)) => Ok(ArmPosition { x, y, z }),
            _ => Err(Error::Decode(format!("non-numeric arm position: {arr:?}"))),
        },
        _ => Err(Error::Decode(format!(
            "expected 3-element arm position, got {} elements",
            arr.len()
        ))),
    }
}

/// Result of starting a webcam stream (`start` action).
#[derive(Debug, Clone, Deserialize)]
pub struct WebcamStream {
    /// MJPEG stream URL viewers should open.
    #[serde(default)]
    pub url: String,
    /// TCP port the stream is served on.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub port: Option<i64>,
    /// `true` when a stream was already up and the box reused it.
    #[serde(default)]
    pub already_running: bool,
}

/// Current state of a webcam stream (`status`/`url` actions).
#[derive(Debug, Clone, Deserialize)]
pub struct WebcamStatus {
    /// Whether a stream is currently running for this net.
    #[serde(default)]
    pub running: bool,
    /// Stream URL, when running.
    #[serde(default)]
    pub url: Option<String>,
    /// Stream TCP port, when running.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub port: Option<i64>,
    /// Backing video device (e.g. `/dev/video0`), when running.
    #[serde(default)]
    pub video_device: Option<String>,
}

/// System information for a router net (`system_info` action).
#[derive(Debug, Clone, Deserialize)]
pub struct RouterSystemInfo {
    /// Router identity name.
    #[serde(default)]
    pub name: Option<String>,
    /// RouterOS version.
    #[serde(default)]
    pub version: Option<String>,
    /// Board model, e.g. `"hAP ac^2"`.
    #[serde(default)]
    pub board: Option<String>,
    /// CPU architecture.
    #[serde(default)]
    pub architecture: Option<String>,
    /// Uptime string, e.g. `"1w2d3h"`.
    #[serde(default)]
    pub uptime: Option<String>,
    /// CPU load (%).
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub cpu_load: Option<i64>,
    /// Free RAM (bytes).
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub free_memory: Option<i64>,
    /// Total RAM (bytes).
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub total_memory: Option<i64>,
    /// Free storage (bytes).
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub free_hdd_space: Option<i64>,
    /// Any extra fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One device found by a BLE or BluFi scan.
#[derive(Debug, Clone, Deserialize)]
pub struct BleDevice {
    /// Advertised name (falls back to the address when unnamed).
    #[serde(default)]
    pub name: String,
    /// BLE MAC address, `XX:XX:XX:XX:XX:XX`.
    #[serde(default)]
    pub address: String,
    /// Signal strength (dBm).
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub rssi: Option<i64>,
    /// Advertised service UUIDs.
    #[serde(default)]
    pub uuids: Vec<String>,
}

/// One GATT characteristic inside a [`BleService`].
#[derive(Debug, Clone, Deserialize)]
pub struct BleCharacteristic {
    /// Characteristic UUID.
    #[serde(default)]
    pub uuid: String,
    /// Human-readable description, when known.
    #[serde(default)]
    pub description: Option<String>,
    /// Supported operations, e.g. `["read", "notify"]`.
    #[serde(default)]
    pub properties: Vec<String>,
}

/// One GATT service enumerated from a connected BLE device.
#[derive(Debug, Clone, Deserialize)]
pub struct BleService {
    /// Service UUID.
    #[serde(default)]
    pub uuid: String,
    /// Human-readable description, when known.
    #[serde(default)]
    pub description: Option<String>,
    /// Characteristics under this service.
    #[serde(default)]
    pub characteristics: Vec<BleCharacteristic>,
}

/// Result of a BLE `info`/`connect`: the device's GATT database.
#[derive(Debug, Clone, Deserialize)]
pub struct BleDeviceInfo {
    /// Device address.
    #[serde(default)]
    pub address: String,
    /// Whether the box reached the device.
    #[serde(default)]
    pub connected: bool,
    /// Enumerated GATT services.
    #[serde(default)]
    pub services: Vec<BleService>,
}

/// Status of one wireless interface on the box (`wifi status`).
#[derive(Debug, Clone, Deserialize)]
pub struct WifiInterface {
    /// Interface name, e.g. `"wlan0"`.
    #[serde(default)]
    pub interface: String,
    /// Connected SSID, or a placeholder like `"Not Connected"`.
    #[serde(default)]
    pub ssid: String,
    /// Connection state, e.g. `"Connected"` / `"Disconnected"`.
    #[serde(default)]
    pub state: String,
}

/// One access point found by a box-side WiFi scan.
#[derive(Debug, Clone, Deserialize)]
pub struct WifiAccessPoint {
    /// Network SSID (`"Hidden"` for hidden networks).
    #[serde(default)]
    pub ssid: Option<String>,
    /// BSSID / AP MAC address.
    #[serde(default)]
    pub address: Option<String>,
    /// Signal strength (approximate %, 0-100).
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub strength: Option<i64>,
    /// `"Open"` or `"Secured"`.
    #[serde(default)]
    pub security: Option<String>,
}

/// Result of connecting the box to a WiFi network.
#[derive(Debug, Clone, Deserialize)]
pub struct WifiConnection {
    /// SSID the box joined.
    #[serde(default)]
    pub ssid: String,
    /// Whether the connection succeeded (always true in a success envelope).
    #[serde(default)]
    pub connected: bool,
    /// Interface used, e.g. `"wlan0"`.
    #[serde(default)]
    pub interface: Option<String>,
    /// Connection method, e.g. `"nmcli"` or `"wpa_supplicant"`.
    #[serde(default)]
    pub method: Option<String>,
}

/// WiFi state reported by a BluFi target device (`status`, and embedded in
/// `connect`). Codes follow the ESP32 BluFi protocol.
#[derive(Debug, Clone, Deserialize)]
pub struct BlufiStatus {
    /// Target device name echo.
    #[serde(default)]
    pub device_name: Option<String>,
    /// Operation mode code (0 NULL, 1 STA, 2 SoftAP, 3 STA+SoftAP).
    #[serde(default, rename = "opMode", deserialize_with = "lenient::opt_i64")]
    pub op_mode: Option<i64>,
    /// Human-readable operation mode.
    #[serde(default, rename = "opModeName")]
    pub op_mode_name: Option<String>,
    /// Station connection code (0 connected, 1 failed, 2 connecting, 3 no IP).
    #[serde(default, rename = "staConn", deserialize_with = "lenient::opt_i64")]
    pub sta_conn: Option<i64>,
    /// Human-readable station connection state.
    #[serde(default, rename = "staConnName")]
    pub sta_conn_name: Option<String>,
    /// SoftAP connection count/state code.
    #[serde(default, rename = "softAPConn", deserialize_with = "lenient::opt_i64")]
    pub soft_ap_conn: Option<i64>,
}

/// Result of a BluFi `connect`: firmware version plus WiFi state.
#[derive(Debug, Clone, Deserialize)]
pub struct BlufiDeviceInfo {
    /// Target firmware version, when the device reports one.
    #[serde(default)]
    pub version: Option<String>,
    /// WiFi state of the target.
    #[serde(flatten)]
    pub status: BlufiStatus,
}

/// Result of a BluFi `provision`.
#[derive(Debug, Clone, Deserialize)]
pub struct BlufiProvisionResult {
    /// Target device name echo.
    #[serde(default)]
    pub device_name: Option<String>,
    /// SSID the target was provisioned onto.
    #[serde(default)]
    pub ssid: String,
    /// Station connection code after provisioning (0 = connected).
    #[serde(default, rename = "staConn", deserialize_with = "lenient::opt_i64")]
    pub sta_conn: Option<i64>,
    /// Human-readable station connection state.
    #[serde(default, rename = "staConnName")]
    pub sta_conn_name: Option<String>,
}

/// One network seen by a BluFi target's own WiFi scan (`wifi_scan`).
#[derive(Debug, Clone, Deserialize)]
pub struct BlufiNetwork {
    /// Network SSID.
    #[serde(default)]
    pub ssid: String,
    /// Signal strength at the target (dBm).
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub rssi: Option<i64>,
}

/// Extract a named list field out of the `value` object (e.g. `devices`
/// from a BLE scan, `access_points` from a WiFi scan).
pub(crate) fn value_list_field<T: DeserializeOwned>(
    resp: CommandResponse,
    field: &str,
) -> Result<Vec<T>> {
    let list = resp
        .value
        .as_ref()
        .and_then(|v| v.get(field))
        .cloned()
        .ok_or_else(|| Error::Decode(format!("response 'value' has no '{field}' list")))?;
    serde_json::from_value(list)
        .map_err(|e| Error::Decode(format!("invalid '{field}' shape: {e}")))
}

// ---------------------------------------------------------------------------
// USB bus enumeration (GET /usb/devices)
// ---------------------------------------------------------------------------

/// Optional filters for [`crate::LagerBox::usb_devices_matching`], applied
/// box-side. `vid`/`pid` are hex strings (with or without `0x`); `serial`
/// is an exact iSerial match. An empty filter returns every device.
#[derive(Debug, Clone, Default)]
pub struct UsbDeviceFilter {
    /// USB vendor id, hex (e.g. `"0483"`).
    pub vid: Option<String>,
    /// USB product id, hex (e.g. `"df11"`).
    pub pid: Option<String>,
    /// Exact iSerial string.
    pub serial: Option<String>,
}

/// One USB device on the box's bus, read from sysfs by `GET /usb/devices`.
/// String fields are `None` when the device does not expose the descriptor
/// (e.g. `serial` on devices with no iSerial).
#[derive(Debug, Clone, Deserialize)]
pub struct UsbDeviceInfo {
    /// sysfs entry name, e.g. `"1-1.4"`.
    #[serde(default)]
    pub sysfs_name: String,
    /// Vendor id, lowercase hex (e.g. `"0483"`).
    #[serde(default)]
    pub vid: Option<String>,
    /// Product id, lowercase hex (e.g. `"df11"`).
    #[serde(default)]
    pub pid: Option<String>,
    /// iSerial descriptor.
    #[serde(default)]
    pub serial: Option<String>,
    /// Product descriptor string.
    #[serde(default)]
    pub product: Option<String>,
    /// Manufacturer descriptor string.
    #[serde(default)]
    pub manufacturer: Option<String>,
    /// Bus number.
    #[serde(default)]
    pub busnum: Option<String>,
    /// Device number on the bus (changes on re-enumeration).
    #[serde(default)]
    pub devnum: Option<String>,
    /// Hub port path, e.g. `"1.4"`.
    #[serde(default)]
    pub devpath: Option<String>,
    /// bDeviceClass, hex.
    #[serde(default)]
    pub device_class: Option<String>,
    /// Negotiated speed in Mbps (`"1.5"`, `"12"`, `"480"`, ...).
    #[serde(default)]
    pub speed: Option<String>,
}

/// Percent-encode one query-string value (RFC 3986 unreserved set).
fn encode_query_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build a `GET /usb/devices` request with optional box-side filters.
pub fn usb_devices(filter: &UsbDeviceFilter) -> HttpRequest {
    let mut query = Vec::new();
    for (key, value) in [
        ("vid", &filter.vid),
        ("pid", &filter.pid),
        ("serial", &filter.serial),
    ] {
        if let Some(value) = value {
            query.push(format!("{key}={}", encode_query_value(value)));
        }
    }
    let path = if query.is_empty() {
        "/usb/devices".to_string()
    } else {
        format!("/usb/devices?{}", query.join("&"))
    };
    HttpRequest {
        method: Method::Get,
        path,
        body: None,
        timeout: Timeout::Default,
    }
}

/// Parse a `GET /usb/devices` response into device records.
pub fn parse_usb_devices(status: u16, body: Value) -> Result<Vec<UsbDeviceInfo>> {
    if status == 404 {
        return Err(usb_devices_unsupported());
    }
    let resp = parse_command(status, body)?;
    let devices = resp
        .extra
        .get("devices")
        .cloned()
        .ok_or_else(|| Error::Decode("response has no 'devices' list".to_string()))?;
    serde_json::from_value(devices)
        .map_err(|e| Error::Decode(format!("invalid 'devices' shape: {e}")))
}

pub(crate) fn usb_devices_unsupported() -> Error {
    Error::UnsupportedByBox {
        message: "this box does not serve GET /usb/devices (requires box software >= 0.33.0)"
            .to_string(),
    }
}

/// Map a route-missing 404 (Flask's non-JSON default page) to
/// [`Error::UnsupportedByBox`] with an endpoint-specific message. Real box
/// errors carry JSON bodies and pass through untouched.
pub(crate) fn map_route_missing(err: Error, unsupported: fn() -> Error) -> Error {
    match err {
        Error::Box {
            status: 404,
            ref message,
        } if message.contains("non-JSON") => unsupported(),
        other => other,
    }
}

// ---------------------------------------------------------------------------
// DFU (POST /usb/dfu)
// ---------------------------------------------------------------------------

/// One device reported by `dfu-util -l` (via [`crate::nets::dfu::Dfu::list`]).
#[derive(Debug, Clone, Deserialize)]
pub struct DfuDevice {
    /// `"DFU"` (in DFU mode) or `"Runtime"` (app running, DFU-capable).
    #[serde(default)]
    pub mode: String,
    /// Vendor id, lowercase hex.
    #[serde(default)]
    pub vid: String,
    /// Product id, lowercase hex.
    #[serde(default)]
    pub pid: String,
    /// Device number on the bus.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub devnum: Option<i64>,
    /// Configuration index.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub cfg: Option<i64>,
    /// Interface index.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub intf: Option<i64>,
    /// Alternate setting index.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub alt: Option<i64>,
    /// Interface name (e.g. `"@Internal Flash /0x08000000/..."`).
    #[serde(default)]
    pub name: Option<String>,
    /// Device serial.
    #[serde(default)]
    pub serial: Option<String>,
    /// Hub port path, e.g. `"1-1.4"`.
    #[serde(default)]
    pub path: Option<String>,
}

/// Captured output of one box-side `dfu-util` run.
#[derive(Debug, Clone, Deserialize)]
pub struct DfuOutput {
    /// dfu-util exit code (0 on the success envelope).
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub exit_code: Option<i64>,
    /// Captured stdout.
    #[serde(default)]
    pub stdout: String,
    /// Captured stderr (dfu-util writes progress here).
    #[serde(default)]
    pub stderr: String,
}

pub(crate) fn dfu_unsupported() -> Error {
    Error::UnsupportedByBox {
        message: "this box does not serve POST /usb/dfu (requires box software >= 0.33.0)"
            .to_string(),
    }
}

// ---------------------------------------------------------------------------
// Box lock / reservation (GET/POST /lock, /lock/heartbeat, /unlock)
// ---------------------------------------------------------------------------

/// Box lock state, as returned by the `/lock` family of endpoints (the same
/// ones `lager boxes lock` uses). `locked: false` with everything else
/// `None` means the box is free.
#[derive(Debug, Clone, Deserialize)]
pub struct BoxLock {
    /// Whether the box is currently locked.
    #[serde(default)]
    pub locked: bool,
    /// Lock holder.
    #[serde(default)]
    pub user: Option<String>,
    /// Holder classification (`"user"`, `"ci"`, `"ephemeral"`, or another
    /// service's reservation origin).
    #[serde(default)]
    pub holder_type: Option<String>,
    /// When the lock was acquired (ISO 8601 UTC).
    #[serde(default)]
    pub locked_at: Option<String>,
    /// Last heartbeat (ISO 8601 UTC).
    #[serde(default)]
    pub last_heartbeat: Option<String>,
    /// Lock TTL in seconds; `None` means the lock never auto-expires.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub ttl_seconds: Option<i64>,
    /// On acquire: the holder before this call (`None` when the box was
    /// free). Distinguishes "just acquired" from "already held it".
    #[serde(default)]
    pub previous_user: Option<String>,
}

/// Build a `GET /lock` request.
pub fn lock_status() -> HttpRequest {
    get("/lock")
}

/// Build a `POST /lock` request. `ttl_seconds: None` sends JSON `null`
/// (an eternal lock, matching `lager boxes lock`).
pub fn lock_acquire(user: &str, holder_type: &str, ttl_seconds: Option<u64>) -> HttpRequest {
    HttpRequest {
        method: Method::Post,
        path: "/lock".to_string(),
        body: Some(json!({
            "user": user,
            "holder_type": holder_type,
            "ttl_seconds": ttl_seconds,
        })),
        timeout: Timeout::Default,
    }
}

/// Build a `POST /lock/heartbeat` request.
pub fn lock_heartbeat(user: &str) -> HttpRequest {
    HttpRequest {
        method: Method::Post,
        path: "/lock/heartbeat".to_string(),
        body: Some(json!({ "user": user })),
        timeout: Timeout::Default,
    }
}

/// Build a `POST /unlock` request.
pub fn unlock(user: &str, force: bool) -> HttpRequest {
    HttpRequest {
        method: Method::Post,
        path: "/unlock".to_string(),
        body: Some(json!({ "user": user, "force": force })),
        timeout: Timeout::Default,
    }
}

/// Parse a `/lock` family response. These endpoints return the raw lock
/// dict on 200 and `{"error": ..., "lock": {...}}` on contention
/// (409 acquire, 403 heartbeat/unlock, 404 heartbeat on an unlocked box).
pub fn parse_lock(status: u16, body: Value) -> Result<BoxLock> {
    if status == 200 {
        return serde_json::from_value(body)
            .map_err(|e| Error::Decode(format!("invalid lock state: {e}")));
    }
    let message = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("lock request failed")
        .to_string();
    Err(Error::Box { status, message })
}

pub(crate) fn lock_unsupported() -> Error {
    Error::UnsupportedByBox {
        message: "this box does not serve the /lock endpoints on port 9000; \
                  update the box software"
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_success_envelope() {
        let resp = parse_command(
            200,
            json!({"success": true, "action": "read", "message": "1.5 V", "value": 1.5}),
        )
        .unwrap();
        assert_eq!(resp.value.as_ref().and_then(as_f64), Some(1.5));
    }

    #[test]
    fn parse_failure_with_http_200_is_box_error() {
        // Cross-role conflicts are reported as success=false with HTTP 200.
        let err = parse_command(200, json!({"success": false, "error": "conflict"})).unwrap_err();
        match err {
            Error::Box { status, message } => {
                assert_eq!(status, 200);
                assert_eq!(message, "conflict");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn parse_501_is_unsupported() {
        let err = parse_command(
            501,
            json!({"success": false, "error": "Role 'gpio' is not supported"}),
        )
        .unwrap_err();
        assert!(matches!(err, Error::UnsupportedByBox { .. }));
    }

    #[test]
    fn base_url_normalization() {
        assert_eq!(base_url("192.168.1.42").unwrap(), "http://192.168.1.42:9000");
        assert_eq!(base_url("mybox.local/").unwrap(), "http://mybox.local:9000");
        assert_eq!(base_url("192.168.1.42:8080").unwrap(), "http://192.168.1.42:8080");
        assert_eq!(base_url("http://box:9000").unwrap(), "http://box:9000");
        assert!(base_url("").is_err());
        assert!(base_url("http://").is_err());
    }

    #[test]
    fn lenient_numbers() {
        assert_eq!(as_f64(&json!("3.3")), Some(3.3));
        assert_eq!(as_f64(&json!(3.3)), Some(3.3));
        assert_eq!(as_bool(&json!("ON")), Some(true));
        assert_eq!(as_bool(&json!(0)), Some(false));
        assert_eq!(as_i64(&json!("42")), Some(42));
    }

    #[test]
    fn supply_state_tolerates_nulls_and_strings() {
        let state: SupplyState = serde_json::from_value(json!({
            "netname": "supply1", "channel": "1", "error": null,
            "voltage": "3.3", "current": null, "enabled": 1,
        }))
        .unwrap();
        assert_eq!(state.channel, Some(1));
        assert_eq!(state.voltage, Some(3.3));
        assert_eq!(state.current, None);
        assert_eq!(state.enabled, Some(true));
    }
}
