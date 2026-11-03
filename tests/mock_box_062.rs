//! Contract tests for the actions box 0.62.0 added: the request each method
//! sends, the response shapes the box handlers emit, and how an older box's
//! refusal comes back. Shapes are taken from
//! `box/lager/http_handlers/{net_command,battery,supply,usb,wifi,ble,blufi}.py`,
//! including the `dmm` role.

#![cfg(feature = "blocking")]

use httpmock::prelude::*;
use lager::{BlufiOpMode, DmmFunction, DmmOptions, Error, GattChar, LagerBox};
use serde_json::json;

fn client(server: &MockServer) -> LagerBox {
    LagerBox::connect(server.address().to_string()).unwrap()
}

fn assert_unsupported(err: Error, action: &str) {
    match err {
        Error::UnsupportedByBox { message } => {
            assert!(message.contains(&format!("'{action}'")), "{message}");
            assert!(message.contains(">= 0.62.0"), "{message}");
        }
        other => panic!("expected UnsupportedByBox for {action}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// E-load input switch
// ---------------------------------------------------------------------------

#[test]
fn eload_enable_and_disable() {
    let server = MockServer::start();
    let on = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "load1", "role": "eload", "action": "enable", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "enable", "message": "E-load 'load1' input enabled"
        }));
    });
    let off = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "load1", "role": "eload", "action": "disable", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "disable", "message": "E-load 'load1' input disabled"
        }));
    });
    let lager = client(&server);
    lager.eload("load1").enable().unwrap();
    lager.eload("load1").disable().unwrap();
    on.assert();
    off.assert();
}

#[test]
fn eload_enable_on_an_older_box_is_unsupported() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/net/command");
        then.status(400).json_body(json!({
            "success": false, "error": "Unknown action 'enable' for eload"
        }));
    });
    let err = client(&server).eload("load1").enable().unwrap_err();
    assert_unsupported(err, "enable");
}

// ---------------------------------------------------------------------------
// Battery ESR
// ---------------------------------------------------------------------------

#[test]
fn battery_set_esr_returns_the_instrument_value() {
    let server = MockServer::start();
    let set = server.mock(|when, then| {
        when.method(POST).path("/battery/command").json_body(json!({
            "netname": "batt1", "action": "set_esr", "params": {"value": 0.25}
        }));
        // The 2281S clamps, so the answer is not always the request.
        then.status(200).json_body(json!({
            "success": true, "action": "set_esr",
            "message": "ESR offset set to 0.2 ohm", "value": 0.2
        }));
    });
    let read = server.mock(|when, then| {
        when.method(POST).path("/battery/command").json_body(json!({
            "netname": "batt1", "action": "set_esr", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "set_esr", "message": "ESR offset: 0.2 ohm", "value": 0.2
        }));
    });
    let lager = client(&server);
    assert_eq!(lager.battery("batt1").set_esr(0.25).unwrap(), 0.2);
    assert_eq!(lager.battery("batt1").esr().unwrap(), 0.2);
    set.assert();
    read.assert();
}

#[test]
fn battery_esr_on_an_older_box_is_unsupported() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/battery/command");
        then.status(400)
            .json_body(json!({"success": false, "error": "Unknown action: set_esr"}));
    });
    let err = client(&server).battery("batt1").esr().unwrap_err();
    assert_unsupported(err, "set_esr");
}

// ---------------------------------------------------------------------------
// Supply protection off
// ---------------------------------------------------------------------------

#[test]
fn supply_disable_ocp_and_ovp() {
    let server = MockServer::start();
    let ocp = server.mock(|when, then| {
        when.method(POST).path("/supply/command").json_body(json!({
            "netname": "psu1", "action": "disable_ocp", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "disable_ocp",
            "message": "OCP off. Setting an OCP limit turns it back on"
        }));
    });
    let ovp = server.mock(|when, then| {
        when.method(POST).path("/supply/command").json_body(json!({
            "netname": "psu1", "action": "disable_ovp", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "disable_ovp",
            "message": "OVP off. Setting an OVP limit turns it back on"
        }));
    });
    let lager = client(&server);
    lager.supply("psu1").disable_ocp().unwrap();
    lager.supply("psu1").disable_ovp().unwrap();
    ocp.assert();
    ovp.assert();
}

#[test]
fn supply_protection_off_refusals_stay_box_errors() {
    // An instrument that cannot do it is a real refusal, not an old box.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/supply/command");
        then.status(400)
            .json_body(json!({"success": false, "error": "PSB cannot turn OCP off"}));
    });
    match client(&server).supply("psu1").disable_ocp().unwrap_err() {
        Error::Box {
            status: 400,
            message,
        } => assert!(message.contains("cannot turn OCP off")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn supply_protection_off_on_an_older_box_is_unsupported() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/supply/command");
        then.status(400)
            .json_body(json!({"success": false, "error": "Unknown action: disable_ovp"}));
    });
    let err = client(&server).supply("psu1").disable_ovp().unwrap_err();
    assert_unsupported(err, "disable_ovp");
}

// ---------------------------------------------------------------------------
// USB recover ports
// ---------------------------------------------------------------------------

#[test]
fn usb_recover_report_lists_the_ports() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/usb/command").json_body(json!({
            "netname": "usb1", "action": "recover"
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "recover", "state": "enabled",
            "message": "USB port 'usb1': power restored", "ports": [1, 2, 3, 4]
        }));
    });
    let report = client(&server).usb("usb1").recover_report().unwrap();
    assert_eq!(report.ports, Some(vec![1, 2, 3, 4]));
    assert_eq!(
        report.message.as_deref(),
        Some("USB port 'usb1': power restored")
    );
    m.assert();
}

#[test]
fn usb_recover_report_without_ports() {
    // An older box, or a driver that only re-powers the net's own port.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/usb/command");
        then.status(200).json_body(json!({
            "success": true, "action": "recover", "state": "enabled",
            "message": "USB port 'usb1': power restored"
        }));
    });
    assert_eq!(
        client(&server).usb("usb1").recover_report().unwrap().ports,
        None
    );
}

// ---------------------------------------------------------------------------
// WiFi disconnect
// ---------------------------------------------------------------------------

#[test]
fn wifi_disconnect_names_the_interface() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/wifi/command").json_body(json!({
            "action": "disconnect", "params": {"interface": "wlan1"}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "disconnect", "message": "Disconnected wlan1",
            "value": {"interface": "wlan1", "connected": false}
        }));
    });
    client(&server).wifi().disconnect("wlan1").unwrap();
    m.assert();
}

#[test]
fn wifi_disconnect_on_an_older_box_is_unsupported() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/wifi/command");
        then.status(400).json_body(json!({
            "success": false, "error": "action (status|scan|connect|delete) is required"
        }));
    });
    let err = client(&server).wifi().disconnect("wlan0").unwrap_err();
    assert_unsupported(err, "disconnect");
}

// ---------------------------------------------------------------------------
// BLE GATT
// ---------------------------------------------------------------------------

const ADDR: &str = "AA:BB:CC:DD:EE:01";
const BATTERY_LEVEL: &str = "00002a19-0000-1000-8000-00805f9b34fb";

#[test]
fn ble_read_decodes_the_hex_value() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/ble/command").json_body(json!({
            "action": "read",
            "params": {"address": ADDR, "char": BATTERY_LEVEL, "timeout": 10.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "read", "message": "Read 1 byte(s)",
            "value": {"address": ADDR, "char": BATTERY_LEVEL, "data": "64"}
        }));
    });
    assert_eq!(
        client(&server).ble().read(ADDR, BATTERY_LEVEL).unwrap(),
        vec![0x64]
    );
    m.assert();
}

#[test]
fn ble_read_by_handle_sends_a_number() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/ble/command").json_body(json!({
            "action": "read", "params": {"address": ADDR, "char": 42, "timeout": 10.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "read",
            "value": {"address": ADDR, "char": 42, "data": ""}
        }));
    });
    assert!(client(&server).ble().read(ADDR, 42u16).unwrap().is_empty());
    m.assert();
}

#[test]
fn ble_write_encodes_hex_and_the_response_choice() {
    let server = MockServer::start();
    let auto = server.mock(|when, then| {
        when.method(POST).path("/ble/command").json_body(json!({
            "action": "write",
            "params": {"address": ADDR, "char": BATTERY_LEVEL, "timeout": 10.0, "data": "01ff"}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "write",
            "value": {"address": ADDR, "char": BATTERY_LEVEL, "length": 2}
        }));
    });
    let with_response = server.mock(|when, then| {
        when.method(POST).path("/ble/command").json_body(json!({
            "action": "write",
            "params": {"address": ADDR, "char": BATTERY_LEVEL, "timeout": 10.0,
                       "data": "00", "response": false}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "write",
            "value": {"address": ADDR, "char": BATTERY_LEVEL, "length": 1}
        }));
    });
    let lager = client(&server);
    lager
        .ble()
        .write(ADDR, BATTERY_LEVEL, &[0x01, 0xff])
        .unwrap();
    lager
        .ble()
        .write_with_response(ADDR, GattChar::Uuid(BATTERY_LEVEL.into()), &[0x00], false)
        .unwrap();
    auto.assert();
    with_response.assert();
}

#[test]
fn ble_notify_collects_messages_and_the_timeout_flag() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/ble/command").json_body(json!({
            "action": "notify",
            "params": {"address": ADDR, "char": BATTERY_LEVEL, "timeout": 10.0,
                       "max_messages": 5, "duration": 2.5}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "notify",
            "message": "Received 2 notification(s)",
            "value": {"address": ADDR, "char": BATTERY_LEVEL,
                      "messages": ["01", "0203"], "timed_out": true}
        }));
    });
    let n = client(&server)
        .ble()
        .notify(ADDR, BATTERY_LEVEL, 5, 2.5)
        .unwrap();
    assert_eq!(n.messages, vec![vec![0x01], vec![0x02, 0x03]]);
    assert!(n.timed_out);
    m.assert();
}

#[test]
fn ble_pair() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/ble/command").json_body(json!({
            "action": "pair", "params": {"address": ADDR, "timeout": 10.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "pair", "message": "Paired with AA:BB:CC:DD:EE:01",
            "value": {"address": ADDR, "paired": true}
        }));
    });
    client(&server).ble().pair(ADDR).unwrap();
    m.assert();
}

#[test]
fn ble_bad_hex_from_the_box_is_a_decode_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/ble/command");
        then.status(200).json_body(json!({
            "success": true, "action": "read",
            "value": {"address": ADDR, "char": BATTERY_LEVEL, "data": "6"}
        }));
    });
    let err = client(&server).ble().read(ADDR, BATTERY_LEVEL).unwrap_err();
    assert!(matches!(err, Error::Decode(_)), "{err:?}");
}

#[test]
fn ble_gatt_on_an_older_box_is_unsupported() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/ble/command");
        then.status(400).json_body(json!({
            "success": false, "error": "action (scan|info|connect|disconnect) is required"
        }));
    });
    let lager = client(&server);
    assert_unsupported(lager.ble().read(ADDR, BATTERY_LEVEL).unwrap_err(), "read");
    assert_unsupported(
        lager.ble().write(ADDR, BATTERY_LEVEL, &[1]).unwrap_err(),
        "write",
    );
    assert_unsupported(
        lager.ble().notify(ADDR, BATTERY_LEVEL, 1, 1.0).unwrap_err(),
        "notify",
    );
    assert_unsupported(lager.ble().pair(ADDR).unwrap_err(), "pair");
}

// ---------------------------------------------------------------------------
// BluFi op mode and custom data
// ---------------------------------------------------------------------------

#[test]
fn blufi_set_mode_reports_requested_and_shown() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/blufi/command").json_body(json!({
            "action": "set_mode",
            "params": {"device_name": "BLUFI_DEVICE", "mode": 2, "timeout": 20.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "set_mode",
            "message": "Requested op mode SoftAP; device reports STA",
            "value": {"device_name": "BLUFI_DEVICE", "requested": 2, "opMode": 1,
                      "opModeName": "STA", "staConn": 0, "staConnName": "Connected",
                      "softAPConn": 0}
        }));
    });
    let report = client(&server)
        .blufi()
        .set_mode("BLUFI_DEVICE", BlufiOpMode::SoftAp)
        .unwrap();
    assert_eq!(report.requested, Some(2));
    assert_eq!(report.status.op_mode, Some(1));
    assert_eq!(report.status.op_mode_name.as_deref(), Some("STA"));
    m.assert();
}

#[test]
fn blufi_op_mode_codes() {
    assert_eq!(BlufiOpMode::Null.code(), 0);
    assert_eq!(BlufiOpMode::Sta.code(), 1);
    assert_eq!(BlufiOpMode::SoftAp.code(), 2);
    assert_eq!(BlufiOpMode::StaSoftAp.code(), 3);
}

#[test]
fn blufi_custom_data_sends_hex_and_decodes_replies() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/blufi/command").json_body(json!({
            "action": "custom_data",
            "params": {"device_name": "BLUFI_DEVICE", "data": "0102",
                       "reply_timeout": 1.5, "timeout": 20.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "custom_data",
            "message": "Sent 2 byte(s), received 1 reply",
            "value": {"device_name": "BLUFI_DEVICE", "sent": 2, "replies": ["aabb"]}
        }));
    });
    let reply = client(&server)
        .blufi()
        .send_custom_data("BLUFI_DEVICE", &[0x01, 0x02], 1.5)
        .unwrap();
    assert_eq!(reply.sent, 2);
    assert_eq!(reply.replies, vec![vec![0xaa, 0xbb]]);
    m.assert();
}

#[test]
fn blufi_new_actions_on_an_older_box_are_unsupported() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/blufi/command");
        then.status(400).json_body(json!({
            "success": false,
            "error": "action (scan|connect|provision|wifi_scan|status|version) is required"
        }));
    });
    let lager = client(&server);
    assert_unsupported(
        lager
            .blufi()
            .set_mode("BLUFI_DEVICE", BlufiOpMode::Sta)
            .unwrap_err(),
        "set_mode",
    );
    assert_unsupported(
        lager
            .blufi()
            .send_custom_data("BLUFI_DEVICE", &[1], 0.0)
            .unwrap_err(),
        "custom_data",
    );
}

// ---------------------------------------------------------------------------
// DMM role
// ---------------------------------------------------------------------------

#[test]
fn dmm_measure_sends_the_function_and_only_the_options_given() {
    let server = MockServer::start();
    let plain = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "dmm1", "role": "dmm", "action": "measure",
            "params": {"function": "dcv"}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "measure", "message": "3.3 V",
            "value": 3.3, "overload": false, "unit": "V"
        }));
    });
    let full = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "dmm1", "role": "dmm", "action": "measure",
            "params": {"function": "temp", "range": 100.0, "nplc": 10.0, "sensor": "ther:k"}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "measure", "message": "23.5 C",
            "value": 23.5, "overload": false, "unit": "C"
        }));
    });
    let lager = client(&server);
    let dmm = lager.dmm("dmm1");
    let r = dmm
        .measure(DmmFunction::DcVolts, &DmmOptions::new())
        .unwrap();
    assert_eq!(
        (r.value, r.unit.as_str(), r.overload),
        (Some(3.3), "V", false)
    );
    let opts = DmmOptions::new().range(100.0).nplc(10.0).sensor("ther:k");
    let r = dmm.measure(DmmFunction::Temperature, &opts).unwrap();
    assert_eq!((r.value, r.unit.as_str()), (Some(23.5), "C"));
    plain.assert();
    full.assert();
}

#[test]
fn dmm_overload_has_no_value() {
    let server = MockServer::start();
    // The box leaves `value` out rather than sending the meter's 9.9E37.
    server.mock(|when, then| {
        when.method(POST).path("/net/command");
        then.status(200).json_body(json!({
            "success": true, "action": "measure", "message": "OPEN",
            "overload": true, "unit": "ohm"
        }));
    });
    let r = client(&server)
        .dmm("dmm1")
        .measure(DmmFunction::Continuity, &DmmOptions::new())
        .unwrap();
    assert_eq!((r.value, r.unit.as_str(), r.overload), (None, "ohm", true));
}

#[test]
fn dmm_reading_without_value_or_overload_is_a_decode_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/net/command");
        then.status(200).json_body(json!({
            "success": true, "action": "measure", "message": "", "unit": "V"
        }));
    });
    let err = client(&server)
        .dmm("dmm1")
        .measure(DmmFunction::DcVolts, &DmmOptions::new())
        .unwrap_err();
    assert!(matches!(err, Error::Decode(_)), "{err:?}");
}

#[test]
fn dmm_identify_and_errors() {
    let server = MockServer::start();
    let idn = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "dmm1", "role": "dmm", "action": "identify", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "identify",
            "message": "Siglent Technologies,SDM3065X,X,1.0",
            "value": "Siglent Technologies,SDM3065X,X,1.0"
        }));
    });
    let errs = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "dmm1", "role": "dmm", "action": "errors", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "errors",
            "message": "-222,\"Data out of range\"", "value": ["-222,\"Data out of range\""]
        }));
    });
    let lager = client(&server);
    assert!(lager.dmm("dmm1").identify().unwrap().contains("SDM3065X"));
    assert_eq!(
        lager.dmm("dmm1").errors().unwrap(),
        vec!["-222,\"Data out of range\"".to_string()]
    );
    idn.assert();
    errs.assert();
}

#[test]
fn dmm_function_names_match_the_box() {
    let all = [
        (DmmFunction::DcVolts, "dcv"),
        (DmmFunction::AcVolts, "acv"),
        (DmmFunction::DcCurrent, "dci"),
        (DmmFunction::AcCurrent, "aci"),
        (DmmFunction::Resistance, "res"),
        (DmmFunction::FourWireResistance, "fres"),
        (DmmFunction::Continuity, "cont"),
        (DmmFunction::Diode, "diode"),
        (DmmFunction::Frequency, "freq"),
        (DmmFunction::Period, "period"),
        (DmmFunction::Capacitance, "cap"),
        (DmmFunction::Temperature, "temp"),
    ];
    for (function, name) in all {
        assert_eq!(function.as_str(), name);
    }
}

#[test]
fn dmm_on_a_box_without_the_role_is_unsupported() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/net/command");
        then.status(501).json_body(json!({
            "success": false, "error": "Role 'dmm' is not supported by /net/command"
        }));
    });
    let err = client(&server).dmm("dmm1").identify().unwrap_err();
    assert!(matches!(err, Error::UnsupportedByBox { .. }), "{err:?}");
}

// ---------------------------------------------------------------------------
// A route an older box does not serve, as the box really answers it
// ---------------------------------------------------------------------------

/// The box's global 404 handler, for a path it does not serve.
fn unknown_route(server: &MockServer, method: httpmock::Method, path: &str) {
    server.mock(|when, then| {
        when.method(method).path(path);
        then.status(404).json_body(json!({
            "error": "Not found", "message": "The requested endpoint does not exist"
        }));
    });
}

#[test]
fn unknown_route_json_404_is_unsupported_for_box_level_calls() {
    let server = MockServer::start();
    unknown_route(&server, GET, "/nets/state");
    unknown_route(&server, PUT, "/nets/supply1/safety-limits");
    unknown_route(&server, GET, "/uart/sessions");
    let lager = client(&server);
    for err in [
        lager.nets_state().unwrap_err(),
        lager.clear_safety_limits("supply1").unwrap_err(),
        lager.uart_sessions().unwrap_err(),
    ] {
        assert!(matches!(err, Error::UnsupportedByBox { .. }), "{err:?}");
    }
}

#[test]
fn a_routes_own_404_stays_a_box_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(PUT).path("/nets/ghost/safety-limits");
        then.status(404)
            .json_body(json!({"error": "no saved net named 'ghost'"}));
    });
    let err = client(&server).clear_safety_limits("ghost").unwrap_err();
    assert!(matches!(err, Error::Box { status: 404, .. }), "{err:?}");
}

// ---------------------------------------------------------------------------
// Async twins: the same requests, and the same old-box mapping
// ---------------------------------------------------------------------------

#[cfg(feature = "async")]
mod async_parity {
    use super::*;
    use lager::AsyncLagerBox;

    #[tokio::test]
    async fn async_eload_enable_on_an_older_box_is_unsupported() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST).path("/net/command");
                then.status(400).json_body(json!({
                    "success": false, "error": "Unknown action 'enable' for eload"
                }));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        assert_unsupported(lager.eload("load1").enable().await.unwrap_err(), "enable");
    }

    #[tokio::test]
    async fn async_ble_read_sends_identical_request() {
        let server = MockServer::start_async().await;
        let m = server
            .mock_async(|when, then| {
                when.method(POST).path("/ble/command").json_body(json!({
                    "action": "read",
                    "params": {"address": ADDR, "char": BATTERY_LEVEL, "timeout": 10.0}
                }));
                then.status(200).json_body(json!({
                    "success": true, "action": "read",
                    "value": {"address": ADDR, "char": BATTERY_LEVEL, "data": "64"}
                }));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        assert_eq!(
            lager.ble().read(ADDR, BATTERY_LEVEL).await.unwrap(),
            vec![0x64]
        );
        m.assert_async().await;
    }

    #[tokio::test]
    async fn async_dmm_measure_sends_identical_request() {
        let server = MockServer::start_async().await;
        let m = server
            .mock_async(|when, then| {
                when.method(POST).path("/net/command").json_body(json!({
                    "netname": "dmm1", "role": "dmm", "action": "measure",
                    "params": {"function": "fres", "range": 1000.0}
                }));
                then.status(200).json_body(json!({
                    "success": true, "action": "measure", "message": "99.8 ohm",
                    "value": 99.8, "overload": false, "unit": "ohm"
                }));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        let r = lager
            .dmm("dmm1")
            .measure(
                DmmFunction::FourWireResistance,
                &DmmOptions::new().range(1000.0),
            )
            .await
            .unwrap();
        assert_eq!((r.value, r.unit.as_str()), (Some(99.8), "ohm"));
        m.assert_async().await;
    }

    #[tokio::test]
    async fn async_unknown_route_json_404_is_unsupported() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/nets/state");
                then.status(404).json_body(json!({
                    "error": "Not found", "message": "The requested endpoint does not exist"
                }));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        let err = lager.nets_state().await.unwrap_err();
        assert!(matches!(err, Error::UnsupportedByBox { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn async_route_missing_page_is_unsupported_like_blocking() {
        // An older box answers a route it does not serve with Flask's HTML
        // 404 page; the async client must report that as unsupported, as
        // the blocking client always has.
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/nets/state");
                then.status(404)
                    .header("content-type", "text/html")
                    .body("<!doctype html><title>404 Not Found</title>");
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        let err = lager.nets_state().await.unwrap_err();
        assert!(matches!(err, Error::UnsupportedByBox { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn async_wifi_disconnect_on_an_older_box_is_unsupported() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST).path("/wifi/command");
                then.status(400).json_body(json!({
                    "success": false, "error": "action (status|scan|connect|delete) is required"
                }));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        assert_unsupported(
            lager.wifi().disconnect("wlan0").await.unwrap_err(),
            "disconnect",
        );
    }
}
