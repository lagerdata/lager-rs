//! Robot-arm nets (Rotrics Dexarm): positioning for physical-interaction
//! tests (pressing buttons, flipping switches).
//!
//! Moves block on the box until the arm reaches the target; the HTTP budget
//! is widened past the box-side move timeout so a healthy slow move is never
//! aborted client-side. The box enforces the Dexarm workspace bounds and
//! rejects out-of-range targets.

use super::net_handle;
pub use crate::wire::ArmPosition;

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::json;

    use crate::wire::{net_command, unit, value_arm_position, ArmPosition, Op, Timeout};

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

    pub(crate) fn set_acceleration(
        name: &str,
        acceleration: u32,
        travel_acceleration: u32,
        retract_acceleration: u32,
    ) -> Op<()> {
        Op {
            req: net_command(
                name,
                ROLE,
                "set_acceleration",
                json!({
                    "acceleration": acceleration,
                    "travel_acceleration": travel_acceleration,
                    "retract_acceleration": retract_acceleration,
                }),
                quick_budget(),
            ),
            parse: unit,
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
        /// Set print/travel/retract acceleration (mm/s², G-code M204).
        fn set_acceleration(
            acceleration: u32,
            travel_acceleration: u32,
            retract_acceleration: u32,
        ) -> () = ops::set_acceleration;
    }
}
