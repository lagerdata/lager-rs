//! Router nets (MikroTik RouterOS): network-condition control for
//! connectivity testing — toggling interfaces, changing SSIDs/passwords,
//! blocking traffic.
//!
//! The common read/control actions get typed methods; the long tail of
//! RouterOS configuration (security profiles, DHCP tuning, bandwidth limits,
//! firewall rules, access lists, raw REST calls) is reachable through the
//! generic [`Router::command`] escape hatch with the same action names the
//! box handler dispatches on.

use serde_json::Value;

use super::net_handle;
pub use crate::wire::RouterSystemInfo;

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::{json, Value};

    use crate::error::Result;
    use crate::wire::{net_command, unit, value_as, CommandResponse, Op, RouterSystemInfo, Timeout};

    // Router saved-net records may carry the role string "router" or the
    // legacy alias "mikrotik" (both dispatch to the same box handler). The
    // box verifies a supplied role hint against the saved string exactly, so
    // sending "router" would 404 on a net saved as "mikrotik". Omit the hint
    // and let the box resolve the role from saved_nets.json.
    const ROLE: Option<&str> = None;

    /// RouterOS REST calls are quick, but reboot/reset actions and busy
    /// routers can lag; give every action a comfortable budget.
    fn budget() -> Timeout {
        Timeout::After(Duration::from_secs(60))
    }

    fn value_raw(resp: CommandResponse) -> Result<Value> {
        Ok(resp.value.unwrap_or(Value::Null))
    }

    pub(crate) fn connect(name: &str) -> Op<Value> {
        Op {
            req: net_command(name, ROLE, "connect", json!({}), budget()),
            parse: value_raw,
        }
    }

    pub(crate) fn system_info(name: &str) -> Op<RouterSystemInfo> {
        Op {
            req: net_command(name, ROLE, "system_info", json!({}), budget()),
            parse: value_as::<RouterSystemInfo>,
        }
    }

    pub(crate) fn interfaces(name: &str) -> Op<Value> {
        Op {
            req: net_command(name, ROLE, "interfaces", json!({}), budget()),
            parse: value_raw,
        }
    }

    pub(crate) fn wireless_interfaces(name: &str) -> Op<Value> {
        Op {
            req: net_command(name, ROLE, "wireless_interfaces", json!({}), budget()),
            parse: value_raw,
        }
    }

    pub(crate) fn wireless_clients(name: &str) -> Op<Value> {
        Op {
            req: net_command(name, ROLE, "wireless_clients", json!({}), budget()),
            parse: value_raw,
        }
    }

    pub(crate) fn dhcp_leases(name: &str) -> Op<Value> {
        Op {
            req: net_command(name, ROLE, "dhcp_leases", json!({}), budget()),
            parse: value_raw,
        }
    }

    pub(crate) fn reboot(name: &str) -> Op<()> {
        Op {
            req: net_command(name, ROLE, "reboot", json!({}), budget()),
            parse: unit,
        }
    }

    pub(crate) fn enable_interface(name: &str, interface: &str) -> Op<()> {
        Op {
            req: net_command(
                name,
                ROLE,
                "enable_interface",
                json!({ "interface": interface }),
                budget(),
            ),
            parse: unit,
        }
    }

    pub(crate) fn disable_interface(name: &str, interface: &str) -> Op<()> {
        Op {
            req: net_command(
                name,
                ROLE,
                "disable_interface",
                json!({ "interface": interface }),
                budget(),
            ),
            parse: unit,
        }
    }

    pub(crate) fn set_wireless_ssid(name: &str, interface: &str, ssid: &str) -> Op<Value> {
        Op {
            req: net_command(
                name,
                ROLE,
                "set_wireless_ssid",
                json!({ "interface": interface, "ssid": ssid }),
                budget(),
            ),
            parse: value_raw,
        }
    }

    pub(crate) fn block_internet(name: &str) -> Op<()> {
        Op {
            req: net_command(name, ROLE, "block_internet", json!({}), budget()),
            parse: unit,
        }
    }

    pub(crate) fn remove_firewall_rules(name: &str) -> Op<()> {
        Op {
            req: net_command(name, ROLE, "remove_firewall_rules", json!({}), budget()),
            parse: unit,
        }
    }

    pub(crate) fn command(name: &str, action: &str, params: Value) -> Op<Value> {
        Op {
            req: net_command(name, ROLE, action, params, budget()),
            parse: value_raw,
        }
    }
}

net_handle! {
    /// Handle for a router net.
    sync: Router,
    async: AsyncRouter,
    methods: {
        /// Verify connectivity; returns identity/version/board/uptime.
        fn connect() -> Value = ops::connect;
        /// System resource information.
        fn system_info() -> RouterSystemInfo = ops::system_info;
        /// All network interfaces (raw RouterOS records).
        fn interfaces() -> Value = ops::interfaces;
        /// Wireless interfaces (raw RouterOS records).
        fn wireless_interfaces() -> Value = ops::wireless_interfaces;
        /// Currently associated wireless clients.
        fn wireless_clients() -> Value = ops::wireless_clients;
        /// Active DHCP leases.
        fn dhcp_leases() -> Value = ops::dhcp_leases;
        /// Reboot the router.
        fn reboot() -> () = ops::reboot;
        /// Enable a network interface by name.
        fn enable_interface(interface: &str) -> () = ops::enable_interface;
        /// Disable a network interface by name.
        fn disable_interface(interface: &str) -> () = ops::disable_interface;
        /// Change the SSID broadcast by a wireless interface.
        fn set_wireless_ssid(interface: &str, ssid: &str) -> Value = ops::set_wireless_ssid;
        /// Drop all forwarded traffic (simulate internet outage).
        fn block_internet() -> () = ops::block_internet;
        /// Remove every Lager-added firewall rule (undo `block_*`).
        fn remove_firewall_rules() -> () = ops::remove_firewall_rules;
        /// Run any other router action by name with raw JSON params
        /// (security profiles, DHCP config, bandwidth limits, firewall
        /// rules, access lists, `run` for raw RouterOS REST paths, ...).
        fn command(action: &str, params: Value) -> Value = ops::command;
    }
}
