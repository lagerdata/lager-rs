# Box endpoints this crate still needs

This crate targets the Lager box HTTP API on port 9000 for instrument nets
(`/net/command`, `/supply/command`, `/battery/command`, `/usb/command`,
`/ble/command`, `/wifi/command`, `/blufi/command`, `/nets/list`, plus the
`/uart` Socket.IO namespace), and the box **debug service on port 8765** for
debug-probe nets.

Only the net types below remain unreachable over an HTTP/JSON API, so the
crate ships them as documented stubs whose methods return
`Error::NotSupportedByBox`. This file is the work list for closing those gaps
box-side.

(Debug-probe nets are already covered — `DebugNet` talks to the box debug
service on port 8765. Arm, webcam, router, and solar nets are covered as
`/net/command` roles, and the box-level BLE / WiFi / BluFi capabilities have
dedicated `POST /{ble,wifi,blufi}/command` endpoints — see the README and the
`nets::{arm,webcam,router,solar,ble,wifi,blufi}` docs.

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
`ConnectOptions`).)

## Halt-in-place (`DebugNet::halt()`)

Today: box 0.43.0's `DebugNet.halt()` — OpenOCD's bare `halt`, stopping the
core without pulsing nRESET, which matters on parts executing in place out
of QSPI — exists only in the on-box `lager python` API, where it drives the
OpenOCD TCL RPC directly. The debug service on port 8765 exposes no
`/debug/halt` endpoint, so no HTTP client can reach it.

Needed: a `POST /debug/halt` route on the debug service (OpenOCD only;
J-Link has no halt-in-place primitive and the service should answer with
the same "use a halt-first .JLinkScript" guidance the Python API gives).
The crate would then add `DebugNet::halt()` next to `reset()`.

## Oscilloscope / logic analyzer (`Scope` stub)

Today: `lager scope` / `lager logic` run over the legacy `:5000` exec path
and the dedicated oscilloscope streaming daemon (ports 8082-8085).

Needed (minimum useful subset for cargo tests):

- `analog` / `logic` roles in `/net/command`'s `ROLE_ACTIONS` covering:
  trigger config, single capture (returning the trace as JSON), and scalar
  measurements (vpp, frequency, ...).
- Streaming capture can stay on the dedicated daemon; the crate would add a
  feature-gated client later if needed.

## Not planned as stubs (out of crate scope for now)

These Python-API workflows are provisioning/utility flows rather than
test-time net access, and were left out of the crate entirely rather than
stubbed:

- `Rotation` / `Actuate` nets
- net CRUD (`PUT/DELETE :9000/nets/...`) — the box already serves these;
  they can be added to the crate quickly if test suites need to manage nets
  programmatically. (The safety-limits corner of this surface —
  `PUT /nets/<name>/safety-limits` — is served since crate 0.4.) FTDI
  per-net channel selection (`params.interface`, box 0.43.0) falls in the
  same bucket: it is saved-net configuration written at net-add time, and
  the crate's `nets()` read-back already carries it inside `params`.
- `lager python` remote-script execution and job control (submit / detach /
  reattach / kill) — the crate exists to replace those box-side Python
  scripts with Rust running on the host, so driving them is a non-goal.
