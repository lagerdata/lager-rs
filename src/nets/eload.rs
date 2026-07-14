//! Electronic-load nets (Rigol DL3000 family).

use super::net_handle;
pub use crate::wire::EloadState;

/// E-load operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EloadMode {
    /// Constant current (A).
    Cc,
    /// Constant voltage (V).
    Cv,
    /// Constant resistance (ohm).
    Cr,
    /// Constant power (W).
    Cp,
}

impl EloadMode {
    /// Wire encoding (the `/net/command` action name).
    pub fn as_str(&self) -> &'static str {
        match self {
            EloadMode::Cc => "cc",
            EloadMode::Cv => "cv",
            EloadMode::Cr => "cr",
            EloadMode::Cp => "cp",
        }
    }
}

pub(crate) mod ops {
    use serde_json::json;

    use super::EloadMode;
    use crate::wire::{net_command, unit, value_as, value_f64, EloadState, Op, Timeout};

    const ROLE: &str = "eload";

    pub(crate) fn state(name: &str) -> Op<EloadState> {
        Op {
            req: net_command(name, ROLE, "state", json!({}), Timeout::Default),
            parse: value_as::<EloadState>,
        }
    }

    pub(crate) fn set(name: &str, mode: EloadMode, value: f64) -> Op<()> {
        // Mode + setpoint is one composite /invoke box-side, so it applies
        // atomically under the instrument lock.
        Op {
            req: net_command(
                name,
                ROLE,
                mode.as_str(),
                json!({ "value": value }),
                Timeout::Default,
            ),
            parse: unit,
        }
    }

    pub(crate) fn setpoint(name: &str, mode: EloadMode) -> Op<f64> {
        Op {
            req: net_command(name, ROLE, mode.as_str(), json!({}), Timeout::Default),
            parse: value_f64,
        }
    }
}

net_handle! {
    /// Handle for an electronic-load net.
    sync: Eload,
    async: AsyncEload,
    methods: {
        /// Read the full load state (mode, input enable, measurements).
        fn state() -> EloadState = ops::state;
        /// Switch to `mode` and apply `value` as its setpoint, atomically.
        fn set(mode: EloadMode, value: f64) -> () = ops::set;
        /// Read the stored setpoint for `mode`.
        fn setpoint(mode: EloadMode) -> f64 = ops::setpoint;
    }
}
