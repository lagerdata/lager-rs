//! Box setup and management (feature `admin`): create, change and delete
//! saved nets, edit net and box metadata, list the instruments plugged in,
//! read the bench manifest, install custom drivers, upload binaries,
//! download files scripts leave on the box, assign USB-serial cables to
//! custom devices, and run the box's diagnostics. These are the calls behind
//! `lager nets add`, `lager nets delete`, `lager nets assign`, `lager
//! instruments`, `lager box bench export`, `lager drivers`, `lager binaries`
//! and `lager diagnose`.
//!
//! ```no_run
//! # #[cfg(feature = "blocking")]
//! # fn demo() -> lager::Result<()> {
//! use lager::{LagerBox, NetRecord};
//!
//! let lager = LagerBox::from_env()?;
//! let admin = lager.admin();
//! let _lock = lager.lock_guard("ci")?;
//! admin.save_net(&NetRecord::new("supply1", "power-supply", "Rigol_DP832").pin(1))?;
//! # Ok(())
//! # }
//! # fn main() {}
//! ```
//!
//! The box does not check the box lock on these routes; the `lager` CLI does,
//! before it sends anything. Hold [`crate::LagerBox::lock_guard`] around a
//! change so another user's session cannot interleave with it.
//!
//! A route the box does not serve comes back as
//! [`crate::Error::UnsupportedByBox`], naming the box version that added it
//! where one is known. A refusal the box explains (an unknown net, a bad
//! field) comes back as [`crate::Error::Box`] with the box's message.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::wire::{NetRecord, RawRequest};

/// The fields of a net's metadata (`GET /nets/<name>/metadata`). Fields
/// with no value read as an empty string or list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct NetMetadataFields {
    /// What the net is for.
    #[serde(default)]
    pub purpose: String,
    /// Free-form notes.
    #[serde(default)]
    pub notes: String,
    /// Tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// How the net is wired to the device under test.
    #[serde(default)]
    pub dut_connection: String,
    /// Hints for test authors.
    #[serde(default)]
    pub test_hints: Vec<String>,
}

/// A net's metadata, from [`Admin::net_metadata`] and
/// [`Admin::set_net_metadata`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct NetMetadata {
    /// Net name.
    #[serde(default)]
    pub name: String,
    /// The metadata fields.
    #[serde(default)]
    pub fields: NetMetadataFields,
    /// When each field last changed (ISO 8601), by field name.
    #[serde(default)]
    pub metadata_timestamps: BTreeMap<String, String>,
    /// Fields the bench file (`/etc/lager/bench.json`) overrides for this
    /// net. After a write, only the fields you sent that it overrides.
    #[serde(default)]
    pub shadowed_by_override: Vec<String>,
    /// How many saved records the write changed (a net can have one record
    /// per role). `None` on a read.
    #[serde(default)]
    pub records_updated: Option<u32>,
}

/// A change to a net's metadata, for [`Admin::set_net_metadata`]. A field
/// left alone is not sent and keeps its value; a field cleared with
/// `clear_*` is sent as `null` and removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetMetadataUpdate {
    fields: Map<String, Value>,
    timestamps: Map<String, Value>,
}

impl NetMetadataUpdate {
    /// A change that sets nothing yet.
    pub fn new() -> Self {
        NetMetadataUpdate::default()
    }

    fn set(mut self, key: &str, value: Value) -> Self {
        self.fields.insert(key.to_string(), value);
        self
    }

    fn list(items: impl IntoIterator<Item = impl Into<String>>) -> Value {
        Value::Array(items.into_iter().map(|s| Value::String(s.into())).collect())
    }

    /// Set `purpose`.
    pub fn purpose(self, purpose: impl Into<String>) -> Self {
        self.set("purpose", Value::String(purpose.into()))
    }

    /// Set `notes`.
    pub fn notes(self, notes: impl Into<String>) -> Self {
        self.set("notes", Value::String(notes.into()))
    }

    /// Set `dut_connection`.
    pub fn dut_connection(self, dut_connection: impl Into<String>) -> Self {
        self.set("dut_connection", Value::String(dut_connection.into()))
    }

    /// Replace `tags`.
    pub fn tags(self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.set("tags", Self::list(tags))
    }

    /// Replace `test_hints`.
    pub fn test_hints(self, hints: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.set("test_hints", Self::list(hints))
    }

    /// Remove a field: `"purpose"`, `"notes"`, `"tags"`, `"dut_connection"`
    /// or `"test_hints"`. The box refuses any other name with
    /// [`crate::Error::Box`].
    pub fn clear(self, field: &str) -> Self {
        self.set(field, Value::Null)
    }

    /// Record when a field changed (ISO 8601). The box refuses a timestamp
    /// for a field this update does not also set.
    pub fn timestamp(mut self, field: &str, iso8601: impl Into<String>) -> Self {
        self.timestamps
            .insert(field.to_string(), Value::String(iso8601.into()));
        self
    }

    fn body(&self) -> Value {
        let mut body = Map::new();
        body.insert("fields".to_string(), Value::Object(self.fields.clone()));
        if !self.timestamps.is_empty() {
            body.insert(
                "timestamps".to_string(),
                Value::Object(self.timestamps.clone()),
            );
        }
        Value::Object(body)
    }
}

/// The box's description, from [`Admin::box_metadata`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct BoxMetadata {
    /// Free-form description of the box.
    #[serde(default)]
    pub description: Option<String>,
    /// When the description last changed (ISO 8601).
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// One instrument the box sees, from [`Admin::instruments`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct Instrument {
    /// Instrument name as `lager nets add` takes it, e.g. `"Rigol_DP832"`.
    #[serde(default)]
    pub name: String,
    /// USB vendor ID, hex.
    #[serde(default)]
    pub vid: Option<String>,
    /// USB product ID, hex.
    #[serde(default)]
    pub pid: Option<String>,
    /// Serial number.
    #[serde(default)]
    pub serial: Option<String>,
    /// Address to save on a net (VISA resource, serial port, ...).
    #[serde(default)]
    pub address: Option<String>,
    /// Net roles the instrument can serve.
    #[serde(default)]
    pub net_type: Vec<String>,
    /// Channels per role, when the instrument has several.
    #[serde(default)]
    pub channels: BTreeMap<String, Vec<String>>,
    /// Everything else the scanner reports (model, product, port path, ...).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The bench manifest (`GET /bench`, box >= 0.50.0): the box's whole
/// configuration as one document, what `lager box bench export` writes.
/// The schema belongs to the box, so it is kept as JSON.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct BenchManifest {
    /// The manifest document.
    pub document: Value,
}

impl BenchManifest {
    /// `schema_version`, when present.
    pub fn schema_version(&self) -> Option<&Value> {
        self.document.get("schema_version")
    }

    /// `box_id`, when present.
    pub fn box_id(&self) -> Option<&str> {
        self.document.get("box_id").and_then(Value::as_str)
    }
}

/// A custom driver's Python dependencies, as the box installed them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct DriverDeps {
    /// `"ok"`, `"stale"` (installed for another Python or machine) or
    /// `"missing"`.
    #[serde(default)]
    pub status: String,
    /// Installed packages, as `name==version`.
    #[serde(default)]
    pub packages: Vec<String>,
    /// The Python tag they were installed for, e.g. `"cp312"`.
    #[serde(default)]
    pub python: Option<String>,
    /// The machine they were installed for, e.g. `"x86_64"`.
    #[serde(default)]
    pub machine: Option<String>,
}

/// One installed custom driver, from [`Admin::drivers`] and
/// [`Admin::driver`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct DriverEntry {
    /// Driver type, which is also the net role it serves.
    #[serde(default, rename = "type")]
    pub driver_type: String,
    /// `"ok"`, or `"error"` with [`DriverEntry::error`] saying why.
    #[serde(default)]
    pub status: String,
    /// Version from the driver's manifest.
    #[serde(default)]
    pub version: Option<String>,
    /// Description from the driver's manifest.
    #[serde(default)]
    pub description: String,
    /// SHA-256 of the installed bundle.
    #[serde(default)]
    pub sha256: Option<String>,
    /// Its Python dependencies, or `None` when it declares none.
    #[serde(default)]
    pub deps: Option<DriverDeps>,
    /// Saved nets that use the driver.
    #[serde(default)]
    pub nets: Vec<String>,
    /// The driver's manifest.
    #[serde(default)]
    pub manifest: Option<Value>,
    /// Why the driver failed to load, when `status` is `"error"`.
    #[serde(default)]
    pub error: Option<String>,
}

/// Result of [`Admin::install_driver`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct DriverInstall {
    /// Driver type.
    #[serde(default, rename = "type")]
    pub driver_type: String,
    /// Version from the manifest.
    #[serde(default)]
    pub version: Option<String>,
    /// SHA-256 of the bundle.
    #[serde(default)]
    pub sha256: Option<String>,
    /// Whether it replaced an installed version.
    #[serde(default)]
    pub replaced: bool,
    /// Python packages the box installed for it.
    #[serde(default)]
    pub packages: Vec<String>,
}

/// Result of [`Admin::remove_driver`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct DriverRemoval {
    /// Driver type.
    #[serde(default, rename = "type")]
    pub driver_type: String,
    /// Whether the driver was removed.
    #[serde(default)]
    pub removed: bool,
    /// Saved nets left without their driver (only with `force`).
    #[serde(default)]
    pub orphaned_nets: Vec<String>,
}

/// One file in the box's binaries directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct BinaryEntry {
    /// File name.
    #[serde(default)]
    pub name: String,
    /// Size in bytes.
    #[serde(default)]
    pub size: u64,
    /// Whether it is executable.
    #[serde(default)]
    pub executable: bool,
}

/// The box's binaries directory, from [`Admin::binaries`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct BinaryList {
    /// The files.
    #[serde(default)]
    pub binaries: Vec<BinaryEntry>,
    /// The directory on the box host.
    #[serde(default)]
    pub host_path: String,
    /// The same directory inside the box container, where scripts run.
    #[serde(default)]
    pub container_path: String,
    /// Whether the container has the directory mounted.
    #[serde(default)]
    pub mounted: bool,
}

/// Result of [`Admin::add_binary`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct BinaryAdded {
    /// File name.
    #[serde(default)]
    pub name: String,
    /// Where the box wrote it.
    #[serde(default)]
    pub path: String,
    /// Size in bytes.
    #[serde(default)]
    pub size: u64,
    /// Whether the container needs a restart to see it, which `lager
    /// update` does.
    #[serde(default)]
    pub restart_required: bool,
}

/// A USB-serial cable, named by its serial number or by the USB port it is
/// plugged into, for [`Admin::assign_custom_device`]. Use the port for a
/// cable with no serial, or one sharing its serial with another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CableId {
    /// The cable's USB serial number.
    Serial(String),
    /// The USB port path, e.g. `"1-1.4"`.
    PortPath(String),
}

impl CableId {
    fn body(&self) -> Map<String, Value> {
        let (serial, port_path) = match self {
            CableId::Serial(s) => (Value::String(s.clone()), Value::Null),
            CableId::PortPath(p) => (Value::Null, Value::String(p.clone())),
        };
        let mut body = Map::new();
        body.insert("serial".to_string(), serial);
        body.insert("port_path".to_string(), port_path);
        body
    }
}

/// A device definition the box knows, from [`Admin::custom_devices`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct CatalogDevice {
    /// Instrument name to assign, e.g. `"Acme_Thermal_Chamber"`.
    #[serde(default)]
    pub name: String,
    /// Human-readable name.
    #[serde(default)]
    pub display_name: String,
    /// Manufacturer.
    #[serde(default)]
    pub manufacturer: String,
    /// Net roles it serves.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Channels per role.
    #[serde(default)]
    pub channels: BTreeMap<String, Value>,
    /// How the box talks to it, e.g. `"serial"`.
    #[serde(default)]
    pub transport: String,
    /// Baud rate used when an assignment names none.
    #[serde(default)]
    pub default_baud: Option<u32>,
}

/// A cable assigned to a device definition, from [`Admin::custom_devices`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct DeviceAssignment {
    /// The instrument the cable is assigned to.
    #[serde(default)]
    pub instrument: String,
    /// Cable serial number, when it is assigned by serial.
    #[serde(default)]
    pub serial: Option<String>,
    /// USB port path, when it is assigned by port.
    #[serde(default)]
    pub port_path: Option<String>,
    /// Baud rate.
    #[serde(default)]
    pub baud: Option<u32>,
    /// The address to save on its nets.
    #[serde(default)]
    pub address: Option<String>,
    /// The serial device it is on now, when connected.
    #[serde(default)]
    pub tty: Option<String>,
    /// Everything else the box reports (vid, pid, roles, channels, ...).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// An unassigned USB-serial cable the box sees, from
/// [`Admin::custom_devices`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct SerialCable {
    /// USB vendor ID, hex.
    #[serde(default)]
    pub vid: Option<String>,
    /// USB product ID, hex.
    #[serde(default)]
    pub pid: Option<String>,
    /// Serial number, when it has one.
    #[serde(default)]
    pub serial: Option<String>,
    /// USB port path.
    #[serde(default)]
    pub port_path: Option<String>,
    /// Its serial device, e.g. `"/dev/ttyUSB0"`.
    #[serde(default)]
    pub tty: Option<String>,
    /// Everything else the box reports (manufacturer, product, by-id path,
    /// ...).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The box's custom-device state, from [`Admin::custom_devices`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct CustomDevices {
    /// Device definitions a cable can be assigned to.
    #[serde(default)]
    pub catalog: Vec<CatalogDevice>,
    /// Current assignments.
    #[serde(default)]
    pub assignments: Vec<DeviceAssignment>,
    /// Unassigned cables connected now.
    #[serde(default)]
    pub cables: Vec<SerialCable>,
}

/// Result of [`Admin::assign_custom_device`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct DeviceAssigned {
    /// The stored assignment, with its address and serial device.
    #[serde(flatten)]
    pub assignment: DeviceAssignment,
    /// Saved nets the box deleted because the cable now belongs to the
    /// device: stale nets of an earlier assignment, and generic UART nets on
    /// the same cable.
    #[serde(default)]
    pub deleted_nets: Vec<String>,
}

/// Result of [`Admin::remove_custom_device`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct DeviceRemoval {
    /// Whether an assignment matched and was removed.
    #[serde(default)]
    pub removed: bool,
    /// The instrument it was assigned to.
    #[serde(default)]
    pub instrument: Option<String>,
    /// The address its nets used.
    #[serde(default)]
    pub address: Option<String>,
    /// Saved nets the box deleted with it.
    #[serde(default)]
    pub deleted_nets: Vec<String>,
}

/// What the box sees of a USB instrument, from [`Admin::diagnose_usb`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct UsbDiagnosis {
    /// The VISA address diagnosed.
    #[serde(default)]
    pub address: String,
    /// Whether the device is on the USB bus.
    #[serde(default)]
    pub enumerated: bool,
    /// Whether it is a USBTMC device.
    #[serde(default)]
    pub is_usbtmc: bool,
    /// Its device node, when enumerated.
    #[serde(default)]
    pub device_path: Option<String>,
    /// Everything else (sysfs path, processes holding it, `dmesg` tail,
    /// ...).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What the box sees of a USB hub, from [`Admin::diagnose_usbhub`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct UsbHubDiagnosis {
    /// The address diagnosed.
    #[serde(default)]
    pub address: String,
    /// Whether the hub is on the USB bus.
    #[serde(default)]
    pub enumerated: bool,
    /// Whether the hub's SDK opened it: `None` when the probe was skipped
    /// or the hub has no SDK diagnostics.
    #[serde(default)]
    pub hub_opens: Option<bool>,
    /// Everything else (device numbers, SDK scan, holders, ...).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A VISA identity probe, from [`Admin::diagnose_visa`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct VisaDiagnosis {
    /// The address probed.
    #[serde(default)]
    pub address: String,
    /// The instrument's `*IDN?` reply, when it answered.
    #[serde(default)]
    pub idn: Option<String>,
    /// Why the probe failed, when it did.
    #[serde(default)]
    pub error: Option<String>,
    /// `"busy"`, `"nodev"`, `"timeout"` or `"other"`, with `error`.
    #[serde(default)]
    pub error_class: Option<String>,
    /// Whether the box skipped the probe because its hardware service
    /// already holds a session for the address.
    #[serde(default)]
    pub skipped: bool,
    /// How long the probe took.
    #[serde(default)]
    pub elapsed_ms: Option<f64>,
    /// Everything else (the skip reason, ...).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A debug-probe check, from [`Admin::diagnose_jlink`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct JlinkDiagnosis {
    /// The probe's address.
    #[serde(default)]
    pub address: Option<String>,
    /// The target device.
    #[serde(default)]
    pub device: Option<String>,
    /// `"jlink"` or `"openocd"`.
    #[serde(default)]
    pub backend: Option<String>,
    /// Whether the probe is on the USB bus.
    #[serde(default)]
    pub probe_enumerated: Option<bool>,
    /// Everything else: the GDB server, the J-Link emulator list, the
    /// connect attempt and its output, or the OpenOCD log tail.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl NetRecord {
    /// A net record to save with [`Admin::save_net`]. `instrument` is the
    /// name [`Admin::instruments`] reports, e.g. `"Rigol_DP832"`.
    pub fn new(
        name: impl Into<String>,
        role: impl Into<String>,
        instrument: impl Into<String>,
    ) -> Self {
        NetRecord {
            name: name.into(),
            role: role.into(),
            instrument: Some(instrument.into()),
            ..NetRecord::default()
        }
    }

    /// Set the pin or channel identifier.
    pub fn pin(mut self, pin: impl Into<Value>) -> Self {
        self.pin = Some(pin.into());
        self
    }

    /// Set the VISA or device address.
    pub fn address(mut self, address: impl Into<String>) -> Self {
        self.address = Some(address.into());
        self
    }

    /// Set one role-specific saved parameter.
    pub fn param(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.params
            .get_or_insert_with(Map::new)
            .insert(key.into(), value.into());
        self
    }

    /// Set any other record key, such as `serial` or `channel_key`.
    pub fn field(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }
}

/// One admin request and the parser for its answer. Unlike [`crate::wire::Op`]
/// these routes do not use the command envelope, so the parser sees the
/// status and the body. Bodies travel as bytes, so the same path carries
/// JSON, uploads and file downloads.
pub(crate) struct AdminOp<T> {
    pub(crate) req: RawRequest,
    pub(crate) parse: Parse<T>,
    /// The error for a box that does not serve the route.
    pub(crate) unsupported: &'static str,
}

/// How an [`AdminOp`] reads its answer.
pub(crate) enum Parse<T> {
    /// A JSON answer.
    Json(fn(u16, Value) -> Result<T>),
    /// Raw bytes, such as a downloaded file.
    Bytes(fn(u16, Vec<u8>) -> Result<T>),
}

impl<T> Parse<T> {
    /// Turn a response into the result. A JSON route whose answer is not
    /// JSON fails as the JSON client does: `Error::Decode` on success, and
    /// a non-JSON `Error::Box` on an error status, which
    /// [`crate::wire::is_route_missing`] recognizes.
    pub(crate) fn complete(&self, status: u16, bytes: Vec<u8>) -> Result<T> {
        match self {
            Parse::Bytes(parse) => parse(status, bytes),
            Parse::Json(parse) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(body) => parse(status, body),
                Err(e) if status < 400 => Err(Error::Decode(format!("non-JSON response: {e}"))),
                Err(_) => Err(Error::Box {
                    status,
                    message: format!("HTTP {status} (non-JSON body)"),
                }),
            },
        }
    }
}

/// Map a route the box does not serve to [`Error::UnsupportedByBox`]; see
/// [`crate::wire::is_route_missing`].
pub(crate) fn route_missing(err: Error, unsupported: &'static str) -> Error {
    if crate::wire::is_route_missing(&err) {
        Error::UnsupportedByBox {
            message: unsupported.to_string(),
        }
    } else {
        err
    }
}

pub(crate) mod ops {
    use std::time::Duration;

    use serde::de::DeserializeOwned;
    use serde_json::{json, Value};

    use super::{
        AdminOp, BenchManifest, BinaryAdded, BinaryList, BoxMetadata, CableId, CustomDevices,
        DeviceAssigned, DeviceRemoval, DriverEntry, DriverInstall, DriverRemoval, Instrument,
        JlinkDiagnosis, NetMetadata, NetMetadataUpdate, Parse, UsbDiagnosis, UsbHubDiagnosis,
        VisaDiagnosis,
    };
    use crate::error::{Error, Result};
    use crate::wire::{
        encode_query_value, multipart, path_segment, FormPart, Method, NetRecord, RawBody,
        RawRequest, Timeout,
    };

    const NETS: &str = "this box does not serve net changes over HTTP (PUT/DELETE /nets)";
    const NET_METADATA: &str =
        "this box does not serve /nets/<name>/metadata (requires box software >= 0.46.0)";
    const BOX_METADATA: &str = "this box does not serve /box-metadata";
    const INSTRUMENTS: &str = "this box does not serve /instruments/list";
    const BENCH: &str = "this box does not serve /bench (requires box software >= 0.50.0)";
    const DRIVERS: &str =
        "this box does not serve /drivers (custom drivers require box software >= 0.59.0)";
    const BINARIES: &str = "this box does not serve /binaries";
    const DOWNLOAD: &str = "this box does not serve /download-file";
    const CUSTOM_DEVICES: &str = "this box does not serve /custom-devices";
    const DIAGNOSE: &str = "this box does not serve /diagnose";

    /// The box allows pip 300 s; the CLI waits 360 s.
    const PIP_BUDGET: Duration = Duration::from_secs(360);
    /// The CLI's budget for a binary upload.
    const UPLOAD_BUDGET: Duration = Duration::from_secs(120);

    /// The CLI's budget for these routes.
    fn budget() -> Timeout {
        Timeout::After(Duration::from_secs(30))
    }

    fn req(method: Method, path: String, body: Option<Value>) -> RawRequest {
        RawRequest {
            method,
            path,
            body: body.as_ref().map(RawBody::json),
            timeout: budget(),
        }
    }

    fn upload(method: Method, path: String, body: RawBody, timeout: Duration) -> RawRequest {
        RawRequest {
            method,
            path,
            body: Some(body),
            timeout: Timeout::After(timeout),
        }
    }

    /// The box's own refusal, as `Error::Box` (501 as unsupported).
    pub(crate) fn refusal(status: u16, body: &Value) -> Error {
        let message = body
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {status}"));
        if status == 501 {
            Error::UnsupportedByBox { message }
        } else {
            Error::Box { status, message }
        }
    }

    fn ok(status: u16, body: Value) -> Result<Value> {
        if (200..300).contains(&status) {
            Ok(body)
        } else {
            Err(refusal(status, &body))
        }
    }

    fn parse_unit(status: u16, body: Value) -> Result<()> {
        ok(status, body).map(|_| ())
    }

    fn parse_as<T: DeserializeOwned>(status: u16, body: Value) -> Result<T> {
        serde_json::from_value(ok(status, body)?)
            .map_err(|e| Error::Decode(format!("unexpected response shape: {e}")))
    }

    fn parse_manifest(status: u16, body: Value) -> Result<BenchManifest> {
        let document = ok(status, body)?;
        if !document.is_object() {
            return Err(Error::Decode(
                "bench manifest is not a JSON object".to_string(),
            ));
        }
        Ok(BenchManifest { document })
    }

    /// The record as the CLI sends it: everything, minus the box's derived
    /// `live_path`.
    fn record_body(record: &NetRecord) -> Value {
        let mut body = serde_json::to_value(record).unwrap_or_else(|_| json!({}));
        if let Some(map) = body.as_object_mut() {
            map.remove("live_path");
        }
        body
    }

    pub(crate) fn save_net(record: &NetRecord) -> AdminOp<()> {
        rename_net(&record.name, record)
    }

    pub(crate) fn rename_net(old_name: &str, record: &NetRecord) -> AdminOp<()> {
        AdminOp {
            req: req(
                Method::Put,
                format!("/nets/{}", path_segment(old_name)),
                Some(record_body(record)),
            ),
            parse: Parse::Json(parse_unit),
            unsupported: NETS,
        }
    }

    pub(crate) fn delete_net(name: &str, role: Option<&str>) -> AdminOp<()> {
        let mut path = format!("/nets/{}", path_segment(name));
        if let Some(role) = role {
            path.push_str(&format!("?role={}", encode_query_value(role)));
        }
        AdminOp {
            req: req(Method::Delete, path, None),
            parse: Parse::Json(parse_unit),
            unsupported: NETS,
        }
    }

    pub(crate) fn delete_all_nets() -> AdminOp<()> {
        AdminOp {
            req: req(Method::Delete, "/nets".to_string(), None),
            parse: Parse::Json(parse_unit),
            unsupported: NETS,
        }
    }

    pub(crate) fn net_metadata(name: &str) -> AdminOp<NetMetadata> {
        AdminOp {
            req: req(
                Method::Get,
                format!("/nets/{}/metadata", path_segment(name)),
                None,
            ),
            parse: Parse::Json(parse_as::<NetMetadata>),
            unsupported: NET_METADATA,
        }
    }

    pub(crate) fn set_net_metadata(name: &str, update: &NetMetadataUpdate) -> AdminOp<NetMetadata> {
        AdminOp {
            req: req(
                Method::Put,
                format!("/nets/{}/metadata", path_segment(name)),
                Some(update.body()),
            ),
            parse: Parse::Json(parse_as::<NetMetadata>),
            unsupported: NET_METADATA,
        }
    }

    pub(crate) fn box_metadata() -> AdminOp<BoxMetadata> {
        AdminOp {
            req: req(Method::Get, "/box-metadata".to_string(), None),
            parse: Parse::Json(parse_as::<BoxMetadata>),
            unsupported: BOX_METADATA,
        }
    }

    pub(crate) fn set_box_metadata(
        description: Option<&str>,
        updated_at: Option<&str>,
    ) -> AdminOp<BoxMetadata> {
        AdminOp {
            req: req(
                Method::Put,
                "/box-metadata".to_string(),
                Some(json!({ "description": description, "updated_at": updated_at })),
            ),
            parse: Parse::Json(parse_as::<BoxMetadata>),
            unsupported: BOX_METADATA,
        }
    }

    pub(crate) fn instruments() -> AdminOp<Vec<Instrument>> {
        AdminOp {
            req: req(Method::Get, "/instruments/list".to_string(), None),
            parse: Parse::Json(parse_as::<Vec<Instrument>>),
            unsupported: INSTRUMENTS,
        }
    }

    #[derive(serde::Deserialize)]
    struct DriverList {
        #[serde(default)]
        drivers: Vec<DriverEntry>,
    }

    fn parse_drivers(status: u16, body: Value) -> Result<Vec<DriverEntry>> {
        parse_as::<DriverList>(status, body).map(|list| list.drivers)
    }

    /// A file's bytes, or the box's refusal (an error status carries JSON).
    fn parse_file(status: u16, bytes: Vec<u8>) -> Result<Vec<u8>> {
        if (200..300).contains(&status) {
            return Ok(bytes);
        }
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(body) => Err(refusal(status, &body)),
            Err(_) => Err(Error::Box {
                status,
                message: format!("HTTP {status} (non-JSON body)"),
            }),
        }
    }

    pub(crate) fn drivers() -> AdminOp<Vec<DriverEntry>> {
        AdminOp {
            req: req(Method::Get, "/drivers".to_string(), None),
            parse: Parse::Json(parse_drivers),
            unsupported: DRIVERS,
        }
    }

    pub(crate) fn driver(driver_type: &str) -> AdminOp<DriverEntry> {
        AdminOp {
            req: req(
                Method::Get,
                format!("/drivers/{}", path_segment(driver_type)),
                None,
            ),
            parse: Parse::Json(parse_as::<DriverEntry>),
            unsupported: DRIVERS,
        }
    }

    pub(crate) fn install_driver(driver_type: &str, bundle: &[u8]) -> AdminOp<DriverInstall> {
        let filename = format!("{driver_type}.zip");
        let body = multipart(&[FormPart {
            name: "bundle",
            filename: Some(&filename),
            content_type: Some("application/zip"),
            data: bundle,
        }]);
        AdminOp {
            req: upload(
                Method::Put,
                format!("/drivers/{}", path_segment(driver_type)),
                body,
                PIP_BUDGET,
            ),
            parse: Parse::Json(parse_as::<DriverInstall>),
            unsupported: DRIVERS,
        }
    }

    pub(crate) fn reinstall_driver_deps(driver_type: &str) -> AdminOp<Vec<String>> {
        #[derive(serde::Deserialize)]
        struct Packages {
            #[serde(default)]
            packages: Vec<String>,
        }
        fn parse(status: u16, body: Value) -> Result<Vec<String>> {
            parse_as::<Packages>(status, body).map(|p| p.packages)
        }
        AdminOp {
            req: RawRequest {
                method: Method::Post,
                path: format!("/drivers/{}/deps", path_segment(driver_type)),
                body: None,
                timeout: Timeout::After(PIP_BUDGET),
            },
            parse: Parse::Json(parse),
            unsupported: DRIVERS,
        }
    }

    pub(crate) fn remove_driver(driver_type: &str, force: bool) -> AdminOp<DriverRemoval> {
        let mut path = format!("/drivers/{}", path_segment(driver_type));
        if force {
            path.push_str("?force=1");
        }
        AdminOp {
            req: req(Method::Delete, path, None),
            parse: Parse::Json(parse_as::<DriverRemoval>),
            unsupported: DRIVERS,
        }
    }

    pub(crate) fn binaries() -> AdminOp<BinaryList> {
        AdminOp {
            req: req(Method::Get, "/binaries/list".to_string(), None),
            parse: Parse::Json(parse_as::<BinaryList>),
            unsupported: BINARIES,
        }
    }

    pub(crate) fn add_binary(name: &str, data: &[u8]) -> AdminOp<BinaryAdded> {
        let body = multipart(&[
            FormPart {
                name: "binary",
                filename: Some(name),
                content_type: Some("application/octet-stream"),
                data,
            },
            FormPart {
                name: "name",
                filename: None,
                content_type: None,
                data: name.as_bytes(),
            },
        ]);
        AdminOp {
            req: upload(
                Method::Post,
                "/binaries/add".to_string(),
                body,
                UPLOAD_BUDGET,
            ),
            parse: Parse::Json(parse_as::<BinaryAdded>),
            unsupported: BINARIES,
        }
    }

    pub(crate) fn remove_binary(name: &str) -> AdminOp<()> {
        AdminOp {
            req: req(
                Method::Post,
                "/binaries/remove".to_string(),
                Some(json!({ "name": name })),
            ),
            parse: Parse::Json(parse_unit),
            unsupported: BINARIES,
        }
    }

    pub(crate) fn download_file(path: &str) -> AdminOp<Vec<u8>> {
        download_file_with_timeout(path, None)
    }

    pub(crate) fn download_file_with_timeout(
        path: &str,
        timeout: Option<Duration>,
    ) -> AdminOp<Vec<u8>> {
        AdminOp {
            req: RawRequest {
                method: Method::Get,
                path: format!("/download-file?filename={}", encode_query_value(path)),
                body: None,
                timeout: timeout.map_or(Timeout::Unbounded, Timeout::After),
            },
            parse: Parse::Bytes(parse_file),
            unsupported: DOWNLOAD,
        }
    }

    fn timed(method: Method, path: String, body: Option<Value>, secs: u64) -> RawRequest {
        RawRequest {
            timeout: Timeout::After(Duration::from_secs(secs)),
            ..req(method, path, body)
        }
    }

    pub(crate) fn custom_devices() -> AdminOp<CustomDevices> {
        AdminOp {
            // The box scans every USB-serial cable, which takes seconds.
            req: timed(Method::Get, "/custom-devices/list".to_string(), None, 60),
            parse: Parse::Json(parse_as::<CustomDevices>),
            unsupported: CUSTOM_DEVICES,
        }
    }

    pub(crate) fn assign_custom_device(
        instrument: &str,
        cable: &CableId,
        baud: Option<u32>,
    ) -> AdminOp<DeviceAssigned> {
        let mut body = cable.body();
        body.insert("instrument".to_string(), json!(instrument));
        if let Some(baud) = baud {
            body.insert("baud".to_string(), json!(baud));
        }
        AdminOp {
            req: timed(
                Method::Post,
                "/custom-devices/assign".to_string(),
                Some(Value::Object(body)),
                60,
            ),
            parse: Parse::Json(parse_as::<DeviceAssigned>),
            unsupported: CUSTOM_DEVICES,
        }
    }

    pub(crate) fn remove_custom_device(cable: &CableId) -> AdminOp<DeviceRemoval> {
        AdminOp {
            req: req(
                Method::Post,
                "/custom-devices/remove".to_string(),
                Some(Value::Object(cable.body())),
            ),
            parse: Parse::Json(parse_as::<DeviceRemoval>),
            unsupported: CUSTOM_DEVICES,
        }
    }

    fn diagnose<T: DeserializeOwned>(
        route: &str,
        key: &str,
        value: &str,
        secs: u64,
        parse: fn(u16, Value) -> Result<T>,
    ) -> AdminOp<T> {
        AdminOp {
            req: timed(
                Method::Get,
                format!("/diagnose/{route}?{key}={}", encode_query_value(value)),
                None,
                secs,
            ),
            parse: Parse::Json(parse),
            unsupported: DIAGNOSE,
        }
    }

    pub(crate) fn diagnose_usb(address: &str) -> AdminOp<UsbDiagnosis> {
        diagnose("usb", "address", address, 15, parse_as::<UsbDiagnosis>)
    }

    pub(crate) fn diagnose_usbhub(address: &str) -> AdminOp<UsbHubDiagnosis> {
        diagnose(
            "usbhub",
            "address",
            address,
            30,
            parse_as::<UsbHubDiagnosis>,
        )
    }

    pub(crate) fn diagnose_visa(address: &str) -> AdminOp<VisaDiagnosis> {
        diagnose("visa", "address", address, 15, parse_as::<VisaDiagnosis>)
    }

    pub(crate) fn diagnose_jlink(net: &str) -> AdminOp<JlinkDiagnosis> {
        // A list of emulators, then an SWD connect: more than 30 s at worst.
        diagnose("jlink", "net", net, 60, parse_as::<JlinkDiagnosis>)
    }

    pub(crate) fn bench_manifest() -> AdminOp<BenchManifest> {
        AdminOp {
            req: req(Method::Get, "/bench".to_string(), None),
            parse: Parse::Json(parse_manifest),
            unsupported: BENCH,
        }
    }
}

/// Generates the blocking and async admin handles from one method table,
/// as `box_handle!` does for the box-level nets.
macro_rules! admin_handle {
    (
        methods: {
            $(
                $(#[$mdoc:meta])*
                fn $method:ident ( $( $arg:ident : $ty:ty ),* $(,)? ) -> $ret:ty = $op:path;
            )*
        }
    ) => {
        /// Box setup and management (feature `admin`); see [`crate::admin`].
        #[cfg(feature = "blocking")]
        #[derive(Clone)]
        pub struct Admin<'a> {
            pub(crate) client: &'a crate::client::LagerBox,
        }

        #[cfg(feature = "blocking")]
        impl<'a> Admin<'a> {
            $(
                $(#[$mdoc])*
                pub fn $method(&self, $($arg: $ty),*) -> Result<$ret> {
                    self.client.run_admin($op($($arg),*))
                }
            )*
        }

        /// Box setup and management (feature `admin`); see [`crate::admin`].
        #[cfg(feature = "async")]
        #[derive(Clone)]
        pub struct AsyncAdmin<'a> {
            pub(crate) client: &'a crate::async_client::AsyncLagerBox,
        }

        #[cfg(feature = "async")]
        impl<'a> AsyncAdmin<'a> {
            $(
                $(#[$mdoc])*
                pub async fn $method(&self, $($arg: $ty),*) -> Result<$ret> {
                    self.client.run_admin($op($($arg),*)).await
                }
            )*
        }
    };
}

admin_handle! {
    methods: {
        /// Create a net, or replace the saved record with the same name and
        /// role (`PUT /nets/<name>`). The box requires `name`, `role` and
        /// `instrument`. A record with the same name and a different role is
        /// kept beside the new one; delete it first to change a net's role.
        fn save_net(record: &NetRecord) -> () = ops::save_net;
        /// Save `record` in place of the net `old_name`. The box deletes
        /// every record named `old_name`, whatever its role, then saves
        /// `record`.
        fn rename_net(old_name: &str, record: &NetRecord) -> () = ops::rename_net;
        /// Delete a net. With `role`, only the record with that exact role
        /// goes; without it, every record with that name. An unknown net is
        /// [`crate::Error::Box`] (404).
        fn delete_net(name: &str, role: Option<&str>) -> () = ops::delete_net;
        /// Delete every saved net on the box. This cannot be undone: read
        /// [`crate::LagerBox::nets`] first if you need to restore them.
        fn delete_all_nets() -> () = ops::delete_all_nets;
        /// Read a net's metadata (box >= 0.46.0).
        fn net_metadata(name: &str) -> NetMetadata = ops::net_metadata;
        /// Change a net's metadata on every record with that name, and
        /// return what the box stored (box >= 0.46.0).
        fn set_net_metadata(name: &str, update: &NetMetadataUpdate) -> NetMetadata
            = ops::set_net_metadata;
        /// Read the box's description.
        fn box_metadata() -> BoxMetadata = ops::box_metadata;
        /// Replace the box's description. This replaces the whole record:
        /// `None` for `description` clears it.
        fn set_box_metadata(description: Option<&str>, updated_at: Option<&str>) -> BoxMetadata
            = ops::set_box_metadata;
        /// List the instruments the box sees, as `lager instruments` does.
        /// Each call scans the hardware again, and finding a robot arm
        /// sends a short handshake to free serial ports.
        fn instruments() -> Vec<Instrument> = ops::instruments;
        /// Read the bench manifest (box >= 0.50.0).
        fn bench_manifest() -> BenchManifest = ops::bench_manifest;
        /// List the custom drivers installed on the box (box >= 0.59.0).
        fn drivers() -> Vec<DriverEntry> = ops::drivers;
        /// One installed driver. An unknown type is [`crate::Error::Box`]
        /// (404).
        fn driver(driver_type: &str) -> DriverEntry = ops::driver;
        /// Install a custom driver from its zip bundle, as `lager drivers
        /// install` does, replacing an installed version. The box installs
        /// the Python packages the manifest declares, which can take
        /// minutes; the call waits up to 6 minutes. A box before 0.60.0
        /// installs no packages: if the bundle declares any, check
        /// [`crate::BoxCapabilities::custom_driver_deps`] first.
        fn install_driver(driver_type: &str, bundle: &[u8]) -> DriverInstall = ops::install_driver;
        /// Install a driver's Python packages again, and return them.
        fn reinstall_driver_deps(driver_type: &str) -> Vec<String> = ops::reinstall_driver_deps;
        /// Remove a custom driver. The box refuses (409) while saved nets
        /// use it, unless `force`, which leaves those nets without a driver.
        fn remove_driver(driver_type: &str, force: bool) -> DriverRemoval = ops::remove_driver;
        /// List the box's binaries directory, as `lager binaries list` does.
        fn binaries() -> BinaryList = ops::binaries;
        /// Upload an executable into the box's binaries directory, replacing
        /// a file with the same name.
        fn add_binary(name: &str, data: &[u8]) -> BinaryAdded = ops::add_binary;
        /// Delete a file from the box's binaries directory.
        fn remove_binary(name: &str) -> () = ops::remove_binary;
        /// Download a file a script left on the box. The box only serves
        /// files under `/tmp/lager-output`, `/tmp/lager-results` and
        /// `/tmp/lager-job-output`; anything else is [`crate::Error::Box`]
        /// (403). There is no client-side timeout.
        fn download_file(path: &str) -> Vec<u8> = ops::download_file;
        /// [`Admin::download_file`] with a client-side timeout (`None` for
        /// none).
        fn download_file_with_timeout(path: &str, timeout: Option<std::time::Duration>) -> Vec<u8>
            = ops::download_file_with_timeout;
        /// The box's custom-device catalog, its assignments and the
        /// unassigned USB-serial cables it sees, as `lager nets assign
        /// --list` shows. The box scans the cables, which takes seconds.
        fn custom_devices() -> CustomDevices = ops::custom_devices;
        /// Assign a USB-serial cable to a device definition, as `lager nets
        /// assign` does. This deletes saved nets that the assignment makes
        /// stale, and generic UART nets on the same cable; the result lists
        /// them. An unknown device, a cable that is not connected, or a
        /// serial that matches two cables is [`crate::Error::Box`] (400).
        fn assign_custom_device(instrument: &str, cable: &CableId, baud: Option<u32>)
            -> DeviceAssigned = ops::assign_custom_device;
        /// Remove a cable's assignment, and the saved nets that use it. A
        /// cable with no assignment gives `removed: false`, not an error.
        fn remove_custom_device(cable: &CableId) -> DeviceRemoval = ops::remove_custom_device;
        /// What the box sees of a USB instrument, by VISA address, as `lager
        /// diagnose` reports it. Read-only.
        fn diagnose_usb(address: &str) -> UsbDiagnosis = ops::diagnose_usb;
        /// What the box sees of a USB hub. For an Acroname hub this opens
        /// the hub, waiting for its device lock.
        fn diagnose_usbhub(address: &str) -> UsbHubDiagnosis = ops::diagnose_usbhub;
        /// Probe a VISA instrument with `*IDN?`. This opens a new session to
        /// the instrument, unless the box's hardware service already holds
        /// one (then `skipped` is set). A failed probe is data, not an error.
        fn diagnose_visa(address: &str) -> VisaDiagnosis = ops::diagnose_visa;
        /// Check a debug net's probe. When the probe is idle, this connects
        /// to the target over SWD, so do not call it during a flash. It can
        /// take over 30 seconds.
        fn diagnose_jlink(net: &str) -> JlinkDiagnosis = ops::diagnose_jlink;
    }
}
