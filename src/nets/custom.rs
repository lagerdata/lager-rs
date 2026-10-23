//! Custom-driver nets: instruments driven by a driver installed on the box
//! (box >= 0.59.0), rather than one built into it.
//!
//! A custom driver declares its own role and actions in its manifest, so
//! this handle has one generic method. `lager.status()?.capabilities` lists
//! the installed roles in `net_command_roles` when `custom_drivers` is set.
//!
//! ```no_run
//! # #[cfg(feature = "blocking")]
//! # fn demo() -> lager::Result<()> {
//! use lager::LagerBox;
//! use serde_json::json;
//!
//! let lager = LagerBox::from_env()?;
//! let reading = lager.custom("chamber1").command("read_temperature", json!({}))?;
//! println!("{reading}");
//! # Ok(())
//! # }
//! # fn main() {}
//! ```
//!
//! Driver failures arrive as [`crate::Error::Box`] with the box's status:
//! 409 (the device is busy), 503 (unavailable), 504 (timed out) or 502
//! (the driver failed).

use super::net_handle;

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::Value;

    use crate::error::Result;
    use crate::wire::{net_command, CommandResponse, Op, Timeout};

    fn parse_value(resp: CommandResponse) -> Result<Value> {
        Ok(resp.value.unwrap_or(Value::Null))
    }

    /// The box resolves the role from the saved net, so none is sent.
    pub(crate) fn command(name: &str, action: &str, params: Value) -> Op<Value> {
        Op {
            // A driver's actions can be slow; the box enforces the driver's
            // own timeout and answers 504 past it.
            req: net_command(
                name,
                None,
                action,
                params,
                Timeout::After(Duration::from_secs(120)),
            ),
            parse: parse_value,
        }
    }
}

net_handle! {
    /// Handle for a net driven by a custom driver installed on the box.
    sync: CustomNet,
    async: AsyncCustomNet,
    methods: {
        /// Run one of the driver's actions with JSON `params` (use `json!({})`
        /// for none). Returns the action's `value`, or `null` when it has
        /// none.
        fn command(action: &str, params: serde_json::Value) -> serde_json::Value = ops::command;
    }
}
