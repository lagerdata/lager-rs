//! Webcam nets: MJPEG streaming from USB cameras attached to the box, for
//! visually observing a DUT during tests.

use super::net_handle;
pub use crate::wire::{WebcamStatus, WebcamStream};

pub(crate) mod ops {
    use serde_json::json;

    use crate::error::Result;
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
        Op {
            // Starting ffmpeg/the stream subprocess can take a few seconds.
            req: net_command(
                name,
                ROLE,
                "start",
                json!({}),
                Timeout::After(std::time::Duration::from_secs(30)),
            ),
            parse: value_as::<WebcamStream>,
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
        /// Stop the stream; returns `false` if none was running.
        fn stop() -> bool = ops::stop;
        /// Current stream state (running, URL, port, device).
        fn status() -> WebcamStatus = ops::status;
        /// Stream URL, or `None` when no stream is running.
        fn url() -> Option<String> = ops::url;
    }
}
