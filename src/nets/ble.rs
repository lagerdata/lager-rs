//! BLE (box-level): scan for and inspect Bluetooth Low Energy devices using
//! the box's own Bluetooth adapter.
//!
//! There is one adapter per box and the box serializes all BLE and BluFi
//! operations on it, so concurrent calls queue rather than fail.
//!
//! One-shot GATT methods ([`Ble::read`], [`Ble::write`], [`Ble::notify`],
//! [`Ble::pair`]) connect, do one operation, and disconnect. For a held link
//! (interleaved write/notify while driving other nets), open a
//! [`BleSession`] with [`Ble::session_open`].

use super::box_handle;
pub use crate::wire::{BleCharacteristic, BleDevice, BleDeviceInfo, BleNotifications, BleService};

/// Box software that first served `session_*` BLE actions. Coordinate with
/// Lager on the real bump; until the box lands, older images refuse with
/// [`crate::Error::UnsupportedByBox`].
pub const SESSION_SINCE: &str = ">= 0.63.0";

/// A GATT characteristic, by UUID or by attribute handle, for the GATT
/// methods on [`Ble`] and [`BleSession`]. Converts from a `&str` / `String`
/// (a UUID, such as `"00002a19-0000-1000-8000-00805f9b34fb"`) and from a
/// `u16` (a handle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GattChar {
    /// Characteristic UUID.
    Uuid(String),
    /// Attribute handle, for a device with two characteristics that share
    /// a UUID.
    Handle(u16),
}

impl From<&str> for GattChar {
    fn from(uuid: &str) -> Self {
        GattChar::Uuid(uuid.to_string())
    }
}

impl From<String> for GattChar {
    fn from(uuid: String) -> Self {
        GattChar::Uuid(uuid)
    }
}

impl From<u16> for GattChar {
    fn from(handle: u16) -> Self {
        GattChar::Handle(handle)
    }
}

impl GattChar {
    fn to_json(&self) -> serde_json::Value {
        match self {
            GattChar::Uuid(uuid) => serde_json::json!(uuid),
            GattChar::Handle(handle) => serde_json::json!(handle),
        }
    }

    fn from_json(value: &serde_json::Value) -> crate::error::Result<Self> {
        match value {
            serde_json::Value::String(s) => Ok(GattChar::Uuid(s.clone())),
            serde_json::Value::Number(n) => {
                let handle = n.as_u64().ok_or_else(|| {
                    crate::error::Error::Decode(format!("invalid BLE characteristic handle: {n}"))
                })?;
                if handle > u64::from(u16::MAX) {
                    return Err(crate::error::Error::Decode(format!(
                        "BLE characteristic handle out of range: {handle}"
                    )));
                }
                Ok(GattChar::Handle(handle as u16))
            }
            other => Err(crate::error::Error::Decode(format!(
                "invalid BLE characteristic id: {other}"
            ))),
        }
    }
}

/// One notification drained by [`BleSession::notify_poll`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BleSessionMessage {
    /// Characteristic the notification came from (UUID or handle).
    pub characteristic: GattChar,
    /// Notification payload.
    pub data: Vec<u8>,
}

/// Notifications collected by [`BleSession::notify_poll`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BleSessionNotifications {
    /// Messages drained this poll, in round-robin order across characteristics
    /// when no char filter was set.
    pub messages: Vec<BleSessionMessage>,
    /// `true` when the wait ended before `max_messages` arrived.
    pub timed_out: bool,
}

/// Timeouts for a held [`BleSession`].
#[derive(Debug, Clone, Copy)]
pub struct BleSessionOptions {
    /// Box-side connect timeout for `session_open`, seconds (default 10).
    pub connect_timeout: f64,
    /// Box-side timeout for each pair/read/write/notify_start/notify_stop on
    /// the held link, seconds (default 10).
    pub op_timeout: f64,
}

impl Default for BleSessionOptions {
    fn default() -> Self {
        BleSessionOptions {
            connect_timeout: 10.0,
            op_timeout: 10.0,
        }
    }
}

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::json;

    use serde::Deserialize;

    use super::{BleSessionMessage, BleSessionNotifications, GattChar, SESSION_SINCE};
    use crate::error::Result;
    use crate::wire::{
        box_command, hex_decode, hex_encode, unit, value_as, value_list_field, BleDevice,
        BleDeviceInfo, BleNotifications, CommandResponse, Op, Timeout,
    };

    const PATH: &str = "/ble/command";

    /// Box-side connect timeout (s) for info/connect, matching the CLI.
    const CONNECT_TIMEOUT_S: f64 = 10.0;

    fn parse_devices(resp: CommandResponse) -> Result<Vec<BleDevice>> {
        value_list_field(resp, "devices")
    }

    /// Budget: the box blocks for the scan/connect duration plus bleak
    /// overhead, and may queue behind another BLE/BluFi operation on the
    /// adapter lock.
    fn budget(box_side: f64) -> Timeout {
        Timeout::After(Duration::from_secs_f64(box_side + 30.0))
    }

    pub(crate) fn scan(timeout: f64) -> Op<Vec<BleDevice>> {
        Op {
            req: box_command(PATH, "scan", json!({ "timeout": timeout }), budget(timeout)),
            parse: parse_devices,
        }
    }

    pub(crate) fn scan_named(timeout: f64, name_contains: &str) -> Op<Vec<BleDevice>> {
        Op {
            req: box_command(
                PATH,
                "scan",
                json!({ "timeout": timeout, "name_contains": name_contains }),
                budget(timeout),
            ),
            parse: parse_devices,
        }
    }

    pub(crate) fn scan_exact(timeout: f64, name: &str) -> Op<Vec<BleDevice>> {
        Op {
            req: box_command(
                PATH,
                "scan",
                json!({ "timeout": timeout, "name_exact": name }),
                budget(timeout),
            ),
            parse: parse_devices,
        }
    }

    fn enumerate(action: &str, address: &str, timeout: f64) -> Op<BleDeviceInfo> {
        Op {
            req: box_command(
                PATH,
                action,
                json!({ "address": address, "timeout": timeout }),
                budget(timeout),
            ),
            parse: value_as::<BleDeviceInfo>,
        }
    }

    pub(crate) fn info(address: &str) -> Op<BleDeviceInfo> {
        enumerate("info", address, CONNECT_TIMEOUT_S)
    }

    pub(crate) fn info_with_timeout(address: &str, timeout: f64) -> Op<BleDeviceInfo> {
        enumerate("info", address, timeout)
    }

    pub(crate) fn connect(address: &str) -> Op<BleDeviceInfo> {
        enumerate("connect", address, CONNECT_TIMEOUT_S)
    }

    pub(crate) fn connect_with_timeout(address: &str, timeout: f64) -> Op<BleDeviceInfo> {
        enumerate("connect", address, timeout)
    }

    fn gatt(address: &str, characteristic: &GattChar) -> serde_json::Value {
        json!({
            "address": address,
            "char": characteristic.to_json(),
            "timeout": CONNECT_TIMEOUT_S,
        })
    }

    fn parse_read(resp: CommandResponse) -> Result<Vec<u8>> {
        #[derive(Deserialize)]
        struct Read {
            data: String,
        }
        hex_decode(&value_as::<Read>(resp)?.data)
    }

    fn parse_notifications(resp: CommandResponse) -> Result<BleNotifications> {
        #[derive(Deserialize)]
        struct Notified {
            #[serde(default)]
            messages: Vec<String>,
            #[serde(default)]
            timed_out: bool,
        }
        let n = value_as::<Notified>(resp)?;
        Ok(BleNotifications {
            messages: n
                .messages
                .iter()
                .map(|m| hex_decode(m))
                .collect::<Result<_>>()?,
            timed_out: n.timed_out,
        })
    }

    pub(crate) fn read(address: &str, characteristic: impl Into<GattChar>) -> Op<Vec<u8>> {
        let characteristic = characteristic.into();
        Op {
            req: box_command(
                PATH,
                "read",
                gatt(address, &characteristic),
                budget(CONNECT_TIMEOUT_S),
            ),
            parse: parse_read,
        }
    }

    fn write_op(
        address: &str,
        characteristic: GattChar,
        data: &[u8],
        response: Option<bool>,
    ) -> Op<()> {
        let mut params = gatt(address, &characteristic);
        params["data"] = json!(hex_encode(data));
        if let Some(response) = response {
            params["response"] = json!(response);
        }
        Op {
            req: box_command(PATH, "write", params, budget(CONNECT_TIMEOUT_S)),
            parse: unit,
        }
    }

    pub(crate) fn write(address: &str, characteristic: impl Into<GattChar>, data: &[u8]) -> Op<()> {
        write_op(address, characteristic.into(), data, None)
    }

    pub(crate) fn write_with_response(
        address: &str,
        characteristic: impl Into<GattChar>,
        data: &[u8],
        response: bool,
    ) -> Op<()> {
        write_op(address, characteristic.into(), data, Some(response))
    }

    pub(crate) fn notify(
        address: &str,
        characteristic: impl Into<GattChar>,
        max_messages: u32,
        duration: f64,
    ) -> Op<BleNotifications> {
        let characteristic = characteristic.into();
        let mut params = gatt(address, &characteristic);
        params["max_messages"] = json!(max_messages);
        params["duration"] = json!(duration);
        Op {
            req: box_command(PATH, "notify", params, budget(CONNECT_TIMEOUT_S + duration)),
            parse: parse_notifications,
        }
    }

    pub(crate) fn pair(address: &str) -> Op<()> {
        Op {
            req: box_command(
                PATH,
                "pair",
                json!({ "address": address, "timeout": CONNECT_TIMEOUT_S }),
                budget(CONNECT_TIMEOUT_S),
            ),
            parse: unit,
        }
    }

    pub(crate) fn disconnect(address: &str) -> Op<()> {
        Op {
            req: box_command(
                PATH,
                "disconnect",
                json!({ "address": address }),
                budget(CONNECT_TIMEOUT_S),
            ),
            parse: unit,
        }
    }

    // -- held GATT session ----------------------------------------------------

    #[derive(Debug, Clone, Deserialize)]
    pub(crate) struct SessionOpened {
        pub session_id: String,
        pub address: String,
    }

    fn session_params(session_id: &str, op_timeout: f64) -> serde_json::Value {
        json!({ "session_id": session_id, "timeout": op_timeout })
    }

    pub(crate) fn session_open(address: &str, connect_timeout: f64) -> Op<SessionOpened> {
        Op {
            req: box_command(
                PATH,
                "session_open",
                json!({ "address": address, "timeout": connect_timeout }),
                budget(connect_timeout),
            ),
            parse: value_as::<SessionOpened>,
        }
    }

    pub(crate) fn session_pair(session_id: &str, op_timeout: f64) -> Op<()> {
        Op {
            req: box_command(
                PATH,
                "session_pair",
                session_params(session_id, op_timeout),
                budget(op_timeout),
            ),
            parse: unit,
        }
    }

    fn parse_session_read(resp: CommandResponse) -> Result<Vec<u8>> {
        #[derive(Deserialize)]
        struct Read {
            data: String,
        }
        hex_decode(&value_as::<Read>(resp)?.data)
    }

    pub(crate) fn session_read(
        session_id: &str,
        op_timeout: f64,
        characteristic: impl Into<GattChar>,
    ) -> Op<Vec<u8>> {
        let characteristic = characteristic.into();
        let mut params = session_params(session_id, op_timeout);
        params["char"] = characteristic.to_json();
        Op {
            req: box_command(PATH, "session_read", params, budget(op_timeout)),
            parse: parse_session_read,
        }
    }

    fn session_write_op(
        session_id: &str,
        op_timeout: f64,
        characteristic: GattChar,
        data: &[u8],
        response: Option<bool>,
    ) -> Op<()> {
        let mut params = session_params(session_id, op_timeout);
        params["char"] = characteristic.to_json();
        params["data"] = json!(hex_encode(data));
        if let Some(response) = response {
            params["response"] = json!(response);
        }
        Op {
            req: box_command(PATH, "session_write", params, budget(op_timeout)),
            parse: unit,
        }
    }

    pub(crate) fn session_write(
        session_id: &str,
        op_timeout: f64,
        characteristic: impl Into<GattChar>,
        data: &[u8],
    ) -> Op<()> {
        session_write_op(session_id, op_timeout, characteristic.into(), data, None)
    }

    pub(crate) fn session_write_with_response(
        session_id: &str,
        op_timeout: f64,
        characteristic: impl Into<GattChar>,
        data: &[u8],
        response: bool,
    ) -> Op<()> {
        session_write_op(
            session_id,
            op_timeout,
            characteristic.into(),
            data,
            Some(response),
        )
    }

    pub(crate) fn session_notify_start(
        session_id: &str,
        op_timeout: f64,
        characteristic: impl Into<GattChar>,
    ) -> Op<()> {
        let characteristic = characteristic.into();
        let mut params = session_params(session_id, op_timeout);
        params["char"] = characteristic.to_json();
        Op {
            req: box_command(PATH, "session_notify_start", params, budget(op_timeout)),
            parse: unit,
        }
    }

    pub(crate) fn session_notify_stop(
        session_id: &str,
        op_timeout: f64,
        characteristic: impl Into<GattChar>,
    ) -> Op<()> {
        let characteristic = characteristic.into();
        let mut params = session_params(session_id, op_timeout);
        params["char"] = characteristic.to_json();
        Op {
            req: box_command(PATH, "session_notify_stop", params, budget(op_timeout)),
            parse: unit,
        }
    }

    fn parse_session_notifications(resp: CommandResponse) -> Result<BleSessionNotifications> {
        #[derive(Deserialize)]
        struct WireMsg {
            char: serde_json::Value,
            data: String,
        }
        #[derive(Deserialize)]
        struct Polled {
            #[serde(default)]
            messages: Vec<WireMsg>,
            #[serde(default)]
            timed_out: bool,
        }
        let n = value_as::<Polled>(resp)?;
        let mut messages = Vec::with_capacity(n.messages.len());
        for m in n.messages {
            messages.push(BleSessionMessage {
                characteristic: GattChar::from_json(&m.char)?,
                data: hex_decode(&m.data)?,
            });
        }
        Ok(BleSessionNotifications {
            messages,
            timed_out: n.timed_out,
        })
    }

    pub(crate) fn session_notify_poll(
        session_id: &str,
        characteristic: Option<GattChar>,
        max_messages: u32,
        wait: f64,
    ) -> Op<BleSessionNotifications> {
        let mut params = json!({
            "session_id": session_id,
            "max_messages": max_messages,
            "wait": wait,
        });
        if let Some(characteristic) = characteristic {
            params["char"] = characteristic.to_json();
        }
        Op {
            req: box_command(PATH, "session_notify_poll", params, budget(wait)),
            parse: parse_session_notifications,
        }
    }

    pub(crate) fn session_close(session_id: &str) -> Op<()> {
        Op {
            req: box_command(
                PATH,
                "session_close",
                json!({ "session_id": session_id }),
                budget(CONNECT_TIMEOUT_S),
            ),
            parse: unit,
        }
    }

    pub(crate) fn session_force_close() -> Op<()> {
        Op {
            req: box_command(
                PATH,
                "session_close",
                json!({ "force": true }),
                budget(CONNECT_TIMEOUT_S),
            ),
            parse: unit,
        }
    }

    pub(crate) fn session_compat(err: crate::error::Error) -> crate::error::Error {
        crate::wire::action_compat(err, "session_open", SESSION_SINCE)
    }

    pub(crate) fn force_close_compat(err: crate::error::Error) -> crate::error::Error {
        crate::wire::action_compat(err, "session_close", SESSION_SINCE)
    }
}

box_handle! {
    /// Handle for the box's BLE adapter (from [`crate::LagerBox::ble`]).
    sync: Ble,
    async: AsyncBle,
    methods: {
        /// Scan for advertising devices for `timeout` seconds
        /// (0.1–300, sorted named-first).
        fn scan(timeout: f64) -> Vec<BleDevice> = ops::scan;
        /// Scan, keeping only devices whose name contains `name_contains`
        /// (case-insensitive).
        fn scan_named(timeout: f64, name_contains: &str) -> Vec<BleDevice> = ops::scan_named;
        /// Scan, keeping only devices whose advertised name is exactly
        /// `name` (case-sensitive).
        fn scan_exact(timeout: f64, name: &str) -> Vec<BleDevice> = ops::scan_exact;
        /// Connect briefly and enumerate the device's GATT services, with a
        /// 10 s connect timeout.
        fn info(address: &str) -> BleDeviceInfo = ops::info;
        /// [`Ble::info`] with a connect timeout of `timeout` seconds, for a
        /// device that advertises slowly.
        fn info_with_timeout(address: &str, timeout: f64) -> BleDeviceInfo = ops::info_with_timeout;
        /// Connect to a device (and enumerate its services to verify), with
        /// a 10 s connect timeout.
        fn connect(address: &str) -> BleDeviceInfo = ops::connect;
        /// [`Ble::connect`] with a connect timeout of `timeout` seconds.
        fn connect_with_timeout(address: &str, timeout: f64) -> BleDeviceInfo = ops::connect_with_timeout;
        /// Ensure a device is disconnected from the box.
        fn disconnect(address: &str) -> () = ops::disconnect;
        /// Read a GATT characteristic's value.
        ///
        /// Each GATT call connects, does its one operation and disconnects
        /// (10 s connect timeout), so a device that needs a connection held
        /// between operations cannot be driven this way — use
        /// [`Ble::session_open`] instead. Box >= 0.62.0; older boxes fail
        /// with [`crate::Error::UnsupportedByBox`].
        fn read(address: &str, characteristic: impl Into<GattChar>) -> Vec<u8>
            = ops::read, compat("read", ">= 0.62.0");
        /// Write a GATT characteristic, letting the box pick write-with- or
        /// without-response from the characteristic's properties. See
        /// [`Ble::read`] for how the link is handled.
        fn write(address: &str, characteristic: impl Into<GattChar>, data: &[u8]) -> ()
            = ops::write, compat("write", ">= 0.62.0");
        /// [`Ble::write`], choosing write-with-response (`true`) or
        /// without-response (`false`) yourself.
        fn write_with_response(
            address: &str,
            characteristic: impl Into<GattChar>,
            data: &[u8],
            response: bool,
        ) -> () = ops::write_with_response, compat("write", ">= 0.62.0");
        /// Subscribe to a characteristic and collect notifications until
        /// `max_messages` (1-10000) arrive or `duration` seconds (0.1-300)
        /// pass, whichever is first, then unsubscribe. A short count comes
        /// back with [`BleNotifications::timed_out`] set, not as an error.
        fn notify(
            address: &str,
            characteristic: impl Into<GattChar>,
            max_messages: u32,
            duration: f64,
        ) -> BleNotifications = ops::notify, compat("notify", ">= 0.62.0");
        /// Pair with a device through the box's BlueZ, which keeps the bond
        /// (box >= 0.62.0). For pairing on a held link, see
        /// [`BleSession::pair`].
        fn pair(address: &str) -> () = ops::pair, compat("pair", ">= 0.62.0");
    }
}

#[cfg(feature = "blocking")]
impl<'a> Ble<'a> {
    /// Open a held GATT session to `address`.
    ///
    /// The session owns the box adapter until [`BleSession::close`] or drop
    /// (max lifetime 300 s on the box). One-shot BLE and BluFi refuse with
    /// HTTP 409 while it is open. Requires box software matching
    /// [`SESSION_SINCE`].
    pub fn session_open(
        &self,
        address: &str,
        opts: &BleSessionOptions,
    ) -> crate::error::Result<BleSession<'a>> {
        let opened = self
            .client
            .run(ops::session_open(address, opts.connect_timeout))
            .map_err(ops::session_compat)?;
        Ok(BleSession {
            client: self.client,
            session_id: opened.session_id,
            address: opened.address,
            op_timeout: opts.op_timeout,
            closed: false,
        })
    }

    /// Close whatever session is open on the box, succeeding if none is.
    ///
    /// Recovery for a test harness holding the box lock when a previous
    /// process died with a session open. It closes any session, so only call
    /// it when you own the box.
    pub fn session_force_close(&self) -> crate::error::Result<()> {
        self.client
            .run(ops::session_force_close())
            .map_err(ops::force_close_compat)
    }
}

#[cfg(feature = "async")]
impl<'a> AsyncBle<'a> {
    /// Open a held GATT session to `address`; see [`Ble::session_open`].
    pub async fn session_open(
        &self,
        address: &str,
        opts: &BleSessionOptions,
    ) -> crate::error::Result<AsyncBleSession<'a>> {
        let opened = self
            .client
            .run(ops::session_open(address, opts.connect_timeout))
            .await
            .map_err(ops::session_compat)?;
        Ok(AsyncBleSession {
            client: self.client,
            session_id: opened.session_id,
            address: opened.address,
            op_timeout: opts.op_timeout,
            closed: false,
        })
    }

    /// Close whatever session is open on the box; see
    /// [`Ble::session_force_close`].
    pub async fn session_force_close(&self) -> crate::error::Result<()> {
        self.client
            .run(ops::session_force_close())
            .await
            .map_err(ops::force_close_compat)
    }
}

/// A held BLE GATT connection on the box (from [`Ble::session_open`]).
///
/// Drop closes the session best-effort. Call [`BleSession::close`] to surface
/// errors. Only one session may be open per box.
#[cfg(feature = "blocking")]
pub struct BleSession<'a> {
    client: &'a crate::client::LagerBox,
    session_id: String,
    address: String,
    op_timeout: f64,
    closed: bool,
}

#[cfg(feature = "blocking")]
impl BleSession<'_> {
    /// Box-assigned session id.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Peer address this session is connected to.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Pair/bond on the held link.
    pub fn pair(&self) -> crate::error::Result<()> {
        self.client
            .run(ops::session_pair(&self.session_id, self.op_timeout))
            .map_err(ops::session_compat)
    }

    /// Read a GATT characteristic on the held link.
    pub fn read(&self, characteristic: impl Into<GattChar>) -> crate::error::Result<Vec<u8>> {
        self.client
            .run(ops::session_read(
                &self.session_id,
                self.op_timeout,
                characteristic,
            ))
            .map_err(ops::session_compat)
    }

    /// Write a GATT characteristic, letting the box pick response mode.
    pub fn write(
        &self,
        characteristic: impl Into<GattChar>,
        data: &[u8],
    ) -> crate::error::Result<()> {
        self.client
            .run(ops::session_write(
                &self.session_id,
                self.op_timeout,
                characteristic,
                data,
            ))
            .map_err(ops::session_compat)
    }

    /// Write, choosing write-with-response (`true`) or without (`false`).
    pub fn write_with_response(
        &self,
        characteristic: impl Into<GattChar>,
        data: &[u8],
        response: bool,
    ) -> crate::error::Result<()> {
        self.client
            .run(ops::session_write_with_response(
                &self.session_id,
                self.op_timeout,
                characteristic,
                data,
                response,
            ))
            .map_err(ops::session_compat)
    }

    /// Start buffering notifications for a characteristic.
    pub fn notify_start(&self, characteristic: impl Into<GattChar>) -> crate::error::Result<()> {
        self.client
            .run(ops::session_notify_start(
                &self.session_id,
                self.op_timeout,
                characteristic,
            ))
            .map_err(ops::session_compat)
    }

    /// Stop buffering notifications for a characteristic.
    pub fn notify_stop(&self, characteristic: impl Into<GattChar>) -> crate::error::Result<()> {
        self.client
            .run(ops::session_notify_stop(
                &self.session_id,
                self.op_timeout,
                characteristic,
            ))
            .map_err(ops::session_compat)
    }

    /// Drain buffered notifications, waiting up to `wait` seconds for more.
    /// `characteristic` limits the drain to one characteristic; `None` drains
    /// every active one round-robin. A short count comes back with
    /// [`BleSessionNotifications::timed_out`] set, not as an error.
    pub fn notify_poll(
        &self,
        characteristic: Option<GattChar>,
        max_messages: u32,
        wait: f64,
    ) -> crate::error::Result<BleSessionNotifications> {
        self.client
            .run(ops::session_notify_poll(
                &self.session_id,
                characteristic,
                max_messages,
                wait,
            ))
            .map_err(ops::session_compat)
    }

    /// Close the session now, surfacing any error.
    pub fn close(mut self) -> crate::error::Result<()> {
        let result = self
            .client
            .run(ops::session_close(&self.session_id))
            .map_err(ops::session_compat);
        // On failure, Drop retries.
        self.closed = result.is_ok();
        result
    }
}

#[cfg(feature = "blocking")]
impl Drop for BleSession<'_> {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.client.run(ops::session_close(&self.session_id));
        }
    }
}

/// Async counterpart of [`BleSession`] (from [`AsyncBle::session_open`]).
///
/// Prefer [`AsyncBleSession::close`]. Drop cannot await, so an un-closed
/// session is closed best-effort from a short-lived thread of its own.
#[cfg(feature = "async")]
pub struct AsyncBleSession<'a> {
    client: &'a crate::async_client::AsyncLagerBox,
    session_id: String,
    address: String,
    op_timeout: f64,
    closed: bool,
}

#[cfg(feature = "async")]
impl AsyncBleSession<'_> {
    /// Box-assigned session id.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Peer address this session is connected to.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Pair/bond on the held link.
    pub async fn pair(&self) -> crate::error::Result<()> {
        self.client
            .run(ops::session_pair(&self.session_id, self.op_timeout))
            .await
            .map_err(ops::session_compat)
    }

    /// Read a GATT characteristic on the held link.
    pub async fn read(&self, characteristic: impl Into<GattChar>) -> crate::error::Result<Vec<u8>> {
        self.client
            .run(ops::session_read(
                &self.session_id,
                self.op_timeout,
                characteristic,
            ))
            .await
            .map_err(ops::session_compat)
    }

    /// Write a GATT characteristic, letting the box pick response mode.
    pub async fn write(
        &self,
        characteristic: impl Into<GattChar>,
        data: &[u8],
    ) -> crate::error::Result<()> {
        self.client
            .run(ops::session_write(
                &self.session_id,
                self.op_timeout,
                characteristic,
                data,
            ))
            .await
            .map_err(ops::session_compat)
    }

    /// Write, choosing write-with-response (`true`) or without (`false`).
    pub async fn write_with_response(
        &self,
        characteristic: impl Into<GattChar>,
        data: &[u8],
        response: bool,
    ) -> crate::error::Result<()> {
        self.client
            .run(ops::session_write_with_response(
                &self.session_id,
                self.op_timeout,
                characteristic,
                data,
                response,
            ))
            .await
            .map_err(ops::session_compat)
    }

    /// Start buffering notifications for a characteristic.
    pub async fn notify_start(
        &self,
        characteristic: impl Into<GattChar>,
    ) -> crate::error::Result<()> {
        self.client
            .run(ops::session_notify_start(
                &self.session_id,
                self.op_timeout,
                characteristic,
            ))
            .await
            .map_err(ops::session_compat)
    }

    /// Stop buffering notifications for a characteristic.
    pub async fn notify_stop(
        &self,
        characteristic: impl Into<GattChar>,
    ) -> crate::error::Result<()> {
        self.client
            .run(ops::session_notify_stop(
                &self.session_id,
                self.op_timeout,
                characteristic,
            ))
            .await
            .map_err(ops::session_compat)
    }

    /// Drain buffered notifications; see [`BleSession::notify_poll`].
    pub async fn notify_poll(
        &self,
        characteristic: Option<GattChar>,
        max_messages: u32,
        wait: f64,
    ) -> crate::error::Result<BleSessionNotifications> {
        self.client
            .run(ops::session_notify_poll(
                &self.session_id,
                characteristic,
                max_messages,
                wait,
            ))
            .await
            .map_err(ops::session_compat)
    }

    /// Close the session now, surfacing any error.
    pub async fn close(mut self) -> crate::error::Result<()> {
        let result = self
            .client
            .run(ops::session_close(&self.session_id))
            .await
            .map_err(ops::session_compat);
        // On failure, Drop retries.
        self.closed = result.is_ok();
        result
    }
}

#[cfg(feature = "async")]
impl Drop for AsyncBleSession<'_> {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        // Own client and runtime: the borrowed client's connections belong to
        // the caller's runtime, and blocking its worker is not an option.
        let detached = self.client.detached();
        let session_id = std::mem::take(&mut self.session_id);
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            let _ = runtime.block_on(detached.run(ops::session_close(&session_id)));
        });
    }
}
