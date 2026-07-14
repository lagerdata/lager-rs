//! WiFi (box-level): manage the box's own wireless interface — useful for
//! testing DUT-hosted access points or moving the box between networks.

use super::box_handle;
pub use crate::wire::{WifiAccessPoint, WifiConnection, WifiInterface};

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::json;

    use crate::error::Result;
    use crate::wire::{
        box_command, unit, value_as, value_list_field, CommandResponse, Op, Timeout,
        WifiAccessPoint, WifiConnection, WifiInterface,
    };

    const PATH: &str = "/wifi/command";

    fn parse_interfaces(resp: CommandResponse) -> Result<Vec<WifiInterface>> {
        value_list_field(resp, "interfaces")
    }

    fn parse_access_points(resp: CommandResponse) -> Result<Vec<WifiAccessPoint>> {
        value_list_field(resp, "access_points")
    }

    /// nmcli/iwlist calls block for seconds (connect can retry through
    /// wpa_supplicant), and actions queue on the box's wifi lock.
    fn budget() -> Timeout {
        Timeout::After(Duration::from_secs(90))
    }

    pub(crate) fn status() -> Op<Vec<WifiInterface>> {
        Op {
            req: box_command(PATH, "status", json!({}), budget()),
            parse: parse_interfaces,
        }
    }

    pub(crate) fn scan(interface: &str) -> Op<Vec<WifiAccessPoint>> {
        Op {
            req: box_command(PATH, "scan", json!({ "interface": interface }), budget()),
            parse: parse_access_points,
        }
    }

    pub(crate) fn connect(ssid: &str, password: &str) -> Op<WifiConnection> {
        Op {
            req: box_command(
                PATH,
                "connect",
                json!({ "ssid": ssid, "password": password }),
                budget(),
            ),
            parse: value_as::<WifiConnection>,
        }
    }

    pub(crate) fn delete(ssid: &str) -> Op<()> {
        Op {
            req: box_command(PATH, "delete", json!({ "ssid": ssid }), budget()),
            parse: unit,
        }
    }
}

box_handle! {
    /// Handle for the box's WiFi interface (from [`crate::LagerBox::wifi`]).
    sync: Wifi,
    async: AsyncWifi,
    methods: {
        /// Status of every wireless interface on the box.
        fn status() -> Vec<WifiInterface> = ops::status;
        /// Scan for access points on an interface (e.g. `"wlan0"`),
        /// strongest first.
        fn scan(interface: &str) -> Vec<WifiAccessPoint> = ops::scan;
        /// Join a network (empty password for open networks).
        fn connect(ssid: &str, password: &str) -> WifiConnection = ops::connect;
        /// Delete a saved connection profile by SSID/name.
        fn delete(ssid: &str) -> () = ops::delete;
    }
}
