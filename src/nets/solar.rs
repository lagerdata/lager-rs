//! Solar-simulator nets (EA PSB supplies in photovoltaic mode).
//!
//! A solar net drives PV-simulation mode on an EA PSB supply — the same
//! physical VISA instrument a power-supply net can point at. Box-side, both
//! roles serialize under hardware_service's per-VISA-address lock, so solar
//! and supply commands on one EA can never interleave SCPI I/O.

use super::net_handle;

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::json;

    use crate::wire::{net_command, unit, value_f64, Op, Timeout};

    const ROLE: &str = "solar";

    /// Every solar action re-asserts PV mode on the instrument (enable() with
    /// settle retries) before doing its work, so even reads can be slow. The
    /// box widens its internal proxy budget to 90s for set/stop and 60s for
    /// reads; the client budget sits above both.
    fn mode_timeout() -> Timeout {
        Timeout::After(Duration::from_secs(120))
    }

    fn read_timeout() -> Timeout {
        Timeout::After(Duration::from_secs(90))
    }

    pub(crate) fn set(name: &str) -> Op<()> {
        Op {
            req: net_command(name, ROLE, "set", json!({}), mode_timeout()),
            parse: unit,
        }
    }

    pub(crate) fn stop(name: &str) -> Op<()> {
        Op {
            req: net_command(name, ROLE, "stop", json!({}), mode_timeout()),
            parse: unit,
        }
    }

    fn read(name: &str, action: &str) -> Op<f64> {
        Op {
            req: net_command(name, ROLE, action, json!({}), read_timeout()),
            parse: value_f64,
        }
    }

    fn write(name: &str, action: &str, value: f64) -> Op<f64> {
        Op {
            req: net_command(name, ROLE, action, json!({ "value": value }), read_timeout()),
            parse: value_f64,
        }
    }

    pub(crate) fn irradiance(name: &str) -> Op<f64> {
        read(name, "irradiance")
    }

    pub(crate) fn set_irradiance(name: &str, watts_per_m2: f64) -> Op<f64> {
        write(name, "irradiance", watts_per_m2)
    }

    pub(crate) fn mpp_current(name: &str) -> Op<f64> {
        read(name, "mpp_current")
    }

    pub(crate) fn mpp_voltage(name: &str) -> Op<f64> {
        read(name, "mpp_voltage")
    }

    pub(crate) fn resistance(name: &str) -> Op<f64> {
        read(name, "resistance")
    }

    pub(crate) fn set_resistance(name: &str, ohms: f64) -> Op<f64> {
        write(name, "resistance", ohms)
    }

    pub(crate) fn temperature(name: &str) -> Op<f64> {
        read(name, "temperature")
    }

    pub(crate) fn voc(name: &str) -> Op<f64> {
        read(name, "voc")
    }
}

net_handle! {
    /// Handle for a solar-simulator net.
    sync: Solar,
    async: AsyncSolar,
    methods: {
        /// Initialize the instrument and start PV simulation mode.
        fn set() -> () = ops::set;
        /// Stop PV simulation and release the instrument's remote lock.
        fn stop() -> () = ops::stop;
        /// Read the configured irradiance (W/m²).
        fn irradiance() -> f64 = ops::irradiance;
        /// Set the irradiance (W/m², 0–1500); returns the applied value.
        fn set_irradiance(watts_per_m2: f64) -> f64 = ops::set_irradiance;
        /// Read the maximum-power-point current (A).
        fn mpp_current() -> f64 = ops::mpp_current;
        /// Read the maximum-power-point voltage (V).
        fn mpp_voltage() -> f64 = ops::mpp_voltage;
        /// Read the dynamic panel resistance (Voc / Isc, ohm).
        fn resistance() -> f64 = ops::resistance;
        /// Set the dynamic panel resistance (ohm); returns the applied value.
        fn set_resistance(ohms: f64) -> f64 = ops::set_resistance;
        /// Read the cell temperature (°C).
        fn temperature() -> f64 = ops::temperature;
        /// Read the open-circuit voltage (V).
        fn voc() -> f64 = ops::voc;
    }
}
