//! DAC nets: analog voltage output.

use super::net_handle;

pub(crate) mod ops {
    use serde_json::json;

    use crate::wire::{net_command, unit, value_f64, Op, Timeout};

    const ROLE: &str = "dac";

    pub(crate) fn set(name: &str, volts: f64) -> Op<()> {
        Op {
            req: net_command(name, ROLE, "set", json!({ "value": volts }), Timeout::Default),
            parse: unit,
        }
    }

    pub(crate) fn read(name: &str) -> Op<f64> {
        Op {
            req: net_command(name, ROLE, "read", json!({}), Timeout::Default),
            parse: value_f64,
        }
    }
}

net_handle! {
    /// Handle for a DAC net.
    sync: Dac,
    async: AsyncDac,
    methods: {
        /// Set the output voltage in volts.
        fn set(volts: f64) -> () = ops::set;
        /// Read back the current output voltage in volts.
        fn read() -> f64 = ops::read;
    }
}
