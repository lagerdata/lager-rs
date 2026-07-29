# Changelog

All notable changes to the `lager-net` crate are documented here.

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
