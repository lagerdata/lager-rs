//! UART nets: interactive serial streaming over the box's Socket.IO
//! `/uart` namespace (feature `uart`).
//!
//! The box owns the serial port (exclusive open) and streams data as hex
//! chunks over `uart_data` events; writes go up as `uart_write` events.
//! Only one session per net or device is allowed — a second opener gets a
//! clear "already in use" error.
//!
//! With the `async` feature too, `AsyncUart` is the same session for the
//! async client (`AsyncLagerBox::uart`). Both share the event wiring; the
//! async one waits on the runtime's timer and makes its socket calls off
//! the runtime's worker threads.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use rust_socketio::ClientBuilder;
use serde_json::{json, Map, Value};

use crate::error::{Error, Result};
use crate::nets::sio::{hex_decode, hex_encode, payload_json, take_through};

/// How long to wait for the box to confirm the serial port opened.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Parity setting for [`UartConfig::parity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    /// No parity bit.
    None,
    /// Even parity.
    Even,
    /// Odd parity.
    Odd,
    /// Parity bit always 1.
    Mark,
    /// Parity bit always 0.
    Space,
}

impl Parity {
    /// Wire encoding understood by the box.
    pub fn as_str(&self) -> &'static str {
        match self {
            Parity::None => "none",
            Parity::Even => "even",
            Parity::Odd => "odd",
            Parity::Mark => "mark",
            Parity::Space => "space",
        }
    }
}

/// Stop-bit setting for [`UartConfig::stopbits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopBits {
    /// One stop bit.
    One,
    /// One and a half stop bits.
    OnePointFive,
    /// Two stop bits.
    Two,
}

impl StopBits {
    /// Wire encoding understood by the box.
    pub fn as_str(&self) -> &'static str {
        match self {
            StopBits::One => "1",
            StopBits::OnePointFive => "1.5",
            StopBits::Two => "2",
        }
    }
}

/// Serial settings for one session, for
/// [`LagerBox::uart_with`](crate::LagerBox::uart_with). Each field left
/// `None` keeps the net's saved setting; nothing here changes the saved net.
#[derive(Debug, Clone, Copy, Default)]
pub struct UartConfig {
    /// Baud rate.
    pub baudrate: Option<u32>,
    /// Data bits (5-8).
    pub bytesize: Option<u8>,
    /// Parity.
    pub parity: Option<Parity>,
    /// Stop bits.
    pub stopbits: Option<StopBits>,
    /// XON/XOFF software flow control.
    pub xonxoff: Option<bool>,
    /// RTS/CTS hardware flow control.
    pub rtscts: Option<bool>,
    /// DSR/DTR hardware flow control.
    pub dsrdtr: Option<bool>,
    /// Translate `\n` to `\r\n` in the data the box sends back.
    pub opost: Option<bool>,
}

impl UartConfig {
    fn overrides(&self) -> Value {
        let mut o = Map::new();
        if let Some(v) = self.baudrate {
            o.insert("baudrate".to_string(), json!(v));
        }
        if let Some(v) = self.bytesize {
            o.insert("bytesize".to_string(), json!(v));
        }
        if let Some(v) = self.parity {
            o.insert("parity".to_string(), json!(v.as_str()));
        }
        if let Some(v) = self.stopbits {
            o.insert("stopbits".to_string(), json!(v.as_str()));
        }
        if let Some(v) = self.xonxoff {
            o.insert("xonxoff".to_string(), json!(v));
        }
        if let Some(v) = self.rtscts {
            o.insert("rtscts".to_string(), json!(v));
        }
        if let Some(v) = self.dsrdtr {
            o.insert("dsrdtr".to_string(), json!(v));
        }
        if let Some(v) = self.opost {
            o.insert("opost".to_string(), json!(v));
        }
        Value::Object(o)
    }
}

#[derive(Debug)]
enum UartEvent {
    Connected { device_path: String, baudrate: u64 },
    Data(Vec<u8>),
    Status(String),
    Error(Error),
    Stopped,
}

/// Turn the box's `error` event payload into an [`Error`]. A net-in-use
/// refusal carries `code: "net_in_use"` (box >= 0.49.0); older boxes send
/// only the message, which still says "already in use".
fn session_error(payload: Option<&Value>) -> Error {
    let message = payload
        .and_then(|v| v.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("unknown UART error")
        .to_string();
    let code = payload.and_then(|v| v.get("code")).and_then(Value::as_str);
    if code == Some("net_in_use") || message.contains("already in use") {
        let held_by = payload
            .and_then(|v| v.get("held_by"))
            .and_then(Value::as_str)
            .map(str::to_string);
        Error::UartInUse { message, held_by }
    } else {
        Error::Stream(message)
    }
}

/// A live UART streaming session.
///
/// Created via [`crate::LagerBox::uart`]. Data received from the device is
/// buffered internally; pull it with [`Uart::read`] or scan for expected
/// output with [`Uart::wait_for`]. The session is stopped cleanly on
/// [`Uart::stop`] or drop.
pub struct Uart {
    socket: Option<rust_socketio::client::Client>,
    rx: Receiver<UartEvent>,
    netname: String,
    device_path: String,
    baudrate: u64,
    buf: Vec<u8>,
    last_status: Option<String>,
}

/// Open the `/uart` Socket.IO connection and ask the box to start a session,
/// delivering every event to `sink`. Shared by [`Uart`] and `AsyncUart`, so
/// the two cannot drift.
fn connect(
    base_url: &str,
    netname: &str,
    config: &UartConfig,
    bearer_token: Option<&str>,
    sink: impl Fn(UartEvent) + Clone + Send + 'static,
) -> Result<rust_socketio::client::Client> {
    let mut builder = ClientBuilder::new(base_url).namespace("/uart");
    // Boxes behind an authenticating gateway need the bearer token on the
    // Socket.IO handshake too (same reverse proxy).
    if let Some(token) = bearer_token {
        builder = builder.opening_header("Authorization", format!("Bearer {token}"));
    }
    let (on_connected, on_data, on_status, on_error, on_stopped) =
        (sink.clone(), sink.clone(), sink.clone(), sink.clone(), sink);
    let socket = builder
        .on("uart_connected", move |payload, _| {
            let info = payload_json(payload).unwrap_or(Value::Null);
            let device_path = info
                .get("device_path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let baudrate = info.get("baudrate").and_then(Value::as_u64).unwrap_or(0);
            on_connected(UartEvent::Connected {
                device_path,
                baudrate,
            });
        })
        .on("uart_data", move |payload, _| {
            if let Some(bytes) = payload_json(payload)
                .as_ref()
                .and_then(|v| v.get("data"))
                .and_then(Value::as_str)
                .and_then(hex_decode)
            {
                on_data(UartEvent::Data(bytes));
            }
        })
        .on("uart_status", move |payload, _| {
            // reconnecting/reconnected notifications during device
            // re-enumeration; informational only.
            if let Some(status) = payload_json(payload)
                .as_ref()
                .and_then(|v| v.get("status"))
                .and_then(Value::as_str)
            {
                on_status(UartEvent::Status(status.to_string()));
            }
        })
        .on("error", move |payload, _| {
            let payload = payload_json(payload);
            on_error(UartEvent::Error(session_error(payload.as_ref())));
        })
        .on("uart_stopped", move |_, _| on_stopped(UartEvent::Stopped))
        .connect()
        .map_err(|e| Error::Connection(format!("Socket.IO connect failed: {e}")))?;

    socket
        .emit(
            "start_uart",
            json!({ "netname": netname, "overrides": config.overrides() }),
        )
        .map_err(|e| Error::Stream(format!("could not start UART session: {e}")))?;
    Ok(socket)
}

/// Tell the box to close the port, then drop the connection.
fn hang_up(socket: &rust_socketio::client::Client) {
    let _ = socket.emit("stop_uart", json!({}));
    let _ = socket.disconnect();
}

impl Uart {
    pub(crate) fn open(
        base_url: &str,
        netname: String,
        config: &UartConfig,
        bearer_token: Option<String>,
    ) -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<UartEvent>();
        let sink = move |event| {
            let _ = tx.send(event);
        };
        let socket = connect(base_url, &netname, config, bearer_token.as_deref(), sink)?;

        // Wait for the box to open the port (or refuse).
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let mut uart = Uart {
            socket: Some(socket),
            rx,
            netname,
            device_path: String::new(),
            baudrate: 0,
            buf: Vec::new(),
            last_status: None,
        };
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout(format!(
                    "box did not confirm UART session for '{}' within {CONNECT_TIMEOUT:?}",
                    uart.netname
                )));
            }
            match uart.rx.recv_timeout(remaining) {
                Ok(UartEvent::Connected {
                    device_path,
                    baudrate,
                }) => {
                    uart.device_path = device_path;
                    uart.baudrate = baudrate;
                    return Ok(uart);
                }
                // Data can beat the connected event; keep it.
                Ok(UartEvent::Data(bytes)) => uart.buf.extend_from_slice(&bytes),
                Ok(UartEvent::Error(err)) => return Err(err),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::Stream(
                        "UART session closed unexpectedly".to_string(),
                    ))
                }
            }
        }
    }

    /// Name of the net this session streams.
    pub fn netname(&self) -> &str {
        &self.netname
    }

    /// Device path on the box (e.g. `/dev/ttyUSB0`).
    pub fn device_path(&self) -> &str {
        &self.device_path
    }

    /// Baud rate the port was opened at.
    pub fn baudrate(&self) -> u64 {
        self.baudrate
    }

    /// The most recent session status notification from the box, e.g.
    /// `"reconnecting"` / `"reconnected"` while the adapter re-enumerates
    /// (hub power-cycle, DUT reflash). Updated during reads.
    pub fn last_status(&self) -> Option<&str> {
        self.last_status.as_deref()
    }

    /// Write raw bytes to the device.
    pub fn write(&self, data: &[u8]) -> Result<()> {
        let socket = self
            .socket
            .as_ref()
            .ok_or_else(|| Error::Stream("UART session already stopped".to_string()))?;
        socket
            .emit("uart_write", json!({ "data": hex_encode(data) }))
            .map_err(|e| Error::Stream(format!("UART write failed: {e}")))
    }

    /// Write a string to the device.
    pub fn write_str(&self, s: &str) -> Result<()> {
        self.write(s.as_bytes())
    }

    /// Drain incoming events into the internal buffer. Returns an error if
    /// the box reported a session error.
    fn pump(&mut self, wait: Duration) -> Result<()> {
        let mut wait = wait;
        loop {
            match self.rx.recv_timeout(wait) {
                Ok(UartEvent::Data(bytes)) => {
                    self.buf.extend_from_slice(&bytes);
                    // Something arrived: keep draining whatever is already
                    // queued without waiting again.
                    wait = Duration::ZERO;
                }
                Ok(UartEvent::Error(err)) => return Err(err),
                Ok(UartEvent::Status(status)) => self.last_status = Some(status),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => return Ok(()),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::Stream(
                        "UART session closed unexpectedly".to_string(),
                    ))
                }
            }
        }
    }

    /// Return whatever bytes arrive within `timeout`. May return an empty
    /// vector if the device was quiet.
    ///
    /// Note this waits out the **full** `timeout` when the device stays
    /// idle (data that does arrive is still drained without further
    /// waiting). Poll loops that only want what has already arrived should
    /// use [`Uart::try_read`] instead, which never blocks.
    pub fn read(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        self.pump(timeout)?;
        Ok(std::mem::take(&mut self.buf))
    }

    /// Return the bytes already received, without waiting.
    ///
    /// Drains everything the session has queued (and anything buffered by
    /// a previous [`Uart::wait_for`]) and returns immediately — an empty
    /// vector when the device has been quiet. This is the non-blocking
    /// complement to [`Uart::read`] for poll loops.
    pub fn try_read(&mut self) -> Result<Vec<u8>> {
        self.pump(Duration::ZERO)?;
        Ok(std::mem::take(&mut self.buf))
    }

    /// Accumulate output until `needle` appears or `timeout` elapses.
    ///
    /// On success returns everything received up to and including `needle`;
    /// bytes after the needle stay buffered for the next read.
    pub fn wait_for(&mut self, needle: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(found) = take_through(&mut self.buf, needle) {
                return Ok(found);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout(format!(
                    "'{}' did not appear on UART '{}' within {timeout:?}",
                    String::from_utf8_lossy(needle),
                    self.netname
                )));
            }
            self.pump(remaining)?;
        }
    }

    /// Stop the session cleanly (tells the box to close the port).
    pub fn stop(mut self) -> Result<()> {
        self.shutdown();
        Ok(())
    }

    fn shutdown(&mut self) {
        if let Some(socket) = self.socket.take() {
            hang_up(&socket);
        }
    }
}

impl Drop for Uart {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A live UART streaming session (async). Feature `uart` with `async`.
///
/// Created via [`crate::AsyncLagerBox::uart`]. The same session as [`Uart`],
/// with every wait an `.await`: data is buffered internally, [`read`] and
/// [`wait_for`](AsyncUart::wait_for) wait on the runtime's timer, and
/// writes run off the runtime's worker threads. Stopped cleanly on
/// [`stop`](AsyncUart::stop) or drop.
///
/// [`read`]: AsyncUart::read
#[cfg(feature = "async")]
pub struct AsyncUart {
    socket: Option<rust_socketio::client::Client>,
    rx: tokio::sync::mpsc::UnboundedReceiver<UartEvent>,
    netname: String,
    device_path: String,
    baudrate: u64,
    buf: Vec<u8>,
    last_status: Option<String>,
}

#[cfg(feature = "async")]
impl AsyncUart {
    pub(crate) async fn open(
        base_url: String,
        netname: String,
        config: UartConfig,
        bearer_token: Option<String>,
    ) -> Result<Self> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = move |event| {
            let _ = tx.send(event);
        };
        let name = netname.clone();
        let socket = crate::nets::sio::blocking(move || {
            connect(&base_url, &name, &config, bearer_token.as_deref(), sink)
        })
        .await?;

        // Wait for the box to open the port (or refuse).
        let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
        let mut uart = AsyncUart {
            socket: Some(socket),
            rx,
            netname,
            device_path: String::new(),
            baudrate: 0,
            buf: Vec::new(),
            last_status: None,
        };
        loop {
            match tokio::time::timeout_at(deadline, uart.rx.recv()).await {
                Err(_) => {
                    return Err(Error::Timeout(format!(
                        "box did not confirm UART session for '{}' within {CONNECT_TIMEOUT:?}",
                        uart.netname
                    )))
                }
                Ok(Some(UartEvent::Connected {
                    device_path,
                    baudrate,
                })) => {
                    uart.device_path = device_path;
                    uart.baudrate = baudrate;
                    return Ok(uart);
                }
                // Data can beat the connected event; keep it.
                Ok(Some(UartEvent::Data(bytes))) => uart.buf.extend_from_slice(&bytes),
                Ok(Some(UartEvent::Error(err))) => return Err(err),
                Ok(Some(_)) => {}
                Ok(None) => return Err(closed()),
            }
        }
    }

    /// Name of the net this session streams.
    pub fn netname(&self) -> &str {
        &self.netname
    }

    /// Device path on the box (e.g. `/dev/ttyUSB0`).
    pub fn device_path(&self) -> &str {
        &self.device_path
    }

    /// Baud rate the port was opened at.
    pub fn baudrate(&self) -> u64 {
        self.baudrate
    }

    /// The most recent session status notification from the box; see
    /// [`Uart::last_status`].
    pub fn last_status(&self) -> Option<&str> {
        self.last_status.as_deref()
    }

    /// Write raw bytes to the device.
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        let socket = self
            .socket
            .clone()
            .ok_or_else(|| Error::Stream("UART session already stopped".to_string()))?;
        let payload = json!({ "data": hex_encode(data) });
        crate::nets::sio::blocking(move || {
            socket
                .emit("uart_write", payload)
                .map_err(|e| Error::Stream(format!("UART write failed: {e}")))
        })
        .await
    }

    /// Write a string to the device.
    pub async fn write_str(&self, s: &str) -> Result<()> {
        self.write(s.as_bytes()).await
    }

    /// Take one event into the session. `true` when it carried data.
    fn absorb(&mut self, event: UartEvent) -> Result<bool> {
        match event {
            UartEvent::Data(bytes) => {
                self.buf.extend_from_slice(&bytes);
                return Ok(true);
            }
            UartEvent::Error(err) => return Err(err),
            UartEvent::Status(status) => self.last_status = Some(status),
            _ => {}
        }
        Ok(false)
    }

    /// Take every event already queued, without waiting.
    fn drain(&mut self) -> Result<()> {
        loop {
            match self.rx.try_recv() {
                Ok(event) => {
                    self.absorb(event)?;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => return Ok(()),
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return Err(closed()),
            }
        }
    }

    /// Wait until data arrives or `deadline` passes, then take everything
    /// queued.
    async fn pump(&mut self, deadline: tokio::time::Instant) -> Result<()> {
        loop {
            match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Err(_) => return Ok(()),
                Ok(None) => return Err(closed()),
                Ok(Some(event)) => {
                    if self.absorb(event)? {
                        return self.drain();
                    }
                }
            }
        }
    }

    /// Return whatever bytes arrive within `timeout`; see [`Uart::read`].
    pub async fn read(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        self.pump(tokio::time::Instant::now() + timeout).await?;
        Ok(std::mem::take(&mut self.buf))
    }

    /// Return the bytes already received, without waiting.
    pub fn try_read(&mut self) -> Result<Vec<u8>> {
        self.drain()?;
        Ok(std::mem::take(&mut self.buf))
    }

    /// Accumulate output until `needle` appears or `timeout` elapses; see
    /// [`Uart::wait_for`].
    pub async fn wait_for(&mut self, needle: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(found) = take_through(&mut self.buf, needle) {
                return Ok(found);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "'{}' did not appear on UART '{}' within {timeout:?}",
                    String::from_utf8_lossy(needle),
                    self.netname
                )));
            }
            self.pump(deadline).await?;
        }
    }

    /// Stop the session cleanly (tells the box to close the port).
    pub async fn stop(mut self) -> Result<()> {
        if let Some(socket) = self.socket.take() {
            crate::nets::sio::blocking(move || {
                hang_up(&socket);
                Ok(())
            })
            .await?;
        }
        Ok(())
    }
}

#[cfg(feature = "async")]
impl Drop for AsyncUart {
    fn drop(&mut self) {
        // Drop cannot await; hang up on a thread of its own so the runtime's
        // worker is not blocked on the socket.
        if let Some(socket) = self.socket.take() {
            std::thread::spawn(move || hang_up(&socket));
        }
    }
}

#[cfg(feature = "async")]
fn closed() -> Error {
    Error::Stream("UART session closed unexpectedly".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_sends_no_overrides() {
        assert_eq!(UartConfig::default().overrides(), json!({}));
    }

    #[test]
    fn config_sends_only_the_fields_set() {
        let cfg = UartConfig {
            baudrate: Some(9600),
            parity: Some(Parity::Even),
            stopbits: Some(StopBits::OnePointFive),
            rtscts: Some(true),
            ..UartConfig::default()
        };
        assert_eq!(
            cfg.overrides(),
            json!({"baudrate": 9600, "parity": "even", "stopbits": "1.5", "rtscts": true})
        );
    }

    #[test]
    fn net_in_use_names_the_holder() {
        let err = session_error(Some(&json!({
            "message": "UART device /dev/ttyACM0 is already in use by net 'uart2'",
            "code": "net_in_use", "netname": "uart1", "held_by": "uart2"
        })));
        match err {
            Error::UartInUse { held_by, .. } => assert_eq!(held_by.as_deref(), Some("uart2")),
            other => panic!("expected UartInUse, got {other:?}"),
        }
    }

    #[test]
    fn older_box_in_use_message_is_still_uart_in_use() {
        let err = session_error(Some(&json!({
            "message": "UART net 'uart1' is already in use by another session"
        })));
        assert!(
            matches!(err, Error::UartInUse { held_by: None, .. }),
            "{err:?}"
        );
    }

    #[test]
    fn other_errors_stay_stream_errors() {
        let err = session_error(Some(
            &json!({"message": "UART device not found: /dev/ttyUSB9"}),
        ));
        assert!(matches!(err, Error::Stream(_)), "{err:?}");
    }
}
