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
        /// Read whether the port is currently enabled.
        fn state() -> bool = ops::state;
    }
}
