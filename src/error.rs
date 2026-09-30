//! Crate-wide error type.

use std::fmt;

/// Convenience alias used by every fallible API in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// All the ways a Lager box interaction can fail.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The box could not be reached at all (DNS, TCP connect, transport
    /// failure). Check network/Tailscale and that the box is online.
    Connection(String),
    /// The HTTP request timed out client-side. For long-running actions
    /// (energy integration windows, `wait_for_level`) the crate widens the
    /// timeout automatically, so hitting this usually means the box stalled.
    Timeout(String),
    /// The box accepted the request but reported failure. Carries the HTTP
    /// status and the box's `error` message. Note the box can report failure
    /// with HTTP 200 (e.g. a cross-role instrument conflict); this variant
    /// covers that too.
    Box {
        /// HTTP status code of the response (200 is possible: the box
        /// reports some conflicts as `success: false` with HTTP 200).
        status: u16,
        /// Human-readable error message from the box.
        message: String,
    },
    /// HTTP 501: this box image does not serve the endpoint. The box needs a
    /// software update.
    UnsupportedByBox {
        /// Human-readable error message from the box.
        message: String,
    },
    /// The box's response could not be parsed into the expected shape.
    Decode(String),
    /// The net type exists in the Lager Python API but its box endpoint is
    /// not yet available on the `:9000` HTTP API, so this crate ships it as a
    /// documented stub. See `MISSING_ENDPOINTS.md` in the crate repository.
    NotSupportedByBox {
        /// Which net/feature was invoked (e.g. `"debug"`).
        feature: &'static str,
        /// What the box side would need to expose for this to work.
        details: &'static str,
    },
    /// The box sits behind an authenticating gateway and the request was
    /// denied (HTTP 401 with the `X-Gateway-Auth-Url` discovery header),
    /// with no usable credential available. Sign in with the Lager CLI
    /// (`lager login <auth_url>`) — this crate reuses the CLI's session —
    /// or supply a token via `LagerBoxBuilder::bearer_token` /
    /// `LAGER_GATEWAY_TOKEN`.
    AuthRequired {
        /// Hostname of the gated box.
        box_host: String,
        /// The auth server URL announced by the gateway.
        auth_url: String,
        /// What happened (no credential vs. rejected credential).
        message: String,
    },
    /// Client-side configuration problem (bad host string, missing env var).
    Config(String),
    /// A streaming session (UART) reported an error, e.g. the net is in use
    /// by another session or the device disappeared.
    Stream(String),
    /// A BLE GATT session operation failed, or the session has ended
    /// (feature `ble-session`). `kind` is the box's error code or close
    /// reason; `message` is the box's explanation.
    Ble {
        /// What went wrong, from the box's error code or close reason.
        kind: BleErrorKind,
        /// Human-readable error message from the box.
        message: String,
    },
}

/// The kind of an [`Error::Ble`]: one variant per error code the box's BLE
/// session reports, plus the reasons a session can end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BleErrorKind {
    /// The address was not seen during the connect scan: the device is not
    /// advertising, or is out of range. The session was not opened.
    DeviceNotFound,
    /// Connecting failed or timed out. The session was not opened.
    ConnectFailed,
    /// Another session, or a BLE/BluFi operation, holds the box's Bluetooth
    /// adapter. The message names the holder.
    AdapterBusy,
    /// BlueZ is not running on the box host; the message says how to fix it.
    BluezUnavailable,
    /// A session is already open on this connection.
    SessionActive,
    /// No session is open (the box ended it, or it was closed).
    NotOpen,
    /// The UUID or handle is not in the device's GATT table.
    UnknownCharacteristic,
    /// The UUID appears more than once; the message lists the handles to
    /// use instead.
    AmbiguousCharacteristic,
    /// The characteristic lacks the needed property, or the peripheral
    /// refused the operation.
    NotPermitted,
    /// A bad address, timeout, hex payload or payload size.
    InvalidArgument,
    /// The peripheral disconnected. The session has ended.
    Disconnected,
    /// A box-side operation bound ran out. The session has ended.
    Timeout,
    /// The client broke the session protocol (a missing `seq`). The session
    /// has ended.
    ProtocolError,
    /// The client did not drain notifications fast enough and the box's
    /// buffer filled. The session has ended.
    Overflow,
    /// No client operation within the idle timeout. The session has ended.
    IdleTimeout,
    /// Another client force-released the session. The session has ended.
    Released,
    /// Any other BLE error (the box's `ble_error`), or a code this crate
    /// does not know yet.
    Other,
}

impl BleErrorKind {
    /// Map a box error code (the `code` of a failed `ble_result`) to a kind.
    /// Unknown codes map to [`BleErrorKind::Other`].
    pub fn from_code(code: &str) -> Self {
        match code {
            "device_not_found" => BleErrorKind::DeviceNotFound,
            "connect_failed" => BleErrorKind::ConnectFailed,
            "adapter_busy" => BleErrorKind::AdapterBusy,
            "bluez_unavailable" => BleErrorKind::BluezUnavailable,
            "session_active" => BleErrorKind::SessionActive,
            "not_open" => BleErrorKind::NotOpen,
            "unknown_characteristic" => BleErrorKind::UnknownCharacteristic,
            "ambiguous_characteristic" => BleErrorKind::AmbiguousCharacteristic,
            "not_permitted" => BleErrorKind::NotPermitted,
            "invalid_argument" => BleErrorKind::InvalidArgument,
            "disconnected" => BleErrorKind::Disconnected,
            "timeout" => BleErrorKind::Timeout,
            "protocol_error" => BleErrorKind::ProtocolError,
            "overflow" => BleErrorKind::Overflow,
            "idle_timeout" => BleErrorKind::IdleTimeout,
            "released" => BleErrorKind::Released,
            _ => BleErrorKind::Other,
        }
    }

    /// Map a session close reason (the `reason` of `ble_closed`) to the kind
    /// later operations fail with. `client` (the session was closed from
    /// this side) maps to [`BleErrorKind::NotOpen`]; `shutdown` (the box
    /// server stopped) and unknown reasons map to [`BleErrorKind::Other`].
    pub fn from_close_reason(reason: &str) -> Self {
        match reason {
            "client" => BleErrorKind::NotOpen,
            "shutdown" => BleErrorKind::Other,
            other => BleErrorKind::from_code(other),
        }
    }

    /// The box's code for this kind (`"ble_error"` for
    /// [`BleErrorKind::Other`]).
    pub fn as_code(&self) -> &'static str {
        match self {
            BleErrorKind::DeviceNotFound => "device_not_found",
            BleErrorKind::ConnectFailed => "connect_failed",
            BleErrorKind::AdapterBusy => "adapter_busy",
            BleErrorKind::BluezUnavailable => "bluez_unavailable",
            BleErrorKind::SessionActive => "session_active",
            BleErrorKind::NotOpen => "not_open",
            BleErrorKind::UnknownCharacteristic => "unknown_characteristic",
            BleErrorKind::AmbiguousCharacteristic => "ambiguous_characteristic",
            BleErrorKind::NotPermitted => "not_permitted",
            BleErrorKind::InvalidArgument => "invalid_argument",
            BleErrorKind::Disconnected => "disconnected",
            BleErrorKind::Timeout => "timeout",
            BleErrorKind::ProtocolError => "protocol_error",
            BleErrorKind::Overflow => "overflow",
            BleErrorKind::IdleTimeout => "idle_timeout",
            BleErrorKind::Released => "released",
            BleErrorKind::Other => "ble_error",
        }
    }
}

impl fmt::Display for BleErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Connection(msg) => write!(
                f,
                "cannot reach the Lager box: {msg}. Check network/Tailscale and that the box is online and updated"
            ),
            Error::Timeout(msg) => write!(f, "request to the Lager box timed out: {msg}"),
            Error::Box { status, message } => {
                write!(f, "box error (HTTP {status}): {message}")
            }
            Error::UnsupportedByBox { message } => write!(
                f,
                "{message}. This box image does not support this endpoint; update the box"
            ),
            Error::Decode(msg) => write!(f, "could not decode box response: {msg}"),
            Error::NotSupportedByBox { feature, details } => write!(
                f,
                "'{feature}' is not yet available over the box HTTP API: {details}"
            ),
            Error::AuthRequired { auth_url, message, .. } => write!(
                f,
                "{message}. Sign in with `lager login {auth_url}` (this crate reuses the \
                 CLI's session), or set LAGER_GATEWAY_TOKEN / use \
                 LagerBoxBuilder::bearer_token"
            ),
            Error::Config(msg) => write!(f, "configuration error: {msg}"),
            Error::Stream(msg) => write!(f, "streaming session error: {msg}"),
            Error::Ble { kind, message } => write!(f, "BLE session error ({kind}): {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Decode(e.to_string())
    }
}
