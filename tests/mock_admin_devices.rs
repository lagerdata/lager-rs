//! Contract tests for the `admin` feature's custom-device assignment and
//! diagnostics: the exact request each method sends and the response shapes
//! from `box/lager/http_handlers/{custom_devices_handler,diagnose}.py`.

#![cfg(all(feature = "admin", feature = "blocking"))]

use httpmock::prelude::*;
use lager::{CableId, Error, LagerBox};
use serde_json::json;

fn client(server: &MockServer) -> LagerBox {
    LagerBox::connect(server.address().to_string()).unwrap()
}

fn route_missing(server: &MockServer, method: httpmock::Method, path: &str) {
    server.mock(|when, then| {
        when.method(method).path(path);
        then.status(404).json_body(json!({
            "error": "Not found", "message": "The requested endpoint does not exist"
        }));
    });
}

fn assert_box(err: Error, want_status: u16, needle: &str) {
    match err {
        Error::Box { status, message } => {
            assert_eq!(status, want_status, "{message}");
            assert!(message.contains(needle), "{message}");
        }
        other => panic!("expected Error::Box, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Custom devices
// ---------------------------------------------------------------------------

#[test]
fn custom_devices_decode_catalog_assignments_and_cables() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/custom-devices/list");
        then.status(200).json_body(json!({
            "catalog": [{"name": "Acme_Chamber", "display_name": "Acme chamber",
                         "manufacturer": "Acme", "roles": ["thermal"],
                         "channels": {"thermal": ["1"]}, "transport": "serial",
                         "default_baud": 9600}],
            "assignments": [{"instrument": "Acme_Chamber", "vid": "0403", "pid": "6001",
                             "serial": "A1", "port_path": null, "baud": 9600,
                             "address": "serial:A1", "tty": "/dev/ttyUSB0"}],
            "cables": [{"vid": "0403", "pid": "6015", "serial": null, "port_path": "1-1.4",
                        "tty": "/dev/ttyUSB1", "manufacturer": "FTDI",
                        "product": "FT231X", "interface": null,
                        "by_id": null, "by_path": "/dev/serial/by-path/x"}]
        }));
    });
    let state = client(&server).admin().custom_devices().unwrap();
    assert_eq!(state.catalog[0].default_baud, Some(9600));
    assert_eq!(state.catalog[0].roles, vec!["thermal"]);
    let a = &state.assignments[0];
    assert_eq!(
        (a.serial.as_deref(), a.address.as_deref()),
        (Some("A1"), Some("serial:A1"))
    );
    assert_eq!(a.extra["vid"], "0403");
    assert_eq!(state.cables[0].port_path.as_deref(), Some("1-1.4"));
    assert_eq!(state.cables[0].extra["product"], "FT231X");
}

#[test]
fn assign_by_serial_sends_both_keys_and_the_baud() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/custom-devices/assign")
            .json_body(json!({
                "instrument": "Acme_Chamber", "serial": "A1", "port_path": null, "baud": 19200
            }));
        then.status(200).json_body(json!({
            "instrument": "Acme_Chamber", "vid": "0403", "pid": "6001", "serial": "A1",
            "port_path": null, "baud": 19200, "address": "serial:A1",
            "tty": "/dev/ttyUSB0", "roles": ["thermal"], "channels": {},
            "deleted_nets": ["uart1"]
        }));
    });
    let assigned = client(&server)
        .admin()
        .assign_custom_device("Acme_Chamber", &CableId::Serial("A1".into()), Some(19200))
        .unwrap();
    assert_eq!(assigned.deleted_nets, vec!["uart1"]);
    assert_eq!(assigned.assignment.baud, Some(19200));
    assert!(!assigned.assignment.extra.contains_key("deleted_nets"));
    m.assert();
}

#[test]
fn assign_by_port_without_baud() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/custom-devices/assign")
            .json_body(json!({
                "instrument": "Acme_Chamber", "serial": null, "port_path": "1-1.4"
            }));
        then.status(200).json_body(json!({
            "instrument": "Acme_Chamber", "serial": null, "port_path": "1-1.4",
            "address": "usb-port:1-1.4", "tty": "/dev/ttyUSB1", "roles": [],
            "channels": {}, "deleted_nets": []
        }));
    });
    let assigned = client(&server)
        .admin()
        .assign_custom_device("Acme_Chamber", &CableId::PortPath("1-1.4".into()), None)
        .unwrap();
    assert_eq!(
        assigned.assignment.address.as_deref(),
        Some("usb-port:1-1.4")
    );
    m.assert();
}

#[test]
fn assign_refusal_is_a_box_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/custom-devices/assign");
        then.status(400)
            .json_body(json!({"error": "No connected cable has serial 'Z9'"}));
    });
    let err = client(&server)
        .admin()
        .assign_custom_device("Acme_Chamber", &CableId::Serial("Z9".into()), None)
        .unwrap_err();
    assert_box(err, 400, "No connected cable");
}

#[test]
fn remove_reports_what_matched() {
    let server = MockServer::start();
    let hit = server.mock(|when, then| {
        when.method(POST)
            .path("/custom-devices/remove")
            .json_body(json!({"serial": "A1", "port_path": null}));
        then.status(200).json_body(json!({
            "removed": true, "instrument": "Acme_Chamber", "address": "serial:A1",
            "deleted_nets": ["chamber1"]
        }));
    });
    let miss = server.mock(|when, then| {
        when.method(POST)
            .path("/custom-devices/remove")
            .json_body(json!({"serial": null, "port_path": "9-9"}));
        then.status(200).json_body(json!({"removed": false}));
    });
    let lager = client(&server);
    let removed = lager
        .admin()
        .remove_custom_device(&CableId::Serial("A1".into()))
        .unwrap();
    assert!(removed.removed);
    assert_eq!(removed.deleted_nets, vec!["chamber1"]);
    let nothing = lager
        .admin()
        .remove_custom_device(&CableId::PortPath("9-9".into()))
        .unwrap();
    assert!(!nothing.removed);
    assert_eq!(nothing.instrument, None);
    hit.assert();
    miss.assert();
}

#[test]
fn custom_devices_on_an_older_box_is_unsupported() {
    let server = MockServer::start();
    route_missing(&server, GET, "/custom-devices/list");
    match client(&server).admin().custom_devices().unwrap_err() {
        Error::UnsupportedByBox { message } => {
            assert!(message.contains("/custom-devices"), "{message}")
        }
        other => panic!("expected UnsupportedByBox, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

const VISA: &str = "USB0::0x1AB1::0x0E11::DP8C1&2::INSTR";

#[test]
fn diagnose_usb_encodes_the_address() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/diagnose/usb")
            // The & would split the query if it were not encoded.
            .query_param("address", VISA);
        then.status(200).json_body(json!({
            "address": VISA, "enumerated": true, "sysfs_path": "/sys/bus/usb/devices/1-1",
            "device_path": "/dev/usbtmc0", "vid": "1ab1", "pid": "0e11",
            "serial": "DP8C1&2", "is_usbtmc": true, "lsof": [], "usbtmc_loaded": true,
            "dmesg_tail": ""
        }));
    });
    let d = client(&server).admin().diagnose_usb(VISA).unwrap();
    assert!(d.enumerated && d.is_usbtmc);
    assert_eq!(d.device_path.as_deref(), Some("/dev/usbtmc0"));
    assert_eq!(d.extra["usbtmc_loaded"], true);
    m.assert();
}

#[test]
fn diagnose_usb_bad_address_is_a_box_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/diagnose/usb");
        then.status(400).json_body(json!({
            "address": "LJ-T7", "error": "not a USB VISA address"
        }));
    });
    let err = client(&server).admin().diagnose_usb("LJ-T7").unwrap_err();
    assert_box(err, 400, "not a USB VISA address");
}

#[test]
fn diagnose_usbhub_decodes_the_hub_probe() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/diagnose/usbhub")
            .query_param("address", "USB0::0x24FF::0x0013::H1::INSTR");
        then.status(200).json_body(json!({
            "address": "USB0::0x24FF::0x0013::H1::INSTR", "enumerated": true,
            "hub_opens": false, "hub_open_error": "device busy",
            "hub_diagnostics_supported": true
        }));
    });
    let d = client(&server)
        .admin()
        .diagnose_usbhub("USB0::0x24FF::0x0013::H1::INSTR")
        .unwrap();
    assert_eq!(d.hub_opens, Some(false));
    assert_eq!(d.extra["hub_open_error"], "device busy");
}

#[test]
fn diagnose_visa_failure_and_skip_are_data() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/diagnose/visa")
            .query_param("address", "A");
        then.status(200).json_body(json!({
            "address": "A", "error": "VI_ERROR_RSRC_BUSY", "error_class": "busy",
            "elapsed_ms": 12.5
        }));
    });
    server.mock(|when, then| {
        when.method(GET)
            .path("/diagnose/visa")
            .query_param("address", "B");
        then.status(200).json_body(json!({
            "address": "B", "skipped": true, "reason": "hardware service holds a session"
        }));
    });
    let lager = client(&server);
    let busy = lager.admin().diagnose_visa("A").unwrap();
    assert_eq!(busy.error_class.as_deref(), Some("busy"));
    assert_eq!(busy.idn, None);
    let skipped = lager.admin().diagnose_visa("B").unwrap();
    assert!(skipped.skipped);
    assert_eq!(skipped.extra["reason"], "hardware service holds a session");
}

#[test]
fn diagnose_jlink_sends_the_net() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/diagnose/jlink")
            .query_param("net", "debug 1");
        then.status(200).json_body(json!({
            "address": "000123456", "device": "NRF52840_XXAA", "backend": "jlink",
            "serial": "000123456", "probe_enumerated": true,
            "connect": {"vtref_mv": 3300, "connect_ok": true}
        }));
    });
    let d = client(&server).admin().diagnose_jlink("debug 1").unwrap();
    assert_eq!(d.backend.as_deref(), Some("jlink"));
    assert_eq!(d.extra["connect"]["connect_ok"], true);
    m.assert();
}

#[test]
fn diagnose_on_an_older_box_is_unsupported() {
    let server = MockServer::start();
    route_missing(&server, GET, "/diagnose/visa");
    match client(&server).admin().diagnose_visa("A").unwrap_err() {
        Error::UnsupportedByBox { message } => assert!(message.contains("/diagnose"), "{message}"),
        other => panic!("expected UnsupportedByBox, got {other:?}"),
    }
}

#[cfg(feature = "async")]
mod async_parity {
    use super::*;
    use lager::AsyncLagerBox;

    #[tokio::test]
    async fn async_assign_sends_identical_request() {
        let server = MockServer::start_async().await;
        let m = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/custom-devices/assign")
                    .json_body(json!({
                        "instrument": "Acme_Chamber", "serial": "A1", "port_path": null
                    }));
                then.status(200).json_body(json!({
                    "instrument": "Acme_Chamber", "serial": "A1", "deleted_nets": []
                }));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        lager
            .admin()
            .assign_custom_device("Acme_Chamber", &CableId::Serial("A1".into()), None)
            .await
            .unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn async_diagnose_jlink_sends_the_net() {
        let server = MockServer::start_async().await;
        let m = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/diagnose/jlink")
                    .query_param("net", "debug1");
                then.status(200).json_body(json!({"backend": "openocd"}));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        let d = lager.admin().diagnose_jlink("debug1").await.unwrap();
        assert_eq!(d.backend.as_deref(), Some("openocd"));
        m.assert_async().await;
    }
}
