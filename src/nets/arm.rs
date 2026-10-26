//! Robot-arm nets (Rotrics Dexarm): positioning for physical-interaction
//! tests (pressing buttons, flipping switches).
//!
//! Moves block on the box until the arm reaches the target; the HTTP budget
//! is widened past the box-side move timeout so a healthy slow move is never
//! aborted client-side. The box enforces the Dexarm workspace bounds and
//! rejects out-of-range targets.

use serde::Deserialize;

use super::net_handle;
pub use crate::wire::ArmPosition;

/// The arm's acceleration settings (mm/s², G-code M204), as the arm reports
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct ArmAcceleration {
    /// Print acceleration (M204 P).
    pub acceleration: f64,
    /// Travel acceleration (M204 T).
    pub travel_acceleration: f64,
    /// Retract acceleration (M204 R).
    pub retract_acceleration: f64,
}

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::json;

    use super::ArmAcceleration;
    use crate::error::{Error, Result};
    use crate::wire::{
        net_command, unit, value_arm_position, value_as, ArmPosition, CommandResponse, Op, Timeout,
    };

    const ROLE: &str = "arm";

    /// Box-side move timeout (s), matching the CLI default.
    const MOVE_TIMEOUT_S: f64 = 15.0;

    /// Budget for a move: the box waits up to the move timeout plus its own
    /// proxy margin, so give the HTTP layer that plus headroom.
    fn move_budget(timeout_s: f64) -> Timeout {
        Timeout::After(Duration::from_secs_f64(timeout_s + 30.0))
    }

    /// Non-move actions still touch the serial port (and may queue behind a
    /// move on the arm's device lock), so they get a widened budget too.
    fn quick_budget() -> Timeout {
        Timeout::After(Duration::from_secs(45))
    }

    pub(crate) fn position(name: &str) -> Op<ArmPosition> {
        Op {
            req: net_command(name, ROLE, "position", json!({}), quick_budget()),
            parse: value_arm_position,
        }
    }

    pub(crate) fn move_to_with_timeout(
        name: &str,
        x: f64,
        y: f64,
        z: f64,
        timeout_s: f64,
    ) -> Op<ArmPosition> {
        Op {
            req: net_command(
                name,
                ROLE,
                "move",
                json!({ "x": x, "y": y, "z": z, "timeout": timeout_s }),
                move_budget(timeout_s),
            ),
            parse: value_arm_position,
        }
    }

    pub(crate) fn move_to(name: &str, x: f64, y: f64, z: f64) -> Op<ArmPosition> {
        move_to_with_timeout(name, x, y, z, MOVE_TIMEOUT_S)
    }

    pub(crate) fn move_by_with_timeout(
        name: &str,
        dx: f64,
        dy: f64,
        dz: f64,
        timeout_s: f64,
    ) -> Op<ArmPosition> {
        Op {
            req: net_command(
                name,
                ROLE,
                "move_by",
                json!({ "dx": dx, "dy": dy, "dz": dz, "timeout": timeout_s }),
                move_budget(timeout_s),
            ),
            parse: value_arm_position,
        }
    }

    pub(crate) fn move_by(name: &str, dx: f64, dy: f64, dz: f64) -> Op<ArmPosition> {
        move_by_with_timeout(name, dx, dy, dz, MOVE_TIMEOUT_S)
    }

    pub(crate) fn go_home(name: &str) -> Op<()> {
        Op {
            req: net_command(name, ROLE, "go_home", json!({}), quick_budget()),
            parse: unit,
        }
    }

    pub(crate) fn enable_motor(name: &str) -> Op<()> {
        Op {
            req: net_command(name, ROLE, "enable_motor", json!({}), quick_budget()),
            parse: unit,
        }
    }

    pub(crate) fn disable_motor(name: &str) -> Op<()> {
        Op {
            req: net_command(name, ROLE, "disable_motor", json!({}), quick_budget()),
            parse: unit,
        }
    }

    pub(crate) fn read_and_save_position(name: &str) -> Op<ArmPosition> {
        Op {
            req: net_command(
                name,
                ROLE,
                "read_and_save_position",
                json!({}),
                quick_budget(),
            ),
            parse: value_arm_position,
        }
    }

    pub(crate) fn get_acceleration(name: &str) -> Op<ArmAcceleration> {
        Op {
            req: net_command(name, ROLE, "get_acceleration", json!({}), quick_budget()),
            parse: value_as::<ArmAcceleration>,
        }
    }

    /// A box >= 0.60.0 answers with the arm's readback; older boxes
    /// send no value.
    fn parse_readback(resp: CommandResponse) -> Result<Option<ArmAcceleration>> {
        match resp.value {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(v) => serde_json::from_value(v)
                .map(Some)
                .map_err(|e| Error::Decode(format!("invalid acceleration readback: {e}"))),
        }
    }

    pub(crate) fn set_acceleration(
        name: &str,
        acceleration: u32,
        travel_acceleration: u32,
        retract_acceleration: Option<u32>,
    ) -> Op<Option<ArmAcceleration>> {
        let mut params = json!({
            "acceleration": acceleration,
            "travel_acceleration": travel_acceleration,
        });
        if let Some(r) = retract_acceleration {
            params["retract_acceleration"] = json!(r);
        }
        Op {
            req: net_command(name, ROLE, "set_acceleration", params, quick_budget()),
            parse: parse_readback,
        }
    }

    /// The arm actions box 0.60.0 added: `get_acceleration`,
    /// and `set_acceleration` without a retract value.
    pub(crate) fn acceleration_compat(err: Error) -> Error {
        let err = crate::wire::unknown_action_compat(err, "get_acceleration", ">= 0.60.0");
        match err {
            Error::Box {
                status: 400,
                ref message,
            } if message.contains("Missing required value")
                && message.contains("retract_acceleration") =>
            {
                Error::UnsupportedByBox {
                    message: "set_acceleration without a retract value requires box \
                              software >= 0.60.0; pass all three values"
                        .to_string(),
                }
            }
            other => other,
        }
    }
}

net_handle! {
    /// Handle for a robot-arm net.
    ///
    /// Coordinates are millimeters in the arm's workspace. Moves block until
    /// the arm reaches the target (box-side timeout 15s).
    sync: Arm,
    async: AsyncArm,
    methods: {
        /// Read the current end-effector position.
        fn position() -> ArmPosition = ops::position;
        /// Move to an absolute position; returns the position after the move.
        fn move_to(x: f64, y: f64, z: f64) -> ArmPosition = ops::move_to;
        /// [`Arm::move_to`] with an explicit box-side move timeout in seconds
        /// (default 15 s), for slow or long moves. The HTTP budget widens
        /// automatically to stay above it.
        fn move_to_with_timeout(x: f64, y: f64, z: f64, timeout_s: f64) -> ArmPosition = ops::move_to_with_timeout;
        /// Move relative to the current position; returns the new position.
        fn move_by(dx: f64, dy: f64, dz: f64) -> ArmPosition = ops::move_by;
        /// [`Arm::move_by`] with an explicit box-side move timeout in seconds
        /// (default 15 s).
        fn move_by_with_timeout(dx: f64, dy: f64, dz: f64, timeout_s: f64) -> ArmPosition = ops::move_by_with_timeout;
        /// Move the arm to its home position (X0 Y300 Z0).
        fn go_home() -> () = ops::go_home;
        /// Energize the arm motors.
        fn enable_motor() -> () = ops::enable_motor;
        /// Release the arm motors (the arm can then be moved by hand).
        fn disable_motor() -> () = ops::disable_motor;
        /// Read the current position and persist it on the arm; returns it.
        fn read_and_save_position() -> ArmPosition = ops::read_and_save_position;
    }
}

// The acceleration methods live outside the macro so an older box's refusal
// can be mapped to `Error::UnsupportedByBox`.

#[cfg(feature = "blocking")]
impl Arm<'_> {
    /// Read the arm's print, travel and retract acceleration (mm/s², from
    /// its settings report).
    ///
    /// Requires box software >= 0.60.0; older boxes fail with
    /// [`crate::Error::UnsupportedByBox`].
    pub fn get_acceleration(&self) -> crate::error::Result<ArmAcceleration> {
        self.client
            .run(ops::get_acceleration(&self.name))
            .map_err(ops::acceleration_compat)
    }

    /// Set print/travel/retract acceleration (mm/s², G-code M204).
    ///
    /// Returns what the arm reports after the change, so a value it did not
    /// take shows up here; `None` from a box at 0.59.0 or older, which sends
    /// no readback.
    pub fn set_acceleration(
        &self,
        acceleration: u32,
        travel_acceleration: u32,
        retract_acceleration: u32,
    ) -> crate::error::Result<Option<ArmAcceleration>> {
        self.client.run(ops::set_acceleration(
            &self.name,
            acceleration,
            travel_acceleration,
            Some(retract_acceleration),
        ))
    }

    /// Set print and travel acceleration, leaving retract as it is.
    ///
    /// Requires box software >= 0.60.0; older boxes fail with
    /// [`crate::Error::UnsupportedByBox`].
    pub fn set_acceleration_keep_retract(
        &self,
        acceleration: u32,
        travel_acceleration: u32,
    ) -> crate::error::Result<Option<ArmAcceleration>> {
        self.client
            .run(ops::set_acceleration(
                &self.name,
                acceleration,
                travel_acceleration,
                None,
            ))
            .map_err(ops::acceleration_compat)
    }
}

#[cfg(feature = "async")]
impl AsyncArm<'_> {
    /// See [`Arm::get_acceleration`].
    pub async fn get_acceleration(&self) -> crate::error::Result<ArmAcceleration> {
        self.client
            .run(ops::get_acceleration(&self.name))
            .await
            .map_err(ops::acceleration_compat)
    }

    /// See [`Arm::set_acceleration`].
    pub async fn set_acceleration(
        &self,
        acceleration: u32,
        travel_acceleration: u32,
        retract_acceleration: u32,
    ) -> crate::error::Result<Option<ArmAcceleration>> {
        self.client
            .run(ops::set_acceleration(
                &self.name,
                acceleration,
                travel_acceleration,
                Some(retract_acceleration),
            ))
            .await
    }

    /// See [`Arm::set_acceleration_keep_retract`].
    pub async fn set_acceleration_keep_retract(
        &self,
        acceleration: u32,
        travel_acceleration: u32,
    ) -> crate::error::Result<Option<ArmAcceleration>> {
        self.client
            .run(ops::set_acceleration(
                &self.name,
                acceleration,
                travel_acceleration,
                None,
            ))
            .await
            .map_err(ops::acceleration_compat)
    }
}
