//! USB-DFU via box-side `dfu-util` (box-level, not a saved net): list
//! DFU-capable devices, download firmware, and detach — no host-side USB
//! tooling required.
//!
//! The box must have `dfu-util` installed (`lager box-config apt add
//! dfu-util`) and serve `POST /usb/dfu` (box software >= 0.33.0; older
//! boxes fail with [`crate::Error::UnsupportedByBox`]).
//!
//! ```no_run
//! # #[cfg(feature = "blocking")]
//! # fn demo() -> lager::Result<()> {
//! use lager::{DfuOptions, LagerBox};
//!
//! let lager = LagerBox::from_env()?;
//! let dfu = lager.dfu();
//!
//! // Wait for the DUT to show up in DFU mode, then flash it.
//! let devices = dfu.list()?;
//! assert!(devices.iter().any(|d| d.mode == "DFU"));
//! let firmware = std::fs::read("firmware.bin").expect("read firmware");
//! dfu.download(
//!     &firmware,
//!     &DfuOptions {
//!         vid_pid: Some("0483:df11".into()),
//!         alt: Some(0),
//!         dfuse_address: Some("0x08000000:leave".into()),
//!         ..Default::default()
//!     },
//! )?;
//! # Ok(())
//! # }
//! # fn main() {}
//! ```

pub use crate::wire::{DfuDevice, DfuOutput};

/// Device selection and transfer options for DFU operations, mirroring the
/// corresponding `dfu-util` flags. The default selects whatever single DFU
/// device is on the bus (dfu-util errors out when the selection is
/// ambiguous).
#[derive(Debug, Clone, Default)]
pub struct DfuOptions {
    /// Match by `vid:pid` (hex, e.g. `"0483:df11"`) — `dfu-util -d`.
    pub vid_pid: Option<String>,
    /// Match by device serial — `dfu-util -S`.
    pub serial: Option<String>,
    /// Alternate interface setting — `dfu-util -a`.
    pub alt: Option<u32>,
    /// DfuSe address (and modifiers, e.g. `"0x08000000:leave"`) —
    /// `dfu-util -s`. STM32 system-bootloader targets need this.
    pub dfuse_address: Option<String>,
    /// Reset the device after download — `dfu-util -R`.
    pub reset: bool,
    /// How long the box lets `dfu-util` run, in seconds (box default 120,
    /// box maximum 600). Raise it for a large image on a slow bootloader;
    /// the crate widens its HTTP timeout to match.
    pub timeout_seconds: Option<u32>,
}

pub(crate) mod ops {
    use std::time::Duration;

    use serde_json::{json, Value};

    use super::DfuOptions;
    use crate::error::Result;
    use crate::nets::debug::base64_encode;
    use crate::wire::{
        box_command, dfu_unsupported, map_route_missing, parse_dfu, value_as, value_list_field,
        DfuDevice, DfuOutput, Op, Timeout,
    };

    const PATH: &str = "/usb/dfu";

    /// Box-side dfu-util budget is 120s by default; leave headroom for the
    /// upload and queueing behind another DFU run.
    const LIST_TIMEOUT: Timeout = Timeout::After(Duration::from_secs(30));
    const RUN_TIMEOUT: Timeout = Timeout::After(Duration::from_secs(180));

    fn params(opts: &DfuOptions) -> Value {
        let mut params = json!({});
        if let Some(vid_pid) = &opts.vid_pid {
            params["vid_pid"] = json!(vid_pid);
        }
        if let Some(serial) = &opts.serial {
            params["serial"] = json!(serial);
        }
        if let Some(alt) = opts.alt {
            params["alt"] = json!(alt);
        }
        if let Some(addr) = &opts.dfuse_address {
            params["dfuse_address"] = json!(addr);
        }
        if opts.reset {
            params["reset"] = json!(true);
        }
        if let Some(t) = opts.timeout_seconds {
            params["timeout_seconds"] = json!(t);
        }
        params
    }

    /// Client budget for a dfu-util run: the box's own budget (default 120 s,
    /// clamped to 600 s box-side) plus 60 s for the upload and queueing.
    fn run_timeout(opts: &DfuOptions) -> Timeout {
        match opts.timeout_seconds {
            Some(t) => Timeout::After(Duration::from_secs(u64::from(t.clamp(1, 600)) + 60)),
            None => RUN_TIMEOUT,
        }
    }

    pub(crate) fn list() -> Op<Vec<DfuDevice>> {
        Op {
            req: box_command(PATH, "list", json!({}), LIST_TIMEOUT),
            parse: |resp| value_list_field(resp, "devices"),
        }
    }

    pub(crate) fn download(firmware: &[u8], opts: &DfuOptions) -> Op<DfuOutput> {
        let mut p = params(opts);
        p["firmware"] = json!(base64_encode(firmware));
        Op {
            req: box_command(PATH, "download", p, run_timeout(opts)),
            parse: value_as::<DfuOutput>,
        }
    }

    pub(crate) fn detach(opts: &DfuOptions) -> Op<DfuOutput> {
        Op {
            req: box_command(PATH, "detach", params(opts), run_timeout(opts)),
            parse: value_as::<DfuOutput>,
        }
    }

    /// Finish one DFU call from the raw response: a dfu-util failure keeps
    /// its output ([`crate::Error::DfuFailed`]) instead of collapsing into
    /// the box's message, and a route-missing 404 (box image predating
    /// `/usb/dfu`) maps to [`crate::Error::UnsupportedByBox`].
    pub(crate) fn finish<T>(op: &Op<T>, response: Result<(u16, Value)>) -> Result<T> {
        response
            .and_then(|(status, body)| parse_dfu(status, body))
            .and_then(op.parse)
            .map_err(|e| map_route_missing(e, dfu_unsupported))
    }
}

/// Handle for box-side DFU (from [`crate::LagerBox::dfu`]).
#[cfg(feature = "blocking")]
#[derive(Clone)]
pub struct Dfu<'a> {
    pub(crate) client: &'a crate::client::LagerBox,
}

#[cfg(feature = "blocking")]
impl Dfu<'_> {
    /// List DFU-capable devices on the box's bus (`dfu-util -l`).
    pub fn list(&self) -> crate::Result<Vec<DfuDevice>> {
        let op = ops::list();
        ops::finish(&op, self.client.execute(&op.req))
    }

    /// Download `firmware` to the selected device (`dfu-util -D`). Returns
    /// the captured dfu-util output; a non-zero exit is
    /// [`crate::Error::DfuFailed`], carrying the same output.
    pub fn download(&self, firmware: &[u8], opts: &DfuOptions) -> crate::Result<DfuOutput> {
        let op = ops::download(firmware, opts);
        ops::finish(&op, self.client.execute(&op.req))
    }

    /// Detach the selected device from DFU mode (`dfu-util -e`).
    pub fn detach(&self, opts: &DfuOptions) -> crate::Result<DfuOutput> {
        let op = ops::detach(opts);
        ops::finish(&op, self.client.execute(&op.req))
    }
}

/// Handle for box-side DFU (from [`crate::AsyncLagerBox::dfu`]).
#[cfg(feature = "async")]
#[derive(Clone)]
pub struct AsyncDfu<'a> {
    pub(crate) client: &'a crate::async_client::AsyncLagerBox,
}

#[cfg(feature = "async")]
impl AsyncDfu<'_> {
    /// List DFU-capable devices on the box's bus (`dfu-util -l`).
    pub async fn list(&self) -> crate::Result<Vec<DfuDevice>> {
        let op = ops::list();
        ops::finish(&op, self.client.execute(&op.req).await)
    }

    /// Download `firmware` to the selected device (`dfu-util -D`). Returns
    /// the captured dfu-util output; a non-zero exit is
    /// [`crate::Error::DfuFailed`], carrying the same output.
    pub async fn download(&self, firmware: &[u8], opts: &DfuOptions) -> crate::Result<DfuOutput> {
        let op = ops::download(firmware, opts);
        ops::finish(&op, self.client.execute(&op.req).await)
    }

    /// Detach the selected device from DFU mode (`dfu-util -e`).
    pub async fn detach(&self, opts: &DfuOptions) -> crate::Result<DfuOutput> {
        let op = ops::detach(opts);
        ops::finish(&op, self.client.execute(&op.req).await)
    }
}
