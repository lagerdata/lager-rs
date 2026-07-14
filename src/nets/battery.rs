//! Battery-simulator nets (Keithley 2281S in battery mode).

use super::net_handle;
pub use crate::wire::BatteryState;

/// Battery simulation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryMode {
    /// Static simulation: SOC stays where you set it.
    Static,
    /// Dynamic simulation: SOC evolves with the load current.
    Dynamic,
}

impl BatteryMode {
    /// Wire encoding.
    pub fn as_str(&self) -> &'static str {
        match self {
            BatteryMode::Static => "static",
            BatteryMode::Dynamic => "dynamic",
        }
    }
}

pub(crate) mod ops {
    use serde_json::json;

    use super::BatteryMode;
    use crate::wire::{battery_command, state_as, unit, BatteryState, Op};

    fn set_value(name: &str, action: &'static str, value: f64) -> Op<()> {
        Op {
            req: battery_command(name, action, json!({ "value": value })),
            parse: unit,
        }
    }

    fn simple(name: &str, action: &'static str) -> Op<()> {
        Op {
            req: battery_command(name, action, json!({})),
            parse: unit,
        }
    }

    pub(crate) fn set_soc(name: &str, percent: f64) -> Op<()> {
        set_value(name, "set_soc", percent)
    }

    pub(crate) fn set_voc(name: &str, volts: f64) -> Op<()> {
        set_value(name, "set_voc", volts)
    }

    pub(crate) fn set_volt_full(name: &str, volts: f64) -> Op<()> {
        set_value(name, "set_volt_full", volts)
    }

    pub(crate) fn set_volt_empty(name: &str, volts: f64) -> Op<()> {
        set_value(name, "set_volt_empty", volts)
    }

    pub(crate) fn set_capacity(name: &str, amp_hours: f64) -> Op<()> {
        set_value(name, "set_capacity", amp_hours)
    }

    pub(crate) fn set_current_limit(name: &str, amps: f64) -> Op<()> {
        set_value(name, "set_current_limit", amps)
    }

    pub(crate) fn set_ocp(name: &str, amps: f64) -> Op<()> {
        set_value(name, "set_ocp", amps)
    }

    pub(crate) fn set_ovp(name: &str, volts: f64) -> Op<()> {
        set_value(name, "set_ovp", volts)
    }

    pub(crate) fn set_mode(name: &str, mode: BatteryMode) -> Op<()> {
        Op {
            req: battery_command(name, "set_mode", json!({ "mode_type": mode.as_str() })),
            parse: unit,
        }
    }

    pub(crate) fn set_model(name: &str, partnumber: &str) -> Op<()> {
        Op {
            req: battery_command(name, "set_model", json!({ "partnumber": partnumber })),
            parse: unit,
        }
    }

    pub(crate) fn enable(name: &str) -> Op<()> {
        simple(name, "enable_battery")
    }

    pub(crate) fn disable(name: &str) -> Op<()> {
        simple(name, "disable_battery")
    }

    pub(crate) fn init_battery_mode(name: &str) -> Op<()> {
        simple(name, "set_to_battery_mode")
    }

    pub(crate) fn clear_ocp(name: &str) -> Op<()> {
        simple(name, "clear_ocp")
    }

    pub(crate) fn clear_ovp(name: &str) -> Op<()> {
        simple(name, "clear_ovp")
    }

    pub(crate) fn clear(name: &str) -> Op<()> {
        simple(name, "clear")
    }

    pub(crate) fn state(name: &str) -> Op<BatteryState> {
        Op {
            req: battery_command(name, "state", json!({})),
            parse: state_as::<BatteryState>,
        }
    }
}

net_handle! {
    /// Handle for a battery-simulator net.
    ///
    /// Reads (terminal voltage, SOC, current, ...) come from
    /// [`Battery::state`], which gathers everything in a single instrument
    /// transaction.
    sync: Battery,
    async: AsyncBattery,
    methods: {
        /// Put the instrument into battery-simulator mode. Run this once
        /// before other battery commands on a dual-role instrument.
        fn init_battery_mode() -> () = ops::init_battery_mode;
        /// Set state of charge in percent (0–100).
        fn set_soc(percent: f64) -> () = ops::set_soc;
        /// Set open-circuit voltage in volts.
        fn set_voc(volts: f64) -> () = ops::set_voc;
        /// Set the voltage considered "full" in volts.
        fn set_volt_full(volts: f64) -> () = ops::set_volt_full;
        /// Set the voltage considered "empty" in volts.
        fn set_volt_empty(volts: f64) -> () = ops::set_volt_empty;
        /// Set battery capacity in amp-hours.
        fn set_capacity(amp_hours: f64) -> () = ops::set_capacity;
        /// Set the output current limit in amps.
        fn set_current_limit(amps: f64) -> () = ops::set_current_limit;
        /// Set the over-current protection limit in amps.
        fn set_ocp(amps: f64) -> () = ops::set_ocp;
        /// Set the over-voltage protection limit in volts.
        fn set_ovp(volts: f64) -> () = ops::set_ovp;
        /// Set the simulation mode (static/dynamic).
        fn set_mode(mode: BatteryMode) -> () = ops::set_mode;
        /// Load a predefined battery model by part number.
        fn set_model(partnumber: &str) -> () = ops::set_model;
        /// Enable the simulator output.
        fn enable() -> () = ops::enable;
        /// Disable the simulator output. Call this in test teardown.
        fn disable() -> () = ops::disable;
        /// Clear an over-current protection trip.
        fn clear_ocp() -> () = ops::clear_ocp;
        /// Clear an over-voltage protection trip.
        fn clear_ovp() -> () = ops::clear_ovp;
        /// Clear all protection trips.
        fn clear() -> () = ops::clear;
        /// Read the full structured simulator state.
        fn state() -> BatteryState = ops::state;
    }
}
