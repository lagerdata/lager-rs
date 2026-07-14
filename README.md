# lager (Rust)

First-class Rust access to [Lager](https://lagerdata.com) nets, so embedded
developers can write their entire hardware-in-the-loop test suite in Rust
and run it with `cargo test` — no Python required.

The crate is a pure HTTP/JSON client of the Lager box's API (port 9000):
power supplies, battery simulators, e-loads, solar simulators, GPIO, ADC,
DAC, thermocouples, watt meters, energy analyzers, SPI, I2C, USB hub ports,
robot arms, webcams, routers, and streaming UART — plus the box-level capabilities (its own BLE
adapter, WiFi interface, and BluFi ESP32 provisioning). Debug-probe nets
(flash / erase / reset / memory reads / RTT) talk to the box's debug service
on port 8765.

> The package publishes as **`lager-net`** (the bare `lager` name is taken
> on crates.io), but the library target is named `lager`, so your code reads
> `use lager::LagerBox;`.

## Quickstart

```toml
# Cargo.toml
[dev-dependencies]
lager = { package = "lager-net", version = "0.1" }
```

```rust
// tests/boot.rs
use lager::{LagerBox, Level};

#[test]
fn dut_boots_at_3v3() -> lager::Result<()> {
    let lager = LagerBox::from_env()?; // reads LAGER_BOX_HOST
    let supply = lager.supply("supply1");
    let boot_ok = lager.gpio("boot_ok");

    supply.set_voltage(3.3)?;
    supply.enable()?;

    // Hardware-timed wait on the box; returns elapsed seconds.
    let t = boot_ok.wait_for_level(Level::High, 5.0)?;
    println!("booted in {t:.3}s");

    supply.disable()
}
```

```sh
LAGER_BOX_HOST=192.168.1.42 cargo test
```

`LagerBox::connect("hostname-or-ip")` also works, with an optional
`host:port` or full URL.

## Net types

| Handle | Constructor | Highlights |
| --- | --- | --- |
| `Supply` | `lager.supply(name)` | `set_voltage`, `set_current`, `enable`/`disable`, OCP/OVP, `state()` |
| `Battery` | `lager.battery(name)` | `set_soc`, `set_voc`, model/capacity/mode, `state()` |
| `Eload` | `lager.eload(name)` | `set(EloadMode::Cc, 0.5)`, `setpoint`, `state()` |
| `Solar` | `lager.solar(name)` | `set`/`stop` PV simulation, `irradiance`/`set_irradiance`, `voc`, `mpp_voltage`/`mpp_current`, `resistance`, `temperature` |
| `Gpio` | `lager.gpio(name)` | `input`, `output`, `toggle`, `wait_for_level` (hardware-timed) |
| `Adc` / `Dac` | `lager.adc(name)` / `lager.dac(name)` | `read()`; `set(volts)` |
| `Thermocouple` | `lager.thermocouple(name)` | `read()` in °C |
| `WattMeter` | `lager.watt_meter(name)` | `power/current/voltage/all(duration)` |
| `EnergyAnalyzer` | `lager.energy_analyzer(name)` | `read_energy`, `read_stats` |
| `Spi` | `lager.spi(name)` | `configure`, `read`, `write`, `read_write`, `transfer` |
| `I2c` | `lager.i2c(name)` | `configure`, `scan`, `read`, `write`, `write_read` |
| `UsbPort` | `lager.usb(name)` | `enable`/`disable`/`toggle`/`state` |
| `Arm` | `lager.arm(name)` | `position`, `move_to`/`move_by`, `go_home`, motor enable/disable, `set_acceleration` |
| `Webcam` | `lager.webcam(name)` | `start`/`stop` MJPEG stream, `url`, `status` |
| `Router` | `lager.router(name)` | `system_info`, interfaces/clients/leases, `block_internet`, generic `command(action, params)` |
| `DebugNet` | `lager.debug(name)` | `connect`, `flash`, `erase`, `reset`, `read_memory`, `info`/`status`, `rtt` (blocking) |
| `Uart` | `lager.uart(name)?` *(feature `uart`)* | streaming `read`, `write`, `wait_for(b"boot ok", ...)` |

Box-level capabilities (the box's own hardware, no net name):

| Handle | Constructor | Highlights |
| --- | --- | --- |
| `Ble` | `lager.ble()` | `scan`/`scan_named`, `info`/`connect` (GATT enumeration), `disconnect` |
| `Wifi` | `lager.wifi()` | `status`, `scan`, `connect(ssid, password)`, `delete` |
| `Blufi` | `lager.blufi()` | `scan`, `connect`, `provision(device, ssid, password)`, `wifi_scan`, `status`, `version` |

Discovery and box health: `lager.nets()`, `lager.health()`, `lager.status()`
(`status().capabilities.net_command` tells you the box image is new enough;
`net_command_roles` / `ble_command` / `wifi_command` / `blufi_command` report
the newer arm/webcam/router roles and box-level endpoints).

## Features

| Feature | Default | What you get |
| --- | --- | --- |
| `blocking` | yes | `LagerBox` on [`ureq`] — tiny dependency tree, no tokio |
| `async` | no | `AsyncLagerBox` on [`reqwest`]/tokio; same methods, `.await`ed |
| `uart` | no | `Uart` streaming sessions over the box's Socket.IO `/uart` namespace |

Both clients execute the exact same request builders and response parsers
(the `wire` module), so the two transports cannot drift apart.

```toml
lager = { package = "lager-net", version = "0.1", features = ["async"] }
```

## Parallel tests and instrument safety

The box serializes access per physical instrument: every net command runs
under a per-device lock in the box's single-owner hardware service, so
parallel `cargo test` threads can never interleave I/O on one instrument
(e.g. a LabJack shared across GPIO/ADC/SPI nets, or a Keithley shared by
supply and battery roles). Tests sharing a *net* still observe each other's
state changes — partition nets across tests, or run
`cargo test -- --test-threads=1` when that matters.

Timeout budgets mirror the Lager CLI: quick commands use 10 s; watt/energy
integration windows and `wait_for_level` widen (or drop) the client timeout
automatically so a healthy long measurement is never aborted mid-flight.

## Errors

Everything returns `lager::Result<T>` with a single `Error` enum:

- `Connection` — box unreachable (network/Tailscale/box offline)
- `Timeout` — the box stalled past the (already widened) budget
- `Box { status, message }` — the box refused or the hardware failed
- `UnsupportedByBox` — HTTP 501: the box image predates this endpoint; update the box
- `NotSupportedByBox` — the net type is a documented stub (see below)

## Firmware, flashing, and RTT

`DebugNet` drives a J-Link/OpenOCD debug probe through the box debug service
(port 8765, published on the box host):

```rust
let debug = lager.debug("debug1");
debug.connect()?;
debug.erase()?;
debug.flash("firmware.hex")?;   // .hex/.elf/.bin inferred from extension
debug.reset(false)?;
let head = debug.read_memory(0x0800_0000, 16)?;

// Stream RTT logs (blocking client) and assert on target output:
use std::io::{BufRead, BufReader};
let mut lines = BufReader::new(debug.rtt()?).lines();
assert!(lines.next().transpose()?.unwrap().contains("boot ok"));
```

If the debug service is reached through an SSH tunnel, point the crate at it
with `LagerBox::builder(host).debug_service_url("http://127.0.0.1:8765")` or
the `LAGER_DEBUG_SERVICE_URL` env var.

## Not yet on the HTTP API

Oscilloscope / logic-analyzer workflows are not exposed on any box HTTP API
yet. `Scope` ships as a documented stub whose methods return
`Error::NotSupportedByBox`; see
[`MISSING_ENDPOINTS.md`](MISSING_ENDPOINTS.md) for the endpoint sketch.

## Hardware smoke tests

The crate's own test suite is hermetic (`cargo test` runs against a mock
box). To exercise a real box:

```sh
LAGER_BOX_HOST=192.168.1.42 cargo test --test hardware -- --ignored
# opt into net-specific tests:
LAGER_BOX_HOST=... LAGER_TEST_SUPPLY_NET=supply1 cargo test --test hardware -- --ignored
```

There is also a runnable example:

```sh
LAGER_BOX_HOST=192.168.1.42 cargo run --example power_cycle -- supply1 adc1
```

## Requirements

- A Lager box with software new enough to serve `POST /net/command`
  (check `lager.status()?.capabilities.net_command`).
- Rust 1.75+.

## License

Apache-2.0

[`ureq`]: https://crates.io/crates/ureq
[`reqwest`]: https://crates.io/crates/reqwest
