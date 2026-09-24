# lager (Rust)

First-class Rust access to [Lager](https://lagerdata.com) nets, so embedded
developers can write their entire hardware-in-the-loop test suite in Rust
and run it with `cargo test` — no Python required.

The crate is a pure HTTP/JSON client of the Lager box's API (port 9000):
power supplies, battery simulators, e-loads, solar simulators, GPIO, ADC,
DAC, thermocouples, watt meters, energy analyzers, SPI, I2C, USB hub ports,
robot arms, webcams, routers, and streaming UART — plus the box-level
capabilities (its own BLE adapter, WiFi interface, BluFi ESP32 provisioning,
USB bus enumeration, box-side `dfu-util` flashing, and box lock/reservation).
Debug-probe nets (flash / erase / reset / memory reads / RTT) talk to the
box's debug service on port 8765.

> The package publishes as **`lager-net`** (the bare `lager` name is taken
> on crates.io), but the library target is named `lager`, so your code reads
> `use lager::LagerBox;`.

## Quickstart

```toml
# Cargo.toml
[dev-dependencies]
lager = { package = "lager-net", version = "0.6" }
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
| `UsbPort` | `lager.usb(name)` | `enable`/`disable`/`toggle`/`state`, `cycle` (off-wait-on, reports re-enumeration), `recover` |
| `Arm` | `lager.arm(name)` | `position`, `move_to`/`move_by`, `go_home`, motor enable/disable, `set_acceleration` |
| `Webcam` | `lager.webcam(name)` | `start`/`stop` MJPEG stream, `url`, `status` |
| `Router` | `lager.router(name)` | `system_info`, interfaces/clients/leases, `block_internet`, generic `command(action, params)` |
| `DebugNet` | `lager.debug(name)` | `connect`, `flash`, `erase`, `reset`, `read_memory`, `info`/`status`, `rtt` (blocking), `rtt_interactive` *(feature `rtt`)* |
| `Uart` | `lager.uart(name)?` *(feature `uart`)* | streaming `read`, non-blocking `try_read`, `write`, `wait_for(b"boot ok", ...)` |

Box-level capabilities (the box's own hardware, no net name):

| Handle | Constructor | Highlights |
| --- | --- | --- |
| `Ble` | `lager.ble()` | `scan`/`scan_named`, `info`/`connect` (GATT enumeration), `disconnect` |
| `Wifi` | `lager.wifi()` | `status`, `scan`, `connect(ssid, password)`, `delete` |
| `Blufi` | `lager.blufi()` | `scan`, `connect`, `provision(device, ssid, password)`, `wifi_scan`, `status`, `version` |
| `Dfu` | `lager.dfu()` | box-side `dfu-util`: `list`, `download(firmware, opts)`, `detach` |

USB bus enumeration: `lager.usb_devices()` (or `usb_devices_matching` with
vid/pid/serial filters) returns every USB device on the box's bus straight
from sysfs — vid, pid, iSerial, product, manufacturer, bus/dev numbers, and
speed. It takes a few milliseconds with no exclusive device access, so it is
safe to poll while waiting for a DUT to re-enumerate.

Box locking (the same `/lock` endpoints `lager boxes lock` uses):
`lager.lock(user)` / `lock_with(user, holder_type, ttl)` /
`lock_heartbeat(user)` / `unlock(user)` / `lock_status()`, plus a
`lager.lock_guard(user)` RAII guard (blocking client) that releases on drop.
Locks with a TTL auto-expire when heartbeats stop, so a crashed CI runner
cannot wedge the box.

Discovery and box health: `lager.nets()`, `lager.health()`, `lager.status()`
(`status().capabilities.net_command` tells you the box image is new enough;
`net_command_roles` / `ble_command` / `wifi_command` / `blufi_command` report
the newer arm/webcam/router roles and box-level endpoints), and
`lager.nets_state()` — brief live state for every saved net under the box's
shared probe budget, with a `reason` on every net it could not read, so one
wedged instrument neither fails the request nor hides the healthy ones.

## Minimum box version

Most of the API works on any box that serves `POST /net/command`. A few
newer surfaces need a newer box image and fail with
`Error::UnsupportedByBox` (naming the required version) on older ones:

| Crate API | Requires box |
| --- | --- |
| `UsbPort::state()` | >= 0.29.0 |
| `usb_devices()` / `usb_devices_matching()` | >= 0.33.0 |
| `dfu()` (`list`/`download`/`detach`) | >= 0.33.0 (plus `dfu-util` installed: `lager box-config apt add dfu-util`) |
| `lock()` / `unlock()` / `lock_status()` / `lock_heartbeat()` | any box serving `/lock` on port 9000 |
| `nets_state()` | >= 0.34.0 |
| `set_safety_limits()` / `clear_safety_limits()` / `safety_limits()` | >= 0.35.0 (`status().capabilities.safety_limits`) |
| `DebugNet::rtt_interactive()` *(feature `rtt`)* | >= 0.35.0 |
| `UsbPort::cycle()` / `recover()` | >= 0.39.0 |
| `ConnectOptions::halt` honored on the OpenOCD backend | >= 0.43.0 |

## Features

| Feature | Default | What you get |
| --- | --- | --- |
| `blocking` | yes | `LagerBox` on [`ureq`] — tiny dependency tree, no tokio |
| `async` | no | `AsyncLagerBox` on [`reqwest`]/tokio; same methods, `.await`ed |
| `uart` | no | `Uart` streaming sessions over the box's Socket.IO `/uart` namespace |
| `rtt` | no | bi-directional `RttSession` over the box's Socket.IO `/rtt` namespace (box >= 0.35.0) |

Both clients execute the exact same request builders and response parsers
(the `wire` module), so the two transports cannot drift apart.

```toml
lager = { package = "lager-net", version = "0.6", features = ["async"] }
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

## Boxes behind an authenticating gateway

Boxes fronted by an authenticating reverse proxy reject unauthenticated
traffic with 401 + an `X-Gateway-Auth-Url` header (the same contract the
Lager CLI speaks). The crate handles this transparently:

- **CLI session reuse (zero config):** after `lager login <auth_url>`, the
  crate picks up the session from the CLI's token store
  (`~/.lager_gateway_auth`, or `LAGER_GATEWAY_AUTH_FILE`), attaches
  `Authorization: Bearer` to every request — including debug-service and
  UART Socket.IO traffic — and refreshes expired access tokens
  automatically. The box→auth-server link is learned from the gateway's
  discovery header on first contact and the denied request is retried
  within the same call.
- **Pinned token (CI):** supply a token directly when there is no CLI
  login on the machine:

  ```rust,no_run
  # fn main() -> lager::Result<()> {
  let lager = lager::LagerBox::builder("192.168.1.42")
      .bearer_token(std::env::var("MY_CI_TOKEN").unwrap())
      .build()?;
  # Ok(()) }
  ```

  or set the `LAGER_GATEWAY_TOKEN` environment variable.

Plain (ungated) boxes are unaffected: no header is sent and none of this
code runs. When a gateway asks for auth and no usable credential exists,
calls fail with `Error::AuthRequired` naming the auth server to log into.

### Raw debug ports: `debug_tunnel`

A gated box does not publish its raw-TCP debug ports (GDB 2331-2342,
OpenOCD 4444-4447 and 6666-6669, RTT 9090-9097), because a gateway can only
check a token on HTTP. `debug_tunnel` gets a stream to one anyway: it asks
the gateway for an HTTP `CONNECT` tunnel on the debug-service port, with the
same token (pinned or refreshed) every other call uses. On a plain box it
connects to the published port directly, so the same code runs on both.

```rust,no_run
# fn main() -> lager::Result<()> {
# let lager_box = lager::LagerBox::connect("192.168.1.42")?;
lager_box.debug("debug1").connect()?;          // start the GDB server first
let stream = lager_box.debug_tunnel(2332)?;    // a std::net::TcpStream
// hand `stream` to any GDB remote-protocol client
# drop(stream);
# Ok(()) }
```

The stream has `TCP_NODELAY` set and no timeouts. A server that was just
started can take a moment to open its port, so a 502 (nothing listening) is
retried for up to 5 s; you can dial straight after `connect()`. A gateway
may close the tunnel if your access to the box is revoked; the caller sees
the stream close. `AsyncLagerBox::debug_tunnel` returns a `tokio::net::TcpStream`.
`examples/debug_tunnel.rs` sends `qSupported` and a memory read over one.

### Raw HTTP: `bearer_token`

For an HTTP endpoint the typed API does not cover yet,
`lager_box.bearer_token()?` returns the token this client would attach
(refreshed if near expiry), or `None` for a plain box. Send it as
`Authorization: Bearer <token>`, and ask for it again per request: tokens
are short-lived. Prefer a typed method where one exists.

## Errors

Everything returns `lager::Result<T>` with a single `Error` enum:

- `Connection` — box unreachable (network/Tailscale/box offline)
- `Timeout` — the box stalled past the (already widened) budget
- `Box { status, message }` — the box refused or the hardware failed
- `UnsupportedByBox` — the box image predates this endpoint (HTTP 501, a missing route, or a pre-0.29.0 `state` rejection); the message names the box version required
- `AuthRequired` — the box's gateway wants a bearer token and none is available: run `lager login <auth_url>` or set `LAGER_GATEWAY_TOKEN`
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

With the `rtt` feature (box >= 0.35.0), the session can also **write** to
the target's RTT down-channel, so firmware with an RTT console can be
driven from the test:

```rust
use std::time::Duration;

let debug = lager.debug("debug1");
debug.connect()?;                       // gdbserver must be up first

let mut rtt = debug.rtt_interactive()?;
rtt.write_str("self_test\n")?;
let out = rtt.wait_for(b"self_test: pass", Duration::from_secs(5))?;
rtt.stop()?;
```

Two prerequisites worth knowing before debugging a "silent" session: the
box refuses `start_rtt` without a connected gdbserver (call
`debug.connect()` first), and writing needs a firmware-declared RTT
**down** buffer on the channel — `defmt-rtt` alone only provides the up
buffer, and a target without a down buffer silently discards what it is
sent. Bytes are raw in both directions: `defmt` output stays compressed
binary (pipe it through `defmt-print -e <elf>` to read it), while
plain-text consoles work with `wait_for` directly.

`connect_with(&ConnectOptions { .. })` takes the probe options: `speed`,
`force`, `halt` (reset-then-halt; honored on the OpenOCD backend by box >=
0.43.0), and per-connection `jlink_script` / `openocd_config` contents that
override a script saved on the net (an OpenOCD override must be a complete
cfg that selects the adapter driver). Halt-in-place — OpenOCD's bare `halt`,
no nRESET pulse — is not reachable over the debug service's HTTP API; see
`MISSING_ENDPOINTS.md`.

If the debug service is reached through an SSH tunnel, point the crate at it
with `LagerBox::builder(host).debug_service_url("http://127.0.0.1:8765")` or
the `LAGER_DEBUG_SERVICE_URL` env var.

## Per-net safety limits

Boxes >= 0.35.0 enforce per-net voltage/current ceilings in their hardware
service, out of reach of test scripts — a setpoint (or inline `ovp=`/`ocp=`
trip) above a ceiling is refused before it touches the instrument:

```rust
use lager::SafetyLimits;

lager.set_safety_limits("supply1", &SafetyLimits {
    max_voltage: Some(5.0),
    max_current: Some(0.5),
    ..Default::default()
})?;

let limits = lager.safety_limits("supply1")?;   // rides on /nets/list
lager.clear_safety_limits("supply1")?;          // back to unrestricted
```

A set call **replaces** the net's whole limits record (unset fields are
removed, not preserved), `allow_destructive: Some(false)` makes the box
refuse erase/flash on the net, and there is deliberately no `max_power` —
the box refuses the key rather than storing a limit nothing enforces.

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
