//! Oscilloscope nets (Rigol MSO5000, PicoScope): channel setup, timebase,
//! trigger, acquisition control and measurements (box >= 0.55.0).
//!
//! A scope is saved on the box as one `scope` net for the instrument and one
//! `scope-channel` net per input. Channel settings (enable, V/div, offset,
//! coupling, probe) and measurements go to a channel net. Everything else
//! (timebase, horizontal position, trigger, run/stop/single) belongs to the
//! instrument, and works through either kind of net. A channel action sent
//! to the instrument's `scope` net fails with [`crate::Error::Box`] (HTTP
//! 400) naming the channel nets to use instead.
//!
//! An older box answers HTTP 501 for scope nets, which surfaces as
//! [`crate::Error::UnsupportedByBox`].
//!
//! Trace capture (the waveform stream behind the box's `/scope` page) is not
//! on the HTTP API; see `MISSING_ENDPOINTS.md` in this crate's repository.
//!
//! ```no_run
//! # #[cfg(feature = "blocking")]
//! # fn demo() -> lager::Result<()> {
//! use lager::{EdgeTrigger, LagerBox, Measurement, TriggerMode};
//!
//! let lager = LagerBox::from_env()?;
//! let ch1 = lager.scope("scope-ch1");
//! ch1.enable()?;
//! ch1.set_scale(0.5)?;
//! ch1.set_timebase(1e-3)?;
//! ch1.trigger_edge(&EdgeTrigger {
//!     source: Some("CHAN1".into()),
//!     level: Some(1.2),
//!     mode: Some(TriggerMode::Normal),
//!     ..Default::default()
//! })?;
//! if let Some(hz) = ch1.measure(Measurement::Frequency)? {
//!     println!("{hz} Hz");
//! }
//! # Ok(())
//! # }
//! # fn main() {}
//! ```

use serde::Deserialize;

use super::net_handle;

/// One quantity [`Scope::measure`] can read from a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Measurement {
    /// Peak-to-peak voltage (V).
    Vpp,
    /// Maximum voltage (V).
    Vmax,
    /// Minimum voltage (V).
    Vmin,
    /// RMS voltage (V).
    Vrms,
    /// Average voltage (V).
    Vavg,
    /// Period (s).
    Period,
    /// Frequency (Hz).
    Frequency,
    /// Positive duty cycle (%).
    DutyCyclePositive,
    /// Negative duty cycle (%).
    DutyCycleNegative,
    /// Positive pulse width (s).
    PulseWidthPositive,
    /// Negative pulse width (s).
    PulseWidthNegative,
    /// Rise time (s).
    RiseTime,
    /// Fall time (s).
    FallTime,
    /// Overshoot (%).
    Overshoot,
}

impl Measurement {
    /// The box action that reads this quantity.
    fn action(self) -> &'static str {
        match self {
            Measurement::Vpp => "measure_vpp",
            Measurement::Vmax => "measure_vmax",
            Measurement::Vmin => "measure_vmin",
            Measurement::Vrms => "measure_vrms",
            Measurement::Vavg => "measure_vavg",
            Measurement::Period => "measure_period",
            Measurement::Frequency => "measure_freq",
            Measurement::DutyCyclePositive => "measure_dc_pos",
            Measurement::DutyCycleNegative => "measure_dc_neg",
            Measurement::PulseWidthPositive => "measure_pulse_width_pos",
            Measurement::PulseWidthNegative => "measure_pulse_width_neg",
            Measurement::RiseTime => "measure_rise_time",
            Measurement::FallTime => "measure_fall_time",
            Measurement::Overshoot => "measure_overshoot",
        }
    }
}

/// Every quantity [`Scope::measure_all`] read from one capture. A quantity
/// the capture does not support (a period on a DC level) is `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct Measurements {
    /// Peak-to-peak voltage (V).
    pub vpp: Option<f64>,
    /// Maximum voltage (V).
    pub vmax: Option<f64>,
    /// Minimum voltage (V).
    pub vmin: Option<f64>,
    /// RMS voltage (V).
    pub vrms: Option<f64>,
    /// Average voltage (V).
    pub vavg: Option<f64>,
    /// Period (s).
    pub period: Option<f64>,
    /// Frequency (Hz).
    pub frequency: Option<f64>,
    /// Positive duty cycle (%).
    pub duty_cycle_positive: Option<f64>,
    /// Negative duty cycle (%).
    pub duty_cycle_negative: Option<f64>,
    /// Positive pulse width (s).
    pub pulse_width_positive: Option<f64>,
    /// Negative pulse width (s).
    pub pulse_width_negative: Option<f64>,
    /// Rise time (s).
    pub rise_time: Option<f64>,
    /// Fall time (s).
    pub fall_time: Option<f64>,
    /// Overshoot (%).
    pub overshoot: Option<f64>,
}

impl Measurements {
    /// The value for one [`Measurement`].
    pub fn get(&self, m: Measurement) -> Option<f64> {
        match m {
            Measurement::Vpp => self.vpp,
            Measurement::Vmax => self.vmax,
            Measurement::Vmin => self.vmin,
            Measurement::Vrms => self.vrms,
            Measurement::Vavg => self.vavg,
            Measurement::Period => self.period,
            Measurement::Frequency => self.frequency,
            Measurement::DutyCyclePositive => self.duty_cycle_positive,
            Measurement::DutyCycleNegative => self.duty_cycle_negative,
            Measurement::PulseWidthPositive => self.pulse_width_positive,
            Measurement::PulseWidthNegative => self.pulse_width_negative,
            Measurement::RiseTime => self.rise_time,
            Measurement::FallTime => self.fall_time,
            Measurement::Overshoot => self.overshoot,
        }
    }
}

/// How the scope re-arms after a trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerMode {
    /// Trigger on the edge, or free-run when none arrives.
    Auto,
    /// Trigger only on the edge.
    Normal,
    /// Arm for one acquisition. Setting this mode arms the scope, as on a
    /// bench scope.
    Single,
}

impl TriggerMode {
    fn as_str(self) -> &'static str {
        match self {
            TriggerMode::Auto => "auto",
            TriggerMode::Normal => "normal",
            TriggerMode::Single => "single",
        }
    }
}

/// Edge-trigger settings for [`Scope::trigger_edge`]. Only the fields that
/// are set are applied, so `level` alone moves the level and leaves the
/// source and slope as they are. At least one must be set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EdgeTrigger {
    /// Trigger source as the instrument names it (`"CHAN1"` on a Rigol,
    /// `"A"` on a PicoScope).
    pub source: Option<String>,
    /// Edge to trigger on, as the instrument names it (`"rising"`,
    /// `"falling"`, ...).
    pub slope: Option<String>,
    /// The trigger path's filter (`"dc"`, `"ac"`, `"lfreject"`,
    /// `"hfreject"`). This is not the channel's input coupling; see
    /// [`Scope::set_coupling`] for that.
    pub coupling: Option<String>,
    /// Trigger level (V).
    pub level: Option<f64>,
    /// Re-arm mode.
    pub mode: Option<TriggerMode>,
}

/// The trigger as the instrument has it, from [`Scope::trigger`]. Fields are
/// the instrument's own spellings.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct TriggerSettings {
    /// Capture mode (`auto`, `normal`, `single`, ...).
    #[serde(default)]
    pub mode: Option<String>,
    /// Trigger source.
    #[serde(default)]
    pub source: Option<String>,
    /// Trigger slope.
    #[serde(default)]
    pub slope: Option<String>,
    /// Trigger level (V).
    #[serde(default)]
    pub level: Option<f64>,
    /// Trigger holdoff (s), when the scope's daemon reports one.
    #[serde(default)]
    pub holdoff: Option<f64>,
}

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::{json, Value};

    use super::{EdgeTrigger, Measurement, Measurements, TriggerSettings};
    use crate::error::{Error, Result};
    use crate::wire::{
        as_f64, net_command, unit, value_as, value_f64, CommandResponse, Op, Timeout,
    };

    /// The box accepts `scope` for both scope roles and returns the saved
    /// one, which its per-channel check reads.
    const ROLE: &str = "scope";

    /// The box allows its scope adapter 30 s per command.
    fn budget() -> Timeout {
        Timeout::After(Duration::from_secs(45))
    }

    fn cmd<T>(
        name: &str,
        action: &str,
        params: Value,
        parse: fn(CommandResponse) -> Result<T>,
    ) -> Op<T> {
        Op {
            req: net_command(name, ROLE, action, params, budget()),
            parse,
        }
    }

    fn parse_string(resp: CommandResponse) -> Result<String> {
        match resp.value {
            Some(Value::String(s)) => Ok(s),
            other => Err(Error::Decode(format!(
                "expected string 'value', got {other:?}"
            ))),
        }
    }

    fn parse_bool(resp: CommandResponse) -> Result<bool> {
        match resp.value {
            Some(Value::Bool(b)) => Ok(b),
            other => Err(Error::Decode(format!(
                "expected boolean 'value', got {other:?}"
            ))),
        }
    }

    /// A quantity the capture does not contain comes back as success with
    /// no `value`.
    fn parse_measurement(resp: CommandResponse) -> Result<Option<f64>> {
        match resp.value {
            None | Some(Value::Null) => Ok(None),
            Some(v) => as_f64(&v)
                .map(Some)
                .ok_or_else(|| Error::Decode(format!("expected numeric 'value', got {v:?}"))),
        }
    }

    fn parse_value(resp: CommandResponse) -> Result<Value> {
        Ok(resp.value.unwrap_or(Value::Null))
    }

    pub(crate) fn enable(name: &str) -> Op<()> {
        cmd(name, "enable_net", json!({}), unit)
    }

    pub(crate) fn disable(name: &str) -> Op<()> {
        cmd(name, "disable_net", json!({}), unit)
    }

    pub(crate) fn is_enabled(name: &str) -> Op<bool> {
        cmd(name, "get_net_enabled", json!({}), parse_bool)
    }

    pub(crate) fn run(name: &str) -> Op<()> {
        cmd(name, "start_capture", json!({}), unit)
    }

    pub(crate) fn single(name: &str) -> Op<()> {
        cmd(name, "start_single", json!({}), unit)
    }

    pub(crate) fn stop(name: &str) -> Op<()> {
        cmd(name, "stop_capture", json!({}), unit)
    }

    pub(crate) fn force_trigger(name: &str) -> Op<()> {
        cmd(name, "force_trigger", json!({}), unit)
    }

    pub(crate) fn autoscale(name: &str) -> Op<()> {
        cmd(name, "autoscale", json!({}), unit)
    }

    pub(crate) fn set_scale(name: &str, volts_per_div: f64) -> Op<()> {
        cmd(
            name,
            "set_scale",
            json!({ "volts_per_div": volts_per_div }),
            unit,
        )
    }

    pub(crate) fn scale(name: &str) -> Op<f64> {
        cmd(name, "get_scale", json!({}), value_f64)
    }

    pub(crate) fn set_timebase(name: &str, seconds_per_div: f64) -> Op<()> {
        cmd(
            name,
            "set_timebase",
            json!({ "seconds_per_div": seconds_per_div }),
            unit,
        )
    }

    pub(crate) fn timebase(name: &str) -> Op<f64> {
        cmd(name, "get_timebase", json!({}), value_f64)
    }

    pub(crate) fn set_coupling(name: &str, mode: &str) -> Op<()> {
        cmd(name, "set_coupling", json!({ "mode": mode }), unit)
    }

    pub(crate) fn coupling(name: &str) -> Op<String> {
        cmd(name, "get_coupling", json!({}), parse_string)
    }

    pub(crate) fn set_probe(name: &str, ratio: f64) -> Op<()> {
        cmd(name, "set_probe", json!({ "ratio": ratio }), unit)
    }

    pub(crate) fn probe(name: &str) -> Op<f64> {
        cmd(name, "get_probe", json!({}), value_f64)
    }

    pub(crate) fn set_offset(name: &str, volts: f64) -> Op<()> {
        cmd(name, "set_offset", json!({ "offset": volts }), unit)
    }

    pub(crate) fn offset(name: &str) -> Op<f64> {
        cmd(name, "get_offset", json!({}), value_f64)
    }

    pub(crate) fn set_time_offset(name: &str, seconds: f64) -> Op<()> {
        cmd(name, "set_time_offset", json!({ "offset": seconds }), unit)
    }

    pub(crate) fn time_offset(name: &str) -> Op<f64> {
        cmd(name, "get_time_offset", json!({}), value_f64)
    }

    pub(crate) fn trigger_edge(name: &str, trigger: &EdgeTrigger) -> Op<()> {
        let mut params = json!({});
        if let Some(source) = &trigger.source {
            params["source"] = json!(source);
        }
        if let Some(slope) = &trigger.slope {
            params["slope"] = json!(slope);
        }
        if let Some(coupling) = &trigger.coupling {
            params["coupling"] = json!(coupling);
        }
        if let Some(level) = trigger.level {
            params["level"] = json!(level);
        }
        if let Some(mode) = trigger.mode {
            params["mode"] = json!(mode.as_str());
        }
        cmd(name, "trigger_edge", params, unit)
    }

    pub(crate) fn trigger(name: &str) -> Op<TriggerSettings> {
        cmd(name, "get_trigger", json!({}), value_as::<TriggerSettings>)
    }

    pub(crate) fn measure(name: &str, m: Measurement) -> Op<Option<f64>> {
        cmd(name, m.action(), json!({}), parse_measurement)
    }

    pub(crate) fn measure_all(name: &str) -> Op<Measurements> {
        cmd(name, "measure_all", json!({}), value_as::<Measurements>)
    }

    pub(crate) fn command(name: &str, action: &str, params: Value) -> Op<Value> {
        cmd(name, action, params, parse_value)
    }
}

net_handle! {
    /// Handle for an oscilloscope net: an instrument (`scope`) or one of
    /// its channels (`scope-channel`). See the [module docs](self) for which
    /// actions need a channel net.
    sync: Scope,
    async: AsyncScope,
    methods: {
        /// Turn this net's channel on.
        fn enable() -> () = ops::enable;
        /// Turn this net's channel off.
        fn disable() -> () = ops::disable;
        /// Whether this net's channel is on, as the instrument reports it.
        fn is_enabled() -> bool = ops::is_enabled;
        /// Start continuous acquisition.
        fn run() -> () = ops::run;
        /// Arm for a single acquisition.
        fn single() -> () = ops::single;
        /// Stop acquisition.
        fn stop() -> () = ops::stop;
        /// Force a trigger now.
        fn force_trigger() -> () = ops::force_trigger;
        /// Let the instrument pick scale, timebase and trigger.
        fn autoscale() -> () = ops::autoscale;
        /// Set this channel's vertical scale (V/div).
        fn set_scale(volts_per_div: f64) -> () = ops::set_scale;
        /// This channel's vertical scale (V/div).
        fn scale() -> f64 = ops::scale;
        /// Set the timebase (s/div).
        fn set_timebase(seconds_per_div: f64) -> () = ops::set_timebase;
        /// The timebase (s/div).
        fn timebase() -> f64 = ops::timebase;
        /// Set this channel's input coupling (`"dc"`, `"ac"`; a Rigol also
        /// takes `"gnd"`).
        fn set_coupling(mode: &str) -> () = ops::set_coupling;
        /// This channel's input coupling.
        fn coupling() -> String = ops::coupling;
        /// Set this channel's probe attenuation (1 for 1x, 10 for 10x).
        fn set_probe(ratio: f64) -> () = ops::set_probe;
        /// This channel's probe attenuation.
        fn probe() -> f64 = ops::probe;
        /// Set this channel's vertical offset (V).
        fn set_offset(volts: f64) -> () = ops::set_offset;
        /// This channel's vertical offset (V).
        fn offset() -> f64 = ops::offset;
        /// Move the capture window in time relative to the trigger (s).
        fn set_time_offset(seconds: f64) -> () = ops::set_time_offset;
        /// The capture window's time offset from the trigger (s).
        fn time_offset() -> f64 = ops::time_offset;
        /// Configure the edge trigger, applying only the fields that are
        /// set.
        fn trigger_edge(trigger: &EdgeTrigger) -> () = ops::trigger_edge;
        /// The trigger as the instrument has it.
        fn trigger() -> TriggerSettings = ops::trigger;
        /// Read one quantity from this channel. `None` when the capture does
        /// not contain it (a period on a DC level).
        fn measure(m: Measurement) -> Option<f64> = ops::measure;
        /// Every quantity this channel's capture supports, read from one
        /// capture so the values agree with each other.
        fn measure_all() -> Measurements = ops::measure_all;
        /// Run any other scope action by name with raw JSON params, and
        /// return its `value` (`null` when it has none): `capabilities`,
        /// `get_state`, `set_acquire`, `set_trigger_holdoff`, `set_roll`,
        /// `fft`, the cursor actions, and so on.
        fn command(action: &str, params: serde_json::Value) -> serde_json::Value = ops::command;
    }
}
