//! Contract tests for gpio power-up levels (feature `admin`, box >= 0.66.0):
//! the request each method sends and the response shapes the box emits.
//! Shapes are taken from `box/lager/http_handlers/nets_handler.py`
//! (`nets_set_power_up`, `nets_power_up`, `_power_up_report`) and
//! `box/lager/io/gpio/labjack_boot_config.py` (`public`).

#![cfg(all(feature = "admin", feature = "blocking"))]

use httpmock::prelude::*;
use lager::{Error, LagerBox, NetRecord, PowerUpLevel};
use serde_json::json;

fn client(server: &MockServer) -> LagerBox {
    LagerBox::connect(server.address().to_string()).unwrap()
}

fn assert_unsupported(err: Error) {
    match err {
        Error::UnsupportedByBox { message } => {
            assert!(message.contains(">= 0.66.0"), "{message}")
        }
        other => panic!("expected UnsupportedByBox, got {other:?}"),
    }
}

/// A plan for one T7 with two declared pins, one stored differently, and
/// an undeclared U3 net elsewhere.
fn t7_plan() -> serde_json::Value {
    json!({
        "mode": "plan",
        "devices": [{
            "device_id": "labjack:USB0::0x0CD5::0x0007::470000001::INSTR",
            "instrument": "LabJack_T7",
            "model": "T7",
            "serial": 470000001,
            "factory": false,
            "stored_known": true,
            "pins": [
                {"dio_name": "FIO0", "declared": "low", "stored": "low", "net": "relay", "dio": 0},
                {"dio_name": "FIO1", "declared": "high", "stored": "input", "net": "enable", "dio": 1}
            ],
            "undeclared_outputs": ["FIO3"],
            "target_direction": 3,
            "target_state": 2,
            "in_sync": false
        }],
        "undeclared": ["u3_spare"],
        "in_sync": false
    })
}

// ---------------------------------------------------------------------------
// Declaring a level
// ---------------------------------------------------------------------------

#[test]
fn set_power_up_puts_the_level() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(PUT)
            .path("/nets/relay/power-up")
            .json_body(json!({"power_up": "low"}));
        then.status(200)
            .json_body(json!({"ok": true, "name": "relay", "power_up": "low"}));
    });
    let set = client(&server)
        .admin()
        .set_power_up("relay", Some(PowerUpLevel::Low))
        .unwrap();
    m.assert();
    assert_eq!(set.name, "relay");
    assert_eq!(set.power_up, Some(PowerUpLevel::Low));
}

#[test]
fn clearing_a_level_sends_null() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(PUT)
            .path("/nets/enable/power-up")
            .json_body(json!({"power_up": null}));
        then.status(200)
            .json_body(json!({"ok": true, "name": "enable", "power_up": null}));
    });
    let set = client(&server)
        .admin()
        .set_power_up("enable", None)
        .unwrap();
    m.assert();
    assert_eq!(set.power_up, None);
}

#[test]
fn a_net_name_is_escaped_in_the_path() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(PUT).path("/nets/a%20b/power-up");
        then.status(200)
            .json_body(json!({"ok": true, "name": "a b", "power_up": "high"}));
    });
    client(&server)
        .admin()
        .set_power_up("a b", Some(PowerUpLevel::High))
        .unwrap();
    m.assert();
}

#[test]
fn the_boxs_refusal_of_a_net_is_an_error_with_its_message() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(PUT).path("/nets/supply1/power-up");
        then.status(400).json_body(json!({
            "error": "power_up applies to gpio nets only; 'supply1' is a power-supply net"
        }));
    });
    let err = client(&server)
        .admin()
        .set_power_up("supply1", Some(PowerUpLevel::Low))
        .unwrap_err();
    match err {
        Error::Box { status, message } => {
            assert_eq!(status, 400);
            assert!(message.contains("gpio nets only"), "{message}");
        }
        other => panic!("expected Error::Box, got {other:?}"),
    }
}

#[test]
fn an_unknown_net_is_a_404_from_the_box_not_unsupported() {
    // The box answers an unknown net with its own JSON 404, which must not
    // read as a box that lacks the route.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(PUT).path("/nets/nope/power-up");
        then.status(404)
            .json_body(json!({"error": "no saved net named 'nope'"}));
    });
    let err = client(&server)
        .admin()
        .set_power_up("nope", Some(PowerUpLevel::Low))
        .unwrap_err();
    assert!(matches!(err, Error::Box { status: 404, .. }), "{err:?}");
}

#[test]
fn a_box_before_0_66_does_not_serve_the_routes() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(PUT).path("/nets/relay/power-up");
        then.status(404).json_body(json!({
            "error": "Not found", "message": "The requested endpoint does not exist"
        }));
    });
    server.mock(|when, then| {
        when.method(POST).path("/nets/power-up");
        then.status(405)
            .header("content-type", "text/html")
            .body("<!doctype html><title>405 Method Not Allowed</title>");
    });
    let lager = client(&server);
    assert_unsupported(
        lager
            .admin()
            .set_power_up("relay", Some(PowerUpLevel::Low))
            .unwrap_err(),
    );
    assert_unsupported(lager.admin().power_up_plan().unwrap_err());
    assert_unsupported(lager.admin().apply_power_up(false).unwrap_err());
}

// ---------------------------------------------------------------------------
// Checking and writing
// ---------------------------------------------------------------------------

#[test]
fn a_plan_posts_plan_mode_and_reads_the_report() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/nets/power-up")
            .json_body(json!({"mode": "plan"}));
        then.status(200).json_body(t7_plan());
    });
    let report = client(&server).admin().power_up_plan().unwrap();
    m.assert();
    assert_eq!(report.mode, "plan");
    assert!(!report.in_sync);
    assert_eq!(report.undeclared, vec!["u3_spare".to_string()]);
    let dev = &report.devices[0];
    assert_eq!(dev.model.as_deref(), Some("T7"));
    // A T7 serial arrives as a number.
    assert_eq!(dev.serial.as_deref(), Some("470000001"));
    assert!(!dev.in_sync);
    assert_eq!(dev.written, None);
    assert_eq!(dev.undeclared_outputs, vec!["FIO3".to_string()]);
    assert_eq!(dev.pins.len(), 2);
    let enable = &dev.pins[1];
    assert_eq!(enable.net.as_deref(), Some("enable"));
    assert_eq!(enable.dio, Some(1));
    assert_eq!(enable.dio_name.as_deref(), Some("FIO1"));
    assert_eq!(enable.declared.as_deref(), Some("high"));
    assert_eq!(enable.stored.as_deref(), Some("input"));
    // Fields the crate does not type are kept.
    assert_eq!(dev.extra.get("target_direction"), Some(&json!(3)));
}

#[test]
fn an_apply_posts_apply_mode_with_allow_undeclared() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/nets/power-up")
            .json_body(json!({"mode": "apply", "allow_undeclared": true}));
        then.status(200).json_body(json!({
            "mode": "apply",
            "devices": [{
                "device_id": "labjack:labjack_u3:USB0::0x0CD5::0x0003::port-1-1.2::INSTR",
                "instrument": "LabJack_U3",
                "model": "U3",
                "serial": "320000002",
                "factory": null,
                "stored_known": true,
                "pins": [{"dio_name": "FIO4", "declared": "low", "stored": "low",
                          "net": "relay", "dio": 4}],
                "undeclared_outputs": [],
                "in_sync": true,
                "written": true
            }],
            "undeclared": [],
            "in_sync": true
        }));
    });
    let report = client(&server).admin().apply_power_up(true).unwrap();
    m.assert();
    assert!(report.in_sync);
    let dev = &report.devices[0];
    assert_eq!(dev.model.as_deref(), Some("U3"));
    assert_eq!(dev.serial.as_deref(), Some("320000002"));
    assert_eq!(dev.written, Some(true));
}

#[test]
fn a_device_the_box_could_not_reach_is_data_not_an_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/nets/power-up");
        then.status(200).json_body(json!({
            "mode": "apply",
            "devices": [{
                "device_id": "labjack:USB0::0x0CD5::0x0007::470000001::INSTR",
                "instrument": "LabJack_T7",
                "error": "LabJack T7 not found on USB"
            }],
            "undeclared": [],
            "in_sync": false
        }));
    });
    let report = client(&server).admin().apply_power_up(false).unwrap();
    assert!(!report.in_sync);
    let dev = &report.devices[0];
    assert_eq!(dev.error.as_deref(), Some("LabJack T7 not found on USB"));
    assert!(!dev.in_sync);
    assert!(dev.pins.is_empty());
}

#[test]
fn fields_a_later_box_adds_are_kept_not_refused() {
    // A later box may name the device's backend and say more about what it
    // stores; the report must still parse.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/nets/power-up");
        then.status(200).json_body(json!({
            "mode": "plan",
            "devices": [{
                "device_id": "labjack:USB0::0x0CD5::0x0007::470000001::INSTR",
                "instrument": "LabJack_T7",
                "backend": "labjack",
                "storage": "flash",
                "pins": [{"dio_name": "FIO0", "declared": "low", "stored": null,
                          "net": "relay", "dio": 0, "coverage": "full"}],
                "in_sync": true
            }],
            "undeclared": [],
            "in_sync": true,
            "families": ["labjack"]
        }));
    });
    let report = client(&server).admin().power_up_plan().unwrap();
    let dev = &report.devices[0];
    assert_eq!(dev.extra.get("backend"), Some(&json!("labjack")));
    assert_eq!(dev.pins[0].stored, None);
    assert_eq!(dev.pins[0].extra.get("coverage"), Some(&json!("full")));
}

// ---------------------------------------------------------------------------
// Saved records and capabilities
// ---------------------------------------------------------------------------

#[test]
fn a_saved_record_carries_its_power_up_level() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(PUT).path("/nets/relay").json_body(json!({
            "name": "relay", "role": "gpio", "instrument": "LabJack_T7",
            "pin": "FIO0", "power_up": "low"
        }));
        then.status(200).json_body(json!({"ok": true}));
    });
    let record = NetRecord::new("relay", "gpio", "LabJack_T7")
        .pin("FIO0")
        .power_up(PowerUpLevel::Low);
    assert_eq!(record.declared_power_up(), Some(PowerUpLevel::Low));
    client(&server).admin().save_net(&record).unwrap();
    m.assert();
}

#[test]
fn a_listed_record_reports_its_power_up_level() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/nets/list");
        then.status(200).json_body(json!([
            {"name": "relay", "role": "gpio", "instrument": "LabJack_T7",
             "pin": "FIO0", "power_up": "high"},
            {"name": "spare", "role": "gpio", "instrument": "LabJack_T7", "pin": "FIO1"}
        ]));
    });
    let nets = client(&server).nets().unwrap();
    assert_eq!(nets[0].declared_power_up(), Some(PowerUpLevel::High));
    assert_eq!(nets[1].declared_power_up(), None);
}

#[test]
fn status_reports_the_power_up_capability() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/status");
        then.status(200).json_body(json!({
            "healthy": true, "version": "0.66.0", "nets": [],
            "capabilities": {"netCommand": true, "netPowerUp": true}
        }));
    });
    assert!(client(&server).status().unwrap().capabilities.net_power_up);
}

#[test]
fn an_older_status_has_no_power_up_capability() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/status");
        then.status(200).json_body(json!({
            "healthy": true, "version": "0.65.0", "nets": [],
            "capabilities": {"netCommand": true}
        }));
    });
    assert!(!client(&server).status().unwrap().capabilities.net_power_up);
}

#[test]
fn the_levels_serialize_as_the_box_writes_them() {
    assert_eq!(PowerUpLevel::Low.as_str(), "low");
    assert_eq!(PowerUpLevel::High.as_str(), "high");
    assert_eq!(
        serde_json::to_value(PowerUpLevel::High).unwrap(),
        json!("high")
    );
}

#[cfg(feature = "async")]
mod async_parity {
    use super::*;
    use lager::AsyncLagerBox;

    #[tokio::test]
    async fn async_set_power_up_sends_identical_request() {
        let server = MockServer::start_async().await;
        let m = server
            .mock_async(|when, then| {
                when.method(PUT)
                    .path("/nets/relay/power-up")
                    .json_body(json!({"power_up": "high"}));
                then.status(200)
                    .json_body(json!({"ok": true, "name": "relay", "power_up": "high"}));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        let set = lager
            .admin()
            .set_power_up("relay", Some(PowerUpLevel::High))
            .await
            .unwrap();
        assert_eq!(set.power_up, Some(PowerUpLevel::High));
        m.assert_async().await;
    }

    #[tokio::test]
    async fn async_plan_and_apply_send_identical_requests() {
        let server = MockServer::start_async().await;
        let plan = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/nets/power-up")
                    .json_body(json!({"mode": "plan"}));
                then.status(200).json_body(t7_plan());
            })
            .await;
        let apply = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/nets/power-up")
                    .json_body(json!({"mode": "apply", "allow_undeclared": false}));
                then.status(200).json_body(
                    json!({"mode": "apply", "devices": [], "undeclared": [], "in_sync": true}),
                );
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        let report = lager.admin().power_up_plan().await.unwrap();
        assert_eq!(report.devices[0].pins.len(), 2);
        assert!(lager.admin().apply_power_up(false).await.unwrap().in_sync);
        plan.assert_async().await;
        apply.assert_async().await;
    }
}
