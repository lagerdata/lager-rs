//! Contract tests for `ble-sniffer` nets on `/net/command` (box >= 0.65.0).
//!
//! Shapes are taken from the box source: `_ble_sniffer` in
//! `box/lager/http_handlers/net_command.py`, `CaptureJob.to_dict()` in
//! `box/lager/protocols/ble_sniffer/capture_jobs.py`, and the capture result
//! of `BleSnifferBase.capture` / `NrfSniffer._record`. The capture flow
//! (start, poll `job`, `GET /download-file`, `delete`) is the one
//! `lager ble-sniffer NET capture` runs.

use httpmock::prelude::*;
use lager::wire::{net_command, Timeout};
use lager::{is_retry_safe, Error, SnifferJobState, SnifferOptions};
use serde_json::{json, Value};

const NET: &str = "sniffer1";
const JOB: &str = "0123456789ab";
const FILE: &str = "/tmp/lager-output/ble-sniffer/sniffer1-20261123T120000Z.pcap";
/// A pcap global header (classic, little-endian, link type 272) and nothing else.
const PCAP: &[u8] = b"\xd4\xc3\xb2\xa1\x02\x00\x04\x00\0\0\0\0\0\0\0\0\xff\xff\0\0\x10\x01\0\0";

fn job(state: &str, extra: Value) -> Value {
    let mut job = json!({
        "job": JOB, "net": NET, "state": state, "file": FILE, "duration": 1.0,
        "elapsed": 0.0, "packets": 0, "result": null, "error": null,
        "error_kind": null, "file_saved": false
    });
    for (k, v) in extra.as_object().unwrap() {
        job[k] = v.clone();
    }
    job
}

fn done_job() -> Value {
    job(
        "done",
        json!({
            "elapsed": 1.004, "packets": 3,
            "result": {
                "file": FILE, "bytes": PCAP.len(), "packets": 3, "port": "/dev/ttyACM0",
                "duration": 1.004, "stopped": false, "firmware": "4.1.1",
                "followed": null, "follow_seen": null
            }
        }),
    )
}

fn ok(action: &str, value: Value) -> Value {
    json!({"success": true, "action": action, "message": "ok", "value": value})
}

fn mock_action<'a>(
    server: &'a MockServer,
    action: &str,
    params: Value,
    value: Value,
) -> httpmock::Mock<'a> {
    let reply = ok(action, value);
    server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": NET, "role": "ble-sniffer", "action": action, "params": params
        }));
        then.status(200).json_body(reply);
    })
}

fn mock_start<'a>(server: &'a MockServer) -> httpmock::Mock<'a> {
    mock_action(
        server,
        "start",
        json!({"duration": 1.0, "only_advertising": false, "coded": false}),
        job("running", json!({})),
    )
}

fn mock_download<'a>(server: &'a MockServer) -> httpmock::Mock<'a> {
    server.mock(|when, then| {
        when.method(GET)
            .path("/download-file")
            .query_param("filename", FILE);
        then.status(200)
            .header("content-type", "application/octet-stream")
            .body(PCAP);
    })
}

fn mock_delete<'a>(server: &'a MockServer) -> httpmock::Mock<'a> {
    mock_action(
        server,
        "delete",
        json!({"file": FILE}),
        json!({"file": FILE, "deleted": true}),
    )
}

#[test]
fn status_and_job_polls_are_retried_and_nothing_else_is() {
    let req = |action: &str| net_command(NET, "ble-sniffer", action, json!({}), Timeout::Default);
    assert!(is_retry_safe(&req("status")));
    assert!(is_retry_safe(&req("job")));
    for action in ["start", "stop", "delete"] {
        assert!(!is_retry_safe(&req(action)), "{action}");
    }
}

#[cfg(feature = "blocking")]
mod blocking {
    use super::*;
    use lager::{AddressType, LagerBox, SnifferErrorKind};

    fn client(server: &MockServer) -> LagerBox {
        LagerBox::connect(server.address().to_string()).unwrap()
    }

    #[test]
    fn start_sends_only_the_options_given() {
        let server = MockServer::start();
        let plain = mock_action(
            &server,
            "start",
            json!({"duration": 10.0, "only_advertising": false, "coded": false}),
            job("running", json!({"duration": 10.0})),
        );
        let full = mock_action(
            &server,
            "start",
            json!({
                "duration": 30.0, "follow": "AA:BB:CC:DD:EE:FF", "address_type": "random",
                "only_advertising": true, "coded": true, "rssi_cut_off": -70
            }),
            job("running", json!({"duration": 30.0})),
        );
        let by_name = mock_action(
            &server,
            "start",
            json!({
                "duration": 5.0, "follow_by_name": "DUT", "only_advertising": false,
                "coded": false
            }),
            job("starting", json!({"duration": 5.0})),
        );
        let lager = client(&server);
        let sniffer = lager.ble_sniffer(NET);
        let j = sniffer.start(&SnifferOptions::default()).unwrap();
        assert_eq!((j.id.as_str(), j.state), (JOB, SnifferJobState::Running));
        assert_eq!(j.file, FILE);
        let opts = SnifferOptions::new(30.0)
            .follow("AA:BB:CC:DD:EE:FF")
            .address_type(AddressType::Random)
            .only_advertising()
            .coded()
            .rssi_cut_off(-70);
        assert_eq!(sniffer.start(&opts).unwrap().duration, 30.0);
        let opts = SnifferOptions::new(5.0).follow_by_name("DUT");
        assert_eq!(
            sniffer.start(&opts).unwrap().state,
            SnifferJobState::Starting
        );
        plain.assert();
        full.assert();
        by_name.assert();
    }

    #[test]
    fn job_stop_status_and_delete_round_trip() {
        let server = MockServer::start();
        let j = mock_action(&server, "job", json!({"job": JOB}), done_job());
        let stop = mock_action(
            &server,
            "stop",
            json!({}),
            json!({"stopped": true, "file": FILE, "job": JOB}),
        );
        let status = mock_action(
            &server,
            "status",
            json!({}),
            json!({
                "serial": "F1A2B3C4D5E6", "port": "/dev/ttyACM0", "present": true,
                "busy": false, "job": done_job()
            }),
        );
        let delete = mock_action(
            &server,
            "delete",
            json!({"file": FILE}),
            json!({"file": FILE, "deleted": true}),
        );
        let lager = client(&server);
        let sniffer = lager.ble_sniffer(NET);

        let done = sniffer.job(JOB).unwrap();
        assert_eq!(done.state, SnifferJobState::Done);
        let result = done.result.unwrap();
        assert_eq!(
            (result.packets, result.firmware.as_deref()),
            (3, Some("4.1.1"))
        );
        assert_eq!(result.follow_seen, None);

        let stopped = sniffer.stop().unwrap();
        assert!(stopped.stopped);
        assert_eq!(stopped.job.as_deref(), Some(JOB));

        let st = sniffer.status().unwrap();
        assert_eq!(st.serial, "F1A2B3C4D5E6");
        assert!(st.present && !st.busy);
        assert_eq!(st.job.unwrap().id, JOB);

        assert!(sniffer.delete(FILE).unwrap());
        for m in [j, stop, status, delete] {
            m.assert();
        }
    }

    #[test]
    fn an_idle_missing_sniffer_and_a_stop_with_nothing_running() {
        let server = MockServer::start();
        mock_action(
            &server,
            "status",
            json!({}),
            json!({"serial": "F1A2B3C4D5E6", "port": null, "present": false, "busy": false, "job": null}),
        );
        mock_action(
            &server,
            "stop",
            json!({}),
            json!({"stopped": false, "file": null, "job": null}),
        );
        mock_action(
            &server,
            "delete",
            json!({"file": FILE}),
            json!({"file": FILE, "deleted": false}),
        );
        let lager = client(&server);
        let sniffer = lager.ble_sniffer(NET);
        let st = sniffer.status().unwrap();
        assert!(!st.present);
        assert_eq!((st.port, st.job), (None, None));
        let stop = sniffer.stop().unwrap();
        assert!(!stop.stopped);
        assert_eq!((stop.file, stop.job), (None, None));
        assert!(!sniffer.delete(FILE).unwrap());
    }

    #[test]
    fn refusals_stay_box_errors() {
        let server = MockServer::start();
        // DriverBusy from capture_jobs.start_job.
        server.mock(|when, then| {
            when.method(POST)
                .path("/net/command")
                .body_contains("\"start\"");
            then.status(409).json_body(json!({
                "success": false,
                "error": "[BleSniffer] Sniffer F1A2B3C4D5E6 is busy with another capture"
            }));
        });
        // ValueError from the `job` action after a box restart.
        server.mock(|when, then| {
            when.method(POST)
                .path("/net/command")
                .body_contains("\"job\"");
            then.status(400).json_body(json!({
                "success": false,
                "error": "No capture job 0123456789ab on this box; it may have restarted"
            }));
        });
        // _resolve_role on a box without the net (or older than 0.65.0).
        server.mock(|when, then| {
            when.method(POST)
                .path("/net/command")
                .body_contains("\"status\"");
            then.status(404).json_body(json!({
                "success": false, "error": "Net 'sniffer1 (role ble-sniffer)' not found"
            }));
        });
        let lager = client(&server);
        let sniffer = lager.ble_sniffer(NET);
        let status_of = |e: Error| match e {
            Error::Box { status, message } => (status, message),
            other => panic!("{other:?}"),
        };
        let (s, m) = status_of(sniffer.start(&SnifferOptions::default()).unwrap_err());
        assert_eq!(s, 409);
        assert!(m.contains("busy"), "{m}");
        let (s, m) = status_of(sniffer.job(JOB).unwrap_err());
        assert_eq!(s, 400);
        assert!(m.contains("may have restarted"), "{m}");
        let (s, _) = status_of(sniffer.status().unwrap_err());
        assert_eq!(s, 404);
    }

    #[test]
    fn download_returns_the_exact_bytes_and_refusals_are_box_errors() {
        let server = MockServer::start();
        let m = mock_download(&server);
        server.mock(|when, then| {
            when.method(GET)
                .path("/download-file")
                .query_param("filename", "/tmp/lager-output/ble-sniffer/gone.pcap");
            then.status(404).json_body(
                json!({"error": "File not found: /tmp/lager-output/ble-sniffer/gone.pcap"}),
            );
        });
        let lager = client(&server);
        let sniffer = lager.ble_sniffer(NET);
        assert_eq!(sniffer.download(FILE).unwrap(), PCAP);
        match sniffer
            .download("/tmp/lager-output/ble-sniffer/gone.pcap")
            .unwrap_err()
        {
            Error::Box { status, message } => {
                assert_eq!(status, 404);
                assert!(message.starts_with("File not found"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        m.assert();
    }

    #[test]
    fn capture_starts_polls_downloads_and_deletes() {
        let server = MockServer::start();
        let start = mock_start(&server);
        let poll = mock_action(&server, "job", json!({"job": JOB}), done_job());
        let download = mock_download(&server);
        let delete = mock_delete(&server);
        let lager = client(&server);
        let capture = lager
            .ble_sniffer(NET)
            .capture(&SnifferOptions::new(1.0))
            .unwrap();
        assert_eq!(capture.pcap, PCAP);
        assert!(capture.is_complete());
        assert!(capture.deleted);
        assert_eq!(capture.box_file, FILE);
        assert_eq!(capture.job.packets, 3);
        start.assert();
        poll.assert();
        download.assert();
        delete.assert();
    }

    #[test]
    fn capture_with_keep_leaves_the_box_copy() {
        let server = MockServer::start();
        mock_start(&server);
        mock_action(&server, "job", json!({"job": JOB}), done_job());
        mock_download(&server);
        let delete = mock_delete(&server);
        let lager = client(&server);
        let capture = lager
            .ble_sniffer(NET)
            .capture(&SnifferOptions::new(1.0).keep())
            .unwrap();
        assert!(!capture.deleted);
        delete.assert_hits(0);
    }

    #[test]
    fn a_job_that_failed_with_nothing_recorded_is_an_error() {
        let server = MockServer::start();
        mock_start(&server);
        mock_action(
            &server,
            "job",
            json!({"job": JOB}),
            job(
                "failed",
                json!({
                    "error": "[BleSniffer] Sniffer F1A2B3C4D5E6 is busy with another capture",
                    "error_kind": "busy"
                }),
            ),
        );
        let download = mock_download(&server);
        let lager = client(&server);
        match lager
            .ble_sniffer(NET)
            .capture(&SnifferOptions::new(1.0))
            .unwrap_err()
        {
            Error::Box { status, message } => {
                assert_eq!(status, 409);
                assert!(message.contains("busy"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        download.assert_hits(0);
    }

    #[test]
    fn a_job_that_failed_mid_capture_returns_what_it_recorded() {
        let server = MockServer::start();
        mock_start(&server);
        let failed = job(
            "failed",
            json!({
                "elapsed": 0.6, "packets": 2,
                "error": "[BleSniffer] Sniffer stopped answering after 2 packet(s)",
                "error_kind": "backend", "file_saved": true
            }),
        );
        mock_action(&server, "job", json!({"job": JOB}), failed);
        mock_download(&server);
        mock_delete(&server);
        let lager = client(&server);
        let capture = lager
            .ble_sniffer(NET)
            .capture(&SnifferOptions::new(1.0))
            .unwrap();
        assert!(!capture.is_complete());
        assert_eq!(capture.job.error_kind, Some(SnifferErrorKind::Backend));
        assert_eq!(capture.pcap, PCAP);
        assert_eq!(capture.box_file, FILE);
    }
}

#[cfg(feature = "async")]
mod async_parity {
    use super::*;
    use lager::AsyncLagerBox;

    fn client(server: &MockServer) -> AsyncLagerBox {
        AsyncLagerBox::connect(server.address().to_string()).unwrap()
    }

    #[tokio::test]
    async fn async_status_and_job() {
        let server = MockServer::start_async().await;
        mock_action(
            &server,
            "status",
            json!({}),
            json!({"serial": "F1A2B3C4D5E6", "port": "/dev/ttyACM0", "present": true, "busy": true, "job": null}),
        );
        mock_action(&server, "job", json!({"job": JOB}), done_job());
        let lager = client(&server);
        let sniffer = lager.ble_sniffer(NET);
        assert!(sniffer.status().await.unwrap().busy);
        assert_eq!(sniffer.job(JOB).await.unwrap().state, SnifferJobState::Done);
    }

    #[tokio::test]
    async fn async_capture_starts_polls_downloads_and_deletes() {
        let server = MockServer::start_async().await;
        let start = mock_start(&server);
        let poll = mock_action(&server, "job", json!({"job": JOB}), done_job());
        let download = mock_download(&server);
        let delete = mock_delete(&server);
        let lager = client(&server);
        let capture = lager
            .ble_sniffer(NET)
            .capture(&SnifferOptions::new(1.0))
            .await
            .unwrap();
        assert_eq!(capture.pcap, PCAP);
        assert!(capture.is_complete() && capture.deleted);
        start.assert();
        poll.assert();
        download.assert();
        delete.assert();
    }

    #[tokio::test]
    async fn async_failed_job_with_nothing_recorded_is_an_error() {
        let server = MockServer::start_async().await;
        mock_start(&server);
        mock_action(
            &server,
            "job",
            json!({"job": JOB}),
            job(
                "failed",
                json!({"error": "duration must be between 1 and 86400 seconds", "error_kind": "invalid"}),
            ),
        );
        let lager = client(&server);
        match lager
            .ble_sniffer(NET)
            .capture(&SnifferOptions::new(1.0))
            .await
            .unwrap_err()
        {
            Error::Box { status, .. } => assert_eq!(status, 400),
            other => panic!("{other:?}"),
        }
    }
}
