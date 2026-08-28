//! USB hub port nets (Acroname, YKUSH): power-cycle a DUT's USB port.

use super::net_handle;

pub(crate) mod ops {
    use serde_json::Value;

    use crate::error::{Error, Result};
    use crate::wire::{unit, usb_command, CommandResponse, Op};

    /// `/usb/command` reports the resulting port state as a top-level
    /// `"state": "enabled" | "disabled"` string.
    fn parse_port_state(resp: CommandResponse) -> Result<bool> {
        match resp.state.as_ref() {
            Some(Value::String(s)) => Ok(s == "enabled"),
            other => Err(Error::Decode(format!(
                "expected \"enabled\"/\"disabled\" state, got {other:?}"
            ))),
        }
    }

    /// Box images before 0.29.0 reject the `state` action with a 400 whose
    /// message enumerates only `enable|disable|toggle`. Surface that as
    /// [`Error::UnsupportedByBox`] instead of a generic box error, so
    /// callers can tell "old box" apart from "bad request".
    pub(crate) fn state_compat(err: Error) -> Error {
        match err {
            Error::Box {
                status: 400,
                ref message,
            } if message.contains("(enable|disable|toggle)")
                && !message.contains("state") =>
            {
                Error::UnsupportedByBox {
                    message: "the 'state' action on /usb/command requires box software \
                              >= 0.29.0; update the box or use toggle/enable/disable"
                        .to_string(),
                }
            }
            other => other,
        }
    }

    /// Box images before 0.39.0 reject `cycle`/`recover` with a 400 whose
    /// action list stops at `state`. Same reclassification as
    /// [`state_compat`], naming the newer required version.
    pub(crate) fn cycle_compat(err: Error) -> Error {
        match err {
            Error::Box {
                status: 400,
                ref message,
            } if message.contains("enable|disable|toggle") && !message.contains("cycle") => {
                Error::UnsupportedByBox {
                    message: "the 'cycle' and 'recover' actions on /usb/command require \
                              box software >= 0.39.0; update the box or sequence \
                              disable/enable yourself"
                        .to_string(),
                }
            }
            other => other,
        }
    }

    pub(crate) fn enable(name: &str) -> Op<()> {
        Op {
            req: usb_command(name, "enable"),
            parse: unit,
        }
    }

    pub(crate) fn disable(name: &str) -> Op<()> {
        Op {
            req: usb_command(name, "disable"),
            parse: unit,
        }
    }

    pub(crate) fn toggle(name: &str) -> Op<bool> {
        Op {
            req: usb_command(name, "toggle"),
            parse: parse_port_state,
        }
    }

    pub(crate) fn state(name: &str) -> Op<bool> {
        Op {
            req: usb_command(name, "state"),
            parse: parse_port_state,
        }
    }

    /// `cycle` responses carry `reconnected: true|false` when the box could
    /// watch for the device to come back, and omit the key when the port was
    /// empty or the driver cannot observe re-enumeration — three honestly
    /// different outcomes, surfaced as `Some(true)`/`Some(false)`/`None`.
    fn parse_reconnected(resp: CommandResponse) -> Result<Option<bool>> {
        match resp.extra.get("reconnected") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            other => Err(Error::Decode(format!(
                "expected boolean 'reconnected', got {other:?}"
            ))),
        }
    }

    pub(crate) fn cycle(name: &str, off_time: Option<f64>) -> Op<Option<bool>> {
        Op {
            req: crate::wire::usb_cycle(name, off_time),
            parse: parse_reconnected,
        }
    }

    pub(crate) fn recover(name: &str) -> Op<()> {
        Op {
            req: usb_command(name, "recover"),
            parse: unit,
        }
    }
}

net_handle! {
    /// Handle for a USB hub port net.
    sync: UsbPort,
    async: AsyncUsbPort,
    methods: {
        /// Power the port on.
        fn enable() -> () = ops::enable;
        /// Power the port off.
        fn disable() -> () = ops::disable;
        /// Toggle the port; returns `true` if it is now enabled.
        fn toggle() -> bool = ops::toggle;
    }
}

// `state` lives outside the macro so the pre-0.29.0 box rejection can be
// mapped to `Error::UnsupportedByBox` (the macro's thin wrappers have no
// error post-processing hook).

#[cfg(feature = "blocking")]
impl UsbPort<'_> {
    /// Read whether the port is currently enabled.
    ///
    /// Requires box software >= 0.29.0; older boxes fail with
    /// [`crate::Error::UnsupportedByBox`].
    pub fn state(&self) -> crate::error::Result<bool> {
        self.client.run(ops::state(&self.name)).map_err(ops::state_compat)
    }

    /// Power-cycle the port — off, wait, on — with the box's default off
    /// window (1s, chosen above the slowest cold boot measured on real
    /// hardware so the DUT's rails fully discharge).
    ///
    /// Returns whether the device re-enumerated: `Some(true)` it came back,
    /// `Some(false)` it did not before the box's timeout, `None` the port
    /// was empty or the hub cannot observe re-enumeration. The port is
    /// powered at the end on every path, including failures.
    ///
    /// Do **not** script around a cycle by checking for the device's
    /// *absence* while the port is off: an unpowered port raises no change
    /// bit, so the device stays in lsusb (and keeps its /dev nodes) until
    /// power returns. This return value is the box watching for the
    /// re-enumeration, which is the observable that actually exists.
    ///
    /// Requires box software >= 0.39.0; older boxes fail with
    /// [`crate::Error::UnsupportedByBox`].
    pub fn cycle(&self) -> crate::error::Result<Option<bool>> {
        self.client
            .run(ops::cycle(&self.name, None))
            .map_err(ops::cycle_compat)
    }

    /// Like [`UsbPort::cycle`] with an explicit unpowered window in seconds
    /// (box-validated range 0.5-10). Prefer *longer* when in doubt: too
    /// short an off time is the failure that matters — the DUT's rails do
    /// not fully discharge and it warm-starts while appearing to have been
    /// reset.
    pub fn cycle_with_off_time(&self, off_time_secs: f64) -> crate::error::Result<Option<bool>> {
        self.client
            .run(ops::cycle(&self.name, Some(off_time_secs)))
            .map_err(ops::cycle_compat)
    }

    /// Re-power a port left dark by an interrupted command. On a Plugable
    /// dock this re-asserts power on every port of that dock — the
    /// situation it exists for is "something died partway through and it is
    /// not obvious what is off".
    ///
    /// Requires box software >= 0.39.0; older boxes fail with
    /// [`crate::Error::UnsupportedByBox`].
    pub fn recover(&self) -> crate::error::Result<()> {
        self.client
            .run(ops::recover(&self.name))
            .map_err(ops::cycle_compat)
    }
}

#[cfg(feature = "async")]
impl AsyncUsbPort<'_> {
    /// Read whether the port is currently enabled.
    ///
    /// Requires box software >= 0.29.0; older boxes fail with
    /// [`crate::Error::UnsupportedByBox`].
    pub async fn state(&self) -> crate::error::Result<bool> {
        self.client
            .run(ops::state(&self.name))
            .await
            .map_err(ops::state_compat)
    }

    /// Power-cycle the port — off, wait, on — with the box's default off
    /// window. See [`UsbPort::cycle`] for the return value's three outcomes
    /// and the "do not poll for absence" caveat.
    ///
    /// Requires box software >= 0.39.0; older boxes fail with
    /// [`crate::Error::UnsupportedByBox`].
    pub async fn cycle(&self) -> crate::error::Result<Option<bool>> {
        self.client
            .run(ops::cycle(&self.name, None))
            .await
            .map_err(ops::cycle_compat)
    }

    /// Like [`AsyncUsbPort::cycle`] with an explicit unpowered window in
    /// seconds (box-validated range 0.5-10).
    pub async fn cycle_with_off_time(
        &self,
        off_time_secs: f64,
    ) -> crate::error::Result<Option<bool>> {
        self.client
            .run(ops::cycle(&self.name, Some(off_time_secs)))
            .await
            .map_err(ops::cycle_compat)
    }

    /// Re-power a port left dark by an interrupted command. See
    /// [`UsbPort::recover`].
    ///
    /// Requires box software >= 0.39.0; older boxes fail with
    /// [`crate::Error::UnsupportedByBox`].
    pub async fn recover(&self) -> crate::error::Result<()> {
        self.client
            .run(ops::recover(&self.name))
            .await
            .map_err(ops::cycle_compat)
    }
}
