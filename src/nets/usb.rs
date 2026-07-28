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
}
