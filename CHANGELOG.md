# Changelog

All notable changes to the `lager-net` crate are documented here.

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
