//! Power-supply nets (Rigol DP800, Keithley 2281S, Keysight E36000, ...).

use super::net_handle;
pub use crate::wire::SupplyState;

pub(crate) mod ops {
    use serde_json::json;

    use crate::wire::{state_as, supply_command, unit, Op, SupplyState};

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
