//! Interactive (bi-directional) RTT over the box's Socket.IO `/rtt`
//! namespace (feature `rtt`, box software >= 0.35.0).
//!
//! The one-way [`crate::nets::debug::DebugNet::rtt`] stream reads the
//! target's RTT **up-channel** over HTTP and cannot answer it. This session
//! speaks both directions: the up-channel arrives as `rtt_data` events and
//! writes ride to the target's RTT **down-channel** as `rtt_write` events,
//! so firmware with an interactive console can be driven from a cargo test.
//!
//! Two prerequisites, both box-side facts worth knowing before debugging a
//! "silent" session:
//!
//! - **A gdbserver must already be running** for the net — call
//!   [`crate::nets::debug::DebugNet::connect`] first. The box refuses
//!   `start_rtt` otherwise, and that refusal surfaces here as
//!   [`Error::Stream`] with the box's message.
//! - **Writing needs a firmware-declared RTT down buffer** on the channel.
//!   `defmt-rtt` alone only sets up the up buffer; with no down buffer the
//!   target silently discards what it is sent, which looks like a host-side
//!   failure and is not one.
//!
//! The RTT telnet port on the box accepts a single client, so one session
//! per probe+channel: a colliding open is refused with an "already in use"
//! error. Two channels of one probe are distinct ports and may run at once.
//!
//! Bytes are raw: firmware logging with `defmt` emits a compressed binary
//! format that must be piped through `defmt-print -e <elf>` to become text.
//! For plain-text firmware consoles, [`RttSession::wait_for`] and friends
//! work directly.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use rust_socketio::ClientBuilder;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::nets::debug::RttOptions;
use crate::nets::sio::{hex_decode, hex_encode, payload_json};

/// How long to wait for the box to confirm the RTT attach. Wider than the
/// UART equivalent: the box may search RAM for the RTT control block and
/// retry the telnet attach while the gdbserver settles.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
enum RttEvent {
    Connected { backend: String },
    Data(Vec<u8>),
    Error(String),
    Stopped,
}

/// Build the `start_rtt` payload. Optional fields are omitted entirely when
/// unset; the box treats a missing key and its default identically, but an
/// explicit `null` would be a needless difference from what the CLI sends.
fn start_payload(netname: &str, opts: &RttOptions) -> Value {
    let mut body = json!({ "netname": netname, "channel": opts.channel });
    if let Some(a) = opts.search_addr {
        body["search_addr"] = json!(a);
    }
    if let Some(s) = opts.search_size {
        body["search_size"] = json!(s);
    }
    if let Some(c) = opts.chunk_size {
        body["chunk_size"] = json!(c);
    }
    body
}

/// A live bi-directional RTT session.
///
/// Created via [`crate::nets::debug::DebugNet::rtt_interactive`]. Up-channel
/// bytes are buffered internally; pull them with [`RttSession::read`] /
/// [`RttSession::try_read`] or scan for expected output with
/// [`RttSession::wait_for`]. Writes go to the target's down-channel
/// immediately. The session is stopped cleanly on [`RttSession::stop`] or
/// drop, releasing the box's RTT telnet port for the next session.
pub struct RttSession {
    socket: Option<rust_socketio::client::Client>,
    rx: Receiver<RttEvent>,
    netname: String,
    channel: u32,
    backend: String,
    buf: Vec<u8>,
}

impl RttSession {
    pub(crate) fn open(
        base_url: &str,
        netname: String,
        opts: &RttOptions,
        bearer_token: Option<String>,
    ) -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<RttEvent>();

        let socket = {
            let tx_connected: Sender<RttEvent> = tx.clone();
            let tx_data = tx.clone();
            let tx_error = tx.clone();
            let tx_stopped = tx;
            let mut builder = ClientBuilder::new(base_url).namespace("/rtt");
            // Boxes behind an authenticating gateway need the bearer token
            // on the Socket.IO handshake too (same reverse proxy).
            if let Some(token) = &bearer_token {
                builder = builder.opening_header("Authorization", format!("Bearer {token}"));
            }
            builder
                .on("rtt_connected", move |payload, _| {
                    let info = payload_json(payload).unwrap_or(Value::Null);
                    let backend = info
                        .get("backend")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let _ = tx_connected.send(RttEvent::Connected { backend });
                })
                .on("rtt_data", move |payload, _| {
                    if let Some(bytes) = payload_json(payload)
                        .as_ref()
                        .and_then(|v| v.get("data"))
                        .and_then(Value::as_str)
                        .and_then(hex_decode)
                    {
                        let _ = tx_data.send(RttEvent::Data(bytes));
                    }
                })
                .on("error", move |payload, _| {
                    let message = payload_json(payload)
                        .as_ref()
                        .and_then(|v| v.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("unknown RTT error")
                        .to_string();
                    let _ = tx_error.send(RttEvent::Error(message));
                })
                .on("rtt_stopped", move |_, _| {
                    let _ = tx_stopped.send(RttEvent::Stopped);
                })
                .connect()
                .map_err(|e| Error::Connection(format!("Socket.IO connect failed: {e}")))?
        };

        socket
            .emit("start_rtt", start_payload(&netname, opts))
            .map_err(|e| Error::Stream(format!("could not start RTT session: {e}")))?;

        // Wait for the box to attach (or refuse: no gdbserver, port in use).
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let mut session = RttSession {
            socket: Some(socket),
            rx,
            netname,
            channel: opts.channel,
            backend: String::new(),
            buf: Vec::new(),
        };
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout(format!(
                    "box did not confirm RTT session for '{}' within {CONNECT_TIMEOUT:?}",
                    session.netname
                )));
            }
            match session.rx.recv_timeout(remaining) {
                Ok(RttEvent::Connected { backend }) => {
                    session.backend = backend;
                    return Ok(session);
                }
                // Data can beat the connected event; keep it.
                Ok(RttEvent::Data(bytes)) => session.buf.extend_from_slice(&bytes),
                Ok(RttEvent::Error(msg)) => return Err(Error::Stream(msg)),
                Ok(RttEvent::Stopped) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::Stream("RTT session closed unexpectedly".to_string()))
                }
            }
        }
    }

    /// Name of the debug net this session streams.
    pub fn netname(&self) -> &str {
        &self.netname
    }

    /// RTT channel of this session (both directions).
    pub fn channel(&self) -> u32 {
        self.channel
    }

    /// Debug backend driving the probe, as reported by the box
    /// (`"jlink"` or `"openocd"`).
    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// Write raw bytes to the target's RTT down-channel.
    ///
    /// Requires a firmware-declared down buffer on this channel; without
    /// one the target silently discards the bytes (see the module docs).
    pub fn write(&self, data: &[u8]) -> Result<()> {
        let socket = self
            .socket
            .as_ref()
            .ok_or_else(|| Error::Stream("RTT session already stopped".to_string()))?;
        socket
            .emit("rtt_write", json!({ "data": hex_encode(data) }))
            .map_err(|e| Error::Stream(format!("RTT write failed: {e}")))
    }

    /// Write a string to the target's RTT down-channel.
    pub fn write_str(&self, s: &str) -> Result<()> {
        self.write(s.as_bytes())
    }

    /// Drain incoming events into the internal buffer. Returns an error if
    /// the box reported a session error.
    fn pump(&mut self, wait: Duration) -> Result<()> {
        let mut wait = wait;
        loop {
            match self.rx.recv_timeout(wait) {
                Ok(RttEvent::Data(bytes)) => {
                    self.buf.extend_from_slice(&bytes);
                    // Something arrived: keep draining whatever is already
                    // queued without waiting again.
                    wait = Duration::ZERO;
                }
                Ok(RttEvent::Error(msg)) => return Err(Error::Stream(msg)),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => return Ok(()),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::Stream("RTT session closed unexpectedly".to_string()))
                }
            }
        }
    }

    /// Return whatever up-channel bytes arrive within `timeout`. May return
    /// an empty vector if the target was quiet.
    ///
    /// Note this waits out the **full** `timeout` when the target stays
    /// idle (data that does arrive is still drained without further
    /// waiting). Poll loops that only want what has already arrived should
    /// use [`RttSession::try_read`] instead, which never blocks.
    pub fn read(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        self.pump(timeout)?;
        Ok(std::mem::take(&mut self.buf))
    }

    /// Return the bytes already received, without waiting.
    ///
    /// Drains everything the session has queued (and anything buffered by
    /// a previous [`RttSession::wait_for`]) and returns immediately — an
    /// empty vector when the target has been quiet.
    pub fn try_read(&mut self) -> Result<Vec<u8>> {
        self.pump(Duration::ZERO)?;
        Ok(std::mem::take(&mut self.buf))
    }

    /// Accumulate up-channel output until `needle` appears or `timeout`
    /// elapses.
    ///
    /// On success returns everything received up to and including `needle`;
    /// bytes after the needle stay buffered for the next read. Only useful
    /// against plain-text firmware output — defmt frames are binary and the
    /// needle would have to match compressed bytes.
    pub fn wait_for(&mut self, needle: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(pos) = self.buf.windows(needle.len()).position(|w| w == needle) {
                let mut rest = self.buf.split_off(pos + needle.len());
                std::mem::swap(&mut self.buf, &mut rest);
                return Ok(rest);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout(format!(
                    "'{}' did not appear on RTT '{}' within {timeout:?}",
                    String::from_utf8_lossy(needle),
                    self.netname
                )));
            }
            self.pump(remaining)?;
        }
    }

    /// Stop the session cleanly (tells the box to detach and release the
    /// RTT telnet port).
    pub fn stop(mut self) -> Result<()> {
        self.shutdown();
        Ok(())
    }

    fn shutdown(&mut self) {
        if let Some(socket) = self.socket.take() {
            let _ = socket.emit("stop_rtt", json!({}));
            let _ = socket.disconnect();
        }
    }
}

impl Drop for RttSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_payload_omits_unset_options() {
        let body = start_payload("dbg", &RttOptions::default());
        assert_eq!(body, json!({ "netname": "dbg", "channel": 0 }));
    }

    #[test]
    fn start_payload_carries_explicit_options() {
        let body = start_payload(
            "dbg",
            &RttOptions {
                channel: 1,
                search_addr: Some(0x2002_0000),
                search_size: Some(0x4000),
                chunk_size: Some(4096),
            },
        );
        assert_eq!(
            body,
            json!({
                "netname": "dbg",
                "channel": 1,
                "search_addr": 0x2002_0000u64,
                "search_size": 0x4000,
                "chunk_size": 4096,
            })
        );
    }
}
