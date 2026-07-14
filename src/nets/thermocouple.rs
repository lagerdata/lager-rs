//! Thermocouple nets: temperature reads.

use super::net_handle;

pub(crate) mod ops {
    use serde_json::json;

    use crate::wire::{net_command, value_f64, Op, Timeout};

    pub(crate) fn read(name: &str) -> Op<f64> {
        Op {
            req: net_command(name, "thermocouple", "read", json!({}), Timeout::Default),
            parse: value_f64,
        }
    }
}

net_handle! {
    /// Handle for a thermocouple net.
    sync: Thermocouple,
    async: AsyncThermocouple,
    methods: {
        /// Read the temperature in degrees Celsius.
        fn read() -> f64 = ops::read;
    }
}
