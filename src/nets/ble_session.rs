//! BLE GATT sessions over the box's Socket.IO `/ble` namespace (feature
//! `ble-session`).
//!
//! [`crate::LagerBox::ble`] connects, enumerates and disconnects inside one
//! HTTP call. A [`BleSession`] instead holds one GATT connection open across
//! many operations, so a cargo test can subscribe to a characteristic, write
//! requests, read values and receive notifications the way firmware on a
//! phone or gateway would. The session moves bytes only: there is no framing,
//! reassembly or application protocol on top.
//!
//! Things worth knowing before debugging a session that misbehaves:
//!
//! - **One session per box.** The box has one Bluetooth adapter and an open
//!   session owns it. A second opener (from this process or anywhere else)
//!   is refused with [`BleErrorKind::AdapterBusy`], and while a session is
//!   open the box also refuses [`crate::LagerBox::ble`] scans and
//!   [`crate::LagerBox::blufi`] operations instead of queueing them. Close
//!   the session (or drop it) to free the adapter.
//! - **Subscribe before writing.** A peripheral that answers a write with a
//!   notification may send it before a late subscribe has written the CCCD.
//!   [`BleSession::subscribe`] returns only once notifications are enabled,
//!   so subscribe first, then write.
//! - **The MTU is reported, not chosen.** BlueZ negotiates the ATT MTU
//!   itself at connect time (offering the host-wide `ExchangeMTU`, 517 by
//!   default). [`BleSession::mtu`] reports the result;
//!   [`BleSession::mtu_is_measured`] is `false` when the box could not read
//!   it and assumed the LE default of 23.
//! - **Long writes.** A with-response write longer than
//!   [`BleSession::max_write_len`] (`mtu - 3`, at most 512) makes BlueZ use the ATT long
//!   write procedure (Prepare Write + Execute Write), which some simple
//!   peripherals reject. [`WriteOptions::chunked`] splits the payload into
//!   `max_write_len()`-byte ATT writes instead, sent in order with nothing else from
//!   the session in between.
//! - **Idle timeout.** The box closes a session after
//!   [`BleSessionOptions::idle_timeout`] with no client *operation*.
//!   Incoming notifications do not count as activity; a test that only
//!   listens should call [`BleSession::ping`] now and then.
//! - **Connection parameters are not controllable.** Interval, latency and
//!   supervision timeout come from the box host's BlueZ defaults
//!   (`/etc/bluetooth/main.conf` `[LE]`) and whatever the peripheral
//!   requests.
//! - **No box lock.** Opening a session does not take the box lock. If
//!   several people or CI jobs share the box, hold it with
//!   [`crate::LagerBox::lock`] (or [`crate::LagerBox::lock_guard`]) around
//!   the session.
//!
//! Failures the box reports carry a [`BleErrorKind`] in
//! [`Error::Ble`]; transport failures stay [`Error::Connection`] /
//! [`Error::Stream`], and client-side waits that run out are
//! [`Error::Timeout`].
//!
//! ```no_run
//! # fn main() -> lager::Result<()> {
//! use std::time::Duration;
//! use lager::{BleSessionOptions, LagerBox, WriteOptions};
//!
//! let lager = LagerBox::from_env()?;
//! let mut s = lager.ble_session("AA:BB:CC:DD:EE:01", BleSessionOptions::default())?;
//! s.subscribe("12345678-1234-5678-1234-56789abcdef2")?;
//! s.write("12345678-1234-5678-1234-56789abcdef1", b"ping", WriteOptions::chunked())?;
//! let reply = s.recv(Duration::from_secs(2))?;
//! assert_eq!(reply.data, b"pong");
//! s.close()
//! # }
//! ```

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

use rust_socketio::ClientBuilder;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{BleErrorKind, Error, Result};
use crate::nets::sio::{hex_decode, hex_encode, payload_json};
use crate::wire::BleService;

/// Extra client-side wait on top of `connect_timeout` for `ble_open`: the
/// box allows connect plus service discovery `connect_timeout + 20` s, and
/// may first wait for the adapter lock.
const OPEN_OVERHEAD: Duration = Duration::from_secs(25);

/// No attribute value is longer than this (ATT), so no single write can carry
/// more; the box refuses a longer unchunked write.
const ATT_MAX_VALUE: usize = 512;

/// How long [`BleSession::close`] waits for the box to confirm the close.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// The box's bound on each ATT write of a chunked write. A chunked write
/// that keeps making progress can take this long per chunk, so the client
/// waits for it in proportion (see [`BleSession::write_timeout`]).
const BOX_CHUNK_BOUND: Duration = Duration::from_secs(10);

/// Options for [`crate::LagerBox::ble_session`].
#[derive(Debug, Clone)]
pub struct BleSessionOptions {
    /// How long the box may take to find and connect to the device
    /// (box-side range 1–120 s). Default 10 s.
    pub connect_timeout: Duration,
    /// The box closes the session after this long with no client operation
    /// (box-side range 5–3600 s). Notifications do not count; see
    /// [`BleSession::ping`]. Default 300 s.
    pub idle_timeout: Duration,
    /// Client-side wait for the box's answer to each operation after the
    /// open. The box bounds each operation at 10 s, so the default of 30 s
    /// only runs out when the box stalls. A chunked write waits longer when
    /// it has many chunks: up to 10 s per chunk plus 5 s, since the box
    /// bounds each chunk separately.
    pub op_timeout: Duration,
    /// Optional label shown to other clients that find the adapter busy
    /// (and in `lager ble sessions`), e.g. the test or CI job name.
    pub holder: Option<String>,
}

impl Default for BleSessionOptions {
    fn default() -> Self {
        BleSessionOptions {
            connect_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(300),
            op_timeout: Duration::from_secs(30),
            holder: None,
        }
    }
}

/// How [`BleSession::write`] sends its payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOptions {
    /// Write with response (ATT Write Request, acknowledged by the
    /// peripheral) when `true`; write without response (ATT Write Command)
    /// when `false`. The characteristic must support the chosen kind.
    pub response: bool,
    /// Split the payload into ATT writes of at most
    /// [`BleSession::max_write_len`] bytes, sent in order. Without it a
    /// longer with-response write becomes a BlueZ long write.
    pub chunk: bool,
}

impl Default for WriteOptions {
    /// With response, not chunked.
    fn default() -> Self {
        WriteOptions {
            response: true,
            chunk: false,
        }
    }
}

impl WriteOptions {
    /// Write without response, not chunked.
    pub fn without_response() -> Self {
        WriteOptions {
            response: false,
            chunk: false,
        }
    }

    /// Write with response, chunked to [`BleSession::max_write_len`] bytes
    /// per ATT write.
    pub fn chunked() -> Self {
        WriteOptions {
            response: true,
            chunk: true,
        }
    }
}

/// One notification or indication received from the peripheral.
#[derive(Debug, Clone, PartialEq)]
pub struct BleNotification {
    /// Per-session counter assigned by the box, starting at 1. A gap means
    /// the box lost data; it never does without also ending the session.
    pub seq: u64,
    /// UUID of the characteristic that sent it, as the box reports it.
    pub char_uuid: String,
    /// ATT handle of that characteristic.
    pub handle: u16,
    /// Box wall-clock time (Unix seconds) at which BlueZ delivered it. Good
    /// for ordering and rough latency, not sub-millisecond timing.
    pub timestamp: f64,
    /// The characteristic value.
    pub data: Vec<u8>,
}

/// The connected device as the box reports it on open and from
/// [`BleSession::info`].
#[derive(Debug, Clone, Deserialize)]
pub struct BleSessionInfo {
    /// Device address, `XX:XX:XX:XX:XX:XX`.
    #[serde(default)]
    pub address: String,
    /// Negotiated ATT MTU.
    #[serde(default = "default_mtu")]
    pub mtu: u16,
    /// `"bluez"` when the MTU was read from BlueZ, `"default"` when the box
    /// assumed 23.
    #[serde(default)]
    pub mtu_source: String,
    /// The GATT table, with each characteristic's handle.
    #[serde(default)]
    pub services: Vec<BleService>,
}

fn default_mtu() -> u16 {
    23
}

/// A `ble_result`: the value, or `(code, message)`.
type Outcome = std::result::Result<Value, (String, String)>;

/// Events forwarded from the Socket.IO callbacks.
#[derive(Debug)]
enum SessionEvent {
    Result { seq: u64, outcome: Outcome },
    Notify(Vec<BleNotification>),
    Closed { reason: String, message: String },
    Error(String),
}

/// A characteristic, named by UUID or by ATT handle.
#[derive(Debug, Clone, Copy)]
enum Target<'a> {
    Uuid(&'a str),
    Handle(u16),
}

// ---------------------------------------------------------------------------
// Payload builders and event parsers (pure, unit-tested)
// ---------------------------------------------------------------------------

/// `ble_open` fields without `seq`. `holder` is omitted when unset.
fn open_payload(address: &str, opts: &BleSessionOptions) -> Value {
    let mut body = json!({
        "address": address,
        "connect_timeout": opts.connect_timeout.as_secs_f64(),
        "idle_timeout": opts.idle_timeout.as_secs_f64(),
    });
    if let Some(holder) = &opts.holder {
        body["holder"] = json!(holder);
    }
    body
}

/// `{char}` or `{handle}`: the box resolves exactly one of the two.
fn target_payload(target: Target<'_>) -> Value {
    match target {
        Target::Uuid(uuid) => json!({ "char": uuid }),
        Target::Handle(handle) => json!({ "handle": handle }),
    }
}

fn write_payload(target: Target<'_>, data: &[u8], opts: WriteOptions) -> Value {
    let mut body = target_payload(target);
    body["data"] = json!(hex_encode(data));
    body["response"] = json!(opts.response);
    body["chunk"] = json!(opts.chunk);
    body
}

fn parse_result(v: &Value) -> Option<SessionEvent> {
    let seq = v.get("seq").and_then(Value::as_u64)?;
    let outcome = if v.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(v.get("value").cloned().unwrap_or_else(|| json!({})))
    } else {
        let code = v
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("ble_error")
            .to_string();
        let message = v
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the box reported a BLE session error")
            .to_string();
        Err((code, message))
    };
    Some(SessionEvent::Result { seq, outcome })
}

/// Parse a `ble_notify` batch. An item with missing fields or data that is
/// not valid hex is skipped: the box never sends one, and the resulting gap
/// in [`BleNotification::seq`] still shows that something was lost.
fn parse_notify(v: &Value) -> Vec<BleNotification> {
    let Some(items) = v.get("items").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            Some(BleNotification {
                seq: item.get("n").and_then(Value::as_u64)?,
                char_uuid: item.get("char").and_then(Value::as_str)?.to_string(),
                handle: u16::try_from(item.get("handle").and_then(Value::as_u64)?).ok()?,
                timestamp: item.get("ts").and_then(Value::as_f64).unwrap_or(0.0),
                data: hex_decode(item.get("data").and_then(Value::as_str)?)?,
            })
        })
        .collect()
}

fn parse_closed(v: &Value) -> SessionEvent {
    let field = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    SessionEvent::Closed {
        reason: field("reason"),
        message: field("message"),
    }
}

fn result_error(code: &str, message: String) -> Error {
    Error::Ble {
        kind: BleErrorKind::from_code(code),
        message,
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// A live BLE GATT session: one connection to one peripheral, held open by
/// the box until [`BleSession::close`], drop, the peripheral disconnecting,
/// or the idle timeout.
///
/// Created via [`crate::LagerBox::ble_session`]. Every operation blocks
/// until the box answers it (or [`BleSessionOptions::op_timeout`] runs out),
/// so an error surfaces at the call that caused it. Notifications that
/// arrive in the meantime are buffered in arrival order and never dropped;
/// pull them with [`BleSession::recv`] / [`BleSession::try_recv`].
///
/// Characteristics are named by UUID: full 128-bit, or a 16/32-bit short
/// form like `"2a19"` (the box expands it). A UUID that appears in more than
/// one service fails with [`BleErrorKind::AmbiguousCharacteristic`]; use the
/// `*_handle` variants with a handle from [`BleSession::services`] instead.
///
/// Once the session has ended (see [`BleSession::close_reason`]), every
/// operation fails with [`Error::Ble`] whose kind matches the reason, e.g.
/// [`BleErrorKind::Disconnected`] when the link dropped.
pub struct BleSession {
    socket: Option<rust_socketio::client::Client>,
    rx: Receiver<SessionEvent>,
    next_seq: u64,
    op_timeout: Duration,
    info: BleSessionInfo,
    notifications: VecDeque<BleNotification>,
    /// Set once `ble_closed` arrived: (reason, error to report).
    closed: Option<(String, BleErrorKind, String)>,
    /// Whether `ble_close` was already sent (by [`BleSession::close`]).
    close_sent: bool,
    /// Events that would have been emitted, for socket-free tests.
    #[cfg(test)]
    sent: Vec<(String, Value)>,
}

impl BleSession {
    pub(crate) fn open(
        base_url: &str,
        address: &str,
        opts: BleSessionOptions,
        bearer_token: Option<String>,
    ) -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<SessionEvent>();

        let socket = {
            let tx_result: Sender<SessionEvent> = tx.clone();
            let tx_notify = tx.clone();
            let tx_closed = tx.clone();
            let tx_error = tx;
            let mut builder = ClientBuilder::new(base_url).namespace("/ble");
            // Boxes behind an authenticating gateway need the bearer token
            // on the Socket.IO handshake too (same reverse proxy).
            if let Some(token) = &bearer_token {
                builder = builder.opening_header("Authorization", format!("Bearer {token}"));
            }
            builder
                .on("ble_result", move |payload, _| {
                    if let Some(ev) = payload_json(payload).as_ref().and_then(parse_result) {
                        let _ = tx_result.send(ev);
                    }
                })
                .on("ble_notify", move |payload, _| {
                    let items = payload_json(payload)
                        .as_ref()
                        .map(parse_notify)
                        .unwrap_or_default();
                    if !items.is_empty() {
                        let _ = tx_notify.send(SessionEvent::Notify(items));
                    }
                })
                .on("ble_closed", move |payload, _| {
                    let v = payload_json(payload).unwrap_or(Value::Null);
                    let _ = tx_closed.send(parse_closed(&v));
                })
                .on("error", move |payload, _| {
                    let message = payload_json(payload)
                        .as_ref()
                        .and_then(|v| v.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("unknown BLE session error")
                        .to_string();
                    let _ = tx_error.send(SessionEvent::Error(message));
                })
                .connect()
                .map_err(|e| Error::Connection(format!("Socket.IO connect failed: {e}")))?
        };

        let mut session = BleSession::new(Some(socket), rx, opts.op_timeout);
        // On failure `session` drops here, which disconnects the socket.
        session.start(address, &opts)?;
        Ok(session)
    }

    fn new(
        socket: Option<rust_socketio::client::Client>,
        rx: Receiver<SessionEvent>,
        op_timeout: Duration,
    ) -> Self {
        BleSession {
            socket,
            rx,
            next_seq: 1,
            op_timeout,
            info: BleSessionInfo {
                address: String::new(),
                mtu: default_mtu(),
                mtu_source: String::new(),
                services: Vec::new(),
            },
            notifications: VecDeque::new(),
            closed: None,
            close_sent: false,
            #[cfg(test)]
            sent: Vec::new(),
        }
    }

    /// Emit `ble_open` and wait for the box to connect and enumerate.
    fn start(&mut self, address: &str, opts: &BleSessionOptions) -> Result<()> {
        let value = self.call(
            "ble_open",
            open_payload(address, opts),
            opts.connect_timeout + OPEN_OVERHEAD,
        )?;
        self.info = serde_json::from_value(value)?;
        Ok(())
    }

    // -- accessors ------------------------------------------------------------

    /// Address of the connected device, as the box normalized it.
    pub fn address(&self) -> &str {
        &self.info.address
    }

    /// Negotiated ATT MTU (23 when it could not be measured; see
    /// [`BleSession::mtu_is_measured`]).
    pub fn mtu(&self) -> u16 {
        self.info.mtu
    }

    /// Largest payload one ATT write carries: `mtu - 3`, and never more
    /// than 512 (the ATT limit on an attribute value). Longer with-response
    /// writes become BlueZ long writes unless sent with
    /// [`WriteOptions::chunked`]; the box refuses any unchunked write over
    /// 512 bytes with [`BleErrorKind::InvalidArgument`].
    pub fn max_write_len(&self) -> usize {
        usize::from(self.info.mtu.saturating_sub(3)).min(ATT_MAX_VALUE)
    }

    /// Whether [`BleSession::mtu`] was read from BlueZ (`true`) or assumed
    /// to be the LE default of 23 because the box host's BlueZ does not
    /// publish it (`false`).
    pub fn mtu_is_measured(&self) -> bool {
        self.info.mtu_source == "bluez"
    }

    /// The device's GATT table as enumerated at open (or at the last
    /// [`BleSession::info`]), including each characteristic's handle.
    pub fn services(&self) -> &[BleService] {
        &self.info.services
    }

    /// Whether the session has ended (the box sent its final `ble_closed`).
    /// Updated whenever the session processes incoming events.
    pub fn is_closed(&self) -> bool {
        self.closed.is_some()
    }

    /// Why the session ended, as the box reported it: `"disconnected"`,
    /// `"idle_timeout"`, `"overflow"`, `"released"`, `"timeout"`,
    /// `"protocol_error"`, `"bluez_unavailable"`, `"shutdown"` or
    /// `"client"`. `None` while the session is open.
    pub fn close_reason(&self) -> Option<&str> {
        self.closed.as_ref().map(|(reason, _, _)| reason.as_str())
    }

    // -- operations -----------------------------------------------------------

    /// Enable notifications (or indications, when that is all the
    /// characteristic supports) on the characteristic `uuid`. Returns once
    /// the CCCD is written, so a notification sent right after is captured.
    /// Subscribing twice is a no-op.
    pub fn subscribe(&mut self, uuid: &str) -> Result<()> {
        self.op("ble_subscribe", target_payload(Target::Uuid(uuid)))
            .map(drop)
    }

    /// [`BleSession::subscribe`] by ATT handle.
    pub fn subscribe_handle(&mut self, handle: u16) -> Result<()> {
        self.op("ble_subscribe", target_payload(Target::Handle(handle)))
            .map(drop)
    }

    /// Disable notifications on the characteristic `uuid`. Unsubscribing a
    /// characteristic that is not subscribed is a no-op. Notifications
    /// already buffered stay buffered.
    pub fn unsubscribe(&mut self, uuid: &str) -> Result<()> {
        self.op("ble_unsubscribe", target_payload(Target::Uuid(uuid)))
            .map(drop)
    }

    /// [`BleSession::unsubscribe`] by ATT handle.
    pub fn unsubscribe_handle(&mut self, handle: u16) -> Result<()> {
        self.op("ble_unsubscribe", target_payload(Target::Handle(handle)))
            .map(drop)
    }

    /// Write `data` (up to 64 KiB) to the characteristic `uuid`. Returns
    /// once every ATT write has completed (for with-response writes, once
    /// the peripheral acknowledged them).
    pub fn write(&mut self, uuid: &str, data: &[u8], opts: WriteOptions) -> Result<()> {
        let timeout = self.write_timeout(data.len(), opts);
        self.call("ble_write", write_payload(Target::Uuid(uuid), data, opts), timeout)
            .map(drop)
    }

    /// [`BleSession::write`] by ATT handle.
    pub fn write_handle(&mut self, handle: u16, data: &[u8], opts: WriteOptions) -> Result<()> {
        let timeout = self.write_timeout(data.len(), opts);
        self.call(
            "ble_write",
            write_payload(Target::Handle(handle), data, opts),
            timeout,
        )
        .map(drop)
    }

    /// Read the value of the characteristic `uuid`.
    pub fn read(&mut self, uuid: &str) -> Result<Vec<u8>> {
        let value = self.op("ble_read", target_payload(Target::Uuid(uuid)))?;
        read_data(&value)
    }

    /// [`BleSession::read`] by ATT handle.
    pub fn read_handle(&mut self, handle: u16) -> Result<Vec<u8>> {
        let value = self.op("ble_read", target_payload(Target::Handle(handle)))?;
        read_data(&value)
    }

    /// Ask the box for the device's current info (address, MTU, GATT table)
    /// and refresh what [`BleSession::mtu`] and [`BleSession::services`]
    /// report.
    pub fn info(&mut self) -> Result<BleSessionInfo> {
        let value = self.op("ble_info", json!({}))?;
        self.info = serde_json::from_value(value)?;
        Ok(self.info.clone())
    }

    /// Reset the box's idle timer and do nothing else. Call it now and then
    /// from a test that only listens for notifications, which do not count
    /// as activity.
    pub fn ping(&mut self) -> Result<()> {
        self.op("ble_ping", json!({})).map(drop)
    }

    /// Return the next notification, waiting up to `timeout` for one.
    ///
    /// Buffered notifications come first, in arrival order. After the
    /// session has ended, every notification received before the end is
    /// still returned; only then does this fail with [`Error::Ble`] (kind
    /// matching [`BleSession::close_reason`], e.g.
    /// [`BleErrorKind::Disconnected`]). Fails with [`Error::Timeout`] when
    /// nothing arrives in time.
    pub fn recv(&mut self, timeout: Duration) -> Result<BleNotification> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(n) = self.notifications.pop_front() {
                return Ok(n);
            }
            self.check_open()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(remaining) {
                Ok(ev) => {
                    self.absorb(ev)?;
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(Error::Timeout(format!(
                        "no BLE notification from {} within {timeout:?}",
                        self.info.address
                    )))
                }
                Err(RecvTimeoutError::Disconnected) => return Err(channel_gone()),
            }
        }
    }

    /// Return the next notification if one has already arrived, without
    /// waiting: `Ok(None)` when there is none. Like [`BleSession::recv`],
    /// fails with [`Error::Ble`] only after the session has ended **and**
    /// every buffered notification has been returned.
    pub fn try_recv(&mut self) -> Result<Option<BleNotification>> {
        loop {
            match self.rx.try_recv() {
                Ok(ev) => {
                    self.absorb(ev)?;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.notifications.is_empty() && self.closed.is_none() {
                        return Err(channel_gone());
                    }
                    break;
                }
            }
        }
        if let Some(n) = self.notifications.pop_front() {
            return Ok(Some(n));
        }
        self.check_open()?;
        Ok(None)
    }

    /// Close the session: the box disconnects from the device and frees the
    /// adapter. Waits briefly for the box to confirm, then drops the
    /// Socket.IO connection (which ends the session on the box regardless).
    /// Best effort: always returns `Ok`. Dropping the session does the same
    /// without waiting.
    pub fn close(mut self) -> Result<()> {
        if self.closed.is_none() {
            if let Ok(seq) = self.emit("ble_close", json!({})) {
                self.close_sent = true;
                // The box answers the close, then sends ble_closed.
                let _ = self.await_result(seq, CLOSE_WAIT);
            }
        }
        self.shutdown();
        Ok(())
    }

    // -- internals ------------------------------------------------------------

    fn check_open(&self) -> Result<()> {
        match &self.closed {
            Some((_, kind, message)) => Err(Error::Ble {
                kind: *kind,
                message: message.clone(),
            }),
            None => Ok(()),
        }
    }

    /// Send one event with the next `seq` and return that `seq`.
    fn emit(&mut self, event: &str, mut payload: Value) -> Result<u64> {
        self.check_open()?;
        let seq = self.next_seq;
        self.next_seq += 1;
        payload["seq"] = json!(seq);
        match &self.socket {
            Some(socket) => socket
                .emit(event, payload)
                .map_err(|e| Error::Stream(format!("BLE session {event} failed: {e}")))?,
            None => {
                #[cfg(test)]
                self.sent.push((event.to_string(), payload));
                #[cfg(not(test))]
                return Err(Error::Stream("BLE session already closed".to_string()));
            }
        }
        Ok(seq)
    }

    fn call(&mut self, event: &str, payload: Value, timeout: Duration) -> Result<Value> {
        let seq = self.emit(event, payload)?;
        self.await_result(seq, timeout)
    }

    fn op(&mut self, event: &str, payload: Value) -> Result<Value> {
        self.call(event, payload, self.op_timeout)
    }

    /// Client-side wait for a write of `len` bytes: `op_timeout`, or for a
    /// chunked write the box's per-chunk bound times the chunk count plus
    /// 5 s, whichever is longer. A stalled chunk still fails fast: the box
    /// reports it as `timeout` after 10 s.
    fn write_timeout(&self, len: usize, opts: WriteOptions) -> Duration {
        if !opts.chunk {
            return self.op_timeout;
        }
        let chunks = len.div_ceil(self.max_write_len().max(1)).max(1);
        let chunked = BOX_CHUNK_BOUND * u32::try_from(chunks).unwrap_or(u32::MAX)
            + Duration::from_secs(5);
        self.op_timeout.max(chunked)
    }

    /// Record one incoming event. Returns a result event for the caller to
    /// match against its `seq`; results nobody waits for any more (a late
    /// answer after a client-side timeout) are dropped by the caller.
    fn absorb(&mut self, ev: SessionEvent) -> Result<Option<(u64, Outcome)>> {
        match ev {
            SessionEvent::Result { seq, outcome } => Ok(Some((seq, outcome))),
            SessionEvent::Notify(items) => {
                self.notifications.extend(items);
                Ok(None)
            }
            SessionEvent::Closed { reason, message } => {
                let kind = BleErrorKind::from_close_reason(&reason);
                let message = if message.is_empty() {
                    format!("BLE session closed ({reason})")
                } else {
                    message
                };
                self.closed = Some((reason, kind, message));
                Ok(None)
            }
            SessionEvent::Error(message) => Err(Error::Stream(message)),
        }
    }

    /// Pump events until the `ble_result` for `seq` arrives, buffering
    /// notifications that come first.
    fn await_result(&mut self, seq: u64, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let ev = match self.rx.recv_timeout(remaining) {
                Ok(ev) => ev,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(Error::Timeout(format!(
                        "box did not answer BLE session operation {seq} within {timeout:?}"
                    )))
                }
                Err(RecvTimeoutError::Disconnected) => return Err(channel_gone()),
            };
            if let Some((got, outcome)) = self.absorb(ev)? {
                if got == seq {
                    return outcome.map_err(|(code, message)| result_error(&code, message));
                }
                continue;
            }
            // The box answers an operation before it announces the end of
            // the session, so a close that arrives first means this
            // operation will only ever get `not_open`: report the real
            // reason now instead.
            self.check_open()?;
        }
    }

    fn shutdown(&mut self) {
        if let Some(socket) = self.socket.take() {
            if self.closed.is_none() && !self.close_sent {
                let seq = self.next_seq;
                self.next_seq += 1;
                let _ = socket.emit("ble_close", json!({ "seq": seq }));
            }
            // The box also ends the session when the connection goes away.
            let _ = socket.disconnect();
        }
    }
}

impl Drop for BleSession {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl std::fmt::Debug for BleSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BleSession")
            .field("address", &self.info.address)
            .field("mtu", &self.info.mtu)
            .field("buffered", &self.notifications.len())
            .field("close_reason", &self.close_reason())
            .finish()
    }
}

fn read_data(value: &Value) -> Result<Vec<u8>> {
    let hex = value
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Decode("BLE read result has no data".to_string()))?;
    hex_decode(hex).ok_or_else(|| Error::Decode(format!("BLE read returned invalid hex: {hex:?}")))
}

fn channel_gone() -> Error {
    Error::Stream("BLE session closed unexpectedly".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_write_len_is_capped_at_the_att_limit() {
        let (mut s, _tx) = detached();
        assert_eq!(s.max_write_len(), 244); // mtu 247
        s.info.mtu = 517;
        assert_eq!(s.max_write_len(), 512);
        s.info.mtu = 0;
        assert_eq!(s.max_write_len(), 0);
    }

    #[test]
    fn chunked_writes_wait_in_proportion_to_their_chunks() {
        let (s, _tx) = detached(); // mtu 247 -> 244-byte chunks, op_timeout 200 ms
        let plain = WriteOptions::default();
        assert_eq!(s.write_timeout(60_000, plain), s.op_timeout);
        assert_eq!(s.write_timeout(1, WriteOptions::chunked()), Duration::from_secs(15));
        // 600 bytes -> 3 chunks
        assert_eq!(s.write_timeout(600, WriteOptions::chunked()), Duration::from_secs(35));
    }

    const WRITE_UUID: &str = "12345678-1234-5678-1234-56789abcdef1";
    const NOTIFY_UUID: &str = "12345678-1234-5678-1234-56789abcdef2";

    /// A socket-free session plus the sender that plays the box.
    fn detached() -> (BleSession, Sender<SessionEvent>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut s = BleSession::new(None, rx, Duration::from_millis(200));
        s.info = serde_json::from_value(open_value()).unwrap();
        (s, tx)
    }

    fn open_value() -> Value {
        json!({
            "address": "AA:BB:CC:DD:EE:01",
            "mtu": 247,
            "mtu_source": "bluez",
            "services": [{
                "uuid": "12345678-1234-5678-1234-56789abcdef0",
                "description": "Vendor specific",
                "characteristics": [
                    {"uuid": WRITE_UUID, "handle": 12, "description": "Vendor specific",
                     "properties": ["write", "write-without-response"]},
                    {"uuid": NOTIFY_UUID, "handle": 14, "description": "Vendor specific",
                     "properties": ["notify"]}
                ]
            }]
        })
    }

    fn ok(seq: u64, value: Value) -> SessionEvent {
        parse_result(&json!({ "seq": seq, "ok": true, "value": value })).unwrap()
    }

    fn notify(ns: &[(u64, &str)]) -> SessionEvent {
        let items: Vec<Value> = ns
            .iter()
            .map(|(n, data)| {
                json!({ "n": n, "char": NOTIFY_UUID, "handle": 14, "ts": 1.5, "data": data })
            })
            .collect();
        SessionEvent::Notify(parse_notify(&json!({ "items": items })))
    }

    fn closed(reason: &str, message: &str) -> SessionEvent {
        parse_closed(
            &json!({ "reason": reason, "message": message, "address": "AA:BB:CC:DD:EE:01" }),
        )
    }

    #[test]
    fn open_payload_omits_unset_holder() {
        let body = open_payload("AA:BB:CC:DD:EE:01", &BleSessionOptions::default());
        assert_eq!(
            body,
            json!({ "address": "AA:BB:CC:DD:EE:01", "connect_timeout": 10.0, "idle_timeout": 300.0 })
        );
    }

    #[test]
    fn open_payload_carries_holder_and_timeouts() {
        let opts = BleSessionOptions {
            connect_timeout: Duration::from_millis(2500),
            idle_timeout: Duration::from_secs(60),
            op_timeout: Duration::from_secs(5),
            holder: Some("ci-job".to_string()),
        };
        assert_eq!(
            open_payload("AA:BB:CC:DD:EE:01", &opts),
            json!({ "address": "AA:BB:CC:DD:EE:01", "connect_timeout": 2.5,
                    "idle_timeout": 60.0, "holder": "ci-job" })
        );
    }

    #[test]
    fn target_and_write_payloads() {
        assert_eq!(
            target_payload(Target::Uuid("2a19")),
            json!({ "char": "2a19" })
        );
        assert_eq!(target_payload(Target::Handle(14)), json!({ "handle": 14 }));
        assert_eq!(
            write_payload(
                Target::Uuid(WRITE_UUID),
                &[0x01, 0xab],
                WriteOptions::default()
            ),
            json!({ "char": WRITE_UUID, "data": "01ab", "response": true, "chunk": false })
        );
        assert_eq!(
            write_payload(Target::Handle(12), b"", WriteOptions::without_response()),
            json!({ "handle": 12, "data": "", "response": false, "chunk": false })
        );
        assert_eq!(
            WriteOptions::chunked(),
            WriteOptions {
                response: true,
                chunk: true
            }
        );
    }

    #[test]
    fn result_success_and_error_parsing() {
        match parse_result(&json!({ "seq": 3, "ok": true, "value": { "bytes": 4 } })) {
            Some(SessionEvent::Result {
                seq: 3,
                outcome: Ok(v),
            }) => assert_eq!(v["bytes"], 4),
            other => panic!("unexpected {other:?}"),
        }
        match parse_result(&json!({ "seq": 4, "ok": false, "code": "not_permitted",
                                     "message": "no write" }))
        {
            Some(SessionEvent::Result {
                seq: 4,
                outcome: Err((code, msg)),
            }) => {
                assert_eq!(code, "not_permitted");
                assert_eq!(msg, "no write");
            }
            other => panic!("unexpected {other:?}"),
        }
        // A result without a usable seq cannot be matched to anything.
        assert!(
            parse_result(&json!({ "seq": null, "ok": false, "code": "invalid_argument" }))
                .is_none()
        );
    }

    #[test]
    fn codes_map_to_kinds() {
        use BleErrorKind::*;
        for (code, kind) in [
            ("device_not_found", DeviceNotFound),
            ("connect_failed", ConnectFailed),
            ("adapter_busy", AdapterBusy),
            ("bluez_unavailable", BluezUnavailable),
            ("session_active", SessionActive),
            ("not_open", NotOpen),
            ("unknown_characteristic", UnknownCharacteristic),
            ("ambiguous_characteristic", AmbiguousCharacteristic),
            ("not_permitted", NotPermitted),
            ("invalid_argument", InvalidArgument),
            ("disconnected", Disconnected),
            ("timeout", Timeout),
            ("protocol_error", ProtocolError),
            ("ble_error", Other),
            ("something_new", Other),
        ] {
            assert_eq!(BleErrorKind::from_code(code), kind, "{code}");
        }
        for (reason, kind) in [
            ("disconnected", Disconnected),
            ("idle_timeout", IdleTimeout),
            ("overflow", Overflow),
            ("released", Released),
            ("timeout", Timeout),
            ("protocol_error", ProtocolError),
            ("bluez_unavailable", BluezUnavailable),
            ("client", NotOpen),
            ("shutdown", Other),
        ] {
            assert_eq!(BleErrorKind::from_close_reason(reason), kind, "{reason}");
        }
        let err = result_error("adapter_busy", "held by ci-job".to_string());
        assert!(matches!(
            err,
            Error::Ble {
                kind: AdapterBusy,
                ..
            }
        ));
        assert!(err.to_string().contains("adapter_busy"));
        assert!(err.to_string().contains("held by ci-job"));
    }

    #[test]
    fn notify_batch_parsing_skips_bad_items() {
        let items = parse_notify(&json!({ "items": [
            { "n": 1, "char": NOTIFY_UUID, "handle": 14, "ts": 1700000000.25, "data": "0102" },
            { "n": 2, "char": NOTIFY_UUID, "handle": 14, "ts": 1700000000.5, "data": "zz" },
            { "n": 3, "char": NOTIFY_UUID, "handle": 14, "ts": 1700000000.75, "data": "" },
            { "n": 4, "char": NOTIFY_UUID, "handle": 70000, "ts": 1.0, "data": "00" },
        ]}));
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0],
            BleNotification {
                seq: 1,
                char_uuid: NOTIFY_UUID.to_string(),
                handle: 14,
                timestamp: 1700000000.25,
                data: vec![1, 2],
            }
        );
        assert_eq!(items[1].seq, 3);
        assert!(items[1].data.is_empty());
        assert!(parse_notify(&json!({})).is_empty());
    }

    #[test]
    fn op_waits_for_its_seq_and_buffers_notifications() {
        let (mut s, tx) = detached();
        tx.send(notify(&[(1, "aa"), (2, "bb")])).unwrap();
        tx.send(ok(
            1,
            json!({ "char": WRITE_UUID, "handle": 12, "bytes": 2, "chunks": 1 }),
        ))
        .unwrap();
        tx.send(notify(&[(3, "cc")])).unwrap();
        s.write(WRITE_UUID, &[0xde, 0xad], WriteOptions::default())
            .unwrap();

        assert_eq!(s.sent.len(), 1);
        assert_eq!(s.sent[0].0, "ble_write");
        assert_eq!(
            s.sent[0].1,
            json!({ "seq": 1, "char": WRITE_UUID, "data": "dead", "response": true, "chunk": false })
        );
        // Two notifications arrived before the result; the third is still
        // queued in the channel.
        assert_eq!(s.try_recv().unwrap().unwrap().data, vec![0xaa]);
        assert_eq!(s.recv(Duration::from_millis(50)).unwrap().data, vec![0xbb]);
        assert_eq!(s.try_recv().unwrap().unwrap().seq, 3);
        assert!(s.try_recv().unwrap().is_none());
    }

    #[test]
    fn stale_result_does_not_satisfy_the_wait() {
        let (mut s, tx) = detached();
        s.next_seq = 5;
        // A late answer to an operation that already timed out client-side.
        tx.send(ok(4, json!({ "data": "ffff" }))).unwrap();
        tx.send(ok(
            5,
            json!({ "char": NOTIFY_UUID, "handle": 14, "data": "0a0b" }),
        ))
        .unwrap();
        assert_eq!(s.read_handle(14).unwrap(), vec![0x0a, 0x0b]);
        assert_eq!(s.sent[0].1, json!({ "seq": 5, "handle": 14 }));
        assert_eq!(s.next_seq, 6);
    }

    #[test]
    fn seq_increments_across_every_event() {
        let (mut s, tx) = detached();
        tx.send(ok(
            1,
            json!({ "char": NOTIFY_UUID, "handle": 14, "mode": "notify" }),
        ))
        .unwrap();
        tx.send(ok(2, json!({ "idle_timeout": 300.0, "idle_s": 0.0 })))
            .unwrap();
        tx.send(ok(3, open_value())).unwrap();
        s.subscribe(NOTIFY_UUID).unwrap();
        s.ping().unwrap();
        let info = s.info().unwrap();
        assert_eq!(info.services[0].characteristics[1].handle, Some(14));
        let seqs: Vec<u64> = s
            .sent
            .iter()
            .map(|(_, p)| p["seq"].as_u64().unwrap())
            .collect();
        assert_eq!(seqs, vec![1, 2, 3]);
        let events: Vec<&str> = s.sent.iter().map(|(e, _)| e.as_str()).collect();
        assert_eq!(events, vec!["ble_subscribe", "ble_ping", "ble_info"]);
    }

    #[test]
    fn op_error_maps_to_ble_error() {
        let (mut s, tx) = detached();
        tx.send(
            parse_result(
                &json!({ "seq": 1, "ok": false, "code": "ambiguous_characteristic",
                                   "message": "appears 2 times (handles 12, 40)" }),
            )
            .unwrap(),
        )
        .unwrap();
        let err = s.subscribe(WRITE_UUID).unwrap_err();
        assert!(matches!(
            err,
            Error::Ble {
                kind: BleErrorKind::AmbiguousCharacteristic,
                ..
            }
        ));
        // The session stays usable after a per-operation failure.
        assert!(!s.is_closed());
    }

    #[test]
    fn op_times_out_without_a_result() {
        let (mut s, _tx) = detached();
        let err = s.ping().unwrap_err();
        assert!(matches!(err, Error::Timeout(_)), "{err:?}");
    }

    #[test]
    fn disconnect_drains_buffered_notifications_first() {
        let (mut s, tx) = detached();
        tx.send(notify(&[(1, "01"), (2, "02")])).unwrap();
        tx.send(closed("disconnected", "The peripheral disconnected"))
            .unwrap();

        assert_eq!(s.recv(Duration::from_millis(50)).unwrap().seq, 1);
        assert_eq!(s.try_recv().unwrap().unwrap().seq, 2);
        assert!(s.is_closed());
        assert_eq!(s.close_reason(), Some("disconnected"));

        let err = s.try_recv().unwrap_err();
        assert!(
            matches!(err, Error::Ble { kind: BleErrorKind::Disconnected, ref message }
                         if message == "The peripheral disconnected")
        );
        let err = s.recv(Duration::from_millis(10)).unwrap_err();
        assert!(matches!(
            err,
            Error::Ble {
                kind: BleErrorKind::Disconnected,
                ..
            }
        ));
        // Other operations fail at once, without emitting anything.
        let err = s
            .write(WRITE_UUID, b"x", WriteOptions::default())
            .unwrap_err();
        assert!(matches!(
            err,
            Error::Ble {
                kind: BleErrorKind::Disconnected,
                ..
            }
        ));
        assert!(s.sent.is_empty());
    }

    #[test]
    fn close_during_an_op_reports_the_close_reason() {
        let (mut s, tx) = detached();
        tx.send(notify(&[(1, "01")])).unwrap();
        tx.send(closed("idle_timeout", "No client operation for 300s"))
            .unwrap();
        // The not_open answer the box would send next is never waited for.
        let err = s.read(WRITE_UUID).unwrap_err();
        assert!(matches!(
            err,
            Error::Ble {
                kind: BleErrorKind::IdleTimeout,
                ..
            }
        ));
        // The notification that came before the close is still delivered.
        assert_eq!(s.try_recv().unwrap().unwrap().seq, 1);
        assert!(s.try_recv().is_err());
    }

    #[test]
    fn disconnected_result_then_closed() {
        // Link lost mid-operation: the box answers the operation with
        // `disconnected`, then sends ble_closed.
        let (mut s, tx) = detached();
        tx.send(
            parse_result(&json!({ "seq": 1, "ok": false, "code": "disconnected",
                                   "message": "The peripheral disconnected" }))
            .unwrap(),
        )
        .unwrap();
        tx.send(closed("disconnected", "The peripheral disconnected"))
            .unwrap();
        let err = s
            .write(WRITE_UUID, b"x", WriteOptions::default())
            .unwrap_err();
        assert!(matches!(
            err,
            Error::Ble {
                kind: BleErrorKind::Disconnected,
                ..
            }
        ));
        assert!(s.try_recv().is_err());
        assert!(s.is_closed());
    }

    #[test]
    fn try_recv_is_none_when_empty_and_recv_times_out() {
        let (mut s, _tx) = detached();
        assert!(s.try_recv().unwrap().is_none());
        assert!(matches!(
            s.recv(Duration::from_millis(20)),
            Err(Error::Timeout(_))
        ));
    }

    #[test]
    fn socket_error_event_surfaces_as_stream_error() {
        let (mut s, tx) = detached();
        tx.send(SessionEvent::Error("handler failed".to_string()))
            .unwrap();
        assert!(matches!(s.try_recv(), Err(Error::Stream(m)) if m == "handler failed"));
    }

    #[test]
    fn mtu_accessors() {
        let (mut s, _tx) = detached();
        assert_eq!(s.address(), "AA:BB:CC:DD:EE:01");
        assert_eq!(s.mtu(), 247);
        assert_eq!(s.max_write_len(), 244);
        assert!(s.mtu_is_measured());
        assert_eq!(s.services().len(), 1);

        s.info = serde_json::from_value(json!({
            "address": "AA:BB:CC:DD:EE:01", "mtu": 23, "mtu_source": "default", "services": []
        }))
        .unwrap();
        assert_eq!(s.max_write_len(), 20);
        assert!(!s.mtu_is_measured());
    }

    #[test]
    fn close_sends_ble_close_and_waits_for_its_result() {
        let (mut s, tx) = detached();
        s.next_seq = 3;
        tx.send(ok(3, json!({}))).unwrap();
        tx.send(closed("client", "Closed by the client")).unwrap();
        // The first half of close(), which consumes the session.
        let seq = s.emit("ble_close", json!({})).unwrap();
        assert_eq!(
            s.await_result(seq, Duration::from_millis(50)).unwrap(),
            json!({})
        );
        assert_eq!(s.sent[0], ("ble_close".to_string(), json!({ "seq": 3 })));
        s.close().unwrap();
    }
}
