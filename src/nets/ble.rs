//! BLE (box-level): scan for and inspect Bluetooth Low Energy devices using
//! the box's own Bluetooth adapter.
//!
//! There is one adapter per box and the box serializes all BLE and BluFi
//! operations on it, so concurrent calls queue rather than fail.

use super::box_handle;
pub use crate::wire::{BleCharacteristic, BleDevice, BleDeviceInfo, BleNotifications, BleService};

/// A GATT characteristic, by UUID or by attribute handle, for the GATT
/// methods on [`Ble`]. Converts from a `&str` / `String` (a UUID, such as
/// `"00002a19-0000-1000-8000-00805f9b34fb"`) and from a `u16` (a handle).
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
}

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::json;

    use serde::Deserialize;

    use super::GattChar;
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
        /// between operations cannot be driven this way. Box >= 0.62.0;
        /// older boxes fail with [`crate::Error::UnsupportedByBox`].
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
        /// (box >= 0.62.0).
        fn pair(address: &str) -> () = ops::pair, compat("pair", ">= 0.62.0");
    }
}
