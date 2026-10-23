//! Webcam nets: MJPEG streaming from USB cameras attached to the box, for
//! visually observing a DUT during tests.

use super::net_handle;
pub use crate::wire::{WebcamStatus, WebcamStream};

pub(crate) mod ops {
    use serde_json::json;

    use crate::error::{Error, Result};
    use crate::nets::debug::base64_decode;
    use crate::wire::{
        net_command, value_as, CommandResponse, Op, Timeout, WebcamStatus, WebcamStream,
    };

    const ROLE: &str = "webcam";

    fn parse_stopped(resp: CommandResponse) -> Result<bool> {
        let status: WebcamStopped = value_as(resp)?;
        Ok(status.stopped)
    }

    #[derive(serde::Deserialize)]
    struct WebcamStopped {
        #[serde(default)]
        stopped: bool,
    }

    fn parse_url(resp: CommandResponse) -> Result<Option<String>> {
        let status: WebcamStatus = value_as(resp)?;
        Ok(status.url)
    }

    pub(crate) fn start(name: &str) -> Op<WebcamStream> {
        start_with(name, json!({}))
    }

    pub(crate) fn start_as(name: &str, source: &str, started_by: Option<&str>) -> Op<WebcamStream> {
        let mut params = json!({ "source": source });
        if let Some(who) = started_by {
            params["started_by"] = json!(who);
        }
        start_with(name, params)
    }

    fn start_with(name: &str, params: serde_json::Value) -> Op<WebcamStream> {
        Op {
            // Starting ffmpeg/the stream subprocess can take a few seconds.
            req: net_command(
                name,
                ROLE,
                "start",
                params,
                Timeout::After(std::time::Duration::from_secs(30)),
            ),
            parse: value_as::<WebcamStream>,
        }
    }

    #[derive(serde::Deserialize)]
    struct Snapshot {
        jpeg_base64: String,
    }

    fn parse_snapshot(resp: CommandResponse) -> Result<Vec<u8>> {
        let snap: Snapshot = value_as(resp)?;
        base64_decode(&snap.jpeg_base64)
            .ok_or_else(|| Error::Decode("snapshot 'jpeg_base64' is not valid base64".to_string()))
    }

    pub(crate) fn snapshot(name: &str) -> Op<Vec<u8>> {
        Op {
            // The box fetches the frame from its streamer with an 8 s limit.
            req: net_command(
                name,
                ROLE,
                "snapshot",
                json!({}),
                Timeout::After(std::time::Duration::from_secs(20)),
            ),
            parse: parse_snapshot,
        }
    }

    pub(crate) fn stop(name: &str) -> Op<bool> {
        Op {
            req: net_command(name, ROLE, "stop", json!({}), Timeout::Default),
            parse: parse_stopped,
        }
    }

    pub(crate) fn status(name: &str) -> Op<WebcamStatus> {
        Op {
            req: net_command(name, ROLE, "status", json!({}), Timeout::Default),
            parse: value_as::<WebcamStatus>,
        }
    }

    pub(crate) fn url(name: &str) -> Op<Option<String>> {
        Op {
            req: net_command(name, ROLE, "url", json!({}), Timeout::Default),
            parse: parse_url,
        }
    }
}

net_handle! {
    /// Handle for a webcam net.
    ///
    /// Stream URLs point at the box host as the client reached it; pass the
    /// URL to any MJPEG viewer (browser, VLC, OpenCV).
    sync: Webcam,
    async: AsyncWebcam,
    methods: {
        /// Start the MJPEG stream (idempotent: reports `already_running`
        /// when a stream is already up).
        fn start() -> WebcamStream = ops::start;
        /// [`Webcam::start`], recording which surface started the stream and
        /// who, as [`WebcamStatus::source`] / `started_by` report back (box
        /// >= 0.47.0; an older box ignores both).
        fn start_as(source: &str, started_by: Option<&str>) -> WebcamStream = ops::start_as;
        /// Stop the stream; returns `false` if none was running.
        fn stop() -> bool = ops::stop;
        /// Current stream state (running, URL, port, device).
        fn status() -> WebcamStatus = ops::status;
        /// Stream URL, or `None` when no stream is running.
        fn url() -> Option<String> = ops::url;
    }
}

// `snapshot` lives outside the macro so an older box's refusal can be mapped
// to `Error::UnsupportedByBox`.

#[cfg(feature = "blocking")]
impl Webcam<'_> {
    /// Capture one JPEG frame from the running stream and return its bytes.
    /// Start the stream first; the box refuses a snapshot with none running.
    ///
    /// Requires box software >= 0.47.0; older boxes fail with
    /// [`crate::Error::UnsupportedByBox`].
    pub fn snapshot(&self) -> crate::error::Result<Vec<u8>> {
        self.client
            .run(ops::snapshot(&self.name))
            .map_err(|e| crate::wire::unknown_action_compat(e, "snapshot", ">= 0.47.0"))
    }
}

#[cfg(feature = "async")]
impl AsyncWebcam<'_> {
    /// See [`Webcam::snapshot`].
    pub async fn snapshot(&self) -> crate::error::Result<Vec<u8>> {
        self.client
            .run(ops::snapshot(&self.name))
            .await
            .map_err(|e| crate::wire::unknown_action_compat(e, "snapshot", ">= 0.47.0"))
    }
}
