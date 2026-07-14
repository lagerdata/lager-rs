//! ADC nets: analog voltage reads.

use super::net_handle;

pub(crate) mod ops {
    use serde_json::json;

    use crate::wire::{net_command, value_f64, Op, Timeout};

    pub(crate) fn read(name: &str) -> Op<f64> {
        Op {
            req: net_command(name, "adc", "read", json!({}), Timeout::Default),
            parse: value_f64,
        }
    }
}

net_handle! {
    /// Handle for an ADC net.
    sync: Adc,
    async: AsyncAdc,
    methods: {
        /// Read the input voltage in volts.
        fn read() -> f64 = ops::read;
    }
}
