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
//! Waveform samples (the stream behind the box's `/scope` page) come from
//! `Scope::capture` and `Scope::stream`, behind the `scope-trace` feature;
//! see [`crate::nets::scope_trace`].
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
use serde_json::{Map, Value};

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

/// What the attached scope supports, from [`Scope::capabilities`], as the
/// scope's daemon detected it when it opened the unit.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct ScopeCapabilities {
    /// Driver family (`"ps2000a"`, `"rigol_mso5000"`, ...).
    #[serde(default)]
    pub family: Option<String>,
    /// Model, e.g. `"2205A"` or `"MSO5074"`.
    #[serde(default)]
    pub model: Option<String>,
    /// Serial number.
    #[serde(default)]
    pub serial: Option<String>,
    /// Number of analog channels.
    #[serde(default)]
    pub analog_channels: Option<u32>,
    /// Channel labels (`"A"`, `"B"`, ...).
    #[serde(default)]
    pub channel_labels: Vec<String>,
    /// Fastest sample rate (Hz).
    #[serde(default)]
    pub max_sample_rate_hz: Option<f64>,
    /// Deepest capture (samples).
    #[serde(default)]
    pub max_memory_samples: Option<u64>,
    /// Analog bandwidth (Hz), when the unit reports it.
    #[serde(default)]
    pub bandwidth_hz: Option<f64>,
    /// Slow timebases can stream continuously ([`RollMode`]).
    #[serde(default)]
    pub roll_mode: bool,
    /// [`AcquisitionMode::Peak`] is available.
    #[serde(default)]
    pub peak_detect: bool,
    /// Hardware trigger types beyond a simple edge.
    #[serde(default)]
    pub advanced_triggers: Vec<String>,
    /// Everything else the daemon reports (voltage ranges, resolution,
    /// signal generator, ...).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Every setting the scope's controls show, from [`Scope::state`], read in
/// one request. Channels are named by letter (`"A"`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct ScopeState {
    /// Increments with every change.
    #[serde(default)]
    pub version: Option<u64>,
    /// Acquisition is running.
    #[serde(default)]
    pub acquiring: bool,
    /// Streaming in roll mode rather than capturing blocks.
    #[serde(default)]
    pub rolling: bool,
    /// Capture mode (`"auto"`, `"normal"`, `"single"`).
    #[serde(default)]
    pub capture_mode: Option<String>,
    /// Trigger settings.
    #[serde(default)]
    pub trigger: Option<ScopeTriggerState>,
    /// Horizontal settings.
    #[serde(default)]
    pub timebase: Option<ScopeTimebase>,
    /// Acquisition mode.
    #[serde(default)]
    pub acquisition: Option<Acquisition>,
    /// Per-channel settings.
    #[serde(default)]
    pub channels: Vec<ScopeChannelState>,
    /// Display settings, in the shape [`Scope::display`] returns.
    #[serde(default)]
    pub display: Value,
    /// Why a scope that takes its live view from a capture card has none.
    #[serde(default)]
    pub live_view_problem: Option<String>,
}

/// The trigger part of a [`ScopeState`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct ScopeTriggerState {
    /// Source channel (`"A"`).
    #[serde(default)]
    pub source: Option<String>,
    /// Level (V at the probe tip).
    #[serde(default)]
    pub level: Option<f64>,
    /// Slope (`"rising"`, `"falling"`, `"either"`).
    #[serde(default)]
    pub slope: Option<String>,
    /// Holdoff (s).
    #[serde(default)]
    pub holdoff_s: Option<f64>,
    /// Where the trigger sits in the capture, 0 to 100 % from its start.
    #[serde(default)]
    pub position_percent: Option<f64>,
}

/// The horizontal part of a [`ScopeState`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct ScopeTimebase {
    /// The timebase in effect (s/div), which is the request rounded to a
    /// sample interval the hardware has.
    #[serde(default)]
    pub time_per_div: Option<f64>,
    /// Capture window offset from the trigger (s).
    #[serde(default)]
    pub time_offset: Option<f64>,
    /// Sample interval (ns).
    #[serde(default)]
    pub sample_interval_ns: Option<f64>,
    /// Samples per capture.
    #[serde(default)]
    pub memory_depth: Option<u64>,
    /// Roll setting (`"auto"`, `"on"`, `"off"`).
    #[serde(default)]
    pub roll: Option<String>,
}

/// One channel of a [`ScopeState`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct ScopeChannelState {
    /// Channel label (`"A"`).
    #[serde(default)]
    pub channel: Option<String>,
    /// Channel is on.
    #[serde(default)]
    pub enabled: bool,
    /// Vertical scale (V/div).
    #[serde(default)]
    pub volts_per_div: Option<f64>,
    /// Vertical offset (V).
    #[serde(default)]
    pub volts_offset: Option<f64>,
    /// Input coupling (`"DC"`, `"AC"`, `"GND"`).
    #[serde(default)]
    pub coupling: Option<String>,
    /// Probe attenuation.
    #[serde(default)]
    pub attenuation: Option<f64>,
}

/// How consecutive captures combine before one is published, for
/// [`Scope::set_acquisition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AcquisitionMode {
    /// Each capture as taken.
    Normal,
    /// A running mean of the last `count` captures (1 to 1024), which
    /// takes random noise down by the square root of the count.
    Average,
    /// The minimum and maximum of every sample interval, so a glitch
    /// narrower than the interval still shows. Refused on a unit without
    /// hardware aggregation ([`ScopeCapabilities::peak_detect`]).
    Peak,
}

impl AcquisitionMode {
    fn as_str(self) -> &'static str {
        match self {
            AcquisitionMode::Normal => "normal",
            AcquisitionMode::Average => "average",
            AcquisitionMode::Peak => "peak",
        }
    }
}

/// The acquisition setting, from [`Scope::acquisition`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct Acquisition {
    /// `"normal"`, `"average"` or `"peak"`.
    #[serde(default)]
    pub mode: Option<String>,
    /// Captures averaged in `"average"` mode.
    #[serde(default)]
    pub average_count: Option<u32>,
}

/// Whether slow timebases stream continuously, for [`Scope::set_roll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollMode {
    /// Roll at 50 ms/div and slower while the trigger is in auto, as a
    /// bench scope does.
    Auto,
    /// Roll at any timebase the scope can stream at, ignoring the trigger.
    On,
    /// Never roll.
    Off,
}

impl RollMode {
    fn as_str(self) -> &'static str {
        match self {
            RollMode::Auto => "auto",
            RollMode::On => "on",
            RollMode::Off => "off",
        }
    }
}

/// The roll setting, from [`Scope::roll`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct Roll {
    /// `"auto"`, `"on"` or `"off"`.
    #[serde(default)]
    pub roll: Option<String>,
    /// Rolling right now.
    #[serde(default)]
    pub rolling: bool,
}

/// FFT window. `Hann` for general use, `Flattop` when a tone's amplitude
/// matters, `Rectangular` for a signal that fits the capture exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FftWindow {
    /// Hann.
    Hann,
    /// Hamming.
    Hamming,
    /// Blackman.
    Blackman,
    /// Flat top.
    Flattop,
    /// No window.
    Rectangular,
}

impl FftWindow {
    fn as_str(self) -> &'static str {
        match self {
            FftWindow::Hann => "hann",
            FftWindow::Hamming => "hamming",
            FftWindow::Blackman => "blackman",
            FftWindow::Flattop => "flattop",
            FftWindow::Rectangular => "rectangular",
        }
    }
}

/// Trace persistence, for [`DisplaySettings`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Persistence {
    /// Traces do not linger.
    Off,
    /// Seconds a trace lingers as it fades, up to 60.
    Seconds(f64),
    /// Traces never fade.
    Infinite,
}

/// Zoom, for [`DisplaySettings`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zoom {
    /// No zoom.
    Off,
    /// Magnify by `factor` (1 to 1000) around `center` seconds from the
    /// trigger.
    Around {
        /// Magnification.
        factor: f64,
        /// Center (s from the trigger).
        center: f64,
    },
}

/// The math trace, for [`DisplaySettings`].
#[derive(Debug, Clone, PartialEq)]
pub enum MathTrace {
    /// No math trace.
    Off,
    /// Two channels and an operator: `"a-b"`, `"a+b"` or `"a*b"`.
    Expr(String),
}

/// The spectrum pane, for [`DisplaySettings`].
#[derive(Debug, Clone, PartialEq)]
pub enum FftPane {
    /// No spectrum pane.
    Off,
    /// Show the spectrum of `channel` (this net's when `None`).
    On {
        /// Channel label (`"A"`).
        channel: Option<String>,
        /// Window; Hann when `None`.
        window: Option<FftWindow>,
    },
}

/// How the box's web UI draws the scope, for [`Scope::set_display`]. Only
/// the fields that are set change, and at least one must be.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DisplaySettings {
    /// Trace persistence.
    pub persistence: Option<Persistence>,
    /// Plot channel B against channel A instead of both against time.
    pub xy: Option<bool>,
    /// Zoom.
    pub zoom: Option<Zoom>,
    /// Math trace.
    pub math: Option<MathTrace>,
    /// Spectrum pane.
    pub fft: Option<FftPane>,
}

/// Options for [`Scope::fft`]. All optional.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FftOptions {
    /// Channel label (`"A"`). Required on the instrument's `scope` net;
    /// a channel net uses its own channel when `None`.
    pub channel: Option<String>,
    /// Window; Hann when `None`.
    pub window: Option<FftWindow>,
    /// How many peaks to report; 5 when `None`.
    pub peaks: Option<u32>,
}

/// A channel's spectrum, from [`Scope::fft`], computed on the box from the
/// capture on screen. Amplitudes are RMS volts at the probe tip.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct Spectrum {
    /// Channel label.
    #[serde(default)]
    pub channel: Option<String>,
    /// Window used.
    #[serde(default)]
    pub window: Option<String>,
    /// Sample rate of the capture (Hz).
    #[serde(default)]
    pub sample_rate_hz: Option<f64>,
    /// Bin width (Hz).
    #[serde(default)]
    pub resolution_hz: Option<f64>,
    /// Samples transformed.
    #[serde(default)]
    pub samples: Option<u64>,
    /// Strongest components, strongest first.
    #[serde(default)]
    pub peaks: Vec<SpectralPeak>,
}

/// One component of a [`Spectrum`].
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct SpectralPeak {
    /// Frequency (Hz), interpolated between bins.
    pub frequency_hz: f64,
    /// Amplitude (V RMS).
    pub vrms: f64,
    /// Amplitude (dB relative to 1 V RMS). `None` for a zero amplitude.
    #[serde(default)]
    pub dbv: Option<f64>,
}

/// Cursors to place with [`Scope::set_cursors`]. Each pair is independent
/// and only the ones given move; at least one must be set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CursorSettings {
    /// Time cursors (s from the trigger).
    pub time: Option<(f64, f64)>,
    /// Voltage cursors (V at the probe tip).
    pub volts: Option<(f64, f64)>,
    /// Channel whose trace the time cursors read (`"A"`); this net's when
    /// `None`.
    pub channel: Option<String>,
}

/// Where the cursors are, from [`Scope::cursors`]. Cursors belong to the
/// instrument, so every net on it sees the same set.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct Cursors {
    /// Time cursors (s from the trigger).
    #[serde(default)]
    pub time: Option<[f64; 2]>,
    /// Voltage cursors (V).
    #[serde(default)]
    pub volts: Option<[f64; 2]>,
    /// Channel the cursors were placed against.
    #[serde(default)]
    pub channel: Option<String>,
}

/// The cursors and what they read, from [`Scope::measure_cursors`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct CursorMeasurement {
    /// Cursor positions.
    pub cursors: Cursors,
    /// Readings. Empty when no cursors are set.
    #[serde(default)]
    pub readings: CursorReadings,
}

/// What a set of cursors reads. A quantity that does not apply (a trace
/// voltage at a time outside the capture) is `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct CursorReadings {
    /// First time cursor (s).
    pub t1: Option<f64>,
    /// Second time cursor (s).
    pub t2: Option<f64>,
    /// `t2 - t1` (s).
    pub delta_t: Option<f64>,
    /// `1 / delta_t` (Hz).
    pub frequency: Option<f64>,
    /// Start of the captured window (s).
    pub window_start: Option<f64>,
    /// End of the captured window (s).
    pub window_end: Option<f64>,
    /// Trace voltage under the first time cursor (V).
    pub trace_v1: Option<f64>,
    /// Trace voltage under the second time cursor (V).
    pub trace_v2: Option<f64>,
    /// `trace_v2 - trace_v1` (V).
    pub trace_delta_v: Option<f64>,
    /// First voltage cursor (V).
    pub v1: Option<f64>,
    /// Second voltage cursor (V).
    pub v2: Option<f64>,
    /// `v2 - v1` (V).
    pub delta_v: Option<f64>,
}

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::{json, Value};

    use super::{
        Acquisition, AcquisitionMode, CursorMeasurement, CursorSettings, Cursors, DisplaySettings,
        EdgeTrigger, FftOptions, FftPane, MathTrace, Measurement, Measurements, Persistence, Roll,
        RollMode, ScopeCapabilities, ScopeState, Spectrum, TriggerSettings, Zoom,
    };
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

    pub(crate) fn capture_mode(name: &str) -> Op<String> {
        cmd(name, "get_capture_mode", json!({}), parse_string)
    }

    pub(crate) fn trigger_source(name: &str) -> Op<String> {
        cmd(name, "get_trigger_source", json!({}), parse_string)
    }

    pub(crate) fn trigger_slope(name: &str) -> Op<String> {
        cmd(name, "get_trigger_slope", json!({}), parse_string)
    }

    pub(crate) fn trigger_level(name: &str) -> Op<f64> {
        cmd(name, "get_trigger_level", json!({}), value_f64)
    }

    pub(crate) fn set_trigger_holdoff(name: &str, seconds: f64) -> Op<()> {
        cmd(
            name,
            "set_trigger_holdoff",
            json!({ "seconds": seconds }),
            unit,
        )
    }

    pub(crate) fn trigger_holdoff(name: &str) -> Op<f64> {
        cmd(name, "get_trigger_holdoff", json!({}), value_f64)
    }

    pub(crate) fn capabilities(name: &str) -> Op<ScopeCapabilities> {
        cmd(
            name,
            "capabilities",
            json!({}),
            value_as::<ScopeCapabilities>,
        )
    }

    pub(crate) fn state(name: &str) -> Op<ScopeState> {
        cmd(name, "get_state", json!({}), value_as::<ScopeState>)
    }

    pub(crate) fn set_acquisition(
        name: &str,
        mode: AcquisitionMode,
        count: Option<u32>,
    ) -> Op<Acquisition> {
        let mut params = json!({ "mode": mode.as_str() });
        if let Some(count) = count {
            params["count"] = json!(count);
        }
        cmd(name, "set_acquire", params, value_as::<Acquisition>)
    }

    pub(crate) fn acquisition(name: &str) -> Op<Acquisition> {
        cmd(name, "get_acquire", json!({}), value_as::<Acquisition>)
    }

    pub(crate) fn set_roll(name: &str, mode: RollMode) -> Op<Roll> {
        cmd(
            name,
            "set_roll",
            json!({ "mode": mode.as_str() }),
            value_as::<Roll>,
        )
    }

    pub(crate) fn roll(name: &str) -> Op<Roll> {
        cmd(name, "get_roll", json!({}), value_as::<Roll>)
    }

    /// The box clears a display setting on `"off"`.
    fn display_params(settings: &DisplaySettings) -> Value {
        let mut params = json!({});
        if let Some(p) = settings.persistence {
            params["persistence"] = match p {
                Persistence::Off => json!("off"),
                Persistence::Seconds(s) => json!(s),
                Persistence::Infinite => json!("infinite"),
            };
        }
        if let Some(xy) = settings.xy {
            params["xy"] = json!(xy);
        }
        if let Some(zoom) = settings.zoom {
            params["zoom"] = match zoom {
                Zoom::Off => json!("off"),
                Zoom::Around { factor, center } => json!({ "factor": factor, "center": center }),
            };
        }
        if let Some(math) = &settings.math {
            params["math"] = match math {
                MathTrace::Off => json!("off"),
                MathTrace::Expr(expr) => json!(expr),
            };
        }
        if let Some(fft) = &settings.fft {
            params["fft"] = match fft {
                FftPane::Off => json!("off"),
                FftPane::On { channel, window } => {
                    let mut pane = json!({});
                    if let Some(channel) = channel {
                        pane["channel"] = json!(channel);
                    }
                    if let Some(window) = window {
                        pane["window"] = json!(window.as_str());
                    }
                    pane
                }
            };
        }
        params
    }

    pub(crate) fn set_display(name: &str, settings: &DisplaySettings) -> Op<Value> {
        cmd(name, "set_display", display_params(settings), parse_value)
    }

    pub(crate) fn display(name: &str) -> Op<Value> {
        cmd(name, "get_display", json!({}), parse_value)
    }

    pub(crate) fn fft(name: &str, opts: &FftOptions) -> Op<Spectrum> {
        let mut params = json!({});
        if let Some(channel) = &opts.channel {
            params["channel"] = json!(channel);
        }
        if let Some(window) = opts.window {
            params["window"] = json!(window.as_str());
        }
        if let Some(peaks) = opts.peaks {
            params["peaks"] = json!(peaks);
        }
        cmd(name, "fft", params, value_as::<Spectrum>)
    }

    pub(crate) fn set_cursors(name: &str, settings: &CursorSettings) -> Op<Cursors> {
        let mut params = json!({});
        if let Some((a, b)) = settings.time {
            params["time"] = json!([a, b]);
        }
        if let Some((a, b)) = settings.volts {
            params["volts"] = json!([a, b]);
        }
        if let Some(channel) = &settings.channel {
            params["channel"] = json!(channel);
        }
        cmd(name, "set_cursor", params, value_as::<Cursors>)
    }

    pub(crate) fn cursors(name: &str) -> Op<Cursors> {
        cmd(name, "get_cursor", json!({}), value_as::<Cursors>)
    }

    pub(crate) fn clear_cursors(name: &str) -> Op<()> {
        cmd(name, "clear_cursor", json!({}), unit)
    }

    pub(crate) fn measure_cursors(name: &str) -> Op<CursorMeasurement> {
        cmd(
            name,
            "measure_cursor",
            json!({}),
            value_as::<CursorMeasurement>,
        )
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
        /// The capture mode (`"auto"`, `"normal"`, `"single"`). Run, single
        /// and a completed single-shot each change it.
        fn capture_mode() -> String = ops::capture_mode;
        /// The trigger source, as the instrument names it.
        fn trigger_source() -> String = ops::trigger_source;
        /// The trigger slope, as the instrument names it.
        fn trigger_slope() -> String = ops::trigger_slope;
        /// The trigger level (V), as the instrument accepted it: a Rigol
        /// clamps the level to its screen.
        fn trigger_level() -> f64 = ops::trigger_level;
        /// Ignore triggers for this long after each one, 0 to 10 s. Gets a
        /// stable trace from a burst or a UART byte, whose edges all cross
        /// the level.
        fn set_trigger_holdoff(seconds: f64) -> () = ops::set_trigger_holdoff;
        /// The trigger holdoff (s).
        fn trigger_holdoff() -> f64 = ops::trigger_holdoff;
        /// What the attached scope supports.
        fn capabilities() -> ScopeCapabilities = ops::capabilities;
        /// Every setting the controls show, read in one request.
        fn state() -> ScopeState = ops::state;
        /// Set how consecutive captures combine. `count` is the number
        /// averaged in [`AcquisitionMode::Average`] (16 until set). Returns
        /// the setting now in effect.
        fn set_acquisition(mode: AcquisitionMode, count: Option<u32>) -> Acquisition = ops::set_acquisition;
        /// The acquisition setting.
        fn acquisition() -> Acquisition = ops::acquisition;
        /// Set whether slow timebases stream continuously. Returns the
        /// setting now in effect.
        fn set_roll(mode: RollMode) -> Roll = ops::set_roll;
        /// The roll setting, and whether the scope is rolling now.
        fn roll() -> Roll = ops::roll;
        /// Change how the box's web UI draws the scope; an open page
        /// follows. Returns the display settings now in effect, in the
        /// box's own shape.
        fn set_display(settings: &DisplaySettings) -> serde_json::Value = ops::set_display;
        /// The display settings, in the box's own shape.
        fn display() -> serde_json::Value = ops::display;
        /// The spectrum of one channel and its strongest components,
        /// computed on the box from the capture on screen.
        fn fft(opts: &FftOptions) -> Spectrum = ops::fft;
        /// Place time cursors, voltage cursors or both. Cursors are held on
        /// the box, so the web UI draws them. Returns where they are now.
        fn set_cursors(settings: &CursorSettings) -> Cursors = ops::set_cursors;
        /// Where the cursors are, without taking a capture.
        fn cursors() -> Cursors = ops::cursors;
        /// Remove every cursor.
        fn clear_cursors() -> () = ops::clear_cursors;
        /// The cursors and what they read. Time cursors take a capture to
        /// read the trace under them; voltage cursors need none.
        fn measure_cursors() -> CursorMeasurement = ops::measure_cursors;
        /// Run any other scope action by name with raw JSON params, and
        /// return its `value` (`null` when it has none).
        fn command(action: &str, params: serde_json::Value) -> serde_json::Value = ops::command;
    }
}
