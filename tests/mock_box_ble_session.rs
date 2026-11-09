//! Contract tests for held BLE GATT sessions (`session_*` on `/ble/command`).
//!
//! Shapes match `lager/box/lager/http_handlers/ble.py`. Compat gate is
//! [`lager::nets::ble::SESSION_SINCE`] (planned box >= 0.63.0).

use httpmock::prelude::*;
use lager::{BleSessionOptions, Error, GattChar, SESSION_SINCE};
use serde_json::json;

const ADDR: &str = "AA:BB:CC:DD:EE:01";
const CHAR: &str = "00002a19-0000-1000-8000-00805f9b34fb";
const SID: &str = "abcdef0123456789abcdef0123456789";

fn mock_open<'a>(server: &'a MockServer) -> httpmock::Mock<'a> {
    server.mock(|when, then| {
        when.method(POST).path("/ble/command").json_body(json!({
            "action": "session_open",
            "params": {"address": ADDR, "timeout": 10.0, "max_lifetime": 300.0, "idle_timeout": 60.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "session_open",
            "value": {"session_id": SID, "address": ADDR, "max_lifetime": 300.0, "idle_timeout": 60.0}
        }));
    })
}

fn mock_close<'a>(server: &'a MockServer) -> httpmock::Mock<'a> {
    server.mock(|when, then| {
        when.method(POST).path("/ble/command").json_body(json!({
            "action": "session_close",
            "params": {"session_id": SID}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "session_close",
            "value": {"closed": true, "session_id": SID}
        }));
    })
}

#[cfg(feature = "blocking")]
mod blocking {
    use super::*;
    use lager::LagerBox;

    const CHAR2: &str = "00002a1a-0000-1000-8000-00805f9b34fb";

    fn mock_poll_reply<'a>(
        server: &'a MockServer,
        char_json: serde_json::Value,
    ) -> httpmock::Mock<'a> {
        server.mock(|when, then| {
            when.method(POST).path("/ble/command").body_contains("session_notify_poll");
            then.status(200).json_body(json!({
                "success": true, "action": "session_notify_poll",
                "value": {"session_id": SID, "messages": [{"char": char_json, "n": 1, "ts": 1.5, "data": "01"}], "timed_out": false, "ended": null}
            }));
        })
    }

    #[test]
    fn session_open_pair_write_poll_and_close() {
        let server = MockServer::start();
        let open = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_open",
                "params": {"address": ADDR, "timeout": 10.0, "max_lifetime": 300.0, "idle_timeout": 60.0}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_open",
                "message": "Opened BLE session",
                "value": {"session_id": SID, "address": ADDR, "max_lifetime": 300.0, "idle_timeout": 60.0}
            }));
        });
        let pair = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_pair",
                "params": {"session_id": SID, "timeout": 10.0}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_pair",
                "value": {"session_id": SID, "address": ADDR, "paired": true}
            }));
        });
        let write = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_write",
                "params": {"session_id": SID, "timeout": 10.0, "char": CHAR,
                           "data": "01ff", "response": true}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_write",
                "value": {"session_id": SID, "char": CHAR, "length": 2}
            }));
        });
        let start = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_notify_start",
                "params": {"session_id": SID, "timeout": 10.0, "char": CHAR}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_notify_start",
                "value": {"session_id": SID, "char": CHAR}
            }));
        });
        let poll = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_notify_poll",
                "params": {"session_id": SID, "max_messages": 8, "wait": 1.5}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_notify_poll",
                "value": {
                    "session_id": SID,
                    "messages": [
                        {"char": CHAR, "n": 1, "ts": 1700000000.25, "data": "01"},
                        {"char": CHAR2, "n": 7, "ts": 1700000001.5, "data": "0203"}
                    ],
                    "timed_out": true
                }
            }));
        });
        let close = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_close",
                "params": {"session_id": SID}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_close",
                "value": {"closed": true, "session_id": SID}
            }));
        });

        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        let session = lager
            .ble()
            .session_open(ADDR, &BleSessionOptions::default())
            .unwrap();
        assert_eq!(session.session_id(), SID);
        assert_eq!(session.address(), ADDR);
        session.pair().unwrap();
        session
            .write_with_response(CHAR, &[0x01, 0xff], true)
            .unwrap();
        session.notify_start(CHAR).unwrap();
        let n = session.notify_poll(None, 8, 1.5).unwrap();
        assert!(n.timed_out);
        assert_eq!(n.ended, None);
        assert_eq!(n.messages.len(), 2);
        assert_eq!(n.messages[0].seq, 1);
        assert_eq!(n.messages[0].timestamp, 1700000000.25);
        assert_eq!(n.messages[1].seq, 7);
        assert_eq!(n.messages[1].timestamp, 1700000001.5);
        assert_eq!(n.messages[0].characteristic, GattChar::Uuid(CHAR.into()));
        assert_eq!(n.messages[0].data, vec![0x01]);
        assert_eq!(n.messages[1].characteristic, GattChar::Uuid(CHAR2.into()));
        assert_eq!(n.messages[1].data, vec![0x02, 0x03]);
        session.close().unwrap();

        open.assert();
        pair.assert();
        write.assert();
        start.assert();
        poll.assert();
        close.assert();
    }

    #[test]
    fn session_read_and_poll_char_filter() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_open",
                "params": {"address": ADDR, "timeout": 10.0, "max_lifetime": 300.0, "idle_timeout": 60.0}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_open",
                "value": {"session_id": SID, "address": ADDR, "max_lifetime": 300.0, "idle_timeout": 60.0}
            }));
        });
        let read = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_read",
                "params": {"session_id": SID, "timeout": 10.0, "char": 42}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_read",
                "value": {"session_id": SID, "char": 42, "data": "ab"}
            }));
        });
        let poll = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_notify_poll",
                "params": {"session_id": SID, "max_messages": 1, "wait": 0.0, "char": CHAR}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_notify_poll",
                "value": {
                    "session_id": SID,
                    "messages": [{"char": CHAR, "n": 1, "ts": 2.0, "data": "ff"}],
                    "timed_out": false
                }
            }));
        });
        server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_close",
                "params": {"session_id": SID}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_close",
                "value": {"closed": true, "session_id": SID}
            }));
        });

        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        let session = lager
            .ble()
            .session_open(ADDR, &BleSessionOptions::default())
            .unwrap();
        assert_eq!(session.read(42u16).unwrap(), vec![0xab]);
        let n = session.notify_poll(Some(CHAR.into()), 1, 0.0).unwrap();
        assert!(!n.timed_out);
        assert_eq!(n.messages[0].data, vec![0xff]);
        session.close().unwrap();
        read.assert();
        poll.assert();
    }

    #[test]
    fn session_drop_closes_best_effort() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_open",
                "params": {"address": ADDR, "timeout": 10.0, "max_lifetime": 300.0, "idle_timeout": 60.0}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_open",
                "value": {"session_id": SID, "address": ADDR, "max_lifetime": 300.0, "idle_timeout": 60.0}
            }));
        });
        let close = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_close",
                "params": {"session_id": SID}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_close",
                "value": {"closed": true, "session_id": SID}
            }));
        });
        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        {
            let _session = lager
                .ble()
                .session_open(ADDR, &BleSessionOptions::default())
                .unwrap();
        }
        close.assert();
    }

    #[test]
    fn session_open_on_an_older_box_is_unsupported() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/ble/command");
            then.status(400).json_body(json!({
                "success": false,
                "error": "action (scan|info|connect|disconnect|read|write|notify|pair) is required"
            }));
        });
        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        let err = match lager
            .ble()
            .session_open(ADDR, &BleSessionOptions::default())
        {
            Err(e) => e,
            Ok(session) => {
                let _ = session.close();
                panic!("expected UnsupportedByBox");
            }
        };
        match err {
            Error::UnsupportedByBox { message } => {
                assert!(message.contains("'session_open'"), "{message}");
                assert!(message.contains(SESSION_SINCE), "{message}");
            }
            other => panic!("expected UnsupportedByBox, got {other:?}"),
        }
    }

    #[test]
    fn handle_round_trips_through_notify_and_poll() {
        let server = MockServer::start();
        mock_open(&server);
        mock_close(&server);
        let start = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_notify_start",
                "params": {"session_id": SID, "timeout": 10.0, "char": 42}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_notify_start",
                "value": {"session_id": SID, "char": 42}
            }));
        });
        mock_poll_reply(&server, json!(42));
        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        let session = lager
            .ble()
            .session_open(ADDR, &BleSessionOptions::default())
            .unwrap();
        session.notify_start(42u16).unwrap();
        let n = session.notify_poll(Some(42u16.into()), 1, 0.0).unwrap();
        assert_eq!(n.messages[0].characteristic, GattChar::Handle(42));
        start.assert();
    }

    #[test]
    fn out_of_range_handle_in_poll_is_a_decode_error() {
        let server = MockServer::start();
        mock_open(&server);
        mock_close(&server);
        mock_poll_reply(&server, json!(70000));
        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        let session = lager
            .ble()
            .session_open(ADDR, &BleSessionOptions::default())
            .unwrap();
        let err = session.notify_poll(None, 1, 0.0).unwrap_err();
        assert!(matches!(err, Error::Decode(_)), "{err:?}");
    }

    #[test]
    fn op_timeout_is_sent_as_timeout() {
        let server = MockServer::start();
        let open = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_open",
                "params": {"address": ADDR, "timeout": 3.0, "max_lifetime": 120.0, "idle_timeout": 15.0}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_open",
                "value": {"session_id": SID, "address": ADDR, "max_lifetime": 300.0, "idle_timeout": 60.0}
            }));
        });
        mock_close(&server);
        let read = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_read",
                "params": {"session_id": SID, "timeout": 25.0, "char": CHAR}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_read",
                "value": {"session_id": SID, "char": CHAR, "data": "01"}
            }));
        });
        let opts = BleSessionOptions {
            connect_timeout: 3.0,
            op_timeout: 25.0,
            max_lifetime: 120.0,
            idle_timeout: 15.0,
        };
        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        let session = lager.ble().session_open(ADDR, &opts).unwrap();
        session.read(CHAR).unwrap();
        open.assert();
        read.assert();
    }

    #[test]
    fn notify_poll_sends_char_only_when_given() {
        let server = MockServer::start();
        mock_open(&server);
        mock_close(&server);
        let with_char = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_notify_poll",
                "params": {"session_id": SID, "max_messages": 2, "wait": 0.5, "char": CHAR}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_notify_poll",
                "value": {"session_id": SID, "messages": [], "timed_out": true}
            }));
        });
        let without_char = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_notify_poll",
                "params": {"session_id": SID, "max_messages": 3, "wait": 0.5}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_notify_poll",
                "value": {"session_id": SID, "messages": [], "timed_out": true}
            }));
        });
        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        let session = lager
            .ble()
            .session_open(ADDR, &BleSessionOptions::default())
            .unwrap();
        session.notify_poll(Some(CHAR.into()), 2, 0.5).unwrap();
        session.notify_poll(None, 3, 0.5).unwrap();
        with_char.assert();
        without_char.assert();
    }

    #[test]
    fn custom_lifetimes_are_sent_on_open() {
        let server = MockServer::start();
        let open = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_open",
                "params": {"address": ADDR, "timeout": 10.0, "max_lifetime": 900.0, "idle_timeout": 5.0}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_open",
                "value": {"session_id": SID, "address": ADDR, "max_lifetime": 900.0, "idle_timeout": 5.0}
            }));
        });
        mock_close(&server);
        let opts = BleSessionOptions {
            max_lifetime: 900.0,
            idle_timeout: 5.0,
            ..BleSessionOptions::default()
        };
        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        lager.ble().session_open(ADDR, &opts).unwrap();
        open.assert();
    }

    fn poll_with_value(server: &MockServer, value: serde_json::Value) {
        server.mock(|when, then| {
            when.method(POST)
                .path("/ble/command")
                .body_contains("session_notify_poll");
            then.status(200).json_body(json!({
                "success": true, "action": "session_notify_poll", "value": value
            }));
        });
    }

    #[test]
    fn poll_reports_why_the_session_ended() {
        let server = MockServer::start();
        mock_open(&server);
        mock_close(&server);
        poll_with_value(
            &server,
            json!({
                "session_id": SID,
                "messages": [{"char": CHAR, "n": 4, "ts": 3.0, "data": "09"}],
                "timed_out": false,
                "ended": "peer disconnected"
            }),
        );
        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        let session = lager
            .ble()
            .session_open(ADDR, &BleSessionOptions::default())
            .unwrap();
        let n = session.notify_poll(None, 8, 0.0).unwrap();
        assert_eq!(n.ended.as_deref(), Some("peer disconnected"));
        assert_eq!(n.messages.len(), 1);
        assert_eq!(n.messages[0].data, vec![0x09]);
    }

    #[test]
    fn poll_without_n_or_ts_is_a_decode_error() {
        for message in [
            json!({"char": CHAR, "ts": 1.0, "data": "01"}),
            json!({"char": CHAR, "n": 1, "data": "01"}),
        ] {
            let server = MockServer::start();
            mock_open(&server);
            mock_close(&server);
            poll_with_value(
                &server,
                json!({
                    "session_id": SID, "messages": [message], "timed_out": false, "ended": null
                }),
            );
            let lager = LagerBox::connect(server.address().to_string()).unwrap();
            let session = lager
                .ble()
                .session_open(ADDR, &BleSessionOptions::default())
                .unwrap();
            let err = session.notify_poll(None, 1, 0.0).unwrap_err();
            assert!(matches!(err, Error::Decode(_)), "{err:?}");
        }
    }

    #[test]
    fn poll_without_ended_means_live_session() {
        let server = MockServer::start();
        mock_open(&server);
        mock_close(&server);
        poll_with_value(
            &server,
            json!({"session_id": SID, "messages": [], "timed_out": true}),
        );
        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        let session = lager
            .ble()
            .session_open(ADDR, &BleSessionOptions::default())
            .unwrap();
        let n = session.notify_poll(None, 1, 0.0).unwrap();
        assert_eq!(n.ended, None);
        assert!(n.timed_out);
    }

    #[test]
    fn force_close_sends_force_without_session_id() {
        let server = MockServer::start();
        let close = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_close",
                "params": {"force": true}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_close",
                "value": {"closed": true, "session_id": null}
            }));
        });
        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        lager.ble().session_force_close().unwrap();
        close.assert();
    }

    #[test]
    fn force_close_on_an_older_box_names_session_close() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/ble/command");
            then.status(400).json_body(json!({
                "success": false,
                "error": "action (scan|info|connect|disconnect|read|write|notify|pair) is required"
            }));
        });
        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        match lager.ble().session_force_close().unwrap_err() {
            Error::UnsupportedByBox { message } => {
                assert!(message.contains("'session_close'"), "{message}")
            }
            other => panic!("expected UnsupportedByBox, got {other:?}"),
        }
    }

    #[test]
    fn failed_close_is_retried_on_drop() {
        let server = MockServer::start();
        mock_open(&server);
        let close = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_close",
                "params": {"session_id": SID}
            }));
            then.status(502).json_body(json!({
                "success": false, "error": "BLE error: busy"
            }));
        });
        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        let session = lager
            .ble()
            .session_open(ADDR, &BleSessionOptions::default())
            .unwrap();
        assert!(session.close().is_err());
        close.assert_hits(2);
    }

    #[test]
    fn open_conflict_surfaces_the_box_message() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(POST).path("/ble/command");
            then.status(409).json_body(json!({
                "success": false, "error": "BLE session in use"
            }));
        });
        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        let err = match lager
            .ble()
            .session_open(ADDR, &BleSessionOptions::default())
        {
            Err(e) => e,
            Ok(_) => panic!("expected a 409"),
        };
        match err {
            Error::Box { status, message } => {
                assert_eq!(status, 409);
                assert!(message.contains("BLE session in use"), "{message}");
            }
            other => panic!("expected Error::Box, got {other:?}"),
        }
    }

    #[test]
    fn ended_session_error_is_not_unsupported() {
        let server = MockServer::start();
        mock_open(&server);
        mock_close(&server);
        let ended = format!("BLE session {SID} ended: peer disconnected");
        server.mock(|when, then| {
            when.method(POST)
                .path("/ble/command")
                .body_contains("session_read");
            then.status(400)
                .json_body(json!({"success": false, "error": ended}));
        });
        let lager = LagerBox::connect(server.address().to_string()).unwrap();
        let session = lager
            .ble()
            .session_open(ADDR, &BleSessionOptions::default())
            .unwrap();
        match session.read(CHAR).unwrap_err() {
            Error::Box { status, message } => {
                assert_eq!(status, 400);
                assert!(message.contains("ended: peer disconnected"), "{message}");
            }
            other => panic!("expected Error::Box, got {other:?}"),
        }
    }
}

#[cfg(feature = "async")]
mod async_parity {
    use std::time::{Duration, Instant};

    use super::*;
    use lager::AsyncLagerBox;

    #[tokio::test]
    async fn async_open_notify_poll_close_sends_identical_requests() {
        let server = MockServer::start_async().await;
        let open = mock_open(&server);
        let start = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_notify_start",
                "params": {"session_id": SID, "timeout": 10.0, "char": CHAR}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_notify_start",
                "value": {"session_id": SID, "char": CHAR}
            }));
        });
        let poll = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_notify_poll",
                "params": {"session_id": SID, "max_messages": 8, "wait": 1.5}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_notify_poll",
                "value": {
                    "session_id": SID,
                    "messages": [{"char": CHAR, "n": 1, "ts": 2.0, "data": "01"}],
                    "timed_out": false,
                    "ended": null
                }
            }));
        });
        let close = mock_close(&server);

        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        let session = lager
            .ble()
            .session_open(ADDR, &BleSessionOptions::default())
            .await
            .unwrap();
        session.notify_start(CHAR).await.unwrap();
        let n = session.notify_poll(None, 8, 1.5).await.unwrap();
        assert!(!n.timed_out);
        assert_eq!(n.ended, None);
        assert_eq!(n.messages[0].seq, 1);
        assert_eq!(n.messages[0].timestamp, 2.0);
        assert_eq!(n.messages[0].characteristic, GattChar::Uuid(CHAR.into()));
        session.close().await.unwrap();

        open.assert();
        start.assert();
        poll.assert();
        close.assert();
    }

    #[tokio::test]
    async fn async_session_open_on_an_older_box_is_unsupported() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(POST).path("/ble/command");
            then.status(400).json_body(json!({
                "success": false,
                "error": "action (scan|info|connect|disconnect|read|write|notify|pair) is required"
            }));
        });
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        let result = lager
            .ble()
            .session_open(ADDR, &BleSessionOptions::default())
            .await;
        match result {
            Err(Error::UnsupportedByBox { message }) => {
                assert!(message.contains(SESSION_SINCE), "{message}");
            }
            Err(other) => panic!("expected UnsupportedByBox, got {other:?}"),
            Ok(_) => panic!("expected UnsupportedByBox"),
        }
    }

    #[tokio::test]
    async fn async_drop_closes_the_session() {
        let server = MockServer::start_async().await;
        mock_open(&server);
        let close = mock_close(&server);
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        let session = lager
            .ble()
            .session_open(ADDR, &BleSessionOptions::default())
            .await
            .unwrap();
        drop(session);

        let deadline = Instant::now() + Duration::from_secs(5);
        while close.hits_async().await == 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        close.assert_async().await;
    }

    #[tokio::test]
    async fn async_force_close_sends_force_without_session_id() {
        let server = MockServer::start_async().await;
        let close = server.mock(|when, then| {
            when.method(POST).path("/ble/command").json_body(json!({
                "action": "session_close",
                "params": {"force": true}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": "session_close",
                "value": {"closed": true, "session_id": null}
            }));
        });
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        lager.ble().session_force_close().await.unwrap();
        close.assert_async().await;
    }
}
