//! I2C bus nets.
//!
//! Bus transactions run box-side under the physical device's shared lock,
//! so a scan or a write-then-read cannot interleave with a concurrent
//! request on the same device.

use serde::Deserialize;

use super::net_handle;
use crate::wire::lenient;

/// The effective I2C configuration the box reports after `configure`.
#[derive(Debug, Clone, Deserialize)]
pub struct I2cEffectiveConfig {
    /// Bus frequency in Hz.
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub frequency_hz: Option<i64>,
    /// Whether internal pull-ups are enabled.
    #[serde(default, deserialize_with = "lenient::opt_bool")]
    pub pull_ups: Option<bool>,
}

pub(crate) mod ops {
    use serde_json::{json, Map, Value};

    use super::I2cEffectiveConfig;
    use crate::error::{Error, Result};
    use crate::wire::{as_i64, net_command, unit, value_as, CommandResponse, Op, Timeout};

    const ROLE: &str = "i2c";

    fn parse_bytes(resp: CommandResponse) -> Result<Vec<u8>> {
        let arr = resp
            .value
            .as_ref()
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Decode(format!("expected byte list, got {:?}", resp.value)))?;
        arr.iter()
            .map(|v| {
                as_i64(v)
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(|| Error::Decode(format!("non-byte element: {v:?}")))
            })
            .collect()
    }

    fn parse_addresses(resp: CommandResponse) -> Result<Vec<u16>> {
        let arr = resp
            .value
            .as_ref()
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Decode(format!("expected address list, got {:?}", resp.value)))?;
        arr.iter()
            .map(|v| {
                as_i64(v)
                    .and_then(|n| u16::try_from(n).ok())
                    .ok_or_else(|| Error::Decode(format!("non-address element: {v:?}")))
            })
            .collect()
    }

    pub(crate) fn configure(
        name: &str,
        frequency_hz: Option<u32>,
        pull_ups: Option<bool>,
    ) -> Op<I2cEffectiveConfig> {
        let mut params = Map::new();
        if let Some(f) = frequency_hz {
            params.insert("frequency_hz".to_string(), json!(f));
        }
        if let Some(p) = pull_ups {
            params.insert("pull_ups".to_string(), json!(p));
        }
        Op {
            req: net_command(name, ROLE, "config", Value::Object(params), Timeout::Default),
            parse: value_as::<I2cEffectiveConfig>,
        }
    }

    pub(crate) fn scan(name: &str) -> Op<Vec<u16>> {
        Op {
            req: net_command(name, ROLE, "scan", json!({}), Timeout::Default),
            parse: parse_addresses,
        }
    }

    pub(crate) fn scan_range(name: &str, start_addr: u16, end_addr: u16) -> Op<Vec<u16>> {
        Op {
            req: net_command(
                name,
                ROLE,
                "scan",
                json!({ "start_addr": start_addr, "end_addr": end_addr }),
                Timeout::Default,
            ),
            parse: parse_addresses,
        }
    }

    pub(crate) fn read(name: &str, address: u16, num_bytes: u32) -> Op<Vec<u8>> {
        Op {
            req: net_command(
                name,
                ROLE,
                "read",
                json!({ "address": address, "num_bytes": num_bytes }),
                Timeout::Default,
            ),
            parse: parse_bytes,
        }
    }

    pub(crate) fn write(name: &str, address: u16, data: &[u8]) -> Op<()> {
        Op {
            req: net_command(
                name,
                ROLE,
                "write",
                json!({ "address": address, "data": data }),
                Timeout::Default,
            ),
            parse: unit,
        }
    }

    pub(crate) fn write_read(
        name: &str,
        address: u16,
        data: &[u8],
        num_bytes: u32,
    ) -> Op<Vec<u8>> {
        Op {
            req: net_command(
                name,
                ROLE,
                "transfer",
                json!({ "address": address, "data": data, "num_bytes": num_bytes }),
                Timeout::Default,
            ),
            parse: parse_bytes,
        }
    }
}

net_handle! {
    /// Handle for an I2C bus net.
    sync: I2c,
    async: AsyncI2c,
    methods: {
        /// Apply configuration overrides (persisted on the box) and return
        /// the effective configuration. `None` fields keep saved values.
        fn configure(frequency_hz: Option<u32>, pull_ups: Option<bool>) -> I2cEffectiveConfig = ops::configure;
        /// Scan the bus with the default address range; returns the 7-bit
        /// addresses that ACKed.
        fn scan() -> Vec<u16> = ops::scan;
        /// Scan the bus over `start_addr..=end_addr`.
        fn scan_range(start_addr: u16, end_addr: u16) -> Vec<u16> = ops::scan_range;
        /// Read `num_bytes` bytes from the device at `address`.
        fn read(address: u16, num_bytes: u32) -> Vec<u8> = ops::read;
        /// Write `data` to the device at `address`.
        fn write(address: u16, data: &[u8]) -> () = ops::write;
        /// Write `data` then read `num_bytes` in one transaction
        /// (repeated start).
        fn write_read(address: u16, data: &[u8], num_bytes: u32) -> Vec<u8> = ops::write_read;
    }
}
