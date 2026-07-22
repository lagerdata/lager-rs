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
        }
    }
}

impl std::error::Error for Error {}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Decode(e.to_string())
    }
}
