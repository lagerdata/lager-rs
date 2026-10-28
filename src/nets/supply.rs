//! Power-supply nets (Rigol DP800, Keithley 2281S, Keysight E36000, ...).

use super::net_handle;
pub use crate::wire::SupplyState;

/// Protection limits to apply in the same instrument transaction as a
/// setpoint, for [`Supply::set_voltage_with`] and
/// [`Supply::set_current_with`]. `None` leaves that limit unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProtectionLimits {
    /// Over-current protection limit (A).
    pub ocp: Option<f64>,
    /// Over-voltage protection limit (V).
    pub ovp: Option<f64>,
}

pub(crate) mod ops {
    use serde_json::{json, Map, Value};

    use super::ProtectionLimits;
    use crate::wire::{state_as, supply_command, unit, Op, SupplyState};

    fn setpoint_params(value: f64, limits: &ProtectionLimits) -> Value {
        let mut params = Map::new();
        params.insert("value".to_string(), json!(value));
        if let Some(ocp) = limits.ocp {
            params.insert("ocp".to_string(), json!(ocp));
        }
        if let Some(ovp) = limits.ovp {
            params.insert("ovp".to_string(), json!(ovp));
        }
        Value::Object(params)
    }

    pub(crate) fn set_voltage(name: &str, volts: f64) -> Op<()> {
        Op {
            req: supply_command(name, "voltage", json!({ "value": volts })),
            parse: unit,
        }
    }

    pub(crate) fn set_current(name: &str, amps: f64) -> Op<()> {
        Op {
            req: supply_command(name, "current", json!({ "value": amps })),
            parse: unit,
        }
    }

    pub(crate) fn set_voltage_with(name: &str, volts: f64, limits: &ProtectionLimits) -> Op<()> {
        Op {
            req: supply_command(name, "voltage", setpoint_params(volts, limits)),
            parse: unit,
        }
    }

    pub(crate) fn set_current_with(name: &str, amps: f64, limits: &ProtectionLimits) -> Op<()> {
        Op {
            req: supply_command(name, "current", setpoint_params(amps, limits)),
            parse: unit,
        }
    }

    pub(crate) fn set_mode(name: &str) -> Op<()> {
        Op {
            req: supply_command(name, "set_mode", json!({})),
            parse: unit,
        }
    }

    pub(crate) fn enable(name: &str) -> Op<()> {
        Op {
            req: supply_command(name, "enable", json!({})),
            parse: unit,
        }
    }

    pub(crate) fn disable(name: &str) -> Op<()> {
        Op {
            req: supply_command(name, "disable", json!({})),
            parse: unit,
        }
    }

    pub(crate) fn set_ocp(name: &str, amps: f64) -> Op<()> {
        Op {
            req: supply_command(name, "ocp", json!({ "value": amps })),
            parse: unit,
        }
    }

    pub(crate) fn set_ovp(name: &str, volts: f64) -> Op<()> {
        Op {
            req: supply_command(name, "ovp", json!({ "value": volts })),
            parse: unit,
        }
    }

    pub(crate) fn clear_ocp(name: &str) -> Op<()> {
        Op {
            req: supply_command(name, "clear_ocp", json!({})),
            parse: unit,
        }
    }

    pub(crate) fn clear_ovp(name: &str) -> Op<()> {
        Op {
            req: supply_command(name, "clear_ovp", json!({})),
            parse: unit,
        }
    }

    pub(crate) fn state(name: &str) -> Op<SupplyState> {
        Op {
            req: supply_command(name, "state", json!({})),
            parse: state_as::<SupplyState>,
        }
    }
}

net_handle! {
    /// Handle for a power-supply net.
    ///
    /// Reads (measured voltage/current, setpoints, protection limits) come
    /// from [`Supply::state`], which gathers everything in a single
    /// instrument transaction.
    sync: Supply,
    async: AsyncSupply,
    methods: {
        /// Set the voltage setpoint in volts. The box rejects values above
        /// the instrument's hardware limit.
        fn set_voltage(volts: f64) -> () = ops::set_voltage;
        /// Set the current limit in amps.
        fn set_current(amps: f64) -> () = ops::set_current;
        /// Set the voltage setpoint together with any of the OCP/OVP
        /// limits, in one request. Each limit given is also enabled, as
        /// with [`Supply::set_ocp`] / [`Supply::set_ovp`]. The box rejects
        /// limits above the instrument's protection range, and some drivers
        /// reject an OVP below the voltage setpoint.
        fn set_voltage_with(volts: f64, limits: &ProtectionLimits) -> () = ops::set_voltage_with;
        /// Set the current limit together with any of the OCP/OVP limits,
        /// in one request. See [`Supply::set_voltage_with`].
        fn set_current_with(amps: f64, limits: &ProtectionLimits) -> () = ops::set_current_with;
        /// Switch the instrument into power-supply mode. Only the Keithley
        /// 2281S has another mode (battery simulator); on every other
        /// supply this is a no-op. Run it before supply commands when the
        /// instrument was last used as a battery simulator.
        fn set_mode() -> () = ops::set_mode;
        /// Enable the output.
        fn enable() -> () = ops::enable;
        /// Disable the output. Call this in test teardown.
        fn disable() -> () = ops::disable;
        /// Set and enable over-current protection (A).
        fn set_ocp(amps: f64) -> () = ops::set_ocp;
        /// Set and enable over-voltage protection (V).
        fn set_ovp(volts: f64) -> () = ops::set_ovp;
        /// Clear an over-current protection trip.
        fn clear_ocp() -> () = ops::clear_ocp;
        /// Clear an over-voltage protection trip.
        fn clear_ovp() -> () = ops::clear_ovp;
        /// Read the full structured state (measurements, setpoints,
        /// protection limits, output enable).
        fn state() -> SupplyState = ops::state;
    }
}
