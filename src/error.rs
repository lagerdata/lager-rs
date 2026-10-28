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
    /// Box-side `dfu-util` ran and exited non-zero (HTTP 502 from
    /// `POST /usb/dfu`). Carries both captured streams in full, and
    /// `Display` quotes both: dfu-util reports a device failure on stdout
    /// for a raw download and on stderr for DfuSe, and its DFU suffix
    /// warnings on stderr.
    DfuFailed {
        /// HTTP status code of the response (502).
        status: u16,
        /// dfu-util's exit code.
        exit_code: Option<i64>,
        /// dfu-util's captured stdout.
        stdout: String,
        /// dfu-util's captured stderr.
        stderr: String,
    },
    /// A debug `flash` ran but programmed nothing (box >= 0.51.0). The box
    /// answers HTTP 200 with `programmed: false` for this, so before this
    /// variant the crate reported success.
    FlashFailed {
        /// The box's verdict: the line showing nothing was programmed.
        error: String,
        /// The backend's output.
        output: Vec<String>,
    },
    /// The debug probe stayed busy with another operation (a flash, erase,
    /// reset, ...) for longer than the box waits (HTTP 503 from the debug
    /// service, box >= 0.51.0). Retry once the other operation is done.
    ProbeBusy {
        /// The box's message, naming the operation holding the probe.
        message: String,
    },
    /// The net's safety settings refused a destructive debug operation
    /// (flash or erase; HTTP 403 from the debug service). See
    /// [`crate::SafetyLimits::allow_destructive`].
    SafetyRefused {
        /// The box's message.
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
    /// documented stub. No net type returns this since the scope stub became
    /// a real net ([`crate::nets::scope`]) in 0.8.0; the variant stays so matches
    /// on it keep compiling.
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
    /// A streaming session (UART) reported an error, e.g. the device
    /// disappeared.
    Stream(String),
    /// A UART session could not start because another session holds the
    /// net or its serial device. If that session should be gone, free the
    /// net with
    /// [`LagerBox::release_uart_session`](crate::LagerBox::release_uart_session)
    /// on `held_by` and open again.
    UartInUse {
        /// The box's message.
        message: String,
        /// The net whose session holds the port (box >= 0.49.0). When two
        /// nets share one serial device this is the *other* net, and that
        /// is the one to release.
        held_by: Option<String>,
    },
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
            Error::DfuFailed {
                status,
                exit_code,
                stdout,
                stderr,
            } => {
                write!(f, "box error (HTTP {status}): dfu-util ")?;
                match exit_code {
                    Some(code) => write!(f, "exited with code {code}")?,
                    None => write!(f, "failed")?,
                }
                let (stderr, stdout) = (stderr.trim(), stdout.trim());
                if stderr.is_empty() && stdout.is_empty() {
                    return write!(f, ": no output");
                }
                if !stderr.is_empty() {
                    write!(f, "\nstderr:\n{stderr}")?;
                }
                if !stdout.is_empty() {
                    write!(f, "\nstdout:\n{stdout}")?;
                }
                Ok(())
            }
            Error::FlashFailed { error, output } => {
                write!(f, "flash failed: {error}")?;
                if !output.is_empty() {
                    write!(f, "\noutput:\n{}", output.join("\n"))?;
                }
                Ok(())
            }
            Error::ProbeBusy { message } => write!(f, "debug probe busy: {message}"),
            Error::SafetyRefused { message } => {
                write!(f, "refused by the net's safety settings: {message}")
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
            Error::UartInUse { message, .. } => write!(f, "UART in use: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Decode(e.to_string())
    }
}
