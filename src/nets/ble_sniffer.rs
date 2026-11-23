//! Bluetooth LE sniffer nets (box >= 0.65.0): record the radio traffic an
//! nRF52840 Dongle running Nordic's nRF Sniffer firmware hears, to a pcap
//! file, optionally following one device and its connections.
//!
//! ```no_run
//! # #[cfg(feature = "blocking")]
//! # fn demo() -> lager::Result<()> {
//! use lager::{LagerBox, SnifferOptions};
//!
//! let lager = LagerBox::from_env()?;
//! let sniffer = lager.ble_sniffer("sniffer1");
//! let capture = sniffer.capture(&SnifferOptions::new(10.0).follow("AA:BB:CC:DD:EE:FF"))?;
//! std::fs::write("capture.pcap", &capture.pcap).unwrap();
//! println!("{} packets", capture.job.packets);
//! # Ok(())
//! # }
//! # fn main() {}
//! ```
//!
//! A capture runs as a job on the box, because it holds the sniffer for its
//! whole duration: [`BleSniffer::start`] returns once the sniffer records,
//! [`BleSniffer::job`] reports progress, [`BleSniffer::stop`] ends it early,
//! and the pcap is fetched with [`BleSniffer::download`] and removed with
//! [`BleSniffer::delete`]. [`BleSniffer::capture`] does all of that in one
//! call. Jobs live in the box HTTP server's process: after a box restart,
//! `job` fails with [`crate::Error::Box`] (HTTP 400). The box keeps a capture
//! nobody deleted for a day.
//!
//! The box checks the options before touching the sniffer: a bad address,
//! an out-of-range duration or RSSI cut-off, or two options that conflict
//! fail with [`crate::Error::Box`] (HTTP 400) naming the reason. A sniffer
//! already busy with a capture fails with HTTP 409. The box lists
//! `ble-sniffer` in [`crate::BoxCapabilities::net_command_roles`] when it
//! serves this role; an older box cannot hold a `ble-sniffer` net, so it
//! answers HTTP 404 (net not found).

use serde::Deserialize;

use super::net_handle;

/// Box software that first served the `ble-sniffer` role (lager v0.65.0).
pub const BLE_SNIFFER_SINCE: &str = ">= 0.65.0";

/// Whether a followed address is public or random.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressType {
    /// A public device address.
    Public,
    /// A random (static or private) device address.
    Random,
}

impl AddressType {
    /// The box's name for the address type.
    pub fn as_str(&self) -> &'static str {
        match self {
            AddressType::Public => "public",
            AddressType::Random => "random",
        }
    }
}

/// One capture request, for [`BleSniffer::start`] and [`BleSniffer::capture`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SnifferOptions {
    /// Capture time in seconds; the box accepts 1 to 86400 (default 10).
    pub duration: f64,
    /// Follow the device with this address (`AA:BB:CC:DD:EE:FF`) and its
    /// connections.
    pub follow: Option<String>,
    /// Whether [`SnifferOptions::follow`] is public or random; `None` takes
    /// the type from the device's first advertisement.
    pub address_type: Option<AddressType>,
    /// Follow the first device whose legacy advertising carries this exact
    /// name. Not with [`SnifferOptions::follow`] or [`SnifferOptions::coded`].
    pub follow_by_name: Option<String>,
    /// Record the followed device's advertising only. Needs `follow` or
    /// `follow_by_name`.
    pub only_advertising: bool,
    /// Scan and follow on the LE Coded PHY.
    pub coded: bool,
    /// Drop advertising packets weaker than this many dBm (-255 to -10).
    pub rssi_cut_off: Option<i32>,
    /// [`BleSniffer::capture`] only: leave the pcap on the box after the
    /// download instead of deleting it.
    pub keep: bool,
}

impl Default for SnifferOptions {
    fn default() -> Self {
        SnifferOptions::new(10.0)
    }
}

impl SnifferOptions {
    /// Capture everything the sniffer hears for `duration` seconds.
    pub fn new(duration: f64) -> Self {
        SnifferOptions {
            duration,
            follow: None,
            address_type: None,
            follow_by_name: None,
            only_advertising: false,
            coded: false,
            rssi_cut_off: None,
            keep: false,
        }
    }

    /// Follow the device with this address and its connections.
    pub fn follow(mut self, address: impl Into<String>) -> Self {
        self.follow = Some(address.into());
        self
    }

    /// Say whether the followed address is public or random.
    pub fn address_type(mut self, address_type: AddressType) -> Self {
        self.address_type = Some(address_type);
        self
    }

    /// Follow the first device advertising this exact name.
    pub fn follow_by_name(mut self, name: impl Into<String>) -> Self {
        self.follow_by_name = Some(name.into());
        self
    }

    /// Record the followed device's advertising only.
    pub fn only_advertising(mut self) -> Self {
        self.only_advertising = true;
        self
    }

    /// Scan and follow on the LE Coded PHY.
    pub fn coded(mut self) -> Self {
        self.coded = true;
        self
    }

    /// Drop advertising packets weaker than `dbm`.
    pub fn rssi_cut_off(mut self, dbm: i32) -> Self {
        self.rssi_cut_off = Some(dbm);
        self
    }

    /// [`BleSniffer::capture`] only: keep the pcap on the box.
    pub fn keep(mut self) -> Self {
        self.keep = true;
        self
    }
}

/// Where a capture job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SnifferJobState {
    /// Waiting for the sniffer to start recording.
    Starting,
    /// Recording.
    Running,
    /// Finished; [`SnifferJob::result`] describes the file.
    Done,
    /// Failed; see [`SnifferJob::error`] and [`SnifferJob::file_saved`].
    Failed,
    /// A state this crate does not know yet.
    #[serde(other)]
    Unknown,
}

impl SnifferJobState {
    /// Whether the job has ended, as done or failed.
    pub fn is_finished(&self) -> bool {
        matches!(self, SnifferJobState::Done | SnifferJobState::Failed)
    }
}

/// Why a capture job failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SnifferErrorKind {
    /// Another capture held the sniffer.
    Busy,
    /// An option was invalid.
    Invalid,
    /// The sniffer was missing, did not start, or stopped answering.
    Backend,
    /// A kind this crate does not know yet.
    #[serde(other)]
    Unknown,
}

/// The finished file of a capture job.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct SnifferResult {
    /// Path of the pcap on the box.
    pub file: String,
    /// File size in bytes.
    #[serde(default)]
    pub bytes: u64,
    /// Complete packets in the file.
    #[serde(default)]
    pub packets: u64,
    /// Measured capture time in seconds.
    #[serde(default)]
    pub duration: f64,
    /// Serial port the sniffer was on.
    #[serde(default)]
    pub port: Option<String>,
    /// Whether [`BleSniffer::stop`] ended the capture early.
    #[serde(default)]
    pub stopped: bool,
    /// The sniffer firmware's version string.
    #[serde(default)]
    pub firmware: Option<String>,
    /// The address a follow request was sent for, if one was.
    #[serde(default)]
    pub followed: Option<String>,
    /// For a follow request, whether the followed device was heard; `None`
    /// when nothing was followed.
    #[serde(default)]
    pub follow_seen: Option<bool>,
}

/// A capture job on the box.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct SnifferJob {
    /// Job ID, for [`BleSniffer::job`].
    #[serde(rename = "job")]
    pub id: String,
    /// Net the job captures on.
    #[serde(default)]
    pub net: String,
    /// Where the job is.
    pub state: SnifferJobState,
    /// Path of the pcap on the box, for [`BleSniffer::download`].
    pub file: String,
    /// Requested capture time in seconds.
    #[serde(default)]
    pub duration: f64,
    /// Seconds recorded so far, or in all once finished.
    #[serde(default)]
    pub elapsed: f64,
    /// Packets recorded so far.
    #[serde(default)]
    pub packets: u64,
    /// The finished file, once [`SnifferJobState::Done`].
    #[serde(default)]
    pub result: Option<SnifferResult>,
    /// Why the job failed.
    #[serde(default)]
    pub error: Option<String>,
    /// What kind of failure it was.
    #[serde(default)]
    pub error_kind: Option<SnifferErrorKind>,
    /// For a failed job, whether its file still holds the packets recorded
    /// before the failure.
    #[serde(default)]
    pub file_saved: bool,
}

/// What [`BleSniffer::status`] reports.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct SnifferStatus {
    /// USB serial number of the sniffer dongle.
    pub serial: String,
    /// Serial port it is on, when present.
    #[serde(default)]
    pub port: Option<String>,
    /// Whether the dongle is on USB.
    #[serde(default)]
    pub present: bool,
    /// Whether a capture holds it.
    #[serde(default)]
    pub busy: bool,
    /// The most recent capture job on this sniffer, if the box HTTP server
    /// has run one since it started.
    #[serde(default)]
    pub job: Option<SnifferJob>,
}

/// What [`BleSniffer::stop`] did.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct SnifferStop {
    /// Whether a capture was running and was asked to stop. It ends within
    /// a fraction of a second; poll [`BleSniffer::job`] for the result.
    #[serde(default)]
    pub stopped: bool,
    /// Path of the stopping capture's pcap on the box.
    #[serde(default)]
    pub file: Option<String>,
    /// ID of the stopping job.
    #[serde(default)]
    pub job: Option<String>,
}

/// A finished capture from [`BleSniffer::capture`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SnifferCapture {
    /// The pcap file (link type 272, Nordic BLE), as the box wrote it.
    pub pcap: Vec<u8>,
    /// The finished job. Its state is [`SnifferJobState::Failed`] when the
    /// sniffer stopped answering mid-capture; `pcap` then holds the packets
    /// recorded before that, and [`SnifferJob::error`] says what happened.
    pub job: SnifferJob,
    /// Path of the pcap on the box.
    pub box_file: String,
    /// Whether the box copy was deleted. False with
    /// [`SnifferOptions::keep`], or when the delete failed; the box removes
    /// it after a day either way.
    pub deleted: bool,
}

impl SnifferCapture {
    /// Whether the capture ran for its whole duration (or until
    /// [`BleSniffer::stop`]) without failing.
    pub fn is_complete(&self) -> bool {
        self.job.state == SnifferJobState::Done
    }
}

pub(crate) mod ops {
    use std::time::Duration;

    use serde::Deserialize;
    use serde_json::{json, Value};

    use super::{SnifferErrorKind, SnifferJob, SnifferOptions, SnifferStatus, SnifferStop};
    use crate::error::{Error, Result};
    use crate::wire::{
        encode_query_value, net_command, value_as, CommandResponse, Method, Op, RawRequest, Timeout,
    };

    const ROLE: &str = "ble-sniffer";

    /// The box waits up to 10 s for the sniffer to start recording, after
    /// locking it and scanning USB. A budget at or under that would give up
    /// on a job that still starts, with nothing left to stop or download it.
    const START_BUDGET: Duration = Duration::from_secs(30);
    /// The CLI's read timeout for the pcap download.
    pub(crate) const DOWNLOAD_BUDGET: Duration = Duration::from_secs(120);
    /// Poll interval while a capture runs, and the shortest wait near its end.
    pub(crate) const POLL: Duration = Duration::from_secs(1);
    pub(crate) const MIN_POLL: Duration = Duration::from_millis(200);
    /// How long past its duration a job may run before `capture` stops it.
    pub(crate) const OVERRUN: Duration = Duration::from_secs(30);
    /// How long `capture` waits for a job it stopped to finish.
    pub(crate) const STOP_GRACE: Duration = Duration::from_secs(10);

    fn budget() -> Timeout {
        Timeout::After(Duration::from_secs(15))
    }

    pub(crate) fn start(name: &str, opts: &SnifferOptions) -> Op<SnifferJob> {
        let mut params = json!({
            "duration": opts.duration,
            "only_advertising": opts.only_advertising,
            "coded": opts.coded,
        });
        if let Some(follow) = &opts.follow {
            params["follow"] = json!(follow);
        }
        if let Some(address_type) = opts.address_type {
            params["address_type"] = json!(address_type.as_str());
        }
        if let Some(name) = &opts.follow_by_name {
            params["follow_by_name"] = json!(name);
        }
        if let Some(dbm) = opts.rssi_cut_off {
            params["rssi_cut_off"] = json!(dbm);
        }
        Op {
            req: net_command(name, ROLE, "start", params, Timeout::After(START_BUDGET)),
            parse: value_as::<SnifferJob>,
        }
    }

    pub(crate) fn job(name: &str, job_id: &str) -> Op<SnifferJob> {
        Op {
            req: net_command(name, ROLE, "job", json!({ "job": job_id }), budget()),
            parse: value_as::<SnifferJob>,
        }
    }

    pub(crate) fn stop(name: &str) -> Op<SnifferStop> {
        Op {
            req: net_command(name, ROLE, "stop", json!({}), budget()),
            parse: value_as::<SnifferStop>,
        }
    }

    pub(crate) fn status(name: &str) -> Op<SnifferStatus> {
        Op {
            req: net_command(name, ROLE, "status", json!({}), budget()),
            parse: value_as::<SnifferStatus>,
        }
    }

    fn parse_deleted(resp: CommandResponse) -> Result<bool> {
        #[derive(Deserialize)]
        struct Deleted {
            #[serde(default)]
            deleted: bool,
        }
        value_as::<Deleted>(resp).map(|d| d.deleted)
    }

    pub(crate) fn delete(name: &str, file: &str) -> Op<bool> {
        Op {
            req: net_command(name, ROLE, "delete", json!({ "file": file }), budget()),
            parse: parse_deleted,
        }
    }

    /// `GET /download-file` for a capture's pcap.
    pub(crate) fn download(file: &str) -> RawRequest {
        RawRequest {
            method: Method::Get,
            path: format!("/download-file?filename={}", encode_query_value(file)),
            body: None,
            timeout: Timeout::After(DOWNLOAD_BUDGET),
        }
    }

    /// The file on a 2xx answer, else the box's refusal.
    pub(crate) fn parse_download(status: u16, bytes: Vec<u8>) -> Result<Vec<u8>> {
        if (200..300).contains(&status) {
            return Ok(bytes);
        }
        let message = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|body| {
                body.get("error")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| format!("HTTP {status} (non-JSON body)"));
        Err(Error::Box { status, message })
    }

    /// A failed job with no file to download, as the error `start` would
    /// have answered: 409 busy, 400 invalid, 500 otherwise.
    pub(crate) fn job_failure(job: &SnifferJob) -> Error {
        let status = match job.error_kind {
            Some(SnifferErrorKind::Busy) => 409,
            Some(SnifferErrorKind::Invalid) => 400,
            _ => 500,
        };
        Error::Box {
            status,
            message: job
                .error
                .clone()
                .unwrap_or_else(|| format!("capture job {} failed", job.id)),
        }
    }

    /// How long to wait before the next poll: a second, less near the end
    /// of the capture, never under `MIN_POLL`.
    pub(crate) fn poll_wait(left: Option<Duration>) -> Duration {
        match left {
            Some(left) => left.clamp(MIN_POLL, POLL),
            None => MIN_POLL,
        }
    }

    /// The job's duration as a `Duration`, for the poll deadline.
    pub(crate) fn capture_time(opts: &SnifferOptions) -> Duration {
        Duration::try_from_secs_f64(opts.duration).unwrap_or(Duration::ZERO)
    }
}

net_handle! {
    /// Handle for a Bluetooth LE sniffer net (box >= 0.65.0). See
    /// [`crate::nets::ble_sniffer`].
    sync: BleSniffer,
    async: AsyncBleSniffer,
    methods: {
        /// Start a capture job. Returns once the sniffer records, so a busy
        /// (HTTP 409), missing or silent sniffer fails here.
        fn start(options: &SnifferOptions) -> SnifferJob = ops::start;
        /// The capture job with this ID.
        fn job(job_id: &str) -> SnifferJob = ops::job;
        /// Ask the running capture to stop early. `stopped` is false when no
        /// capture started from the box API is running.
        fn stop() -> SnifferStop = ops::stop;
        /// Whether the sniffer is on USB and busy, with its latest job.
        fn status() -> SnifferStatus = ops::status;
        /// Delete a capture's pcap from the box: the path a job reports.
        /// Returns false when there was no file there. Only captures under
        /// the box's sniffer output directory can be deleted (HTTP 400).
        fn delete(file: &str) -> bool = ops::delete;
    }
}

#[cfg(feature = "blocking")]
impl BleSniffer<'_> {
    /// Download a capture's pcap from the box: the path a job reports.
    pub fn download(&self, file: &str) -> crate::error::Result<Vec<u8>> {
        let (status, bytes) = self.client.execute_raw(&ops::download(file))?;
        ops::parse_download(status, bytes)
    }

    /// Capture for `options.duration` seconds: start a job, wait for it,
    /// download the pcap and delete the box copy (unless
    /// [`SnifferOptions::keep`]).
    ///
    /// A job that fails with nothing recorded is an error (HTTP 409 for a
    /// busy sniffer, 400 for an invalid option, 500 otherwise). One that
    /// fails mid-capture returns what it recorded; see
    /// [`SnifferCapture::is_complete`]. A job still running 30 s past its
    /// duration is stopped, and [`crate::Error::Timeout`] if it does not end.
    pub fn capture(&self, options: &SnifferOptions) -> crate::error::Result<SnifferCapture> {
        use std::time::Instant;

        let started = self.start(options)?;
        let end = Instant::now() + ops::capture_time(options);
        let mut deadline = end + ops::OVERRUN;
        let mut stop_sent = false;
        let job = loop {
            let left = end.checked_duration_since(Instant::now());
            std::thread::sleep(ops::poll_wait(left));
            let job = self.job(&started.id)?;
            if job.state.is_finished() {
                break job;
            }
            if Instant::now() >= deadline {
                if stop_sent {
                    return Err(crate::error::Error::Timeout(format!(
                        "capture job {} did not finish after a stop",
                        started.id
                    )));
                }
                self.stop()?;
                stop_sent = true;
                deadline = Instant::now() + ops::STOP_GRACE;
            }
        };
        self.finish(job, options.keep)
    }

    fn finish(&self, job: SnifferJob, keep: bool) -> crate::error::Result<SnifferCapture> {
        if job.state == SnifferJobState::Failed && !job.file_saved {
            return Err(ops::job_failure(&job));
        }
        let box_file = job
            .result
            .as_ref()
            .map_or_else(|| job.file.clone(), |r| r.file.clone());
        let pcap = self.download(&box_file)?;
        let deleted = !keep && self.delete(&box_file).unwrap_or(false);
        Ok(SnifferCapture {
            pcap,
            job,
            box_file,
            deleted,
        })
    }
}

#[cfg(feature = "async")]
impl AsyncBleSniffer<'_> {
    /// Download a capture's pcap from the box: the path a job reports.
    pub async fn download(&self, file: &str) -> crate::error::Result<Vec<u8>> {
        let (status, bytes) = self.client.execute_raw(&ops::download(file)).await?;
        ops::parse_download(status, bytes)
    }

    /// Capture for `options.duration` seconds: start a job, wait for it,
    /// download the pcap and delete the box copy (unless
    /// [`SnifferOptions::keep`]). Errors as [`BleSniffer::capture`].
    pub async fn capture(&self, options: &SnifferOptions) -> crate::error::Result<SnifferCapture> {
        use tokio::time::Instant;

        let started = self.start(options).await?;
        let end = Instant::now() + ops::capture_time(options);
        let mut deadline = end + ops::OVERRUN;
        let mut stop_sent = false;
        let job = loop {
            let left = end.checked_duration_since(Instant::now());
            tokio::time::sleep(ops::poll_wait(left)).await;
            let job = self.job(&started.id).await?;
            if job.state.is_finished() {
                break job;
            }
            if Instant::now() >= deadline {
                if stop_sent {
                    return Err(crate::error::Error::Timeout(format!(
                        "capture job {} did not finish after a stop",
                        started.id
                    )));
                }
                self.stop().await?;
                stop_sent = true;
                deadline = Instant::now() + ops::STOP_GRACE;
            }
        };
        if job.state == SnifferJobState::Failed && !job.file_saved {
            return Err(ops::job_failure(&job));
        }
        let box_file = job
            .result
            .as_ref()
            .map_or_else(|| job.file.clone(), |r| r.file.clone());
        let pcap = self.download(&box_file).await?;
        let deleted = !options.keep && self.delete(&box_file).await.unwrap_or(false);
        Ok(SnifferCapture {
            pcap,
            job,
            box_file,
            deleted,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::ops::{job_failure, poll_wait, MIN_POLL, POLL};
    use super::*;

    fn job(value: serde_json::Value) -> SnifferJob {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_job_parses_the_box_shape() {
        // capture_jobs.CaptureJob.to_dict() for a finished nRF capture.
        let j = job(serde_json::json!({
            "job": "0123456789ab", "net": "sniffer1", "state": "done",
            "file": "/tmp/lager-output/ble-sniffer/sniffer1-20261123T120000Z.pcap",
            "duration": 10.0, "elapsed": 10.012, "packets": 42,
            "result": {
                "file": "/tmp/lager-output/ble-sniffer/sniffer1-20261123T120000Z.pcap",
                "bytes": 2048, "packets": 42, "port": "/dev/ttyACM0", "duration": 10.012,
                "stopped": false, "firmware": "4.1.1", "followed": "AA:BB:CC:DD:EE:FF",
                "follow_seen": true
            },
            "error": null, "error_kind": null, "file_saved": false
        }));
        assert_eq!(j.id, "0123456789ab");
        assert_eq!(j.state, SnifferJobState::Done);
        assert_eq!(j.packets, 42);
        let r = j.result.unwrap();
        assert_eq!(r.bytes, 2048);
        assert_eq!(r.followed.as_deref(), Some("AA:BB:CC:DD:EE:FF"));
        assert_eq!(r.follow_seen, Some(true));
    }

    #[test]
    fn unknown_states_and_kinds_do_not_fail_to_parse() {
        let j = job(serde_json::json!({
            "job": "x", "state": "paused", "file": "f.pcap", "error_kind": "quota"
        }));
        assert_eq!(j.state, SnifferJobState::Unknown);
        assert_eq!(j.error_kind, Some(SnifferErrorKind::Unknown));
        assert!(!j.state.is_finished());
    }

    #[test]
    fn a_failed_job_maps_to_the_status_start_would_answer() {
        for (kind, status) in [("busy", 409), ("invalid", 400), ("backend", 500)] {
            let j = job(serde_json::json!({
                "job": "x", "state": "failed", "file": "f.pcap",
                "error": "boom", "error_kind": kind
            }));
            match job_failure(&j) {
                crate::Error::Box { status: s, message } => {
                    assert_eq!(s, status, "{kind}");
                    assert_eq!(message, "boom");
                }
                other => panic!("{kind}: {other:?}"),
            }
        }
    }

    #[test]
    fn polls_once_a_second_and_faster_near_the_end() {
        assert_eq!(poll_wait(Some(Duration::from_secs(5))), POLL);
        assert_eq!(
            poll_wait(Some(Duration::from_millis(500))),
            Duration::from_millis(500)
        );
        assert_eq!(poll_wait(Some(Duration::from_millis(10))), MIN_POLL);
        assert_eq!(poll_wait(None), MIN_POLL);
    }
}
