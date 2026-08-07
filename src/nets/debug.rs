//! Debug-probe nets (J-Link / OpenOCD): flash, erase, reset, memory reads,
//! and RTT log streaming.
//!
//! Unlike the instrument nets, debug talks to the box's dedicated **debug
//! service on port 8765** (published on the box host), not the port-9000
//! server. A [`crate::LagerBox`] transparently reaches both: the debug
//! service lives on the same host, and the crate resolves the debug net's
//! full saved record from `:9000/nets/list` to hand to the service.
//!
//! ```no_run
//! # #[cfg(feature = "blocking")]
//! # fn demo() -> lager::Result<()> {
//! use lager::LagerBox;
//!
//! let lager = LagerBox::from_env()?;
//! let debug = lager.debug("debug1");
//!
//! debug.connect()?;
//! debug.erase()?;
//! debug.flash("firmware.hex")?;
//! debug.reset(false)?;
//! let vector_table = debug.read_memory(0x0800_0000, 16)?;
//! println!("{vector_table:02x?}");
//! # Ok(())
//! # }
//! # fn main() {}
//! ```

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::wire::{self, DebugConnection, DebugInfo, DebugStatus, Timeout};

/// Base64-encode bytes with the standard alphabet (with padding). Kept
/// in-crate so firmware upload adds no extra dependency (also used by
/// [`crate::nets::dfu`]).
pub(crate) fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18 & 0x3F) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6 & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Firmware image kind, inferred from the file extension by
/// [`DebugNet::flash`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareKind {
    /// Intel HEX (`.hex`).
    Hex,
    /// ELF (`.elf`).
    Elf,
    /// Raw binary (`.bin`), flashed at a base address.
    Bin,
}

impl FirmwareKind {
    fn from_path(path: &Path) -> Result<Self> {
        match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase) {
            Some(ext) if ext == "hex" => Ok(FirmwareKind::Hex),
            Some(ext) if ext == "elf" => Ok(FirmwareKind::Elf),
            Some(ext) if ext == "bin" => Ok(FirmwareKind::Bin),
            _ => Err(Error::Config(format!(
                "cannot infer firmware type from '{}'; use flash_with to set it explicitly",
                path.display()
            ))),
        }
    }
}

/// Options for [`DebugNet::connect_with`].
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// SWD/JTAG speed (e.g. `"4000"` kHz or `"adaptive"`). `None` uses the
    /// service default (`adaptive`).
    pub speed: Option<String>,
    /// Force a fresh backend start even if one is already running.
    pub force: bool,
    /// Halt the target immediately after connecting.
    pub halt: bool,
    /// Start a GDB server (needed for reset/read_memory on some backends).
    pub gdb: bool,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        ConnectOptions {
            speed: None,
            force: false,
            halt: false,
            gdb: true,
        }
    }
}

/// Default per-operation timeouts, matching the Python `DebugServiceClient`.
const CONNECT_TIMEOUT: Timeout = Timeout::After(Duration::from_secs(30));
const FLASH_TIMEOUT: Timeout = Timeout::After(Duration::from_secs(180));
const ERASE_TIMEOUT: Timeout = Timeout::After(Duration::from_secs(120));
const RESET_TIMEOUT: Timeout = Timeout::After(Duration::from_secs(10));
const MEMRD_TIMEOUT: Timeout = Timeout::After(Duration::from_secs(30));
const QUICK_TIMEOUT: Timeout = Timeout::After(Duration::from_secs(10));

/// Options for [`DebugNet::rtt`] / RTT streaming (one-way and interactive).
#[derive(Debug, Clone, Copy, Default)]
pub struct RttOptions {
    /// RTT channel (0 or 1).
    pub channel: u32,
    /// RAM start address for the RTT control-block search (advanced).
    pub search_addr: Option<u64>,
    /// Size of the RAM region to search, in bytes (advanced).
    pub search_size: Option<u64>,
    /// Read chunk size on the box side (J-Link only, advanced). Only used
    /// by interactive RTT; the one-way HTTP stream ignores it.
    pub chunk_size: Option<u64>,
}

/// Build the JSON body for a debug op: the full net record plus extra params.
pub(crate) fn debug_body(net_record: &Value, extra: Value) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("net".to_string(), net_record.clone());
    if let Value::Object(map) = extra {
        obj.extend(map);
    }
    Value::Object(obj)
}

/// Shared request-body builders, used by both the blocking and async handles
/// so the two cannot diverge.
pub(crate) mod ops {
    use super::*;

    pub(crate) fn connect(net: &Value, opts: &ConnectOptions) -> (String, Value, Timeout) {
        let mut extra = json!({
            "force": opts.force,
            "halt": opts.halt,
            "gdb": opts.gdb,
        });
        if let Some(speed) = &opts.speed {
            extra["speed"] = json!(speed);
        }
        ("/debug/connect".into(), debug_body(net, extra), CONNECT_TIMEOUT)
    }

    pub(crate) fn disconnect(net: &Value, keep_running: bool) -> (String, Value, Timeout) {
        (
            "/debug/disconnect".into(),
            debug_body(net, json!({ "keep_jlink_running": keep_running })),
            QUICK_TIMEOUT,
        )
    }

    pub(crate) fn reset(net: &Value, halt: bool) -> (String, Value, Timeout) {
        (
            "/debug/reset".into(),
            debug_body(net, json!({ "halt": halt })),
            RESET_TIMEOUT,
        )
    }

    pub(crate) fn erase(net: &Value) -> (String, Value, Timeout) {
        ("/debug/erase".into(), debug_body(net, json!({})), ERASE_TIMEOUT)
    }

    pub(crate) fn read_memory(net: &Value, address: u64, length: usize) -> (String, Value, Timeout) {
        (
            "/debug/memrd".into(),
            debug_body(net, json!({ "start_addr": address, "length": length })),
            MEMRD_TIMEOUT,
        )
    }

    pub(crate) fn info(net: &Value) -> (String, Value, Timeout) {
        ("/debug/info".into(), debug_body(net, json!({})), QUICK_TIMEOUT)
    }

    pub(crate) fn status(net: &Value) -> (String, Value, Timeout) {
        ("/debug/status".into(), debug_body(net, json!({})), QUICK_TIMEOUT)
    }

    /// Build the flash body. `kind` picks the payload field; `address` is
    /// only used for [`FirmwareKind::Bin`].
    pub(crate) fn flash(
        net: &Value,
        contents: &[u8],
        kind: FirmwareKind,
        address: Option<u32>,
    ) -> (String, Value, Timeout) {
        let b64 = base64_encode(contents);
        let payload = match kind {
            FirmwareKind::Hex => json!({ "hexfile": { "content": b64 } }),
            FirmwareKind::Elf => json!({ "elffile": { "content": b64 } }),
            FirmwareKind::Bin => json!({
                "binfile": { "content": b64, "address": address.unwrap_or(0x0800_0000) }
            }),
        };
        ("/debug/flash".into(), debug_body(net, payload), FLASH_TIMEOUT)
    }

    /// Build the RTT body. RTT streaming is blocking-only (the async client
    /// exposes no `rtt()`), so this is unused in async-only builds.
    #[cfg(feature = "blocking")]
    pub(crate) fn rtt_body(net: &Value, opts: &RttOptions) -> Value {
        let mut extra = json!({ "channel": opts.channel, "timeout": Value::Null });
        if let Some(a) = opts.search_addr {
            extra["search_addr"] = json!(a);
        }
        if let Some(s) = opts.search_size {
            extra["search_size"] = json!(s);
        }
        debug_body(net, extra)
    }
}

/// Find the saved record for a debug net by name in a raw nets list.
pub(crate) fn find_debug_record(records: Vec<Value>, name: &str) -> Result<Value> {
    let mut wrong_role = false;
    for rec in records {
        if rec.get("name").and_then(Value::as_str) == Some(name) {
            if rec.get("role").and_then(Value::as_str) == Some("debug") {
                return Ok(rec);
            }
            wrong_role = true;
        }
    }
    Err(Error::Box {
        status: 404,
        message: if wrong_role {
            format!("net '{name}' exists but is not a debug net")
        } else {
            format!("debug net '{name}' not found on this box")
        },
    })
}

/// Read a firmware file and infer its kind from the extension.
pub(crate) fn read_firmware(path: &Path) -> Result<(Vec<u8>, FirmwareKind)> {
    let kind = FirmwareKind::from_path(path)?;
    let bytes = std::fs::read(path)
        .map_err(|e| Error::Config(format!("cannot read firmware '{}': {e}", path.display())))?;
    Ok((bytes, kind))
}

// ---------------------------------------------------------------------------
// Blocking handle
// ---------------------------------------------------------------------------

/// Handle for a debug-probe net (blocking).
///
/// Created via [`crate::LagerBox::debug`]. Cheap to construct; the net's
/// saved record is fetched from the box on first use and cached on the
/// handle (clones share the cache), so back-to-back debug ops pay for
/// `/nets/list` once instead of per call. The cache is invalidated when an
/// operation fails, so a re-saved net record is picked up on retry.
#[cfg(feature = "blocking")]
#[derive(Clone)]
pub struct DebugNet<'a> {
    pub(crate) client: &'a crate::client::LagerBox,
    pub(crate) name: String,
    pub(crate) record: Arc<Mutex<Option<Value>>>,
}

#[cfg(feature = "blocking")]
impl DebugNet<'_> {
    /// Name of the net this handle drives.
    pub fn name(&self) -> &str {
        &self.name
    }

    fn net_record(&self) -> Result<Value> {
        if let Some(rec) = self.record.lock().unwrap().clone() {
            return Ok(rec);
        }
        let rec = self.client.debug_net_record(&self.name)?;
        *self.record.lock().unwrap() = Some(rec.clone());
        Ok(rec)
    }

    fn call(&self, path: &str, body: Value, timeout: Timeout) -> Result<Value> {
        let req = wire::debug_request(path, body, timeout);
        let result = self
            .client
            .execute_debug(&req)
            .and_then(|(status, resp)| wire::parse_debug(status, resp));
        if result.is_err() {
            // The failure may be a stale record (net re-saved, probe
            // reassigned); re-resolve on the next call.
            *self.record.lock().unwrap() = None;
        }
        result
    }

    /// Connect to the probe with default options (starts a GDB server).
    pub fn connect(&self) -> Result<DebugConnection> {
        self.connect_with(&ConnectOptions::default())
    }

    /// Connect to the probe with explicit options.
    pub fn connect_with(&self, opts: &ConnectOptions) -> Result<DebugConnection> {
        let net = self.net_record()?;
        let (path, body, timeout) = ops::connect(&net, opts);
        let resp = self.call(&path, body, timeout)?;
        serde_json::from_value(resp).map_err(Into::into)
    }

    /// Disconnect. If `keep_running` is true the gdbserver is left running so
    /// an external GDB client can stay attached.
    pub fn disconnect(&self, keep_running: bool) -> Result<()> {
        let net = self.net_record()?;
        let (path, body, timeout) = ops::disconnect(&net, keep_running);
        self.call(&path, body, timeout).map(|_| ())
    }

    /// Reset the target, optionally halting at the reset vector.
    pub fn reset(&self, halt: bool) -> Result<()> {
        let net = self.net_record()?;
        let (path, body, timeout) = ops::reset(&net, halt);
        self.call(&path, body, timeout).map(|_| ())
    }

    /// Mass-erase the target flash.
    pub fn erase(&self) -> Result<()> {
        let net = self.net_record()?;
        let (path, body, timeout) = ops::erase(&net);
        self.call(&path, body, timeout).map(|_| ())
    }

    /// Flash a firmware file, inferring the type from its extension
    /// (`.hex`, `.elf`, `.bin`). `.bin` is flashed at `0x08000000`; use
    /// [`DebugNet::flash_bin`] to choose the address.
    pub fn flash(&self, firmware_path: impl AsRef<Path>) -> Result<()> {
        let path = firmware_path.as_ref();
        let (contents, kind) = read_firmware(path)?;
        self.flash_bytes(&contents, kind, None)
    }

    /// Flash a raw binary at an explicit base address.
    pub fn flash_bin(&self, firmware_path: impl AsRef<Path>, address: u32) -> Result<()> {
        let contents = std::fs::read(firmware_path.as_ref())
            .map_err(|e| Error::Config(format!("cannot read firmware: {e}")))?;
        self.flash_bytes(&contents, FirmwareKind::Bin, Some(address))
    }

    /// Flash raw firmware bytes of a known kind.
    pub fn flash_bytes(
        &self,
        contents: &[u8],
        kind: FirmwareKind,
        address: Option<u32>,
    ) -> Result<()> {
        let net = self.net_record()?;
        let (path, body, timeout) = ops::flash(&net, contents, kind, address);
        self.call(&path, body, timeout).map(|_| ())
    }

    /// Read `length` bytes of target memory starting at `address`.
    pub fn read_memory(&self, address: u64, length: usize) -> Result<Vec<u8>> {
        let net = self.net_record()?;
        let (path, body, timeout) = ops::read_memory(&net, address, length);
        let resp = self.call(&path, body, timeout)?;
        wire::debug_memory_bytes(&resp)
    }

    /// Probe/target information (device, arch, backend, connected state).
    pub fn info(&self) -> Result<DebugInfo> {
        let net = self.net_record()?;
        let (path, body, timeout) = ops::info(&net);
        let resp = self.call(&path, body, timeout)?;
        serde_json::from_value(resp).map_err(Into::into)
    }

    /// Whether a gdbserver/daemon is currently running for this probe.
    pub fn status(&self) -> Result<DebugStatus> {
        let net = self.net_record()?;
        let (path, body, timeout) = ops::status(&net);
        let resp = self.call(&path, body, timeout)?;
        serde_json::from_value(resp).map_err(Into::into)
    }

    /// Open an RTT log stream on channel 0.
    ///
    /// Returns a reader that yields the target's RTT output as raw bytes
    /// until the connection closes or the reader is dropped. Wrap it in a
    /// [`std::io::BufReader`] to read lines.
    pub fn rtt(&self) -> Result<RttStream> {
        self.rtt_with(&RttOptions::default())
    }

    /// Open an RTT log stream with explicit options.
    pub fn rtt_with(&self, opts: &RttOptions) -> Result<RttStream> {
        let net = self.net_record()?;
        let body = ops::rtt_body(&net, opts);
        let req = wire::debug_request("/debug/rtt", body, Timeout::Unbounded);
        let reader = self.client.stream_debug(&req)?;
        Ok(RttStream { reader })
    }

    /// Open a bi-directional RTT session on channel 0 (feature `rtt`,
    /// box software >= 0.35.0).
    ///
    /// Unlike [`DebugNet::rtt`], the returned session can also **write** to
    /// the target's RTT down-channel, so firmware that reads commands over
    /// RTT can be driven from a test:
    ///
    /// ```no_run
    /// # #[cfg(all(feature = "blocking", feature = "rtt"))]
    /// # fn demo() -> lager::Result<()> {
    /// use std::time::Duration;
    /// use lager::LagerBox;
    ///
    /// let lager = LagerBox::from_env()?;
    /// let debug = lager.debug("debug1");
    /// debug.connect()?;                       // gdbserver must be up first
    ///
    /// let mut rtt = debug.rtt_interactive()?;
    /// rtt.write_str("self_test\n")?;
    /// rtt.wait_for(b"self_test: pass", Duration::from_secs(5))?;
    /// # Ok(())
    /// # }
    /// # fn main() {}
    /// ```
    ///
    /// Two prerequisites (see [`crate::nets::rtt`] for the full story): the
    /// gdbserver must already be running — call [`DebugNet::connect`] first —
    /// and writing needs a firmware-declared RTT **down** buffer on the
    /// channel (`defmt-rtt` alone only provides the up buffer; without one
    /// the target silently discards writes).
    #[cfg(feature = "rtt")]
    pub fn rtt_interactive(&self) -> Result<crate::nets::rtt::RttSession> {
        self.rtt_interactive_with(&RttOptions::default())
    }

    /// Open a bi-directional RTT session with explicit options
    /// (feature `rtt`). `opts.channel` selects the RTT channel in both
    /// directions.
    #[cfg(feature = "rtt")]
    pub fn rtt_interactive_with(&self, opts: &RttOptions) -> Result<crate::nets::rtt::RttSession> {
        crate::nets::rtt::RttSession::open(
            self.client.base_url(),
            self.name.clone(),
            opts,
            self.client.current_token(),
        )
    }
}

/// A live RTT byte stream (blocking). Implements [`std::io::Read`], so it can
/// be wrapped in a [`std::io::BufReader`] and read line by line.
#[cfg(feature = "blocking")]
pub struct RttStream {
    reader: Box<dyn std::io::Read + Send + Sync>,
}

#[cfg(feature = "blocking")]
impl std::io::Read for RttStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buf)
    }
}

// ---------------------------------------------------------------------------
// Async handle
// ---------------------------------------------------------------------------

/// Handle for a debug-probe net (async).
///
/// The net's saved record is cached after first use exactly like the
/// blocking [`DebugNet`] (invalidated when an operation fails).
///
/// RTT streaming is not provided on the async client yet; use the blocking
/// [`DebugNet::rtt`] for log streaming.
#[cfg(feature = "async")]
#[derive(Clone)]
pub struct AsyncDebugNet<'a> {
    pub(crate) client: &'a crate::async_client::AsyncLagerBox,
    pub(crate) name: String,
    pub(crate) record: Arc<Mutex<Option<Value>>>,
}

#[cfg(feature = "async")]
impl AsyncDebugNet<'_> {
    /// Name of the net this handle drives.
    pub fn name(&self) -> &str {
        &self.name
    }

    async fn net_record(&self) -> Result<Value> {
        if let Some(rec) = self.record.lock().unwrap().clone() {
            return Ok(rec);
        }
        let rec = self.client.debug_net_record(&self.name).await?;
        *self.record.lock().unwrap() = Some(rec.clone());
        Ok(rec)
    }

    async fn call(&self, path: &str, body: Value, timeout: Timeout) -> Result<Value> {
        let req = wire::debug_request(path, body, timeout);
        let result = match self.client.execute_debug(&req).await {
            Ok((status, resp)) => wire::parse_debug(status, resp),
            Err(e) => Err(e),
        };
        if result.is_err() {
            // The failure may be a stale record (net re-saved, probe
            // reassigned); re-resolve on the next call.
            *self.record.lock().unwrap() = None;
        }
        result
    }

    /// Connect to the probe with default options (starts a GDB server).
    pub async fn connect(&self) -> Result<DebugConnection> {
        self.connect_with(&ConnectOptions::default()).await
    }

    /// Connect to the probe with explicit options.
    pub async fn connect_with(&self, opts: &ConnectOptions) -> Result<DebugConnection> {
        let net = self.net_record().await?;
        let (path, body, timeout) = ops::connect(&net, opts);
        let resp = self.call(&path, body, timeout).await?;
        serde_json::from_value(resp).map_err(Into::into)
    }

    /// Disconnect. If `keep_running` is true the gdbserver is left running.
    pub async fn disconnect(&self, keep_running: bool) -> Result<()> {
        let net = self.net_record().await?;
        let (path, body, timeout) = ops::disconnect(&net, keep_running);
        self.call(&path, body, timeout).await.map(|_| ())
    }

    /// Reset the target, optionally halting at the reset vector.
    pub async fn reset(&self, halt: bool) -> Result<()> {
        let net = self.net_record().await?;
        let (path, body, timeout) = ops::reset(&net, halt);
        self.call(&path, body, timeout).await.map(|_| ())
    }

    /// Mass-erase the target flash.
    pub async fn erase(&self) -> Result<()> {
        let net = self.net_record().await?;
        let (path, body, timeout) = ops::erase(&net);
        self.call(&path, body, timeout).await.map(|_| ())
    }

    /// Flash a firmware file, inferring type from extension.
    pub async fn flash(&self, firmware_path: impl AsRef<Path>) -> Result<()> {
        let (contents, kind) = read_firmware(firmware_path.as_ref())?;
        self.flash_bytes(&contents, kind, None).await
    }

    /// Flash a raw binary at an explicit base address.
    pub async fn flash_bin(&self, firmware_path: impl AsRef<Path>, address: u32) -> Result<()> {
        let contents = std::fs::read(firmware_path.as_ref())
            .map_err(|e| Error::Config(format!("cannot read firmware: {e}")))?;
        self.flash_bytes(&contents, FirmwareKind::Bin, Some(address)).await
    }

    /// Flash raw firmware bytes of a known kind.
    pub async fn flash_bytes(
        &self,
        contents: &[u8],
        kind: FirmwareKind,
        address: Option<u32>,
    ) -> Result<()> {
        let net = self.net_record().await?;
        let (path, body, timeout) = ops::flash(&net, contents, kind, address);
        self.call(&path, body, timeout).await.map(|_| ())
    }

    /// Read `length` bytes of target memory starting at `address`.
    pub async fn read_memory(&self, address: u64, length: usize) -> Result<Vec<u8>> {
        let net = self.net_record().await?;
        let (path, body, timeout) = ops::read_memory(&net, address, length);
        let resp = self.call(&path, body, timeout).await?;
        wire::debug_memory_bytes(&resp)
    }

    /// Probe/target information.
    pub async fn info(&self) -> Result<DebugInfo> {
        let net = self.net_record().await?;
        let (path, body, timeout) = ops::info(&net);
        let resp = self.call(&path, body, timeout).await?;
        serde_json::from_value(resp).map_err(Into::into)
    }

    /// Whether a gdbserver/daemon is currently running for this probe.
    pub async fn status(&self) -> Result<DebugStatus> {
        let net = self.net_record().await?;
        let (path, body, timeout) = ops::status(&net);
        let resp = self.call(&path, body, timeout).await?;
        serde_json::from_value(resp).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_reference() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(&[0x00, 0xff, 0x10]), "AP8Q");
    }

    #[test]
    fn firmware_kind_from_extension() {
        assert_eq!(
            FirmwareKind::from_path(Path::new("a/b/fw.hex")).unwrap(),
            FirmwareKind::Hex
        );
        assert_eq!(
            FirmwareKind::from_path(Path::new("FW.ELF")).unwrap(),
            FirmwareKind::Elf
        );
        assert!(FirmwareKind::from_path(Path::new("fw.txt")).is_err());
    }

    #[test]
    fn debug_body_wraps_net_and_params() {
        let net = json!({"name": "debug1", "role": "debug", "pin": "nrf52"});
        let body = debug_body(&net, json!({"halt": true}));
        assert_eq!(body["net"]["name"], "debug1");
        assert_eq!(body["halt"], true);
    }
}
