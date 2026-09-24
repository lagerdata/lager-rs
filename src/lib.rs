//! First-class Rust access to [Lager](https://lagerdata.com) nets, so
//! embedded developers can write their entire hardware-in-the-loop test
//! suite in Rust and run it with `cargo test` — no Python required.
//!
//! The crate is a pure HTTP/JSON client of the Lager box's API on port
//! 9000: power supplies, battery simulators, e-loads, solar simulators,
//! GPIO, ADC, DAC, thermocouples, watt meters, energy analyzers, SPI, I2C,
//! USB hub ports, robot arms, webcams, routers, and (behind the `uart`
//! feature) streaming UART. Box-level capabilities — the box's own BLE adapter
//! ([`LagerBox::ble`]), WiFi interface ([`LagerBox::wifi`]), and BluFi
//! ESP32 provisioning ([`LagerBox::blufi`]) — are exposed the same way.
//! Debug-probe nets ([`nets::debug::DebugNet`]) talk to the box's debug
//! service on port 8765 for flash/erase/reset/memory-read and RTT log
//! streaming; behind the `rtt` feature they also open bi-directional
//! (interactive) RTT sessions against boxes >= 0.35.0.
//!
//! # Quickstart
//!
//! ```toml
//! [dev-dependencies]
//! lager = { package = "lager-net", version = "0.5" }
//! ```
//!
//! ```no_run
//! # #[cfg(feature = "blocking")]
//! # mod demo {
//! use lager::{LagerBox, Level};
//!
//! #[test]
//! fn dut_boots_at_3v3() -> lager::Result<()> {
//!     let lager = LagerBox::from_env()?; // reads LAGER_BOX_HOST
//!     let supply = lager.supply("supply1");
//!     let boot_ok = lager.gpio("boot_ok");
//!
//!     supply.set_voltage(3.3)?;
//!     supply.enable()?;
//!     // Hardware-timed wait on the box; returns elapsed seconds.
//!     let t = boot_ok.wait_for_level(Level::High, 5.0)?;
//!     println!("booted in {t:.3}s");
//!     supply.disable()
//! }
//! # }
//! # fn main() {}
//! ```
//!
//! # Concurrency
//!
//! The box serializes access per physical instrument (a single-owner
//! hardware service with per-device locks), so parallel cargo tests can
//! never interleave I/O on one instrument. Tests sharing a *net* still
//! observe each other's state changes — partition nets across tests or run
//! `cargo test -- --test-threads=1` when that matters.
//!
//! # Boxes behind an authenticating gateway
//!
//! Boxes fronted by an authenticating reverse proxy (gateway) reject
//! unauthenticated traffic with 401 + an `X-Gateway-Auth-Url` header. The
//! crate handles this transparently: it reuses the session created by
//! `lager login <auth_url>` (the Lager CLI's token store in
//! `~/.lager_gateway_auth`), attaches `Authorization: Bearer` to every
//! request — including debug-service and UART Socket.IO traffic — and
//! refreshes expired access tokens automatically. For CI or machines
//! without a CLI login, pin a token with
//! [`LagerBoxBuilder::bearer_token`] or the `LAGER_GATEWAY_TOKEN`
//! environment variable. Plain (ungated) boxes are unaffected: no header
//! is sent and no code path runs. When no usable credential exists, calls
//! fail with [`Error::AuthRequired`] naming the auth server to log into.
//!
//! The raw-TCP debug ports (GDB, OpenOCD, RTT telnet) are not published on
//! a gated box; [`LagerBox::debug_tunnel`] reaches them through the
//! gateway, and connects directly on a plain box. For HTTP endpoints the
//! typed API does not cover yet, [`LagerBox::bearer_token`] returns the
//! token to attach.
//!
//! # Features
//!
//! - `blocking` *(default)*: the [`LagerBox`] client on `ureq` — no tokio
//!   in the dependency tree.
//! - `async`: the [`AsyncLagerBox`] client on `reqwest`/tokio. Both clients
//!   share one wire layer ([`wire`]) so they cannot drift.
//! - `uart`: streaming UART sessions over Socket.IO ([`nets::uart::Uart`]).
//! - `rtt`: bi-directional RTT sessions over Socket.IO
//!   ([`nets::rtt::RttSession`], box >= 0.35.0).
//!
//! # Not yet on the HTTP API
//!
//! Oscilloscope workflows are not yet exposed on any box HTTP API;
//! [`nets::scope::Scope`] ships as a documented stub returning
//! [`Error::NotSupportedByBox`]. See `MISSING_ENDPOINTS.md` in the crate
//! repository.

#![deny(missing_docs)]

mod auth;
mod error;
pub mod nets;
#[cfg(any(feature = "blocking", feature = "async"))]
mod tunnel;
pub mod wire;

#[cfg(feature = "async")]
mod async_client;
#[cfg(feature = "blocking")]
mod client;

pub use error::{Error, Result};

/// Environment variable read by `from_env` constructors (`LAGER_BOX_HOST`).
pub const BOX_HOST_ENV: &str = "LAGER_BOX_HOST";

/// Environment variable that overrides the debug-service base URL
/// (`LAGER_DEBUG_SERVICE_URL`), e.g. `http://127.0.0.1:8765` when tunneling.
pub const DEBUG_SERVICE_URL_ENV: &str = "LAGER_DEBUG_SERVICE_URL";

/// Environment variable holding a bearer token for boxes behind an
/// authenticating gateway (`LAGER_GATEWAY_TOKEN`). When set, every request
/// carries `Authorization: Bearer <token>`. Equivalent to
/// `LagerBoxBuilder::bearer_token`.
pub const GATEWAY_TOKEN_ENV: &str = "LAGER_GATEWAY_TOKEN";

/// Environment variable that overrides the path of the Lager CLI's gateway
/// token store (`LAGER_GATEWAY_AUTH_FILE`; default `~/.lager_gateway_auth`).
/// The crate reads sessions created by `lager login` from this file, so the
/// same name/semantics as the CLI are honored.
pub const GATEWAY_AUTH_FILE_ENV: &str = "LAGER_GATEWAY_AUTH_FILE";

#[cfg(feature = "blocking")]
pub use client::{BoxLockGuard, LagerBox, LagerBoxBuilder};

#[cfg(feature = "async")]
pub use async_client::{AsyncLagerBox, AsyncLagerBoxBuilder};

// Net handle types and their vocabularies, re-exported at the crate root
// for ergonomic imports (`use lager::{LagerBox, Level, EloadMode};`).
pub use nets::battery::BatteryMode;
pub use nets::debug::{ConnectOptions, FirmwareKind, RttOptions};
pub use nets::dfu::DfuOptions;
pub use nets::eload::EloadMode;
pub use nets::gpio::{Level, WaitForLevelOptions};
pub use nets::scope::Scope;
pub use nets::spi::{BitOrder, CsActive, CsMode, SpiConfig, SpiOptions, SpiTransfer};

/// Debug-service response types.
pub use wire::{DebugConnection, DebugInfo, DebugStatus, GdbServer};

#[cfg(feature = "blocking")]
pub use nets::{
    adc::Adc, arm::Arm, battery::Battery, ble::Ble, blufi::Blufi, dac::Dac, debug::DebugNet,
    debug::RttStream, dfu::Dfu, eload::Eload, energy::EnergyAnalyzer, gpio::Gpio, i2c::I2c,
    router::Router, solar::Solar, spi::Spi, supply::Supply, thermocouple::Thermocouple,
    usb::UsbPort, watt::WattMeter, webcam::Webcam, wifi::Wifi,
};

#[cfg(feature = "async")]
pub use nets::{
    adc::AsyncAdc, arm::AsyncArm, battery::AsyncBattery, ble::AsyncBle, blufi::AsyncBlufi,
    dac::AsyncDac, debug::AsyncDebugNet, dfu::AsyncDfu, eload::AsyncEload,
    energy::AsyncEnergyAnalyzer, gpio::AsyncGpio, i2c::AsyncI2c, router::AsyncRouter,
    solar::AsyncSolar, spi::AsyncSpi, supply::AsyncSupply, thermocouple::AsyncThermocouple,
    usb::AsyncUsbPort, watt::AsyncWattMeter, webcam::AsyncWebcam, wifi::AsyncWifi,
};

#[cfg(feature = "uart")]
pub use nets::uart::Uart;

#[cfg(feature = "rtt")]
pub use nets::rtt::RttSession;

// Structured result types, re-exported from the wire layer.
pub use wire::{
    ArmPosition, BatteryState, BleCharacteristic, BleDevice, BleDeviceInfo, BleService,
    BlufiDeviceInfo, BlufiNetwork, BlufiProvisionResult, BlufiStatus, BoxCapabilities, BoxLock,
    BoxStatus, DfuDevice, DfuOutput, EloadState, EnergyReading, EnergyStats, Health, NetRecord,
    NetState, NetSummary, RouterSystemInfo, SafetyLimits, StatSummary, SupplyState,
    UsbDeviceFilter, UsbDeviceInfo, WattReading, WebcamStatus, WebcamStream, WifiAccessPoint,
    WifiConnection, WifiInterface,
};
pub use nets::i2c::I2cEffectiveConfig;
pub use nets::spi::SpiEffectiveConfig;
