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
//! streaming.
//!
//! # Quickstart
//!
//! ```toml
//! [dev-dependencies]
//! lager = { package = "lager-net", version = "0.1" }
//! ```
//!
//! ```no_run
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
//! # Features
//!
//! - `blocking` *(default)*: the [`LagerBox`] client on `ureq` — no tokio
//!   in the dependency tree.
//! - `async`: the [`AsyncLagerBox`] client on `reqwest`/tokio. Both clients
//!   share one wire layer ([`wire`]) so they cannot drift.
//! - `uart`: streaming UART sessions over Socket.IO ([`nets::uart::Uart`]).
//!
//! # Not yet on the HTTP API
//!
//! Oscilloscope workflows are not yet exposed on any box HTTP API;
//! [`nets::scope::Scope`] ships as a documented stub returning
//! [`Error::NotSupportedByBox`]. See `MISSING_ENDPOINTS.md` in the crate
//! repository.

#![deny(missing_docs)]

mod error;
pub mod nets;
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

#[cfg(feature = "blocking")]
pub use client::{LagerBox, LagerBoxBuilder};

#[cfg(feature = "async")]
pub use async_client::{AsyncLagerBox, AsyncLagerBoxBuilder};

// Net handle types and their vocabularies, re-exported at the crate root
// for ergonomic imports (`use lager::{LagerBox, Level, EloadMode};`).
pub use nets::battery::BatteryMode;
pub use nets::debug::{ConnectOptions, FirmwareKind, RttOptions};
pub use nets::eload::EloadMode;
pub use nets::gpio::{Level, WaitForLevelOptions};
pub use nets::scope::Scope;
pub use nets::spi::{BitOrder, CsActive, CsMode, SpiConfig, SpiOptions, SpiTransfer};

/// Debug-service response types.
pub use wire::{DebugConnection, DebugInfo, DebugStatus, GdbServer};

#[cfg(feature = "blocking")]
pub use nets::{
    adc::Adc, arm::Arm, battery::Battery, ble::Ble, blufi::Blufi, dac::Dac, debug::DebugNet,
    debug::RttStream, eload::Eload, energy::EnergyAnalyzer, gpio::Gpio, i2c::I2c, router::Router,
    solar::Solar, spi::Spi, supply::Supply, thermocouple::Thermocouple, usb::UsbPort,
    watt::WattMeter, webcam::Webcam, wifi::Wifi,
};

#[cfg(feature = "async")]
pub use nets::{
    adc::AsyncAdc, arm::AsyncArm, battery::AsyncBattery, ble::AsyncBle, blufi::AsyncBlufi,
    dac::AsyncDac, debug::AsyncDebugNet, eload::AsyncEload, energy::AsyncEnergyAnalyzer,
    gpio::AsyncGpio, i2c::AsyncI2c, router::AsyncRouter, solar::AsyncSolar, spi::AsyncSpi,
    supply::AsyncSupply, thermocouple::AsyncThermocouple, usb::AsyncUsbPort,
    watt::AsyncWattMeter, webcam::AsyncWebcam, wifi::AsyncWifi,
};

#[cfg(feature = "uart")]
pub use nets::uart::Uart;

// Structured result types, re-exported from the wire layer.
pub use wire::{
    ArmPosition, BatteryState, BleCharacteristic, BleDevice, BleDeviceInfo, BleService,
    BlufiDeviceInfo, BlufiNetwork, BlufiProvisionResult, BlufiStatus, BoxCapabilities, BoxStatus,
    EloadState, EnergyReading, EnergyStats, Health, NetRecord, NetSummary, RouterSystemInfo,
    StatSummary, SupplyState, WattReading, WebcamStatus, WebcamStream, WifiAccessPoint,
    WifiConnection, WifiInterface,
};
pub use nets::i2c::I2cEffectiveConfig;
pub use nets::spi::SpiEffectiveConfig;
