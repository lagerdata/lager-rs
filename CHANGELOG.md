# Changelog

All notable changes to the `lager-net` crate are documented here.

## [Unreleased]

### Added

- **`Supply::set_voltage_with()` / `set_current_with()`** set a setpoint and
  any of the OCP/OVP limits in one request, taking a new `ProtectionLimits`.
  Each limit given is also enabled, as with `set_ocp()` / `set_ovp()`.
- **`Supply::set_mode()`** switches a Keithley 2281S into power-supply mode
  after it was used as a battery simulator. It is a no-op on other supplies.
- **`Battery::models()`, `export_model(slot)` and `create_model(slot, voc,
  resistance)`** list, read and write the simulator's battery models, with
  new `BatteryModelEntry`, `BatteryModel` and `BatteryModelPoint` types.
  `create_model` overwrites the slot: the instrument cannot delete or protect
  one.
- **`EloadState` gains `current_setting`, `voltage_setting`,
  `resistance_setting` and `power_setting`**, the setpoint for the active
  mode, which the box already sent. Previously they were only in `extra`.
- **`EnergyReading` gains `energy_wh` and `charge_ah`**, which the box
  already sent. Previously they were only in `extra`.
- **`LagerBox::uart_with(name, &UartConfig)`** opens a UART session with its
  own serial settings (baud rate, data bits, `Parity`, `StopBits`, XON/XOFF,
  RTS/CTS, DSR/DTR, `opost`). Fields left `None` keep the net's saved
  settings, and the saved net is never changed. `uart()` still opens with the
  saved settings.
- **`LagerBox::uart_sessions()` / `release_uart_session(name)`** (both
  clients) list the UART sessions holding a net, with liveness detail, and
  force-release one (box >= 0.46.0; older boxes fail with
  `Error::UnsupportedByBox`).
- **`Error::UartInUse`** when a UART session cannot start because another
  session holds the net or its serial device. `held_by` names the net to
  release (box >= 0.49.0), which is the other net when two nets share one
  device.
- **`Ble::scan_exact()`** (exact name match), and **`Ble::info_with_timeout()`
  / `connect_with_timeout()`** for a device that needs longer than the default
  10 s connect timeout.
- **`Wifi::connect_on(ssid, password, interface)`** joins a network on an
  interface other than `wlan0`.
- **`Blufi::scan_including(timeout, name_contains)`** also returns devices
  that advertise only a name and not the BluFi service UUID, and
  **`Blufi::wifi_scan_for(device_name, scan_timeout)`** gives the target
  longer than the default 15 s for its WiFi scan.
- **`DfuOptions::timeout_seconds`** sets how long the box lets `dfu-util` run
  (box default 120 s, maximum 600 s), and the crate widens its HTTP timeout to
  match.
- **`DebugNet::flash_with()` / `flash_bytes_with()`** take `FlashOptions`:
  `jlink_script` sends a `.JLinkScript` with the flash, `verbose` flashes
  through J-Link Commander alone with full output, and `require_evidence:
  Some(false)` accepts a J-Link flash whose output lacks the usual
  "programmed" lines. Pass the script you connected with: without one the box
  uses the net's saved script, and failing that a temp file holding the script
  of whichever J-Link net connected most recently.
- **`DebugNet::erase_with()` / `erase_range_with()`** take `EraseOptions`: J-Link
  interface `speed`, `transport` (`Transport::Swd` / `Jtag`) and
  `require_evidence`.
- **`RttOptions::timeout`** closes the one-way `rtt()` stream after that many
  seconds.
- **`LagerBox::debug_service_health()`, `debug_service_health_detailed()` and
  `debug_service_status()`** (both clients) read the debug service's version
  and features, its operating state and warnings (`DebugHealthDetailed`), and
  every connection it is tracking (`DebugServiceStatus`).
- **The `debug_tunnel` example** now reads the core registers, and with
  `--write` writes four bytes of RAM, reads them back and restores them: the
  pattern for register access, memory writes and other GDB packets the typed
  API does not cover.
- **Typed `Scope` methods for the actions that were only reachable through
  `Scope::command`:** `capabilities()` (`ScopeCapabilities`), `state()`
  (`ScopeState`: every control setting in one request), the trigger getters
  `capture_mode()`, `trigger_source()`, `trigger_slope()` and
  `trigger_level()`, `set_trigger_holdoff()` / `trigger_holdoff()`,
  `set_acquisition()` / `acquisition()` (`AcquisitionMode`: normal, average,
  peak), `set_roll()` / `roll()` (`RollMode`), `set_display()` / `display()`
  (`DisplaySettings`: persistence, XY, zoom, math trace, FFT pane), `fft()`
  (`FftOptions`, returns a `Spectrum` of `SpectralPeak`s), and the cursors:
  `set_cursors()`, `cursors()`, `clear_cursors()` and `measure_cursors()`.
- **Typed `Router` methods for every remaining router action:** security
  profiles (`security_profiles()`, `create_security_profile()` with a new
  `SecurityProfile`, `create_open_security_profile()`,
  `update_security_profile_password()`, `delete_security_profile()`),
  `configure_wireless()`, `set_interface_disabled()`, `wait_for_ready()`,
  `wait_for_wireless_ready()`, DHCP (`enable_dhcp()`, `disable_dhcp()`,
  `set_dhcp_lease_time()`), bandwidth limits (`add_bandwidth_limit()`,
  `remove_bandwidth_limits()`), `add_firewall_rule()`, `block_dns()`,
  `block_port()`, the access list (`access_list()`,
  `add_access_list_entry()` with a new `AccessListEntry`,
  `remove_access_list_entry()`, `clear_access_list()`),
  `reset_to_defaults()` (`RouterBaseline`) and `rest_get()` for read-only
  RouterOS REST paths. `Router::command` stays for anything else.

### Changed

- **`Solar::resistance()` and `set_resistance()` return `Option<f64>`**, not
  `f64`. When the box cannot compute the panel resistance (Isc <= 0 or an
  unparseable instrument reply) it answers `"n/a"`, which used to fail with
  `Error::Decode`; it is now `None`. A failed `set_resistance()` returns the
  unchanged resistance, not an error, so compare the result with the value you
  set.
- The `EnergyAnalyzer` docs now say that the box silently clamps the
  integration window to 0.1-120 s; `EnergyReading::duration_s` reports the
  window actually used.
- **A UART "already in use" refusal is now `Error::UartInUse`**, not
  `Error::Stream`. Other session errors are still `Error::Stream`.
- **`wire::Method` gains a `Delete` variant** (for `release_uart_session`). An
  exhaustive `match` on `Method` needs an arm for it.
- **`DfuOptions` gains a field.** A struct literal without
  `..Default::default()` needs `timeout_seconds` added.
- **`DebugNet::reset()` and `halt()` return `DebugOutput`** (the probe's
  output lines and the backend), not `()`. Callers that use `?` or `unwrap()`
  are unaffected.
- **`RttOptions` gains a field.** A struct literal without
  `..Default::default()` needs `timeout` added.

### Fixed

- **`DebugNet::read_memory_no_reset()` docs** said the box resets and halts
  before an OpenOCD read. It never does: only a J-Link net on a Renesas
  DA1469x resets and halts first, so that is the only case the method changes.
- **`EloadState::mode` docs** listed lower-case modes. The box reports them
  in upper case, and constant power as `"CW"`: `"CC"`, `"CV"`, `"CR"`,
  `"CW"`.

## [0.8.0] - 2026-10-26

Catches the crate up with box software 0.44.0 through 0.60.0. Several return types
change and some response types become `#[non_exhaustive]`; see the behavior-change
entries under **Changed**. New calls name the box version they need, and fail with
`Error::UnsupportedByBox` on an older box.

### Added

- **`DebugNet::erase_range(start, size)`** erases only part of the flash, and
  `erase()` now returns an `EraseReport` saying which range the box erased
  (`erase_range` with `start`, `end`, `length`, `source`). Requires box
  >= 0.50.0. The crate checks the debug service's `/health` features first
  and fails with `Error::UnsupportedByBox` on an older box, which would
  ignore the range and erase the whole chip.
- **`DebugNet::status_probed()` / `info_probed()`** ask the box to read the
  target, and `DebugStatus` / `DebugInfo` gain `gdbserver_running` and
  `target_attached` (box >= 0.44.0; `None` from older boxes).
  `target_attached` is `None` from plain `status()` unless the box already
  knows the attach failed. `connected` keeps its meaning (a server process
  is running).
- **`DebugNet::halt()`** stops the core where it is without pulsing nRESET,
  unlike `reset(true)`, which matters on a part executing in place out of
  QSPI (OpenOCD nets; box >= 0.60.0). The crate checks the debug
  service's `/health` features first and fails with
  `Error::UnsupportedByBox` on a box without the route. A J-Link net gets
  the box's 400 pointing at a halt-first `.JLinkScript`.
- **`DebugNet::read_memory_no_reset()`** reads without the reset/halt the box
  otherwise does first on OpenOCD.
- **`ConnectOptions` gains `gdb_port`, `swo_port`, `telnet_port` and
  `rtt_telnet_port`** overrides, and `RttOptions::chunk_size` is now sent on
  the one-way `rtt()` stream too.
- **`Error::ProbeBusy`** (HTTP 503: another operation held the probe past the
  box's wait, box >= 0.51.0) and **`Error::SafetyRefused`** (HTTP 403: the
  net's `allow_destructive: false` refused a flash or erase). Both were
  `Error::Box` before.
- **`lager.custom(name).command(action, params)`** drives a net whose
  instrument is run by a custom driver installed on the box (box >= 0.59.0).
  `BoxCapabilities::custom_drivers` says whether the box runs them, and
  `net_command_roles` lists the installed roles.
- **`Webcam::snapshot()`** returns one JPEG frame from the running stream
  (box >= 0.47.0), and **`Webcam::start_as(source, started_by)`** records who
  started a stream, which `WebcamStatus::source` / `started_by` report.
- **`Arm::get_acceleration()`** and **`Arm::set_acceleration_keep_retract()`**
  (box >= 0.60.0). `Arm::set_acceleration()` now returns the arm's
  readback as `Option<ArmAcceleration>` (`None` from a box at 0.59.0 or
  older), so a value the arm did not take shows up.
- **`UsbPort::cycle_report()`** returns the box's `outcome` with the result
  (box >= 0.49.0), which tells "nothing was on the port" apart from "the box
  could not watch for the device". `cycle()` is unchanged.
- **SPI and I2C `configure()` report `achieved_frequency_hz`** (box >=
  0.47.0), the clock the adapter actually runs, which a LabJack U3 rounds
  down, and **`warnings`** (box >= 0.49.0).
- **`NetState::enabled`** (box >= 0.53.0), and `reason_code` documents the
  codes box 0.58.0 added.
- **`BoxCapabilities` gains `custom_drivers`, `custom_driver_deps`,
  `net_metadata_sync`, `box_metadata_sync`, `bench_manifest`,
  `nets_state_enabled` and `nets_state_stream`.**
- **`lager.scope(name)` drives oscilloscope nets** (box >= 0.55.0), where it
  was a stub that returned `Error::NotSupportedByBox`. A channel net takes
  `enable`/`disable`, `set_scale`, `set_coupling`, `set_probe`,
  `set_offset` and their readbacks; either net takes `set_timebase`,
  `set_time_offset`, `trigger_edge` (only the fields set are applied),
  `trigger()`, `run`/`single`/`stop`, `force_trigger` and `autoscale`.
  `measure(Measurement)` returns `None` for a quantity the capture lacks,
  and `measure_all()` reads every quantity from one capture. `command()`
  reaches the remaining scope actions. Trace capture is not on the HTTP API.
- **`lager.hsm(name)` drives a YubiHSM 2 net** (box >= 0.59.0): `info`,
  `generate`, `sign`, `public_key`, `objects`, `delete`, `blink` and
  `reset`. Credentials go in `HsmAuth`, whose `Debug` output leaves out the
  password. Tested against a mock box only.

### Changed

- **A debug flash that programmed nothing is now an error.** The box answers
  `/debug/flash` with HTTP 200 and `programmed: false` when J-Link wrote
  nothing (box >= 0.51.0), and the crate returned `Ok`. `flash()`,
  `flash_bin()` and `flash_bytes()` now fail with the new
  `Error::FlashFailed { error, output }`, and return a `FlashReport`
  (`output`, `backend`, `programmed`) on success.

  **Behavior change:** `flash*()` returns `Result<FlashReport>` and `erase()`
  returns `Result<EraseReport>` instead of `Result<()>`; `debug.flash(..)?;`
  still compiles. A `ConnectOptions { .. }` literal needs `..Default::default()`
  for the new port fields. `DebugInfo`, `DebugStatus` and the new report types
  are `#[non_exhaustive]`, so fields can be added later without a breaking
  release.

- **Behavior change:** `Arm::set_acceleration()` returns
  `Result<Option<ArmAcceleration>>` instead of `Result<()>`;
  `arm.set_acceleration(..)?;` still compiles. `BoxCapabilities`,
  `BoxStatus`, `NetState`, `WebcamStatus`, `WebcamStream`,
  `SpiEffectiveConfig` and `I2cEffectiveConfig` are now `#[non_exhaustive]`,
  so fields the box adds later are not breaking changes.

- **Behavior change:** `Scope` now borrows its client like every other net
  handle (`Scope<'_>`, plus `AsyncScope<'_>` from `AsyncLagerBox::scope`),
  and the stub's `capture()` and `measure(&str)` are gone: use
  `measure(Measurement)`.

- **The `async` feature now uses `reqwest` 0.13** (was 0.12). The async
  client's API is unchanged: no `reqwest` type appears in it. A project that
  also depends on `reqwest` 0.12 directly now builds both versions.

### Fixed

- **`DebugNet::rtt()` yields only the target's RTT output** (#5). Before,
  the `RttStream` also carried the HTTP chunk framing: hex chunk sizes and
  `\r\n` terminators were mixed into the firmware output, so a
  `BufReader::lines()` loop saw lines such as `"64"` and `""` that the
  target never printed. The box debug service sends its RTT stream as
  chunked under an `HTTP/1.0` status line, where chunked encoding is not
  defined, so the HTTP client passed the framing through. The crate now
  decodes it. Works against every box; no box update is needed.
  `rtt_interactive()` (feature `rtt`) was not affected.

- **A pinned token no longer writes the CLI token store.** A gateway denial
  recorded the box-to-auth-server mapping even when the token came from
  `LagerBoxBuilder::bearer_token` or `LAGER_GATEWAY_TOKEN`, so a CI job that
  supplied nothing but a token still left a `~/.lager_gateway_auth` behind
  it. Nothing reads that mapping in pinned mode — the token is attached to
  every request without a store lookup — and a self-hosted runner keeps its
  filesystem between jobs, so the entry outlived the address it named and
  was silently wrong once the box moved. The in-memory value still updates,
  because the denial error names the auth server. This is what the crate's
  own auth page already described, and what §6.3 of the gateway-auth
  contract now states.

## [0.7.0] - 2026-10-15

### Changed

- **A failed DFU run keeps `dfu-util`'s output.** When box-side `dfu-util` exits
  non-zero, `Dfu::download`, `Dfu::detach` and `Dfu::list` (and the async twins)
  now fail with the new `Error::DfuFailed { status, exit_code, stdout, stderr }`
  instead of `Error::Box`. Both streams are complete, and its `Display` quotes
  both. Before, the error carried only the box's summary, which quoted the last
  3 lines of one stream: `dfu-util` reports a raw download's device failure on
  stdout and its DFU suffix warnings on stderr, so the warnings hid the cause.
  This works against every box that serves `POST /usb/dfu`, since the box has
  always sent the full output.

  **Behavior change:** code that matched
  `Error::Box { status: 502, .. }` for a DFU failure needs an
  `Error::DfuFailed` arm. Code that only formats the error just sees more text.
  A box without `dfu-util` installed (HTTP 500) and a box-side timeout
  (HTTP 504) are still `Error::Box`.

## [0.6.0] - 2026-09-24

### Added

- **`LagerBox::debug_tunnel(port)`** (and `AsyncLagerBox::debug_tunnel`):
  a TCP stream to one of the box's raw debug ports, such as a GDB server,
  for a GDB remote-protocol client:

  ```rust
  let stream = lager_box.debug_tunnel(2332)?;
  ```

  On a box behind an authenticating gateway, whose debug ports are not
  published, this opens an HTTP `CONNECT` tunnel through the gateway,
  authorized with the client's own token (pinned or refreshed, with the
  usual single retry after a 401). On a plain box it connects to the port
  directly. Needs a gateway with debug-tunnel support; an older one fails
  with `Error::Box` saying it needs updating, also when the token is pinned.
  A server that was just started can take a moment to open its port, so a
  502 (nothing listening) is retried every 250 ms for up to 5 s before it
  fails; start the server first with `DebugNet::connect`.

- **`LagerBox::bearer_token()`** (and the async twin): the token the client
  attaches (refreshed if near expiry), or `None` for a plain box. An escape
  hatch for raw HTTP calls the typed API does not cover yet, so callers no
  longer need their own copy of the gateway auth.

### Changed

- The `async` feature now enables tokio's `net` and `io-util` features,
  for `AsyncLagerBox::debug_tunnel`.

## [0.5.0] - 2026-08-27

Catch-up release against box software 0.43.0: everything the box gained
since 0.35.0 that is reachable over its HTTP API is now covered.

### Added

- **`usb.cycle()` / `cycle_with_off_time(secs)` / `recover()`** — power-cycle
  a hub port (off, wait, on; box-validated off window 0.5-10s, default 1s
  set above the slowest cold boot measured on real hardware) and re-power a
  port left dark by an interrupted command. `cycle` returns
  `Option<bool>`: whether the device re-enumerated, or `None` when the port
  was empty or the hub cannot observe it. Works on every supported hub type
  (Acroname, Yepkit, and the Plugable RTS5411 docks box 0.39.0 added).
  Requires box >= 0.39.0; older boxes fail with `Error::UnsupportedByBox`.
  Do not script "device absent" checks around these: an unpowered port
  raises no change bit, so the device stays in lsusb until power returns —
  `cycle`'s return value is the observable that actually exists.

- **`lager.nets_state()`** — typed `GET /nets/state` on both clients: brief
  live state for every saved net under the box's shared 8s probe budget,
  with `state: None` plus a `reason` / `reason_code` for instruments that
  are slow, wedged, absent, or have no live-state probe. Safe as a bench
  health check: one wedged instrument can neither fail the request nor
  block the others' answers. Requires box >= 0.34.0.

- **`ConnectOptions.jlink_script` / `openocd_config`** — per-connection
  debug scripts, base64-encoded onto `/debug/connect` and taking precedence
  over a script saved on the net. An OpenOCD override must be a *complete*
  cfg that selects the adapter driver (lager still appends its own
  `ftdi channel <N>`, which dies unless a cfg selected the ftdi adapter
  first).

### Changed

- **`ConnectOptions.halt` documents what it actually does** — reset-then-halt,
  honored on the OpenOCD backend by box >= 0.43.0 (older boxes pinned it to
  `false` there; J-Link always honored it). The box's new halt-in-place
  (`DebugNet.halt()`, OpenOCD's bare `halt`) exists only in the on-box
  `lager python` API — the debug service exposes no HTTP endpoint for it —
  so the crate cannot reach it; see `MISSING_ENDPOINTS.md`.

Not covered, deliberately: oscilloscope / logic-analyzer workflows (still on
the legacy exec path, `Scope` stays a documented stub), FTDI per-net channel
selection (`params.interface` is saved-net configuration, done at net-add
time — the crate drives nets by name and read-back already carries `params`),
and `lager python` remote-script job control (the crate exists to replace
those scripts with Rust).

## [0.4.0] - 2026-08-06

First-class support for the two features box software 0.35.0 shipped:
per-net safety limits and bi-directional (interactive) RTT.

### Added

- **Per-net safety limits** — `lager.set_safety_limits(name, &SafetyLimits)`,
  `clear_safety_limits(name)`, and `safety_limits(name)` on both the
  blocking and async clients, against the box's new
  `PUT /nets/<name>/safety-limits` route. `SafetyLimits` carries
  `max_voltage` / `max_current` ceilings (enforced by the box's hardware
  service, out of reach of test scripts; inline `ovp=`/`ocp=` trips are
  capped too) and `allow_destructive: Some(false)` to refuse erase/flash on
  the net. A PUT **replaces** the whole limits record — unset fields are
  removed, not preserved — and an all-`None` body clears it. Read-back
  rides on `/nets/list`: `NetRecord` gained a typed `safety_limits` field,
  and `BoxCapabilities` a `safety_limits` flag (`safetyLimits` on the
  wire). Requires box >= 0.35.0; older boxes fail with
  `Error::UnsupportedByBox`. The box's validation refusals (unknown key,
  `max_power`, non-positive ceiling) surface verbatim as `Error::Box`.

- **Interactive RTT** (new cargo feature `rtt`, blocking-only like `uart`) —
  `debug.rtt_interactive()` / `rtt_interactive_with(&RttOptions)` open a
  bi-directional `RttSession` over the box's Socket.IO `/rtt` namespace:
  the target's up-channel arrives via `read(timeout)` / `try_read()` /
  `wait_for(needle, timeout)`, and `write()` / `write_str()` reach the
  target's RTT **down** buffer, so firmware with an RTT console can be
  driven from a cargo test. The gdbserver must already be connected
  (`debug.connect()` first), and writing needs a firmware-declared down
  buffer on the channel — `defmt-rtt` alone only provides the up buffer;
  without one the target silently discards writes. The existing
  `debug.rtt()` one-way HTTP stream is unchanged. Requires box >= 0.35.0.

### Changed

- **`RttOptions` gained `chunk_size: Option<u64>`** (box-side read chunk
  size, J-Link only, used by interactive RTT). Breaking only for
  struct-literal construction without `..Default::default()`.

## [0.3.0] - 2026-07-28

### Added

- **`lager.usb_devices()` / `usb_devices_matching(filter)`** — generic USB
  bus enumeration from the box's sysfs (vid, pid, iSerial, product,
  manufacturer, bus/dev numbers, speed), with optional box-side vid/pid/
  serial filters. Cheap enough to poll while waiting for a DUT to
  re-enumerate. Requires box software >= 0.33.0 (`GET /usb/devices`);
  older boxes fail with `Error::UnsupportedByBox`.

- **`lager.dfu()`** — box-side USB-DFU via `dfu-util` (`POST /usb/dfu`):
  `list()` parses `dfu-util -l` into typed `DfuDevice` records;
  `download(firmware, DfuOptions)` uploads the image (base64) and flashes
  it on the box with optional `-d vid:pid`, `-S serial`, `-a alt`,
  `-s` DfuSe address, and `-R` reset; `detach(opts)` maps to `dfu-util -e`.
  Together with `usb_devices()` this removes the last reasons to keep a
  host-side Python USB transport next to the crate. Requires box >= 0.33.0
  and `dfu-util` installed (`lager box-config apt add dfu-util`).

- **Box lock / reservation API** — `lock(user)` (eternal, like `lager
  boxes lock`), `lock_with(user, holder_type, ttl_seconds)`,
  `lock_heartbeat(user)`, `unlock(user)` / `unlock_force(user)`, and
  `lock_status()`, against the box's existing `/lock` endpoints on port
  9000; plus a blocking-client `lock_guard(user)` RAII wrapper that
  releases on drop. TTL locks auto-expire when heartbeats stop.

- **`Uart::try_read()`** — non-blocking drain of everything already
  received. `read(timeout)` is now documented to wait out the full timeout
  when the device is idle; poll loops should use `try_read`.

### Changed

- **`UsbPort::state()` on pre-0.29.0 boxes** now fails with
  `Error::UnsupportedByBox` naming the required box version, instead of a
  generic HTTP 400 box error (those images don't know the `state` action).

- **`DebugNet` caches the resolved net record** after the first operation
  (shared across clones, invalidated when an operation fails), so
  back-to-back debug ops no longer re-fetch `/nets/list` per call.

- README gained a "Minimum box version" matrix covering `state()`,
  `usb_devices()`, `dfu()`, and the lock API.

## [0.2.0] - 2026-07-22

### Added

- **Gateway auth for boxes behind an authenticating reverse proxy.** Such a
  box rejects unauthenticated traffic with 401 and an `X-Gateway-Auth-Url`
  discovery header. The crate now speaks the same contract as the Lager CLI,
  on every transport: port 9000 commands, the port 8765 debug service, RTT
  streaming, and the UART Socket.IO handshake. Plain boxes are unaffected:
  no header is sent and no auth code runs.

- **CLI session reuse.** After `lager login <auth_url>`, the crate reads the
  CLI's token store (`~/.lager_gateway_auth`, or `LAGER_GATEWAY_AUTH_FILE`),
  attaches `Authorization: Bearer`, and refreshes expired access tokens. On
  first contact it learns the box's auth server from the discovery header and
  retries the denied request within the same call.

- **Pinned tokens for CI:** `LagerBox::builder(host).bearer_token(...)`, or
  the `LAGER_GATEWAY_TOKEN` environment variable, for machines with no CLI
  login.

- **`Error::AuthRequired`** names the auth server and the command to run.
  A gateway 403 or 503 maps to an error that says what to do.

### Fixed

- Builds with `--no-default-features --features async`: doctests that use
  the blocking client are gated, and the `power_cycle` example declares
  `required-features = ["blocking"]`.

## [0.1.0] - 2026-07-14

Initial version. It was never published to crates.io: 0.2.0 is the first
`lager-net` release there.

### Added

- **`LagerBox` and `AsyncLagerBox`**, blocking (`ureq`) and async (`reqwest`)
  clients of the box's HTTP API on port 9000, sharing one wire layer.
- **Net types:** power supplies, batteries, e-loads, solar simulators, GPIO,
  ADC, DAC, thermocouples, watt and energy meters, SPI, I2C, USB hub ports,
  robot arms, webcams, routers, and streaming UART (`uart` feature).
- **Debug-probe nets** (flash, erase, reset, memory reads, RTT) through the
  box's debug service on port 8765.
- **Box-level capabilities:** the box's BLE adapter, WiFi interface, and BluFi
  ESP32 provisioning.
