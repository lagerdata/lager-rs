//! UART nets: interactive serial streaming over the box's Socket.IO
//! `/uart` namespace (feature `uart`).
//!
//! The box owns the serial port (exclusive open) and streams data as hex
//! chunks over `uart_data` events; writes go up as `uart_write` events.
//! Only one session per net or device is allowed — a second opener gets a
//! clear "already in use" error.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use rust_socketio::{ClientBuilder, Payload};
use serde_json::{json, Value};

use crate::error::{Error, Result};

/// How long to wait for the box to confirm the serial port opened.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
enum UartEvent {
    Connected { device_path: String, baudrate: u64 },
    Data(Vec<u8>),
    Status(String),
    Error(String),
    Stopped,
}

fn hex_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Pull the first JSON object out of a Socket.IO payload.
fn payload_json(payload: Payload) -> Option<Value> {
    match payload {
        Payload::Text(values) => values.into_iter().next(),
        #[allow(deprecated)]
        Payload::String(s) => serde_json::from_str(&s).ok(),
        Payload::Binary(_) => None,
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

impl Uart {
    pub(crate) fn open(
        base_url: &str,
        netname: String,
        bearer_token: Option<String>,
    ) -> Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<UartEvent>();

        let socket = {
            let tx_connected: Sender<UartEvent> = tx.clone();
            let tx_data = tx.clone();
            let tx_status = tx.clone();
            let tx_error = tx.clone();
            let tx_stopped = tx;
            let mut builder = ClientBuilder::new(base_url).namespace("/uart");
            // Boxes behind an authenticating gateway need the bearer token
            // on the Socket.IO handshake too (same reverse proxy).
            if let Some(token) = &bearer_token {
                builder = builder.opening_header("Authorization", format!("Bearer {token}"));
            }
            builder
                .on("uart_connected", move |payload, _| {
                    let info = payload_json(payload).unwrap_or(Value::Null);
                    let device_path = info
                        .get("device_path")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let baudrate = info.get("baudrate").and_then(Value::as_u64).unwrap_or(0);
                    let _ = tx_connected.send(UartEvent::Connected {
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
                        let _ = tx_data.send(UartEvent::Data(bytes));
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
                        let _ = tx_status.send(UartEvent::Status(status.to_string()));
                    }
                })
                .on("error", move |payload, _| {
                    let message = payload_json(payload)
                        .as_ref()
                        .and_then(|v| v.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("unknown UART error")
                        .to_string();
                    let _ = tx_error.send(UartEvent::Error(message));
                })
                .on("uart_stopped", move |_, _| {
                    let _ = tx_stopped.send(UartEvent::Stopped);
                })
                .connect()
                .map_err(|e| Error::Connection(format!("Socket.IO connect failed: {e}")))?
        };

        socket
            .emit("start_uart", json!({ "netname": netname, "overrides": {} }))
            .map_err(|e| Error::Stream(format!("could not start UART session: {e}")))?;

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
                Ok(UartEvent::Error(msg)) => return Err(Error::Stream(msg)),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::Stream("UART session closed unexpectedly".to_string()))
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
                Ok(UartEvent::Error(msg)) => return Err(Error::Stream(msg)),
                Ok(UartEvent::Status(status)) => self.last_status = Some(status),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => return Ok(()),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::Stream("UART session closed unexpectedly".to_string()))
                }
            }
        }
    }

    /// Return whatever bytes arrive within `timeout`. May return an empty
    /// vector if the device was quiet.
    pub fn read(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        self.pump(timeout)?;
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
            if let Some(pos) = self
                .buf
                .windows(needle.len())
                .position(|w| w == needle)
            {
                let mut rest = self.buf.split_off(pos + needle.len());
                std::mem::swap(&mut self.buf, &mut rest);
                return Ok(rest);
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
            let _ = socket.emit("stop_uart", json!({}));
            let _ = socket.disconnect();
        }
    }
}

impl Drop for Uart {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::{hex_decode, hex_encode};

    #[test]
    fn hex_roundtrip() {
        let data = [0x00, 0x0a, 0xff, 0x42];
        assert_eq!(hex_encode(&data), "000aff42");
        assert_eq!(hex_decode(&hex_encode(&data)).unwrap(), data);
        assert!(hex_decode("zz").is_none());
        assert!(hex_decode("abc").is_none());
    }
}
