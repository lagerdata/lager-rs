# Box endpoints this crate still needs

This crate targets the Lager box HTTP API on port 9000 for instrument nets
(`/net/command`, `/supply/command`, `/battery/command`, `/usb/command`,
`/ble/command`, `/wifi/command`, `/blufi/command`, `/nets/list`, the `/uart`
and `/rtt` Socket.IO namespaces and the `/scope/<net>/ws` capture stream),
and the box **debug service on port 8765** for debug-probe nets.

This file is the work list for what a Rust test still cannot do that a
Python one on the box can. It has three parts: what is **open** (the box
has no HTTP route for it yet), what the box serves that the crate is
**planned** to pick up, and what is **not planned**, with the reason.

## Open: the box has no HTTP route yet

Each of these exists in the box's Python API but not on its HTTP API, so it
needs a box change first. The crate will pick each one up behind a `/status`
capability flag, so an older box answers `Error::UnsupportedByBox`.

- **Logic analyzer nets.** `lager logic` still runs over the `:5000`
  Python-exec path; there is no `logic` role on `/net/command`. Needed:
  channel setup, trigger, a single capture and protocol decode results.
- **Rotation and actuate nets.** No `rotation` or `actuate` role on
  `/net/command`.
- **E-load enable and disable.** The `eload` role sets the CC/CV/CR/CP
  setpoints and reads state, but cannot switch the input on or off.
- **Battery simulator ESR.** `/battery/command` reads the ESR but has no
  action to set it.
- **BLE GATT.** `/ble/command` covers `scan`, `info`, `connect` and
  `disconnect`; reading, writing and subscribing to characteristics are not
  on it.
- **WiFi disconnect.** `/wifi/command` covers `status`, `scan`, `connect`
  and `delete`.
- **Router client checks.** The router driver can report whether a client
  is connected and set client isolation, but neither is an action of the
  `router` role.
- **Scope single capture without a live view.** A Rigol MSO5000 net with no
  HDMI capture card paired is refused a stream ticket, so `Scope::capture`
  cannot fetch one capture from it, although the scope itself could answer.

## Planned: the box serves it, the crate does not wrap it yet

Box administration and provisioning, to come behind an `admin` feature:

- net CRUD (`PUT/DELETE /nets/<name>`, `DELETE /nets`), net and box metadata
  (`/nets/<name>/metadata`, `/box-metadata`)
- custom drivers (`/drivers`, `/drivers/<type>/deps`), `/binaries` and
  `/download-file`
- `/bench`, custom-device definitions (`/custom-devices`),
  `/instruments/list`, `/diagnose/*` and `/hello`

Until then, `LagerBox::bearer_token()` returns the token to attach to a raw
HTTP call against any of these.

## Not planned

- **GDB, OpenOCD and J-Link sessions beyond flash, erase, reset, halt and
  memory reads.** `LagerBox::debug_tunnel()` hands back a TCP stream to any
  debug port, through the gateway on a gated box, which any GDB
  remote-protocol client can drive. The `debug_tunnel` example reads
  registers and writes memory that way. A typed GDB client is out of scope.
- **The `/supply` and `/battery` Socket.IO monitors**, and
  `GET /nets/state?stream=1` (NDJSON). `Supply::state()`,
  `Battery::state()` and `nets_state()` cover the reads; poll them.
- **The webcam per-stream `/api/*` servers.** `Webcam::url()` gives the
  stream; the per-stream servers are for the box's own viewer page.
- **`lager python` remote-script execution and job control** (submit,
  detach, reattach, kill). The crate exists to replace those box-side Python
  scripts with Rust running on the host, so driving them is a non-goal.
- **Box-internal services:** the hardware service on `:8080` (`/invoke`,
  `/cache/*`) and the `:5000` job runner. They are implementation details
  behind the public API, not an API of their own.

## History

- Box 0.33.0 + crate 0.3: USB bus enumeration (`GET /usb/devices` →
  `usb_devices()`), box-side DFU (`POST /usb/dfu` → `dfu()`), and the box
  lock (`/lock`, `/lock/heartbeat`, `/unlock` → `lock()` and friends).
- Box 0.35.0 + crate 0.4: per-net safety limits
  (`PUT /nets/<name>/safety-limits` → `set_safety_limits()`) and
  bi-directional RTT (the `/rtt` namespace → `rtt_interactive()`, feature
  `rtt`).
- Crate 0.5, through box 0.43.0: USB `cycle` / `recover`, `nets_state()`,
  and per-connection debug scripts on `/debug/connect` (`ConnectOptions`).
- Crate 0.8, through box 0.60.0: scope, YubiHSM and custom-driver nets;
  `debug.halt()`; partial erase; flash and erase verdicts; probe-busy and
  safety refusals; probed status; webcam `snapshot`; arm acceleration; USB
  cycle outcomes; SPI/I2C achieved clock; `NetState::enabled`.
- Crate 0.9, everything the box already served:
  - Power: supply OCP/OVP with a setpoint and `set_mode()`, battery model
    files, e-load setpoints and energy totals as typed fields, solar `"n/a"`
    as `None`.
  - Comms: per-session UART serial settings, UART session listing and
    release, exact-name BLE scans and connect timeouts, WiFi on a chosen
    interface, BluFi name filtering and scan timeouts, the DFU timeout.
  - Debug: flash and erase options (`.JLinkScript`, verbose, evidence,
    speed, transport), the RTT timeout, reset and halt output, and the debug
    service's own health and status.
  - Typed methods for every scope and router action that was reachable only
    through `command()`.
  - Scope trace capture (`scope.capture()` / `scope.stream()`, feature
    `scope-trace`), over the WebSocket behind the box's `/scope` page.
  - Async UART, async interactive RTT and the async RTT log stream.
