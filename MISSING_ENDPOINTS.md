# Box endpoints this crate still needs

This crate targets the Lager box HTTP API on port 9000 for instrument nets
(`/net/command`, `/supply/command`, `/battery/command`, `/usb/command`,
`/ble/command`, `/wifi/command`, `/blufi/command`, `/nets/list`, plus the
`/uart` Socket.IO namespace), and the box **debug service on port 8765** for
debug-probe nets.

This file is the work list for the test-time surface the crate cannot reach
yet: what the box does not serve over HTTP, and what it serves that the
crate has not picked up. No net type is a stub any more; the last one,
`Scope`, became a real net in crate 0.8.

## Closed

Closed by box 0.33.0 + crate 0.3: generic USB bus enumeration
(`GET /usb/devices` → `lager.usb_devices()`), box-side USB-DFU flashing
(`POST /usb/dfu` → `lager.dfu()`), and the box lock/reservation API
(`/lock`, `/lock/heartbeat`, `/unlock` → `lager.lock()` and friends).

Closed by box 0.35.0 + crate 0.4: per-net safety limits
(`PUT /nets/<name>/safety-limits` → `lager.set_safety_limits()` and
friends) and bi-directional RTT (the Socket.IO `/rtt` namespace →
`debug.rtt_interactive()`, feature `rtt`).

Closed by crate 0.5 (catch-up through box 0.43.0): USB port power-cycling
and recovery (`cycle`/`recover` on `/usb/command`, box 0.39.0 →
`usb.cycle()` / `recover()`), live net state (`GET /nets/state`, box
0.34.0 → `lager.nets_state()`), and per-connection debug scripts
(`jlink_script` / `openocd_config` on `/debug/connect` →
`ConnectOptions`).

Closed by crate 0.8 (catch-up through box 0.60.0):

- Oscilloscope nets: the `scope` / `scope-channel` roles on `/net/command`
  (box 0.55.0) → `lager.scope()`, with channel setup, timebase, trigger,
  acquisition control and measurements.
- YubiHSM 2 nets: the `hsm` role (box 0.59.0) → `lager.hsm()`.
- Custom-driver nets: any role a driver installed on the box declares (box
  0.59.0) → `lager.custom()`.
- Halt-in-place: `POST /debug/halt` (box 0.60.0), OpenOCD's bare `halt`
  with no nRESET pulse → `debug.halt()`, gated on the debug service's
  `"halt"` feature. J-Link has no halt-in-place primitive; the box answers
  400 pointing at a halt-first `.JLinkScript`.
- Debug service: partial erase (`erase_start` / `erase_size`, box 0.50.0)
  → `erase_range()`; flash and erase verdicts (box 0.51.0) →
  `FlashReport` / `EraseReport` and `Error::FlashFailed`; probe-busy (503)
  and safety refusals (403) → `Error::ProbeBusy` / `Error::SafetyRefused`;
  probed status (box 0.44.0), port overrides on connect, memory reads
  without a reset, and the RTT chunk size.
- Webcam `snapshot` (box 0.47.0) and the stream starter fields; arm
  `get_acceleration` (box 0.60.0); USB `cycle` outcomes (box 0.49.0);
  SPI/I2C achieved clock and warnings (box 0.47.0 / 0.49.0);
  `NetState::enabled` (box 0.53.0); the newer `/status` capability flags.

## Open

### Scope trace capture

Today: the waveform itself streams from the box's oscilloscope daemon to
the `/scope` page in the browser. `/net/command` returns settings and
measurements, never samples.

Needed: an HTTP or WebSocket endpoint that returns one capture's samples
(time base, per-channel arrays, trigger position), so a test can assert on
the trace and not only on scalar measurements.

### Logic analyzer nets

Today: `lager logic` still runs over the `:5000` Python-exec path; there is
no `logic` role on `/net/command`.

Needed: a `logic` role covering at least channel setup, trigger, a single
capture and protocol decode results.

## Not planned (out of crate scope for now)

These are provisioning, configuration or UI flows rather than test-time
net access, and are left out of the crate rather than stubbed:

- `Rotation` / `Actuate` nets
- net CRUD (`PUT/DELETE :9000/nets/...`), net and box metadata sync,
  `/drivers` (installing custom drivers), `/binaries`, `/bench`,
  custom-device definitions, `/instruments/list` and `/diagnose/*`. The box
  already serves these; they can be added to the crate if test suites need
  to manage a box programmatically. (The safety-limits corner of net
  config, `PUT /nets/<name>/safety-limits`, is served since crate 0.4.)
  FTDI per-net channel selection (`params.interface`, box 0.43.0) falls in
  the same bucket: it is saved-net configuration written at net-add time,
  and the crate's `nets()` read-back already carries it inside `params`.
- Instrument configuration that a test does not change between runs:
  battery model files (`list_models` / `export_model` / `create_model`) and
  supply `set_mode`. `Router::command` and `Scope::command` reach actions of
  their roles the typed API leaves out.
- The `/supply` and `/battery` Socket.IO monitors, the webcam per-stream
  `/api/*` servers, UART session management, and `GET /nets/state?stream=1`
  (NDJSON). `nets_state()` covers the one-shot read.
- `lager python` remote-script execution and job control (submit / detach /
  reattach / kill) — the crate exists to replace those box-side Python
  scripts with Rust running on the host, so driving them is a non-goal.
