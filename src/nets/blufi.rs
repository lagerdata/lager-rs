//! BluFi (box-level): provision ESP32 devices onto WiFi over BLE, using the
//! box's Bluetooth adapter and Espressif's BluFi protocol.
//!
//! Every action except `scan` connects to the target by its advertised BLE
//! name and negotiates BluFi security first, so budgets are wide;
//! provisioning end-to-end (BLE connect + credential transfer + the target
//! joining WiFi) routinely takes 30 s+.

use super::box_handle;
pub use crate::wire::{BleDevice, BlufiDeviceInfo, BlufiNetwork, BlufiProvisionResult, BlufiStatus};

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::json;

    use crate::error::Result;
    use crate::wire::{
        box_command, value_as, value_list_field, BleDevice, BlufiDeviceInfo, BlufiNetwork,
        BlufiProvisionResult, BlufiStatus, CommandResponse, Op, Timeout,
    };

    const PATH: &str = "/blufi/command";

    /// Box-side BLE connect timeout (s), matching the CLI default.
    const CONNECT_TIMEOUT_S: f64 = 20.0;

    fn parse_devices(resp: CommandResponse) -> Result<Vec<BleDevice>> {
        value_list_field(resp, "devices")
    }

    fn parse_networks(resp: CommandResponse) -> Result<Vec<BlufiNetwork>> {
        value_list_field(resp, "networks")
    }

    fn budget(box_side: f64) -> Timeout {
        Timeout::After(Duration::from_secs_f64(box_side + 40.0))
    }

    pub(crate) fn scan(timeout: f64) -> Op<Vec<BleDevice>> {
        Op {
            req: box_command(PATH, "scan", json!({ "timeout": timeout }), budget(timeout)),
            parse: parse_devices,
        }
    }

    pub(crate) fn connect(device_name: &str) -> Op<BlufiDeviceInfo> {
        Op {
            req: box_command(
                PATH,
                "connect",
                json!({ "device_name": device_name, "timeout": CONNECT_TIMEOUT_S }),
                budget(CONNECT_TIMEOUT_S),
            ),
            parse: value_as::<BlufiDeviceInfo>,
        }
    }

    pub(crate) fn provision(
        device_name: &str,
        ssid: &str,
        password: &str,
    ) -> Op<BlufiProvisionResult> {
        Op {
            req: box_command(
                PATH,
                "provision",
                json!({
                    "device_name": device_name,
                    "ssid": ssid,
                    "password": password,
                    "timeout": CONNECT_TIMEOUT_S,
                }),
                // Connect + security + credential transfer + the target
                // joining WiFi + status polling.
                budget(CONNECT_TIMEOUT_S + 60.0),
            ),
            parse: value_as::<BlufiProvisionResult>,
        }
    }

    pub(crate) fn wifi_scan(device_name: &str) -> Op<Vec<BlufiNetwork>> {
        // The target runs its own WiFi scan (box default 15s) after the BLE
        // connect completes.
        Op {
            req: box_command(
                PATH,
                "wifi_scan",
                json!({ "device_name": device_name, "timeout": CONNECT_TIMEOUT_S }),
                budget(CONNECT_TIMEOUT_S + 30.0),
            ),
            parse: parse_networks,
        }
    }

    pub(crate) fn status(device_name: &str) -> Op<BlufiStatus> {
        Op {
            req: box_command(
                PATH,
                "status",
                json!({ "device_name": device_name, "timeout": CONNECT_TIMEOUT_S }),
                budget(CONNECT_TIMEOUT_S),
            ),
            parse: value_as::<BlufiStatus>,
        }
    }

    pub(crate) fn version(device_name: &str) -> Op<Option<String>> {
        fn parse_version(resp: CommandResponse) -> Result<Option<String>> {
            #[derive(serde::Deserialize)]
            struct V {
                #[serde(default)]
                version: Option<String>,
            }
            let v: V = value_as(resp)?;
            Ok(v.version)
        }
        Op {
            req: box_command(
                PATH,
                "version",
                json!({ "device_name": device_name, "timeout": CONNECT_TIMEOUT_S }),
                budget(CONNECT_TIMEOUT_S),
            ),
            parse: parse_version,
        }
    }
}

box_handle! {
    /// Handle for BluFi provisioning over the box's Bluetooth adapter
    /// (from [`crate::LagerBox::blufi`]).
    sync: Blufi,
    async: AsyncBlufi,
    methods: {
        /// Scan for BluFi-capable BLE devices for `timeout` seconds.
        fn scan(timeout: f64) -> Vec<BleDevice> = ops::scan;
        /// Connect to a target by its advertised BLE name; returns its
        /// firmware version and WiFi state.
        fn connect(device_name: &str) -> BlufiDeviceInfo = ops::connect;
        /// Provision a target onto a WiFi network. Fails (as an
        /// [`crate::Error::Box`]) when the target does not reach the
        /// connected state.
        fn provision(device_name: &str, ssid: &str, password: &str)
            -> BlufiProvisionResult = ops::provision;
        /// Ask the target to scan for WiFi networks it can see.
        fn wifi_scan(device_name: &str) -> Vec<BlufiNetwork> = ops::wifi_scan;
        /// Read the target's current WiFi state.
        fn status(device_name: &str) -> BlufiStatus = ops::status;
        /// Read the target's BluFi firmware version.
        fn version(device_name: &str) -> Option<String> = ops::version;
    }
}
