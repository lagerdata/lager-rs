//! Router nets (MikroTik RouterOS): network-condition control for
//! connectivity testing — toggling interfaces, changing SSIDs/passwords,
//! blocking traffic.
//!
//! Every action the box handler dispatches on has a typed method. Records
//! that come straight from RouterOS (interfaces, profiles, leases, created
//! rules) are returned as raw JSON in RouterOS's own field names.
//!
//! Configuration added through these methods is tagged on the router, so
//! [`Router::remove_firewall_rules`], [`Router::remove_bandwidth_limits`]
//! and [`Router::reset_to_defaults`] remove exactly what a test added.

use std::time::Duration;

use serde_json::Value;

use super::net_handle;
pub use crate::wire::RouterSystemInfo;

/// A wireless security profile for [`Router::create_security_profile`].
/// Fields left `None` take the box's defaults: `dynamic-keys`, `wpa2-psk`,
/// `aes-ccm`, and empty keys.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SecurityProfile {
    /// Profile name.
    pub name: String,
    /// RouterOS mode (`"dynamic-keys"`, `"none"`, ...).
    pub mode: Option<String>,
    /// Authentication types (`"wpa2-psk"`, `"wpa-psk,wpa2-psk"`, ...).
    pub authentication_types: Option<String>,
    /// Unicast ciphers (`"aes-ccm"`, `"tkip,aes-ccm"`, ...).
    pub unicast_ciphers: Option<String>,
    /// WPA2 passphrase.
    pub wpa2_pre_shared_key: Option<String>,
    /// WPA passphrase.
    pub wpa_pre_shared_key: Option<String>,
}

impl SecurityProfile {
    /// A WPA2-PSK profile with `passphrase`, the box's defaults otherwise.
    pub fn wpa2(name: impl Into<String>, passphrase: impl Into<String>) -> Self {
        SecurityProfile {
            name: name.into(),
            wpa2_pre_shared_key: Some(passphrase.into()),
            ..Default::default()
        }
    }
}

/// A wireless access-list entry for [`Router::add_access_list_entry`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccessListEntry {
    /// Client MAC address.
    pub mac_address: String,
    /// Allow the client to authenticate (`false` rejects it). The box
    /// defaults to `true`.
    pub authentication: Option<bool>,
    /// Limit the entry to one wireless interface.
    pub interface: Option<String>,
    /// RouterOS signal range, e.g. `"-80..120"`.
    pub signal_range: Option<String>,
}

/// The baseline [`Router::reset_to_defaults`] restores.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouterBaseline {
    /// SSID to restore on the wireless interfaces. `None` leaves SSIDs as
    /// they are.
    pub ssid: Option<String>,
    /// WPA2 passphrase for the baseline network. Required with `ssid`.
    pub password: Option<String>,
    /// Wireless interfaces to reset (`["wlan1"]`). `None` re-enables all
    /// of them.
    pub wireless_interfaces: Option<Vec<String>>,
}

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::{json, Value};

    use super::{AccessListEntry, RouterBaseline, SecurityProfile};
    use crate::error::{Error, Result};
    use crate::wire::{
        net_command, unit, value_as, CommandResponse, Op, RouterSystemInfo, Timeout,
    };

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

    fn cmd<T>(
        name: &str,
        action: &str,
        params: Value,
        parse: fn(CommandResponse) -> Result<T>,
    ) -> Op<T> {
        Op {
            req: net_command(name, ROLE, action, params, budget()),
            parse,
        }
    }

    /// A wait action polls the router for up to `timeout` on the box, so
    /// the HTTP budget has to outlast it.
    fn wait_budget(timeout: Duration) -> Timeout {
        Timeout::After(timeout + Duration::from_secs(30))
    }

    fn bool_field(resp: CommandResponse, key: &str) -> Result<bool> {
        resp.value
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                Error::Decode(format!(
                    "expected boolean '{key}' in 'value', got {:?}",
                    resp.value
                ))
            })
    }

    fn parse_updated(resp: CommandResponse) -> Result<bool> {
        bool_field(resp, "updated")
    }

    fn parse_deleted(resp: CommandResponse) -> Result<bool> {
        bool_field(resp, "deleted")
    }

    pub(crate) fn security_profiles(name: &str) -> Op<Value> {
        cmd(name, "security_profiles", json!({}), value_raw)
    }

    pub(crate) fn access_list(name: &str) -> Op<Value> {
        cmd(name, "access_list", json!({}), value_raw)
    }

    pub(crate) fn wait_for_ready(name: &str, timeout: Duration) -> Op<()> {
        Op {
            req: net_command(
                name,
                ROLE,
                "wait_for_ready",
                json!({ "timeout": timeout.as_secs_f64() }),
                wait_budget(timeout),
            ),
            parse: unit,
        }
    }

    pub(crate) fn wait_for_wireless_ready(
        name: &str,
        interface: &str,
        timeout: Duration,
    ) -> Op<()> {
        Op {
            req: net_command(
                name,
                ROLE,
                "wait_for_wireless_ready",
                json!({ "interface": interface, "timeout": timeout.as_secs_f64() }),
                wait_budget(timeout),
            ),
            parse: unit,
        }
    }

    pub(crate) fn set_interface_disabled(name: &str, interface: &str, disabled: bool) -> Op<()> {
        cmd(
            name,
            "set_interface_disabled",
            json!({ "interface": interface, "disabled": disabled }),
            unit,
        )
    }

    pub(crate) fn configure_wireless(name: &str, interface: &str, properties: Value) -> Op<Value> {
        cmd(
            name,
            "configure_wireless",
            json!({ "interface": interface, "kwargs": properties }),
            value_raw,
        )
    }

    pub(crate) fn create_security_profile(name: &str, profile: &SecurityProfile) -> Op<Value> {
        let mut params = json!({ "name": profile.name });
        if let Some(mode) = &profile.mode {
            params["mode"] = json!(mode);
        }
        if let Some(types) = &profile.authentication_types {
            params["authentication_types"] = json!(types);
        }
        if let Some(ciphers) = &profile.unicast_ciphers {
            params["unicast_ciphers"] = json!(ciphers);
        }
        if let Some(key) = &profile.wpa2_pre_shared_key {
            params["wpa2_pre_shared_key"] = json!(key);
        }
        if let Some(key) = &profile.wpa_pre_shared_key {
            params["wpa_pre_shared_key"] = json!(key);
        }
        cmd(name, "create_security_profile", params, value_raw)
    }

    pub(crate) fn create_open_security_profile(name: &str, profile: &str) -> Op<Value> {
        cmd(
            name,
            "create_open_security_profile",
            json!({ "name": profile }),
            value_raw,
        )
    }

    pub(crate) fn update_security_profile_password(
        name: &str,
        profile: &str,
        new_password: &str,
    ) -> Op<bool> {
        cmd(
            name,
            "update_security_profile_password",
            json!({ "name": profile, "new_password": new_password }),
            parse_updated,
        )
    }

    pub(crate) fn delete_security_profile(name: &str, profile: &str) -> Op<bool> {
        cmd(
            name,
            "delete_security_profile",
            json!({ "name": profile }),
            parse_deleted,
        )
    }

    pub(crate) fn enable_dhcp(name: &str) -> Op<()> {
        cmd(name, "enable_dhcp", json!({}), unit)
    }

    pub(crate) fn disable_dhcp(name: &str) -> Op<()> {
        cmd(name, "disable_dhcp", json!({}), unit)
    }

    pub(crate) fn set_dhcp_lease_time(name: &str, lease_time: &str) -> Op<()> {
        cmd(
            name,
            "set_dhcp_lease_time",
            json!({ "lease_time": lease_time }),
            unit,
        )
    }

    pub(crate) fn add_bandwidth_limit(
        name: &str,
        target: &str,
        max_limit: &str,
        queue_name: Option<&str>,
    ) -> Op<Value> {
        let mut params = json!({ "target": target, "max_limit": max_limit });
        if let Some(queue_name) = queue_name {
            params["name"] = json!(queue_name);
        }
        cmd(name, "add_bandwidth_limit", params, value_raw)
    }

    pub(crate) fn remove_bandwidth_limits(name: &str) -> Op<()> {
        cmd(name, "remove_bandwidth_limits", json!({}), unit)
    }

    pub(crate) fn add_firewall_rule(
        name: &str,
        chain: &str,
        action: &str,
        fields: Value,
    ) -> Op<Value> {
        // The box passes every key other than these two straight to
        // RouterOS, so the extra fields ride at the top level.
        let mut params = match fields {
            Value::Object(map) => Value::Object(map),
            _ => json!({}),
        };
        params["chain"] = json!(chain);
        params["rule_action"] = json!(action);
        cmd(name, "add_firewall_rule", params, value_raw)
    }

    pub(crate) fn block_dns(name: &str) -> Op<()> {
        cmd(name, "block_dns", json!({}), unit)
    }

    pub(crate) fn block_port(name: &str, port: u16, protocol: &str) -> Op<()> {
        cmd(
            name,
            "block_port",
            json!({ "port": port, "protocol": protocol }),
            unit,
        )
    }

    pub(crate) fn add_access_list_entry(name: &str, entry: &AccessListEntry) -> Op<Value> {
        let mut params = json!({ "mac_address": entry.mac_address });
        if let Some(authentication) = entry.authentication {
            params["authentication"] = json!(authentication);
        }
        if let Some(interface) = &entry.interface {
            params["interface"] = json!(interface);
        }
        if let Some(range) = &entry.signal_range {
            params["signal_range"] = json!(range);
        }
        cmd(name, "add_access_list_entry", params, value_raw)
    }

    pub(crate) fn remove_access_list_entry(name: &str, mac_address: &str) -> Op<()> {
        cmd(
            name,
            "remove_access_list_entry",
            json!({ "mac_address": mac_address }),
            unit,
        )
    }

    pub(crate) fn clear_access_list(name: &str) -> Op<()> {
        cmd(name, "clear_access_list", json!({}), unit)
    }

    pub(crate) fn reset_to_defaults(name: &str, baseline: &RouterBaseline) -> Op<()> {
        let mut params = json!({});
        if let Some(ssid) = &baseline.ssid {
            params["baseline_ssid"] = json!(ssid);
        }
        if let Some(password) = &baseline.password {
            params["baseline_pass"] = json!(password);
        }
        if let Some(interfaces) = &baseline.wireless_interfaces {
            params["wireless_interfaces"] = json!(interfaces);
        }
        cmd(name, "reset_to_defaults", params, unit)
    }

    pub(crate) fn rest_get(name: &str, path: &str, query: Option<Value>) -> Op<Value> {
        let mut params = json!({ "path": path });
        if let Some(query) = query {
            params["params"] = query;
        }
        cmd(name, "run", params, value_raw)
    }

    pub(crate) fn command(name: &str, action: &str, params: Value) -> Op<Value> {
        cmd(name, action, params, value_raw)
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
        /// Wireless security profiles (raw RouterOS records).
        fn security_profiles() -> Value = ops::security_profiles;
        /// Wireless access-list entries (raw RouterOS records).
        fn access_list() -> Value = ops::access_list;
        /// Wait until the router answers again, after a reboot. Fails with
        /// [`crate::Error::Box`] if it does not within `timeout`.
        fn wait_for_ready(timeout: Duration) -> () = ops::wait_for_ready;
        /// Wait until a wireless interface is enabled, after
        /// [`Router::enable_interface`] or [`Router::configure_wireless`].
        /// Fails with [`crate::Error::Box`] if it is not within `timeout`.
        fn wait_for_wireless_ready(interface: &str, timeout: Duration) -> () = ops::wait_for_wireless_ready;
        /// Enable or disable a network interface by name.
        fn set_interface_disabled(interface: &str, disabled: bool) -> () = ops::set_interface_disabled;
        /// Set any RouterOS wireless-interface properties, given as a JSON
        /// object (`{"ssid": "bench", "band": "2ghz-b/g/n",
        /// "security_profile": "wpa2"}`). Underscores in keys become
        /// hyphens on the router.
        fn configure_wireless(interface: &str, properties: Value) -> Value = ops::configure_wireless;
        /// Create a wireless security profile; returns the created record.
        fn create_security_profile(profile: &SecurityProfile) -> Value = ops::create_security_profile;
        /// Create a security profile with no encryption; returns the
        /// created record.
        fn create_open_security_profile(profile: &str) -> Value = ops::create_open_security_profile;
        /// Change a security profile's passphrase. `false` when no profile
        /// has that name.
        fn update_security_profile_password(profile: &str, new_password: &str) -> bool = ops::update_security_profile_password;
        /// Delete a security profile. `false` when no profile has that
        /// name.
        fn delete_security_profile(profile: &str) -> bool = ops::delete_security_profile;
        /// Turn the DHCP server on.
        fn enable_dhcp() -> () = ops::enable_dhcp;
        /// Turn the DHCP server off.
        fn disable_dhcp() -> () = ops::disable_dhcp;
        /// Set the DHCP lease time, in RouterOS syntax (`"10m"`, `"1h"`).
        fn set_dhcp_lease_time(lease_time: &str) -> () = ops::set_dhcp_lease_time;
        /// Limit bandwidth for an IP or subnet (`"192.168.88.0/24"`) to
        /// `max_limit` as upload/download (`"1M/1M"`, `"256k/2M"`). The
        /// queue is named for the test when `queue_name` is `None`.
        fn add_bandwidth_limit(target: &str, max_limit: &str, queue_name: Option<&str>) -> Value = ops::add_bandwidth_limit;
        /// Remove every bandwidth limit added through Lager.
        fn remove_bandwidth_limits() -> () = ops::remove_bandwidth_limits;
        /// Add a firewall filter rule to `chain` (`"forward"`, `"input"`,
        /// `"output"`) with `action` (`"drop"`, `"accept"`, `"reject"`).
        /// `fields` is a JSON object of further RouterOS rule properties
        /// (`{"protocol": "udp", "dst-port": "53"}`); pass `json!({})` for
        /// none.
        fn add_firewall_rule(chain: &str, action: &str, fields: Value) -> Value = ops::add_firewall_rule;
        /// Drop DNS traffic.
        fn block_dns() -> () = ops::block_dns;
        /// Drop traffic to one port (`protocol` is `"tcp"` or `"udp"`).
        fn block_port(port: u16, protocol: &str) -> () = ops::block_port;
        /// Add a wireless access-list entry; returns the created record.
        fn add_access_list_entry(entry: &AccessListEntry) -> Value = ops::add_access_list_entry;
        /// Remove a wireless access-list entry by MAC address.
        fn remove_access_list_entry(mac_address: &str) -> () = ops::remove_access_list_entry;
        /// Remove every access-list entry added through Lager.
        fn clear_access_list() -> () = ops::clear_access_list;
        /// Restore a known baseline for test isolation: removes added
        /// firewall rules, bandwidth limits, access-list entries and custom
        /// security profiles, re-enables DHCP and the wireless interfaces,
        /// and applies the baseline network if one is given.
        fn reset_to_defaults(baseline: &RouterBaseline) -> () = ops::reset_to_defaults;
        /// A read-only RouterOS REST call: `GET /rest<path>` (`"/ip/address"`)
        /// with optional query parameters as a JSON object.
        fn rest_get(path: &str, query: Option<Value>) -> Value = ops::rest_get;
        /// Run any router action by name with raw JSON params.
        fn command(action: &str, params: Value) -> Value = ops::command;
    }
}
