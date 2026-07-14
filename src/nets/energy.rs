//! Energy-analyzer nets (Joulescope JS220, Nordic PPK2): integrated energy
//! and signal statistics.

use super::net_handle;
pub use crate::wire::{EnergyReading, EnergyStats, StatSummary};

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::json;

    use crate::wire::{net_command, value_as, EnergyReading, EnergyStats, Op, Timeout};

    const ROLE: &str = "energy-analyzer";

    /// Budget: cover the integration window plus margin, floor 30s
    /// (mirrors `lager energy`'s `max(30, duration + 30)`). The box clamps
    /// the window itself to 120s.
    fn budget(duration: f64) -> Timeout {
        Timeout::After(Duration::from_secs_f64((duration + 30.0).max(30.0)))
    }

    pub(crate) fn read_energy(name: &str, duration: f64) -> Op<EnergyReading> {
        Op {
            req: net_command(
                name,
                ROLE,
                "read_energy",
                json!({ "duration": duration }),
                budget(duration),
            ),
            parse: value_as::<EnergyReading>,
        }
    }

    pub(crate) fn read_stats(name: &str, duration: f64) -> Op<EnergyStats> {
        Op {
            req: net_command(
                name,
                ROLE,
                "read_stats",
                json!({ "duration": duration }),
                budget(duration),
            ),
            parse: value_as::<EnergyStats>,
        }
    }
}

net_handle! {
    /// Handle for an energy-analyzer net.
    ///
    /// The box clamps integration windows to 0.1–120 seconds. The HTTP
    /// timeout is widened automatically to cover the window.
    sync: EnergyAnalyzer,
    async: AsyncEnergyAnalyzer,
    methods: {
        /// Integrate energy (J) and charge (C) over `duration` seconds.
        fn read_energy(duration: f64) -> EnergyReading = ops::read_energy;
        /// Gather current/voltage/power statistics over `duration` seconds.
        fn read_stats(duration: f64) -> EnergyStats = ops::read_stats;
    }
}
