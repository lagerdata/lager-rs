//! GPIO nets: digital input, output, and hardware-accelerated level waits.

use super::net_handle;

/// A digital logic level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Logic high (1).
    High,
    /// Logic low (0).
    Low,
}

impl Level {
    /// Wire encoding understood by the box's gpio adapter.
    pub fn as_str(&self) -> &'static str {
        match self {
            Level::High => "high",
            Level::Low => "low",
        }
    }

    /// True for [`Level::High`].
    pub fn is_high(&self) -> bool {
        matches!(self, Level::High)
    }
}

/// Advanced options for [`Gpio::wait_for_level_with`]. All fields are
/// optional; `None` uses the box-side defaults.
#[derive(Debug, Clone, Copy, Default)]
pub struct WaitForLevelOptions {
    /// Give up after this many seconds. `None` waits forever (the client
    /// HTTP timeout is dropped entirely, matching `lager gpi --wait-for`
    /// without `--timeout`).
    pub timeout: Option<f64>,
    /// LabJack streaming sample rate in Hz (advanced).
    pub scan_rate: Option<u32>,
    /// LabJack scans per read batch (advanced).
    pub scans_per_read: Option<u32>,
    /// Poll interval in seconds for non-streaming drivers (advanced).
    pub poll_interval: Option<f64>,
}

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::{json, Map, Value};

    use super::{Level, WaitForLevelOptions};
    use crate::error::Result;
    use crate::wire::{net_command, unit, value_i64, CommandResponse, Op, Timeout};

    const ROLE: &str = "gpio";

    fn parse_level(resp: CommandResponse) -> Result<Level> {
        value_i64(resp).map(|v| if v != 0 { Level::High } else { Level::Low })
    }

    pub(crate) fn input(name: &str) -> Op<Level> {
        Op {
            req: net_command(name, ROLE, "input", json!({}), Timeout::Default),
            parse: parse_level,
        }
    }

    pub(crate) fn output(name: &str, level: Level) -> Op<()> {
        Op {
            req: net_command(
                name,
                ROLE,
                "output",
                json!({ "level": level.as_str() }),
                Timeout::Default,
            ),
            parse: unit,
        }
    }

    pub(crate) fn output_high(name: &str) -> Op<()> {
        Op {
            req: net_command(name, ROLE, "output_high", json!({}), Timeout::Default),
            parse: unit,
        }
    }

    pub(crate) fn output_low(name: &str) -> Op<()> {
        Op {
            req: net_command(name, ROLE, "output_low", json!({}), Timeout::Default),
            parse: unit,
        }
    }

    pub(crate) fn toggle(name: &str) -> Op<Level> {
        Op {
            req: net_command(
                name,
                ROLE,
                "output",
                json!({ "level": "toggle" }),
                Timeout::Default,
            ),
            parse: parse_level,
        }
    }

    fn wait_params(level: Level, opts: &WaitForLevelOptions) -> Value {
        let mut params = Map::new();
        params.insert("level".to_string(), json!(level.as_str()));
        if let Some(t) = opts.timeout {
            params.insert("timeout".to_string(), json!(t));
        }
        if let Some(r) = opts.scan_rate {
            params.insert("scan_rate".to_string(), json!(r));
        }
        if let Some(s) = opts.scans_per_read {
            params.insert("scans_per_read".to_string(), json!(s));
        }
        if let Some(p) = opts.poll_interval {
            params.insert("poll_interval".to_string(), json!(p));
        }
        Value::Object(params)
    }

    pub(crate) fn wait_for_level_with(
        name: &str,
        level: Level,
        opts: &WaitForLevelOptions,
    ) -> Op<f64> {
        // Widen the client timeout past the device-side wait budget (+20s
        // margin, like `lager gpi --wait-for`) so the device method — not
        // the transport — decides when to give up. With no wait timeout the
        // client waits unbounded, matching the CLI.
        let timeout = match opts.timeout {
            Some(t) => Timeout::After(Duration::from_secs_f64(t + 20.0)),
            None => Timeout::Unbounded,
        };
        Op {
            req: net_command(name, ROLE, "wait_for_level", wait_params(level, opts), timeout),
            parse: crate::wire::value_f64,
        }
    }

    pub(crate) fn wait_for_level(name: &str, level: Level, timeout_s: f64) -> Op<f64> {
        wait_for_level_with(
            name,
            level,
            &WaitForLevelOptions {
                timeout: Some(timeout_s),
                ..WaitForLevelOptions::default()
            },
        )
    }
}

net_handle! {
    /// Handle for a GPIO net.
    sync: Gpio,
    async: AsyncGpio,
    methods: {
        /// Read the current input level.
        fn input() -> Level = ops::input;
        /// Drive the output to `level`.
        fn output(level: Level) -> () = ops::output;
        /// Drive the output high.
        fn output_high() -> () = ops::output_high;
        /// Drive the output low.
        fn output_low() -> () = ops::output_low;
        /// Toggle the output and return the new level.
        fn toggle() -> Level = ops::toggle;
        /// Block until the input reaches `level` or `timeout_s` seconds
        /// elapse. Returns the elapsed time in seconds. Times out box-side;
        /// the HTTP timeout is automatically widened past `timeout_s`.
        fn wait_for_level(level: Level, timeout_s: f64) -> f64 = ops::wait_for_level;
        /// [`Gpio::wait_for_level`] with full control over the wait
        /// parameters, including an unbounded wait (`timeout: None`).
        fn wait_for_level_with(level: Level, opts: &WaitForLevelOptions) -> f64 = ops::wait_for_level_with;
    }
}
