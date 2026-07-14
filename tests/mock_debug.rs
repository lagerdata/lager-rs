//! Contract tests for the debug net against a mock debug service.
//!
//! These pin down the exact wire protocol the crate speaks to the box debug
//! service on port 8765 (`box/lager/debug/service.py`): the full saved-net
//! record is resolved from `/nets/list` and wrapped as the `net` object, and
//! responses use a `status` field rather than the `success` envelope.
//!
//! Both the port-9000 endpoints and the debug service are pointed at one
//! mock server via `debug_service_url`, since httpmock serves a single port.

#![cfg(feature = "blocking")]

use httpmock::prelude::*;
use lager::{Error, FirmwareKind, LagerBox};
use serde_json::{json, Value};

fn debug_record() -> Value {
    json!({
        "name": "debug1",
        "role": "debug",
        "pin": "nrf52840_xxaa",
        "instrument": "jlink1",
        "address": "jlink://000440012345"
    })
}

/// A client whose :9000 and debug (:8765) traffic both hit `server`.
fn client(server: &MockServer) -> LagerBox {
    LagerBox::builder(server.address().to_string())
        .debug_service_url(server.base_url())
        .build()
        .unwrap()
}

fn mock_nets_list(server: &MockServer) -> httpmock::Mock<'_> {
    server.mock(|when, then| {
        when.method(GET).path("/nets/list");
        then.status(200).json_body(json!([debug_record()]));
    })
}

#[test]
fn connect_resolves_record_and_sends_defaults() {
    let server = MockServer::start();
    let nets = mock_nets_list(&server);
    let connect = server.mock(|when, then| {
        when.method(POST).path("/debug/connect").json_body(json!({
            "net": debug_record(),
            "force": false,
            "halt": false,
            "gdb": true
        }));
        then.status(200).json_body(json!({
            "status": "connected",
            "device": "nrf52840_xxaa",
            "probe": "jlink1",
            "serial": "000440012345",
            "backend": "jlink",
            "message": "JLinkGDBServer ready for operations",
            "pid": 4321,
            "gdb_server": {
                "status": "started",
                "gdb_port": 2331,
                "swo_port": 2332,
                "telnet_port": 2333,
                "rtt_telnet_port": 9090,
                "pid": 4321
            }
        }));
    });

    let lager = client(&server);
    let conn = lager.debug("debug1").connect().unwrap();

    assert_eq!(conn.status.as_deref(), Some("connected"));
    assert_eq!(conn.backend.as_deref(), Some("jlink"));
    assert_eq!(conn.pid, Some(4321));
    assert_eq!(conn.gdb_server.unwrap().gdb_port, Some(2331));
    nets.assert();
    connect.assert();
}

#[test]
fn flash_sends_base64_hexfile() {
    let server = MockServer::start();
    let nets = mock_nets_list(&server);
    // "foobar" -> base64 "Zm9vYmFy" (verified against reference vectors).
    let flash = server.mock(|when, then| {
        when.method(POST).path("/debug/flash").json_body(json!({
            "net": debug_record(),
            "hexfile": { "content": "Zm9vYmFy" }
        }));
        then.status(200)
            .json_body(json!({"status": "flash_complete", "output": [], "backend": "jlink"}));
    });

    let lager = client(&server);
    lager
        .debug("debug1")
        .flash_bytes(b"foobar", FirmwareKind::Hex, None)
        .unwrap();
    nets.assert();
    flash.assert();
}

#[test]
fn flash_bin_carries_address() {
    let server = MockServer::start();
    let nets = mock_nets_list(&server);
    let flash = server.mock(|when, then| {
        when.method(POST).path("/debug/flash").json_body(json!({
            "net": debug_record(),
            "binfile": { "content": "Zm9v", "address": 0x2000_0000u32 }
        }));
        then.status(200)
            .json_body(json!({"status": "flash_complete", "output": "ok", "backend": "openocd"}));
    });

    let lager = client(&server);
    lager
        .debug("debug1")
        .flash_bytes(b"foo", FirmwareKind::Bin, Some(0x2000_0000))
        .unwrap();
    nets.assert();
    flash.assert();
}

#[test]
fn read_memory_decodes_hex() {
    let server = MockServer::start();
    let nets = mock_nets_list(&server);
    let memrd = server.mock(|when, then| {
        when.method(POST).path("/debug/memrd").json_body(json!({
            "net": debug_record(),
            "start_addr": 0x0800_0000u64,
            "length": 4
        }));
        then.status(200).json_body(json!({
            "status": "read_complete",
            "address": "0x8000000",
            "length": 4,
            "data": "deadbeef"
        }));
    });

    let lager = client(&server);
    let bytes = lager.debug("debug1").read_memory(0x0800_0000, 4).unwrap();
    assert_eq!(bytes, vec![0xde, 0xad, 0xbe, 0xef]);
    nets.assert();
    memrd.assert();
}

#[test]
fn reset_sends_halt_flag() {
    let server = MockServer::start();
    let nets = mock_nets_list(&server);
    let reset = server.mock(|when, then| {
        when.method(POST).path("/debug/reset").json_body(json!({
            "net": debug_record(),
            "halt": true
        }));
        then.status(200).json_body(json!({
            "status": "reset_complete", "halt": true, "output": [], "backend": "jlink"
        }));
    });

    let lager = client(&server);
    lager.debug("debug1").reset(true).unwrap();
    nets.assert();
    reset.assert();
}

#[test]
fn status_reports_connection() {
    let server = MockServer::start();
    let nets = mock_nets_list(&server);
    server.mock(|when, then| {
        when.method(POST).path("/debug/status");
        then.status(200).json_body(json!({
            "connected": true, "pid": 4321, "serial": "000440012345", "backend": "jlink"
        }));
    });

    let lager = client(&server);
    let st = lager.debug("debug1").status().unwrap();
    assert!(st.connected);
    assert_eq!(st.pid, Some(4321));
    nets.assert();
}

#[test]
fn service_error_maps_to_box_error() {
    let server = MockServer::start();
    let nets = mock_nets_list(&server);
    server.mock(|when, then| {
        when.method(POST).path("/debug/reset");
        then.status(400)
            .json_body(json!({"error": "No debugger connection found", "status": "error"}));
    });

    let lager = client(&server);
    let err = lager.debug("debug1").reset(false).unwrap_err();
    match err {
        Error::Box { status, message } => {
            assert_eq!(status, 400);
            assert!(message.contains("No debugger connection"));
        }
        other => panic!("expected Error::Box, got {other:?}"),
    }
    nets.assert();
}

#[test]
fn unknown_debug_net_is_rejected_before_any_debug_call() {
    let server = MockServer::start();
    let nets = mock_nets_list(&server);
    // No /debug/* mock: resolution must fail first.
    let lager = client(&server);
    let err = lager.debug("does_not_exist").status().unwrap_err();
    match err {
        Error::Box { status, message } => {
            assert_eq!(status, 404);
            assert!(message.contains("not found"));
        }
        other => panic!("expected 404 Error::Box, got {other:?}"),
    }
    nets.assert();
}

#[test]
fn non_debug_net_name_is_rejected() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/nets/list");
        then.status(200)
            .json_body(json!([{"name": "supply1", "role": "supply"}]));
    });
    let lager = client(&server);
    let err = lager.debug("supply1").info().unwrap_err();
    match err {
        Error::Box { status: 404, message } => assert!(message.contains("not a debug net")),
        other => panic!("expected 404 Error::Box, got {other:?}"),
    }
}
