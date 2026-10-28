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
fn connect_with_carries_base64_scripts_and_halt() {
    use lager::ConnectOptions;

    let server = MockServer::start();
    let nets = mock_nets_list(&server);
    // "foobar" -> base64 "Zm9vYmFy" (verified against reference vectors);
    // "baz" -> "YmF6".
    let connect = server.mock(|when, then| {
        when.method(POST).path("/debug/connect").json_body(json!({
            "net": debug_record(),
            "force": true,
            "halt": true,
            "gdb": true,
            "jlink_script": "Zm9vYmFy",
            "openocd_config": "YmF6"
        }));
        then.status(200).json_body(json!({
            "status": "connected",
            "backend": "jlink",
            "message": "ready"
        }));
    });

    let lager = client(&server);
    lager
        .debug("debug1")
        .connect_with(&ConnectOptions {
            force: true,
            halt: true,
            jlink_script: Some(b"foobar".to_vec()),
            openocd_config: Some(b"baz".to_vec()),
            ..Default::default()
        })
        .unwrap();
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
            "status": "reset_complete", "halt": true,
            "output": ["Reset delay: 0 ms", "Reset type NORMAL"], "backend": "jlink"
        }));
    });

    let lager = client(&server);
    let out = lager.debug("debug1").reset(true).unwrap();
    assert_eq!(out.output, vec!["Reset delay: 0 ms", "Reset type NORMAL"]);
    assert_eq!(out.backend.as_deref(), Some("jlink"));
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
        Error::Box {
            status: 404,
            message,
        } => assert!(message.contains("not a debug net")),
        other => panic!("expected 404 Error::Box, got {other:?}"),
    }
}

#[test]
fn net_record_is_cached_across_ops_on_one_handle() {
    // Back-to-back debug ops on the same handle must resolve /nets/list
    // once, not per call (the `claim+discover` perf feedback).
    let server = MockServer::start();
    let nets = mock_nets_list(&server);
    server.mock(|when, then| {
        when.method(POST).path("/debug/status");
        then.status(200)
            .json_body(json!({"connected": true, "backend": "jlink"}));
    });
    server.mock(|when, then| {
        when.method(POST).path("/debug/reset");
        then.status(200)
            .json_body(json!({"status": "reset_complete", "output": []}));
    });

    let lager = client(&server);
    let debug = lager.debug("debug1");
    debug.status().unwrap();
    debug.reset(false).unwrap();
    debug.status().unwrap();
    nets.assert(); // exactly one /nets/list fetch for three ops
}

#[test]
fn net_record_cache_is_invalidated_when_an_op_fails() {
    let server = MockServer::start();
    let nets = mock_nets_list(&server);
    server.mock(|when, then| {
        when.method(POST).path("/debug/reset");
        then.status(400)
            .json_body(json!({"error": "No debugger connection found", "status": "error"}));
    });
    server.mock(|when, then| {
        when.method(POST).path("/debug/status");
        then.status(200).json_body(json!({"connected": false}));
    });

    let lager = client(&server);
    let debug = lager.debug("debug1");
    debug.reset(false).unwrap_err(); // failure drops the cached record
    debug.status().unwrap(); // next op re-resolves it
    nets.assert_hits(2);
}

// ---------------------------------------------------------------------------
// Box 0.44.0 - 0.59.0 catch-up. Response bodies are copied from the shapes
// `box/lager/debug/service.py` sends.
// ---------------------------------------------------------------------------

#[test]
fn flash_that_programmed_nothing_is_an_error() {
    // The J-Link path answers 200 with `programmed: false` (box >= 0.51.0).
    let server = MockServer::start();
    let nets = mock_nets_list(&server);
    server.mock(|when, then| {
        when.method(POST).path("/debug/flash");
        then.status(200).json_body(json!({
            "status": "flash_complete",
            "output": ["Downloading file [fw.hex]...", "****** Error: Failed to download RAMCode."],
            "programmed": false,
            "error": "****** Error: Failed to download RAMCode."
        }));
    });

    let lager = client(&server);
    let debug = lager.debug("debug1");
    match debug
        .flash_bytes(b"foobar", FirmwareKind::Hex, None)
        .unwrap_err()
    {
        Error::FlashFailed { error, output } => {
            assert_eq!(error, "****** Error: Failed to download RAMCode.");
            assert_eq!(output.len(), 2);
        }
        other => panic!("expected Error::FlashFailed, got {other:?}"),
    }
    // A failed flash drops the cached record, like any other failure.
    debug.status().unwrap_err();
    nets.assert_hits(2);
}

#[test]
fn flash_report_carries_the_verdict_and_output() {
    let server = MockServer::start();
    mock_nets_list(&server);
    server.mock(|when, then| {
        when.method(POST).path("/debug/flash");
        then.status(200).json_body(json!({
            "status": "flash_complete",
            "output": ["** Programming Finished **", "** Verified OK **"],
            "backend": "openocd",
            "programmed": true,
            "error": null
        }));
    });

    let lager = client(&server);
    let report = lager
        .debug("debug1")
        .flash_bytes(b"foobar", FirmwareKind::Hex, None)
        .unwrap();
    assert_eq!(report.programmed, Some(true));
    assert_eq!(report.backend.as_deref(), Some("openocd"));
    assert_eq!(report.output.len(), 2);
}

#[test]
fn flash_against_a_box_without_a_verdict_still_succeeds() {
    // Box < 0.51.0: no `programmed`, no `backend`.
    let server = MockServer::start();
    mock_nets_list(&server);
    server.mock(|when, then| {
        when.method(POST).path("/debug/flash");
        then.status(200)
            .json_body(json!({"status": "flash_complete", "output": ["O.K."]}));
    });

    let lager = client(&server);
    let report = lager
        .debug("debug1")
        .flash_bytes(b"foobar", FirmwareKind::Hex, None)
        .unwrap();
    assert_eq!(report.programmed, None);
    assert_eq!(report.output, ["O.K."]);
}

#[test]
fn erase_reports_the_range_erased() {
    let server = MockServer::start();
    mock_nets_list(&server);
    server.mock(|when, then| {
        when.method(POST)
            .path("/debug/erase")
            .json_body(json!({"net": debug_record()}));
        then.status(200).json_body(json!({
            "status": "erase_complete",
            "output": "Erasing device...\nErasing done.",
            "backend": "jlink",
            "erase_range": {
                "start": 0, "end": 1048575, "length": 1048576,
                "source": "default", "text": "0x00000000-0x000FFFFF (1024 KiB)"
            }
        }));
    });

    let lager = client(&server);
    let report = lager.debug("debug1").erase().unwrap();
    let range = report.erase_range.unwrap();
    assert_eq!(
        (range.start, range.end, range.length),
        (0, 0xF_FFFF, 0x10_0000)
    );
    assert_eq!(range.source, "default");
    assert_eq!(report.backend.as_deref(), Some("jlink"));
}

#[test]
fn erase_range_sends_the_range_when_the_service_lists_the_feature() {
    let server = MockServer::start();
    mock_nets_list(&server);
    let health = server.mock(|when, then| {
        when.method(GET).path("/health");
        then.status(200).json_body(json!({
            "status": "healthy", "version": "1.0.0",
            "features": ["erase_range", "halt"], "uptime": 12.5
        }));
    });
    let erase = server.mock(|when, then| {
        when.method(POST).path("/debug/erase").json_body(json!({
            "net": debug_record(),
            "erase_start": 0x1600_0000u64,
            "erase_size": 0x2000u64
        }));
        then.status(200).json_body(json!({
            "status": "erase_complete",
            "output": "Erasing done.",
            "backend": "jlink",
            "erase_range": {
                "start": 0x1600_0000u64, "end": 0x1600_1FFFu64, "length": 0x2000,
                "source": "request", "text": "0x16000000-0x16001FFF (8 KiB)"
            }
        }));
    });

    let lager = client(&server);
    let report = lager
        .debug("debug1")
        .erase_range(0x1600_0000, 0x2000)
        .unwrap();
    assert_eq!(report.erase_range.unwrap().source, "request");
    health.assert();
    erase.assert();
}

#[test]
fn erase_range_never_erases_on_a_box_without_the_feature() {
    // An older service ignores erase_start/erase_size and erases the whole
    // chip, so the crate must stop before sending anything.
    let server = MockServer::start();
    mock_nets_list(&server);
    server.mock(|when, then| {
        when.method(GET).path("/health");
        then.status(200)
            .json_body(json!({"status": "healthy", "version": "1.0.0", "uptime": 3.0}));
    });
    let erase = server.mock(|when, then| {
        when.method(POST).path("/debug/erase");
        then.status(200)
            .json_body(json!({"status": "erase_complete"}));
    });

    let lager = client(&server);
    match lager.debug("debug1").erase_range(0, 0x1000).unwrap_err() {
        Error::UnsupportedByBox { message } => {
            assert!(message.contains("erase_range"), "{message}");
            assert!(message.contains("0.50.0"), "{message}");
        }
        other => panic!("expected Error::UnsupportedByBox, got {other:?}"),
    }
    erase.assert_hits(0);
}

#[test]
fn halt_sends_the_net_when_the_service_lists_the_feature() {
    let server = MockServer::start();
    mock_nets_list(&server);
    let health = server.mock(|when, then| {
        when.method(GET).path("/health");
        then.status(200).json_body(json!({
            "status": "healthy", "version": "1.0.0",
            "features": ["erase_range", "halt"], "uptime": 12.5
        }));
    });
    let halt = server.mock(|when, then| {
        when.method(POST)
            .path("/debug/halt")
            .json_body(json!({"net": debug_record()}));
        then.status(200).json_body(json!({
            "status": "halt_complete", "output": [], "backend": "openocd"
        }));
    });

    let lager = client(&server);
    let out = lager.debug("debug1").halt().unwrap();
    assert!(out.output.is_empty());
    assert_eq!(out.backend.as_deref(), Some("openocd"));
    health.assert();
    halt.assert();
}

#[test]
fn halt_sends_nothing_to_a_box_without_the_feature() {
    let server = MockServer::start();
    mock_nets_list(&server);
    server.mock(|when, then| {
        when.method(GET).path("/health");
        then.status(200).json_body(json!({
            "status": "healthy", "version": "1.0.0",
            "features": ["erase_range"], "uptime": 3.0
        }));
    });
    let halt = server.mock(|when, then| {
        when.method(POST).path("/debug/halt");
        then.status(200)
            .json_body(json!({"status": "halt_complete"}));
    });

    let lager = client(&server);
    match lager.debug("debug1").halt().unwrap_err() {
        Error::UnsupportedByBox { message } => {
            assert!(message.contains("'halt'"), "{message}");
            assert!(message.contains(">= 0.60.0"), "{message}");
        }
        other => panic!("expected Error::UnsupportedByBox, got {other:?}"),
    }
    halt.assert_hits(0);
}

#[test]
fn halt_on_a_jlink_net_is_the_box_refusal() {
    let server = MockServer::start();
    mock_nets_list(&server);
    server.mock(|when, then| {
        when.method(GET).path("/health");
        then.status(200).json_body(json!({
            "status": "healthy", "version": "1.0.0", "features": ["halt"]
        }));
    });
    server.mock(|when, then| {
        when.method(POST).path("/debug/halt");
        then.status(400).json_body(json!({
            "error": "halt is OpenOCD-only, and debug net 'debug1' uses the J-Link \
                      backend, which has no halt-in-place primitive (its resets reset \
                      first). Connect with a halt-first .JLinkScript instead.",
            "status": "error"
        }));
    });

    let lager = client(&server);
    match lager.debug("debug1").halt().unwrap_err() {
        Error::Box {
            status: 400,
            message,
        } => {
            assert!(message.contains(".JLinkScript"), "{message}")
        }
        other => panic!("expected Error::Box 400, got {other:?}"),
    }
}

#[test]
fn status_probed_asks_for_a_target_read_and_parses_the_tri_state() {
    let server = MockServer::start();
    mock_nets_list(&server);
    let status = server.mock(|when, then| {
        when.method(POST)
            .path("/debug/status")
            .json_body(json!({"net": debug_record(), "probe": true}));
        then.status(200).json_body(json!({
            "gdbserver_running": true, "target_attached": true, "connected": true,
            "pid": 4321, "serial": "000440012345", "backend": "jlink"
        }));
    });

    let lager = client(&server);
    let st = lager.debug("debug1").status_probed().unwrap();
    assert_eq!(st.gdbserver_running, Some(true));
    assert_eq!(st.target_attached, Some(true));
    assert!(st.connected);
    status.assert();
}

#[test]
fn status_unprobed_leaves_target_attached_unknown() {
    let server = MockServer::start();
    mock_nets_list(&server);
    let status = server.mock(|when, then| {
        when.method(POST)
            .path("/debug/status")
            .json_body(json!({"net": debug_record()}));
        then.status(200).json_body(json!({
            "gdbserver_running": true, "target_attached": null, "connected": true,
            "pid": 4321, "serial": "000440012345", "backend": "jlink"
        }));
    });

    let lager = client(&server);
    let st = lager.debug("debug1").status().unwrap();
    assert_eq!(st.gdbserver_running, Some(true));
    assert_eq!(st.target_attached, None);
    status.assert();
}

#[test]
fn info_probed_parses_the_new_fields() {
    let server = MockServer::start();
    mock_nets_list(&server);
    let info = server.mock(|when, then| {
        when.method(POST)
            .path("/debug/info")
            .json_body(json!({"net": debug_record(), "probe": true}));
        then.status(200).json_body(json!({
            "net_name": "debug1", "device": "nrf52840_xxaa", "arch": "cortex-m4",
            "probe": "jlink1", "serial": "000440012345", "backend": "jlink",
            "gdbserver_running": false, "target_attached": false, "connected": false
        }));
    });

    let lager = client(&server);
    let i = lager.debug("debug1").info_probed().unwrap();
    assert_eq!(i.gdbserver_running, Some(false));
    assert_eq!(i.target_attached, Some(false));
    info.assert();
}

#[test]
fn read_memory_no_reset_sends_the_flag() {
    let server = MockServer::start();
    mock_nets_list(&server);
    let memrd = server.mock(|when, then| {
        when.method(POST).path("/debug/memrd").json_body(json!({
            "net": debug_record(), "start_addr": 536870912u64, "length": 2, "no_reset": true
        }));
        then.status(200).json_body(json!({
            "status": "success", "address": "0x20000000", "length": 2, "data": "beef"
        }));
    });

    let lager = client(&server);
    let bytes = lager
        .debug("debug1")
        .read_memory_no_reset(0x2000_0000, 2)
        .unwrap();
    assert_eq!(bytes, [0xbe, 0xef]);
    memrd.assert();
}

#[test]
fn connect_with_sends_port_overrides() {
    use lager::ConnectOptions;

    let server = MockServer::start();
    mock_nets_list(&server);
    let connect = server.mock(|when, then| {
        when.method(POST).path("/debug/connect").json_body(json!({
            "net": debug_record(),
            "force": false, "halt": false, "gdb": true,
            "gdb_port": 3331, "rtt_telnet_port": 19090
        }));
        then.status(200).json_body(json!({"status": "connected"}));
    });

    let lager = client(&server);
    lager
        .debug("debug1")
        .connect_with(&ConnectOptions {
            gdb_port: Some(3331),
            rtt_telnet_port: Some(19090),
            ..Default::default()
        })
        .unwrap();
    connect.assert();
}

#[test]
fn busy_probe_is_probe_busy() {
    let server = MockServer::start();
    mock_nets_list(&server);
    server.mock(|when, then| {
        when.method(POST).path("/debug/reset");
        then.status(503).json_body(json!({
            "error": "Debug probe 000440012345 is busy with flash (pid 77); reset gave up \
                      after 300 s. Wait for the other operation to finish and retry.",
            "status": "error"
        }));
    });

    let lager = client(&server);
    match lager.debug("debug1").reset(false).unwrap_err() {
        Error::ProbeBusy { message } => assert!(message.contains("busy with flash")),
        other => panic!("expected Error::ProbeBusy, got {other:?}"),
    }
}

#[test]
fn safety_refusal_is_safety_refused() {
    let server = MockServer::start();
    mock_nets_list(&server);
    server.mock(|when, then| {
        when.method(POST).path("/debug/erase");
        then.status(403).json_body(json!({
            "error": "Refused erase on net 'debug1': this net is configured with \
                      allow_destructive: false.",
            "status": "error"
        }));
    });

    let lager = client(&server);
    match lager.debug("debug1").erase().unwrap_err() {
        Error::SafetyRefused { message } => {
            assert!(message.contains("allow_destructive: false"))
        }
        other => panic!("expected Error::SafetyRefused, got {other:?}"),
    }
}

#[test]
fn flash_with_carries_script_verbose_and_evidence() {
    use lager::FlashOptions;

    let server = MockServer::start();
    mock_nets_list(&server);
    // "foobar" -> "Zm9vYmFy", "baz" -> "YmF6".
    let flash = server.mock(|when, then| {
        when.method(POST).path("/debug/flash").json_body(json!({
            "net": debug_record(),
            "hexfile": { "content": "Zm9vYmFy" },
            "verbose": true,
            "jlink_script": "YmF6",
            "require_evidence": false
        }));
        then.status(200).json_body(json!({
            "status": "flash_complete", "output": ["Downloading file", "O.K."],
            "programmed": true, "error": null
        }));
    });

    let lager = client(&server);
    lager
        .debug("debug1")
        .flash_bytes_with(
            b"foobar",
            FirmwareKind::Hex,
            None,
            &FlashOptions {
                verbose: true,
                jlink_script: Some(b"baz".to_vec()),
                require_evidence: Some(false),
            },
        )
        .unwrap();
    flash.assert();
}

#[test]
fn default_flash_options_add_nothing_to_the_request() {
    let server = MockServer::start();
    mock_nets_list(&server);
    let flash = server.mock(|when, then| {
        when.method(POST).path("/debug/flash").json_body(json!({
            "net": debug_record(),
            "elffile": { "content": "Zm9vYmFy" }
        }));
        then.status(200).json_body(json!({
            "status": "flash_complete", "output": [], "programmed": true, "error": null
        }));
    });

    let lager = client(&server);
    lager
        .debug("debug1")
        .flash_bytes(b"foobar", FirmwareKind::Elf, None)
        .unwrap();
    flash.assert();
}

#[test]
fn erase_with_sends_interface_options() {
    use lager::{EraseOptions, Transport};

    let server = MockServer::start();
    mock_nets_list(&server);
    let erase = server.mock(|when, then| {
        when.method(POST).path("/debug/erase").json_body(json!({
            "net": debug_record(),
            "speed": "1000",
            "transport": "JTAG",
            "require_evidence": false
        }));
        then.status(200).json_body(json!({
            "status": "erase_complete", "output": "Erasing done.",
            "backend": "jlink", "erase_range": null
        }));
    });

    let lager = client(&server);
    lager
        .debug("debug1")
        .erase_with(&EraseOptions {
            speed: Some("1000".into()),
            transport: Some(Transport::Jtag),
            require_evidence: Some(false),
        })
        .unwrap();
    erase.assert();
}

#[test]
fn erase_range_with_sends_range_and_options() {
    use lager::{EraseOptions, Transport};

    let server = MockServer::start();
    mock_nets_list(&server);
    server.mock(|when, then| {
        when.method(GET).path("/health");
        then.status(200).json_body(json!({
            "status": "healthy", "features": ["erase_range", "halt"]
        }));
    });
    let erase = server.mock(|when, then| {
        when.method(POST).path("/debug/erase").json_body(json!({
            "net": debug_record(),
            "erase_start": 65536,
            "erase_size": 4096,
            "transport": "SWD"
        }));
        then.status(200).json_body(json!({
            "status": "erase_complete", "output": "Erasing done.", "backend": "jlink",
            "erase_range": {
                "start": 65536, "end": 69631, "length": 4096,
                "source": "request", "text": "0x00010000-0x00010FFF (4 KiB)"
            }
        }));
    });

    let lager = client(&server);
    let report = lager
        .debug("debug1")
        .erase_range_with(
            0x1_0000,
            0x1000,
            &EraseOptions {
                transport: Some(Transport::Swd),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(report.erase_range.unwrap().length, 4096);
    erase.assert();
}

#[test]
fn rtt_timeout_rides_in_the_request() {
    use lager::RttOptions;

    let server = MockServer::start();
    mock_nets_list(&server);
    let rtt = server.mock(|when, then| {
        when.method(POST).path("/debug/rtt").json_body(json!({
            "net": debug_record(),
            "channel": 0,
            "timeout": 5.0
        }));
        then.status(200).body("hello");
    });

    let lager = client(&server);
    let mut stream = lager
        .debug("debug1")
        .rtt_with(&RttOptions {
            timeout: Some(5.0),
            ..Default::default()
        })
        .unwrap();
    let mut text = String::new();
    std::io::Read::read_to_string(&mut stream, &mut text).unwrap();
    assert_eq!(text, "hello");
    rtt.assert();
}

#[test]
fn debug_service_health_routes() {
    let server = MockServer::start();
    let health = server.mock(|when, then| {
        when.method(GET).path("/health");
        then.status(200).json_body(json!({
            "status": "healthy", "version": "1.0.0",
            "features": ["erase_range", "halt"], "uptime": 12.5
        }));
    });
    let detailed = server.mock(|when, then| {
        when.method(GET).path("/health/detailed");
        then.status(200).json_body(json!({
            "status": "healthy", "version": "1.0.0", "features": ["erase_range", "halt"],
            "jlink_gdbserver_running": true, "jlink_gdbserver_pid": 4321,
            "gdb_controllers_cached": 1, "gdb_max_use_count": 9,
            "service_uptime_seconds": 864000.0, "service_uptime_days": 10.0,
            "active_connections": 1,
            "warnings": [
                "Service has been running for >7 days, consider restart for consistency",
                "GDB controller used 9 times, nearing refresh threshold"
            ]
        }));
    });
    let status = server.mock(|when, then| {
        when.method(GET).path("/status");
        then.status(200).json_body(json!({
            "jlink_gdbserver": {"running": true, "pid": 4321},
            "active_connections": ["debug1:nrf52840_xxaa"]
        }));
    });

    let lager = client(&server);
    assert!(lager.debug_service_health().unwrap().supports("halt"));
    let d = lager.debug_service_health_detailed().unwrap();
    assert_eq!(d.jlink_gdbserver_running, Some(true));
    assert_eq!(d.gdb_max_use_count, Some(9));
    assert_eq!(d.warnings.len(), 2);
    let st = lager.debug_service_status().unwrap();
    assert_eq!(st.active_connections, vec!["debug1:nrf52840_xxaa"]);
    health.assert();
    detailed.assert();
    status.assert();
}
