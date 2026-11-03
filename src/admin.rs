//! Box setup and management (feature `admin`): create, change and delete
//! saved nets, edit net and box metadata, list the instruments plugged in,
//! and read the bench manifest. These are the calls behind `lager nets add`,
//! `lager nets delete`, `lager instruments` and `lager box bench export`.
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
use crate::wire::{HttpRequest, NetRecord};

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
/// status and the raw body.
pub(crate) struct AdminOp<T> {
    pub(crate) req: HttpRequest,
    pub(crate) parse: fn(u16, Value) -> Result<T>,
    /// The error for a box that does not serve the route.
    pub(crate) unsupported: &'static str,
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

    use super::{AdminOp, BenchManifest, BoxMetadata, Instrument, NetMetadata, NetMetadataUpdate};
    use crate::error::{Error, Result};
    use crate::wire::{encode_query_value, path_segment, HttpRequest, Method, NetRecord, Timeout};

    const NETS: &str = "this box does not serve net changes over HTTP (PUT/DELETE /nets)";
    const NET_METADATA: &str =
        "this box does not serve /nets/<name>/metadata (requires box software >= 0.46.0)";
    const BOX_METADATA: &str = "this box does not serve /box-metadata";
    const INSTRUMENTS: &str = "this box does not serve /instruments/list";
    const BENCH: &str = "this box does not serve /bench (requires box software >= 0.50.0)";

    /// The CLI's budget for these routes.
    fn budget() -> Timeout {
        Timeout::After(Duration::from_secs(30))
    }

    fn req(method: Method, path: String, body: Option<Value>) -> HttpRequest {
        HttpRequest {
            method,
            path,
            body,
            timeout: budget(),
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
            parse: parse_unit,
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
            parse: parse_unit,
            unsupported: NETS,
        }
    }

    pub(crate) fn delete_all_nets() -> AdminOp<()> {
        AdminOp {
            req: req(Method::Delete, "/nets".to_string(), None),
            parse: parse_unit,
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
            parse: parse_as::<NetMetadata>,
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
            parse: parse_as::<NetMetadata>,
            unsupported: NET_METADATA,
        }
    }

    pub(crate) fn box_metadata() -> AdminOp<BoxMetadata> {
        AdminOp {
            req: req(Method::Get, "/box-metadata".to_string(), None),
            parse: parse_as::<BoxMetadata>,
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
            parse: parse_as::<BoxMetadata>,
            unsupported: BOX_METADATA,
        }
    }

    pub(crate) fn instruments() -> AdminOp<Vec<Instrument>> {
        AdminOp {
            req: req(Method::Get, "/instruments/list".to_string(), None),
            parse: parse_as::<Vec<Instrument>>,
            unsupported: INSTRUMENTS,
        }
    }

    pub(crate) fn bench_manifest() -> AdminOp<BenchManifest> {
        AdminOp {
            req: req(Method::Get, "/bench".to_string(), None),
            parse: parse_manifest,
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
    }
}
