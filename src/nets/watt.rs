//! Watt-meter nets (Joulescope, PPK2, Yocto): timed power measurements.

use super::net_handle;
pub use crate::wire::WattReading;

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::json;

    use crate::wire::{net_command, value_as, value_f64, Op, Timeout, WattReading};

    const ROLE: &str = "watt-meter";

    /// Budget: cover the integration window plus margin, floor 30s
    /// (mirrors `lager watt`'s `max(30, duration + 20)`).
    fn budget(duration: f64) -> Timeout {
        Timeout::After(Duration::from_secs_f64((duration + 20.0).max(30.0)))
    }

    fn scalar(name: &str, action: &'static str, duration: f64) -> Op<f64> {
        Op {
            req: net_command(
                name,
                ROLE,
                action,
                json!({ "duration": duration }),
                budget(duration),
            ),
            parse: value_f64,
        }
    }

    pub(crate) fn power(name: &str, duration: f64) -> Op<f64> {
        scalar(name, "power", duration)
    }

    pub(crate) fn current(name: &str, duration: f64) -> Op<f64> {
        scalar(name, "current", duration)
    }

    pub(crate) fn voltage(name: &str, duration: f64) -> Op<f64> {
        scalar(name, "voltage", duration)
    }

    pub(crate) fn all(name: &str, duration: f64) -> Op<WattReading> {
        Op {
            req: net_command(
                name,
                ROLE,
                "all",
                json!({ "duration": duration }),
                budget(duration),
            ),
            parse: value_as::<WattReading>,
        }
    }
}

net_handle! {
    /// Handle for a watt-meter net.
    ///
    /// Every measurement integrates over a caller-supplied window in seconds
    /// (the CLI default is `0.1`); the HTTP timeout is widened automatically
    /// to cover the window.
    sync: WattMeter,
    async: AsyncWattMeter,
    methods: {
        /// Measure mean power in watts over `duration` seconds.
        fn power(duration: f64) -> f64 = ops::power;
        /// Measure mean current in amps over `duration` seconds.
        /// Requires a meter with current readout (Joulescope/PPK2).
        fn current(duration: f64) -> f64 = ops::current;
        /// Measure mean voltage in volts over `duration` seconds.
        /// Requires a meter with voltage readout (Joulescope/PPK2).
        fn voltage(duration: f64) -> f64 = ops::voltage;
        /// Measure current, voltage, and power together over `duration`
        /// seconds.
        fn all(duration: f64) -> WattReading = ops::all;
    }
}
