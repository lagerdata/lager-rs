//! Bench multimeter nets (box >= 0.62.0; Siglent SDM3065X): one reading of
//! volts, current, resistance, continuity, diode, frequency, period,
//! capacitance or temperature.
//!
//! ```no_run
//! # #[cfg(feature = "blocking")]
//! # fn demo() -> lager::Result<()> {
//! use lager::{DmmFunction, DmmOptions, LagerBox};
//!
//! let lager = LagerBox::from_env()?;
//! let dmm = lager.dmm("dmm1");
//! let reading = dmm.measure(DmmFunction::DcVolts, &DmmOptions::new().range(10.0))?;
//! println!("{:?} {}", reading.value, reading.unit);
//! # Ok(())
//! # }
//! # fn main() {}
//! ```
//!
//! The box checks a request against the meter before touching it: a
//! function, range, NPLC value or sensor the meter cannot do fails with
//! [`crate::Error::Box`] (HTTP 400) naming the reason. The box lists `dmm`
//! in [`crate::BoxCapabilities::net_command_roles`] when it serves this role.

use super::net_handle;

/// What a DMM measures, and in which unit [`DmmReading::value`] comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmmFunction {
    /// DC volts (V).
    DcVolts,
    /// AC volts (V).
    AcVolts,
    /// DC current (A).
    DcCurrent,
    /// AC current (A).
    AcCurrent,
    /// 2-wire resistance (ohm).
    Resistance,
    /// 4-wire resistance (ohm).
    FourWireResistance,
    /// Continuity (ohm); an open input reads as overload.
    Continuity,
    /// Diode forward voltage (V); an open input reads as overload.
    Diode,
    /// Frequency (Hz).
    Frequency,
    /// Period (s).
    Period,
    /// Capacitance (F).
    Capacitance,
    /// Temperature, always in degrees Celsius; pick the probe with
    /// [`DmmOptions::sensor`].
    Temperature,
}

impl DmmFunction {
    /// The box's name for the function, as `lager dmm` takes it.
    pub fn as_str(&self) -> &'static str {
        match self {
            DmmFunction::DcVolts => "dcv",
            DmmFunction::AcVolts => "acv",
            DmmFunction::DcCurrent => "dci",
            DmmFunction::AcCurrent => "aci",
            DmmFunction::Resistance => "res",
            DmmFunction::FourWireResistance => "fres",
            DmmFunction::Continuity => "cont",
            DmmFunction::Diode => "diode",
            DmmFunction::Frequency => "freq",
            DmmFunction::Period => "period",
            DmmFunction::Capacitance => "cap",
            DmmFunction::Temperature => "temp",
        }
    }
}

/// Optional settings for one [`Dmm::measure`]. The defaults let the meter
/// auto-range and keep its own integration time and sensor.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct DmmOptions {
    /// Full-scale range in the function's unit; `None` auto-ranges.
    pub range: Option<f64>,
    /// Integration time in power-line cycles.
    pub nplc: Option<f64>,
    /// Temperature probe, such as `"rtd:pt100"` or `"ther:k"`.
    pub sensor: Option<String>,
}

impl DmmOptions {
    /// Auto-range, with the meter's own integration time and sensor.
    pub fn new() -> Self {
        DmmOptions::default()
    }

    /// Use a fixed full-scale range, in the function's unit.
    pub fn range(mut self, range: f64) -> Self {
        self.range = Some(range);
        self
    }

    /// Integrate for this many power-line cycles.
    pub fn nplc(mut self, nplc: f64) -> Self {
        self.nplc = Some(nplc);
        self
    }

    /// Read temperature with this probe, such as `"rtd:pt100"` or `"ther:k"`.
    pub fn sensor(mut self, sensor: impl Into<String>) -> Self {
        self.sensor = Some(sensor.into());
        self
    }
}

/// One DMM reading.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct DmmReading {
    /// The reading in [`DmmReading::unit`]; `None` when the meter reports
    /// overload, or an open input for continuity and diode.
    pub value: Option<f64>,
    /// `"V"`, `"A"`, `"ohm"`, `"Hz"`, `"s"`, `"F"` or `"C"`.
    pub unit: String,
    /// Whether the meter reported overload or an open input.
    pub overload: bool,
}

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::{json, Value};

    use super::{DmmFunction, DmmOptions, DmmReading};
    use crate::error::{Error, Result};
    use crate::wire::{as_f64, net_command, value_as, CommandResponse, Op, Timeout};

    const ROLE: &str = "dmm";

    /// The box allows one reading 60 s (top capacitance ranges and slow
    /// auto-ranging take seconds per reading); this leaves room for the
    /// round trip.
    fn budget() -> Timeout {
        Timeout::After(Duration::from_secs(75))
    }

    fn parse_reading(resp: CommandResponse) -> Result<DmmReading> {
        let unit = resp
            .extra
            .get("unit")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Decode("DMM response missing 'unit'".to_string()))?
            .to_string();
        let overload = resp
            .extra
            .get("overload")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let value = match (&resp.value, overload) {
            (_, true) => None,
            (Some(v), false) => Some(
                as_f64(v)
                    .ok_or_else(|| Error::Decode(format!("DMM 'value' is not a number: {v}")))?,
            ),
            (None, false) => {
                return Err(Error::Decode(
                    "DMM response has neither 'value' nor 'overload'".to_string(),
                ))
            }
        };
        Ok(DmmReading {
            value,
            unit,
            overload,
        })
    }

    pub(crate) fn measure(name: &str, function: DmmFunction, opts: &DmmOptions) -> Op<DmmReading> {
        let mut params = json!({ "function": function.as_str() });
        if let Some(range) = opts.range {
            params["range"] = json!(range);
        }
        if let Some(nplc) = opts.nplc {
            params["nplc"] = json!(nplc);
        }
        if let Some(sensor) = &opts.sensor {
            params["sensor"] = json!(sensor);
        }
        Op {
            req: net_command(name, ROLE, "measure", params, budget()),
            parse: parse_reading,
        }
    }

    pub(crate) fn identify(name: &str) -> Op<String> {
        Op {
            req: net_command(name, ROLE, "identify", json!({}), budget()),
            parse: value_as::<String>,
        }
    }

    pub(crate) fn errors(name: &str) -> Op<Vec<String>> {
        Op {
            req: net_command(name, ROLE, "errors", json!({}), budget()),
            parse: value_as::<Vec<String>>,
        }
    }
}

net_handle! {
    /// Handle for a bench multimeter (DMM) net.
    sync: Dmm,
    async: AsyncDmm,
    methods: {
        /// Take one reading. The value is `None`, with `overload` set, when
        /// the meter is over range or the input is open.
        fn measure(function: DmmFunction, options: &DmmOptions) -> DmmReading = ops::measure;
        /// The meter's `*IDN?` reply.
        fn identify() -> String = ops::identify;
        /// Drain the meter's error queue; empty when it is clear.
        fn errors() -> Vec<String> = ops::errors;
    }
}
