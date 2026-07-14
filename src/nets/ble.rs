//! BLE (box-level): scan for and inspect Bluetooth Low Energy devices using
//! the box's own Bluetooth adapter.
//!
//! There is one adapter per box and the box serializes all BLE and BluFi
//! operations on it, so concurrent calls queue rather than fail.

use super::box_handle;
pub use crate::wire::{BleCharacteristic, BleDevice, BleDeviceInfo, BleService};

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::json;

    use crate::error::Result;
    use crate::wire::{
        box_command, unit, value_as, value_list_field, BleDevice, BleDeviceInfo, CommandResponse,
        Op, Timeout,
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

    pub(crate) fn info(address: &str) -> Op<BleDeviceInfo> {
        Op {
            req: box_command(
                PATH,
                "info",
                json!({ "address": address, "timeout": CONNECT_TIMEOUT_S }),
                budget(CONNECT_TIMEOUT_S),
            ),
            parse: value_as::<BleDeviceInfo>,
        }
    }

    pub(crate) fn connect(address: &str) -> Op<BleDeviceInfo> {
        Op {
            req: box_command(
                PATH,
                "connect",
                json!({ "address": address, "timeout": CONNECT_TIMEOUT_S }),
                budget(CONNECT_TIMEOUT_S),
            ),
            parse: value_as::<BleDeviceInfo>,
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
        /// Connect briefly and enumerate the device's GATT services.
        fn info(address: &str) -> BleDeviceInfo = ops::info;
        /// Connect to a device (and enumerate its services to verify).
        fn connect(address: &str) -> BleDeviceInfo = ops::connect;
        /// Ensure a device is disconnected from the box.
        fn disconnect(address: &str) -> () = ops::disconnect;
    }
}
