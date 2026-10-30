//! Scope trace capture: the waveform samples behind the box's `/scope` page.
//!
//! The box's oscilloscope daemon publishes each capture as one LSCP/1 binary
//! frame: a 64-byte header, a 16-byte descriptor per channel, then raw ADC
//! counts, channel-major. The box relays them over a WebSocket at
//! `/scope/<net>/ws`, opened with a short-lived ticket from
//! `GET /scope/<net>/stream`.
//!
//! [`ScopeCapture::decode`] reads one frame and needs no feature. Fetching
//! frames from a box, [`Scope::capture`] for one and [`Scope::stream`] for
//! every capture from now on, needs the `scope-trace` feature (blocking
//! client only).
//!
//! Every PicoScope net streams. A Rigol MSO5000 net streams only through an
//! HDMI capture card paired with it (`lager nets set-capture`); without one,
//! and on any other make, the box refuses the ticket with
//! [`crate::Error::Box`] (HTTP 400) saying why. A box that predates the
//! stream answers 404, which surfaces as [`crate::Error::UnsupportedByBox`].
//!
//! ```no_run
//! # #[cfg(feature = "scope-trace")]
//! # fn demo() -> lager::Result<()> {
//! let lager = lager::LagerBox::from_env()?;
//! let scope = lager.scope("scope1");
//! scope.run()?;
//! let capture = scope.capture()?;
//! let volts = capture.volts('A').expect("channel A is enabled");
//! let peak = volts.iter().copied().fold(f64::NAN, f64::max);
//! println!("{} samples at {} Hz, peak {peak} V", volts.len(), capture.sample_rate_hz());
//! # Ok(())
//! # }
//! # fn main() {}
//! ```
//!
//! [`Scope::capture`]: crate::nets::scope::Scope::capture
//! [`Scope::stream`]: crate::nets::scope::Scope::stream

use crate::error::{Error, Result};

/// `"LSCP"` read as a little-endian u32.
const MAGIC: u32 = 0x5043_534C;
const VERSION: u16 = 1;
const HEADER_SIZE: usize = 64;
const CHANNEL_DESC_SIZE: usize = 16;

/// A sample that was not captured: the part of a rolling screen the stream
/// has not reached yet. No ADC produces it; every range tops out at
/// `+/-i16::MAX`. [`ScopeCapture::volts`] turns it into NaN.
pub const NO_SAMPLE: i16 = i16::MIN;

/// Input coupling of a captured channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TraceCoupling {
    /// DC coupling.
    Dc,
    /// AC coupling.
    Ac,
    /// Input grounded.
    Gnd,
}

/// One channel's descriptor in a [`ScopeCapture`].
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct TraceChannel {
    /// Channel label, `'A'` for the first input.
    pub label: char,
    /// The driver's input-range code.
    pub range_code: u8,
    /// Input coupling.
    pub coupling: TraceCoupling,
    /// Volts represented by one ADC count.
    pub scale_v_per_count: f32,
    /// Volts added after scaling, for analogue offset.
    pub offset_v: f32,
}

/// One oscilloscope capture: timing, channel descriptors and raw samples.
///
/// `samples` holds raw ADC counts, channel-major: all of the first channel
/// in [`channels`](Self::channels), then all of the second. Use
/// [`counts`](Self::counts) or [`volts`](Self::volts) to read one channel.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ScopeCapture {
    /// The daemon's capture sequence number.
    pub seq: u64,
    /// Box monotonic clock, in nanoseconds, when the capture left the
    /// hardware.
    pub capture_mono_ns: u64,
    /// Time between samples, in nanoseconds.
    pub sample_interval_ns: f64,
    /// Samples per channel before the trigger point.
    pub pre_trigger_samples: u32,
    /// Samples per channel after the trigger point.
    pub post_trigger_samples: u32,
    /// Samples in each channel.
    pub samples_per_channel: u32,
    /// ADC resolution in bits.
    pub resolution_bits: u8,
    /// One bit per entry in `channels`, set when that channel clipped.
    pub overflow_mask: u16,
    /// Raw flag bits; see [`triggered`](Self::triggered),
    /// [`streaming`](Self::streaming) and [`is_envelope`](Self::is_envelope).
    pub flags: u16,
    /// For a rolling screen, how many samples per channel the screen spans,
    /// counted back from the newest; the ones before are history. Zero for a
    /// block capture, where all of it is the screen.
    pub screen_samples: u32,
    /// The captured channels, in payload order.
    pub channels: Vec<TraceChannel>,
    /// Raw ADC counts, channel-major.
    pub samples: Vec<i16>,
}

impl ScopeCapture {
    /// Flag bit: the capture carried a hardware trigger.
    pub const FLAG_TRIGGERED: u16 = 1 << 0;
    /// Flag bit: the capture came from continuous (roll) streaming.
    pub const FLAG_STREAMING: u16 = 1 << 1;
    /// Flag bit: samples are (minimum, maximum) pairs.
    pub const FLAG_ENVELOPE: u16 = 1 << 2;

    /// Decode one LSCP/1 frame. Fails with [`Error::Decode`] on a short,
    /// truncated or foreign frame, or one of another LSCP version.
    pub fn decode(buf: &[u8]) -> Result<Self> {
        if buf.len() < HEADER_SIZE {
            return Err(decode_error(format!(
                "frame too short: need {HEADER_SIZE} bytes, got {}",
                buf.len()
            )));
        }
        let magic = u32_at(buf, 0);
        if magic != MAGIC {
            return Err(decode_error(format!(
                "bad magic 0x{magic:08x}, expected 0x{MAGIC:08x}"
            )));
        }
        let version = u16_at(buf, 4);
        if version != VERSION {
            return Err(decode_error(format!(
                "unsupported LSCP version {version}, this crate reads {VERSION}"
            )));
        }

        let channel_count = buf[44] as usize;
        let descriptors_end = HEADER_SIZE + channel_count * CHANNEL_DESC_SIZE;
        if buf.len() < descriptors_end {
            return Err(decode_error(format!(
                "frame too short: need {descriptors_end} bytes, got {}",
                buf.len()
            )));
        }
        let mut channels = Vec::with_capacity(channel_count);
        for i in 0..channel_count {
            let at = HEADER_SIZE + i * CHANNEL_DESC_SIZE;
            let code = buf[at];
            if code > 25 {
                return Err(decode_error(format!(
                    "channel code {code} names no channel A to Z"
                )));
            }
            channels.push(TraceChannel {
                label: char::from(b'A' + code),
                range_code: buf[at + 1],
                coupling: match buf[at + 2] {
                    1 => TraceCoupling::Ac,
                    2 => TraceCoupling::Gnd,
                    _ => TraceCoupling::Dc,
                },
                scale_v_per_count: f32::from_le_bytes(bytes4(buf, at + 4)),
                offset_v: f32::from_le_bytes(bytes4(buf, at + 8)),
            });
        }

        let samples_per_channel = u32_at(buf, 40);
        let expected = samples_per_channel as usize * channel_count * 2;
        let payload = &buf[descriptors_end..];
        if payload.len() != expected {
            return Err(decode_error(format!(
                "payload length mismatch: expected {expected} bytes, got {}",
                payload.len()
            )));
        }
        let samples = payload
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
            .collect();

        Ok(ScopeCapture {
            seq: u64::from_le_bytes(bytes8(buf, 8)),
            capture_mono_ns: u64::from_le_bytes(bytes8(buf, 16)),
            sample_interval_ns: f64::from_le_bytes(bytes8(buf, 24)),
            pre_trigger_samples: u32_at(buf, 32),
            post_trigger_samples: u32_at(buf, 36),
            samples_per_channel,
            resolution_bits: buf[45],
            overflow_mask: u16_at(buf, 46),
            flags: u16_at(buf, 6),
            screen_samples: u32_at(buf, 48),
            channels,
            samples,
        })
    }

    /// Whether the capture carried a hardware trigger. Clear for an auto or
    /// forced capture.
    pub fn triggered(&self) -> bool {
        self.flags & Self::FLAG_TRIGGERED != 0
    }

    /// Whether the capture came from continuous (roll) streaming rather than
    /// a block.
    pub fn streaming(&self) -> bool {
        self.flags & Self::FLAG_STREAMING != 0
    }

    /// Whether samples are (minimum, maximum) pairs, one pair per
    /// `2 * sample_interval_ns`, as roll mode sends them.
    pub fn is_envelope(&self) -> bool {
        self.flags & Self::FLAG_ENVELOPE != 0
    }

    /// Samples per second, or 0 when the interval is not positive.
    pub fn sample_rate_hz(&self) -> f64 {
        if self.sample_interval_ns <= 0.0 {
            0.0
        } else {
            1e9 / self.sample_interval_ns
        }
    }

    /// Length of the capture in seconds.
    pub fn duration_s(&self) -> f64 {
        f64::from(self.samples_per_channel) * self.sample_interval_ns / 1e9
    }

    /// Position of a channel in [`channels`](Self::channels), by label
    /// (`'A'` or `'a'`).
    pub fn channel_index(&self, label: char) -> Option<usize> {
        let label = label.to_ascii_uppercase();
        self.channels.iter().position(|c| c.label == label)
    }

    /// Raw ADC counts for one channel, or `None` when it was not captured.
    pub fn counts(&self, label: char) -> Option<&[i16]> {
        let index = self.channel_index(label)?;
        let n = self.samples_per_channel as usize;
        self.samples.get(index * n..(index + 1) * n)
    }

    /// One channel in volts, or `None` when it was not captured. A sample
    /// that was not captured ([`NO_SAMPLE`]) is NaN rather than the bottom
    /// of the range.
    pub fn volts(&self, label: char) -> Option<Vec<f64>> {
        let channel = self.channels[self.channel_index(label)?];
        let scale = f64::from(channel.scale_v_per_count);
        let offset = f64::from(channel.offset_v);
        Some(
            self.counts(label)?
                .iter()
                .map(|&count| {
                    if count == NO_SAMPLE {
                        f64::NAN
                    } else {
                        f64::from(count) * scale + offset
                    }
                })
                .collect(),
        )
    }

    /// Each sample's time in seconds relative to the trigger, so t = 0 is
    /// the trigger point and pre-trigger samples are negative. Spaced one
    /// `sample_interval_ns` apart, envelope pairs included.
    pub fn time_axis(&self) -> Vec<f64> {
        let interval_s = self.sample_interval_ns / 1e9;
        let start = -f64::from(self.pre_trigger_samples) * interval_s;
        (0..self.samples_per_channel)
            .map(|i| start + f64::from(i) * interval_s)
            .collect()
    }

    /// Whether a channel clipped during this capture. `false` for a channel
    /// that was not captured.
    pub fn overflowed(&self, label: char) -> bool {
        match self.channel_index(label) {
            Some(index) if index < 16 => self.overflow_mask & (1 << index) != 0,
            _ => false,
        }
    }
}

fn decode_error(message: String) -> Error {
    Error::Decode(format!("LSCP frame: {message}"))
}

fn bytes4(buf: &[u8], at: usize) -> [u8; 4] {
    [buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]
}

fn bytes8(buf: &[u8], at: usize) -> [u8; 8] {
    let mut out = [0u8; 8];
    out.copy_from_slice(&buf[at..at + 8]);
    out
}

fn u16_at(buf: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([buf[at], buf[at + 1]])
}

fn u32_at(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes4(buf, at))
}

/// The sequence number of a frame, without decoding its samples. `None`
/// for anything shorter than a header or without the LSCP magic.
#[cfg(feature = "scope-trace")]
fn peek_seq(buf: &[u8]) -> Option<u64> {
    (buf.len() >= HEADER_SIZE && u32_at(buf, 0) == MAGIC)
        .then(|| u64::from_le_bytes(bytes8(buf, 8)))
}

#[cfg(feature = "scope-trace")]
pub use transport::ScopeStream;

#[cfg(feature = "scope-trace")]
mod transport {
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    use serde_json::{json, Value};
    use tungstenite::client::IntoClientRequest;
    use tungstenite::protocol::WebSocketConfig;
    use tungstenite::{Message, WebSocket};

    use super::{peek_seq, ScopeCapture};
    use crate::error::{Error, Result};
    use crate::nets::scope::Scope;
    use crate::wire;

    /// Captures the daemon may send ahead of the ones this side has read,
    /// as the box's own Python client asks for.
    const STREAM_CREDITS: u32 = 4;

    /// The largest capture accepted. A deep PicoScope capture of four
    /// channels runs to tens of MiB; the default 16 MiB frame cap would
    /// refuse it.
    const MAX_CAPTURE_BYTES: usize = 512 << 20;

    impl<'a> Scope<'a> {
        /// Fetch the scope's latest capture, waiting up to the client's
        /// default timeout. Feature `scope-trace`.
        ///
        /// The daemon answers with the capture it has, or takes one now if
        /// it has none, so start the scope first ([`Scope::run`] or
        /// [`Scope::single`]). See [`crate::nets::scope_trace`] for which
        /// scope nets stream.
        pub fn capture(&self) -> Result<ScopeCapture> {
            self.capture_timeout(self.client.default_timeout())
        }

        /// [`capture`](Self::capture) with an explicit wait. Fails with
        /// [`Error::Timeout`] when no capture arrives in time.
        pub fn capture_timeout(&self, timeout: Duration) -> Result<ScopeCapture> {
            let mut socket = Socket::open(self, timeout)?;
            let result = socket.capture(Instant::now() + timeout);
            socket.close();
            result
        }

        /// Subscribe to every capture the scope takes from now on. Feature
        /// `scope-trace`.
        ///
        /// The stream has a connection of its own and is credit-paced: the
        /// daemon sends a capture only when this side has room, and always
        /// the newest one, so a slow reader sees a lower frame rate rather
        /// than an ever-growing backlog. Read with
        /// [`ScopeStream::next_capture`].
        pub fn stream(&self) -> Result<ScopeStream> {
            let timeout = self.client.default_timeout();
            let mut socket = Socket::open(self, timeout)?;
            socket.send(json!({"command": "Subscribe", "credits": STREAM_CREDITS}))?;
            if let Err(e) = socket.reply("Subscribe", Instant::now() + timeout) {
                socket.close();
                return Err(e);
            }
            Ok(ScopeStream { socket })
        }
    }

    /// A subscription to a scope's captures, from [`Scope::stream`].
    /// Closed on drop.
    pub struct ScopeStream {
        socket: Socket,
    }

    impl ScopeStream {
        /// Wait up to `timeout` for the next capture. Fails with
        /// [`Error::Timeout`] when none arrives: check the scope is running,
        /// and in normal trigger mode, that the signal crosses the trigger
        /// level.
        pub fn next_capture(&mut self, timeout: Duration) -> Result<ScopeCapture> {
            let deadline = Instant::now() + timeout;
            loop {
                match self.socket.read(deadline, "a streamed capture")? {
                    Message::Binary(frame) => {
                        let capture = ScopeCapture::decode(&frame)?;
                        self.socket.send(json!({"command": "Credit", "count": 1}))?;
                        return Ok(capture);
                    }
                    Message::Text(text) => {
                        // Nothing is answered on a subscribed socket but an
                        // error notice; surface it.
                        daemon_reply(&text, "a stream")?;
                    }
                    _ => {}
                }
            }
        }

        /// Unsubscribe and close the connection.
        pub fn close(mut self) {
            self.socket.close();
        }
    }

    impl Drop for ScopeStream {
        fn drop(&mut self) {
            self.socket.close();
        }
    }

    struct Socket {
        ws: WebSocket<TcpStream>,
        closed: bool,
    }

    impl Socket {
        /// Ask the box for a stream ticket, then open the WebSocket it names.
        fn open(scope: &Scope<'_>, timeout: Duration) -> Result<Self> {
            let client = scope.client;
            let path = format!("/scope/{}/stream", wire::path_segment(&scope.name));
            let (status, body) = match client.execute(&wire::get(&path)) {
                // A 404 without the JSON a missing net gets: no such route.
                Err(Error::Box { status: 404, .. }) => {
                    return Err(Error::UnsupportedByBox {
                        message: "this box does not serve scope trace streams \
                                  (GET /scope/<net>/stream)"
                            .to_string(),
                    })
                }
                other => other?,
            };
            let message = body.get("error").and_then(Value::as_str);
            if status != 200 || body.get("success").and_then(Value::as_bool) == Some(false) {
                return Err(Error::Box {
                    status,
                    message: message.unwrap_or("scope stream ticket refused").to_string(),
                });
            }
            let ws_path = body
                .get("ws_path")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Decode("stream ticket carries no ws_path".to_string()))?;

            let base = client.base_url();
            let Some(authority) = base.strip_prefix("http://") else {
                return Err(Error::Config(format!(
                    "scope trace capture needs a plain http:// box address, not '{base}'"
                )));
            };
            let (host, port) = crate::tunnel::host_port(base)?;
            let stream = crate::client::connect_tcp(&host, port, timeout)?;
            let io = |e| crate::tunnel::io_error("scope stream", e);
            stream.set_nodelay(true).map_err(io)?;
            stream.set_read_timeout(Some(timeout)).map_err(io)?;
            stream.set_write_timeout(Some(timeout)).map_err(io)?;

            let mut request = format!("ws://{authority}{ws_path}")
                .into_client_request()
                .map_err(|e| Error::Config(format!("scope stream URL: {e}")))?;
            if let Some(token) = client.current_token() {
                let value = format!("Bearer {token}")
                    .parse()
                    .map_err(|_| Error::Config("bearer token is not a valid header".into()))?;
                request.headers_mut().insert("Authorization", value);
            }
            let config = WebSocketConfig {
                max_message_size: Some(MAX_CAPTURE_BYTES),
                max_frame_size: Some(MAX_CAPTURE_BYTES),
                ..Default::default()
            };
            let (ws, _) = tungstenite::client::client_with_config(request, stream, Some(config))
                .map_err(handshake_error)?;
            Ok(Socket { ws, closed: false })
        }

        fn send(&mut self, command: Value) -> Result<()> {
            self.ws
                .send(Message::Text(command.to_string()))
                .map_err(|e| ws_error(e, "sending a scope command"))
        }

        /// Read the next message before `deadline`, skipping control frames.
        fn read(&mut self, deadline: Instant, waiting_for: &str) -> Result<Message> {
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(Error::Timeout(format!(
                        "no {waiting_for} from the scope in time"
                    )));
                }
                let _ = self.ws.get_mut().set_read_timeout(Some(remaining));
                match self.ws.read() {
                    Ok(Message::Close(_)) => {
                        return Err(Error::Stream(format!(
                            "the box closed the scope stream while waiting for {waiting_for}"
                        )))
                    }
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                    Ok(message) => return Ok(message),
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(e) => return Err(ws_error(e, waiting_for)),
                }
            }
        }

        /// The daemon's JSON reply to `command`, skipping captures pushed
        /// ahead of it.
        fn reply(&mut self, command: &str, deadline: Instant) -> Result<Value> {
            loop {
                if let Message::Text(text) = self.read(deadline, &format!("{command} reply"))? {
                    let reply = daemon_reply(&text, command)?;
                    if reply.get("response").and_then(Value::as_str) != Some("State") {
                        return Ok(reply);
                    }
                }
            }
        }

        fn capture(&mut self, deadline: Instant) -> Result<ScopeCapture> {
            self.send(json!({"command": "GetTriggeredData"}))?;
            let ack = self.reply("GetTriggeredData", deadline)?;
            let seq = ack.get("seq").and_then(Value::as_u64);
            loop {
                match self.read(deadline, "capture")? {
                    Message::Binary(frame) => {
                        // Match the acknowledged sequence, as a capture
                        // pushed to the socket could arrive first.
                        if seq.is_none() || peek_seq(&frame) == seq {
                            return ScopeCapture::decode(&frame);
                        }
                    }
                    Message::Text(text) => {
                        daemon_reply(&text, "GetTriggeredData")?;
                    }
                    _ => {}
                }
            }
        }

        fn close(&mut self) {
            if self.closed {
                return;
            }
            self.closed = true;
            let _ = self.ws.close(None);
            let _ = self.ws.flush();
        }
    }

    /// Parse a daemon reply, unwrapping `{"Response": {...}}` and turning
    /// its `Error` response into [`Error::Stream`].
    fn daemon_reply(text: &str, command: &str) -> Result<Value> {
        let message: Value = serde_json::from_str(text)
            .map_err(|e| Error::Decode(format!("malformed {command} reply: {e}")))?;
        let reply = match message.get("Response") {
            Some(inner) => inner.clone(),
            None => message,
        };
        if reply.get("response").and_then(Value::as_str) == Some("Error") {
            let detail = reply
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown daemon error");
            return Err(Error::Stream(format!("scope daemon: {detail}")));
        }
        Ok(reply)
    }

    fn ws_error(e: tungstenite::Error, context: &str) -> Error {
        match e {
            tungstenite::Error::Io(e) => crate::tunnel::io_error(context, e),
            tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
                Error::Stream(format!("the scope stream closed during {context}"))
            }
            other => Error::Stream(format!("scope stream error during {context}: {other}")),
        }
    }

    fn handshake_error(
        e: tungstenite::HandshakeError<tungstenite::ClientHandshake<TcpStream>>,
    ) -> Error {
        match e {
            tungstenite::HandshakeError::Interrupted(_) => {
                Error::Timeout("scope stream handshake did not finish in time".to_string())
            }
            tungstenite::HandshakeError::Failure(tungstenite::Error::Http(response)) => {
                let status = response.status().as_u16();
                let message = response
                    .body()
                    .as_deref()
                    .and_then(|body| serde_json::from_slice::<Value>(body).ok())
                    .and_then(|body| {
                        body.get("error")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| format!("scope stream refused with HTTP {status}"));
                Error::Box { status, message }
            }
            tungstenite::HandshakeError::Failure(e) => ws_error(e, "the scope stream handshake"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The daemon's canonical frame, as `protocol/examples/lscp_fixture.rs`
    /// in the lager repository prints it. The box's Python decoder tests
    /// against the same bytes.
    const RUST_FIXTURE: &str = concat!(
        "4c534350010001000700000000000000081a99be1c000000000000000000304002",
        "00000002000000040000000208020000000000000000000000000000000000000700",
        "000ad7a33a0000000000000000010401000ad7233b000080be000000000000e80318",
        "fcff7f00800500fbff0000"
    );

    fn fixture() -> Vec<u8> {
        (0..RUST_FIXTURE.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&RUST_FIXTURE[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn decodes_the_daemon_fixture() {
        let capture = ScopeCapture::decode(&fixture()).unwrap();
        assert_eq!(capture.seq, 7);
        assert_eq!(capture.capture_mono_ns, 123_456_789_000);
        assert_eq!(capture.sample_interval_ns, 16.0);
        assert_eq!(capture.pre_trigger_samples, 2);
        assert_eq!(capture.post_trigger_samples, 2);
        assert_eq!(capture.samples_per_channel, 4);
        assert_eq!(capture.resolution_bits, 8);
        assert_eq!(capture.screen_samples, 0);
        assert!(capture.triggered());
        assert!(!capture.streaming());
        assert!(!capture.is_envelope());

        let labels: Vec<char> = capture.channels.iter().map(|c| c.label).collect();
        assert_eq!(labels, ['A', 'B']);
        assert_eq!(capture.channels[0].coupling, TraceCoupling::Dc);
        assert_eq!(capture.channels[1].coupling, TraceCoupling::Ac);
        assert_eq!(capture.channels[0].range_code, 7);
        assert_eq!(capture.channels[1].offset_v, -0.25);

        assert_eq!(capture.counts('A').unwrap(), [0, 1000, -1000, 32767]);
        assert_eq!(capture.counts('b').unwrap(), [-32768, 5, -5, 0]);
        assert!(capture.counts('C').is_none());
        assert!(!capture.overflowed('A'));
        assert!(capture.overflowed('B'));
    }

    #[test]
    fn volts_scale_offset_and_mark_missing_samples() {
        let capture = ScopeCapture::decode(&fixture()).unwrap();
        let a = capture.volts('A').unwrap();
        assert!((a[1] - 1.25).abs() < 1e-6);
        let b = capture.volts('B').unwrap();
        assert!(b[0].is_nan(), "NO_SAMPLE reads as NaN, not the range floor");
        assert!((b[1] - (5.0 * 0.0025 - 0.25)).abs() < 1e-6);
    }

    #[test]
    fn timing_is_relative_to_the_trigger() {
        let capture = ScopeCapture::decode(&fixture()).unwrap();
        assert_eq!(capture.sample_rate_hz(), 62_500_000.0);
        assert!((capture.duration_s() - 64e-9).abs() < 1e-15);
        let t = capture.time_axis();
        assert_eq!(t.len(), 4);
        assert!((t[0] + 32e-9).abs() < 1e-15);
        assert!(t[2].abs() < 1e-15);
    }

    #[test]
    fn malformed_frames_are_decode_errors() {
        let good = fixture();
        let cases: [(&[u8], &str); 3] = [
            (&good[..10], "too short"),
            (&good[..good.len() - 1], "payload length mismatch"),
            (&[0u8; 64], "bad magic"),
        ];
        for (bytes, expect) in cases {
            match ScopeCapture::decode(bytes) {
                Err(Error::Decode(message)) => assert!(message.contains(expect), "{message}"),
                other => panic!("expected a decode error, got {other:?}"),
            }
        }
        let mut wrong_version = good.clone();
        wrong_version[4] = 2;
        assert!(matches!(
            ScopeCapture::decode(&wrong_version),
            Err(Error::Decode(_))
        ));
        let mut bad_channel = good;
        bad_channel[64] = 26;
        assert!(matches!(
            ScopeCapture::decode(&bad_channel),
            Err(Error::Decode(_))
        ));
    }
}
