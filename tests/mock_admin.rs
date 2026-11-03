//! Contract tests for the `admin` feature: the request each method sends and
//! the response shapes the box handlers emit. Shapes are taken from
//! `box/lager/http_handlers/{nets_handler,net_metadata_handler,
//! box_metadata_handler,instruments_handler,bench_manifest_handler}.py`.

#![cfg(all(feature = "admin", feature = "blocking"))]

use httpmock::prelude::*;
use lager::{Error, LagerBox, NetMetadataUpdate, NetRecord};
use serde_json::json;

fn client(server: &MockServer) -> LagerBox {
    LagerBox::connect(server.address().to_string()).unwrap()
}

/// Flask's default page for a route this box does not serve.
fn route_missing(server: &MockServer, method: httpmock::Method, path: &str, status: u16) {
    server.mock(|when, then| {
        when.method(method).path(path);
        then.status(status)
            .header("content-type", "text/html")
            .body("<!doctype html><title>404 Not Found</title>");
    });
}

fn assert_unsupported(err: Error, needle: &str) {
    match err {
        Error::UnsupportedByBox { message } => assert!(message.contains(needle), "{message}"),
        other => panic!("expected UnsupportedByBox, got {other:?}"),
    }
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
// Saved nets
// ---------------------------------------------------------------------------

#[test]
fn save_net_puts_the_record_without_unset_fields() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(PUT).path("/nets/supply1").json_body(json!({
            "name": "supply1", "role": "power-supply", "instrument": "Rigol_DP832",
            "pin": 1, "address": "USB0::0x1AB1::0x0E11::DP8C1::INSTR",
            "params": {"channel_key": "CH1"}, "serial": "DP8C1"
        }));
        then.status(200).json_body(json!({"ok": true}));
    });
    let record = NetRecord::new("supply1", "power-supply", "Rigol_DP832")
        .pin(1)
        .address("USB0::0x1AB1::0x0E11::DP8C1::INSTR")
        .param("channel_key", "CH1")
        .field("serial", "DP8C1");
    client(&server).admin().save_net(&record).unwrap();
    m.assert();
}

#[test]
fn a_minimal_record_sends_only_the_required_keys() {
    // The box defaults an absent pin to 0; a null would be stored as null.
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(PUT).path("/nets/router1").json_body(json!({
            "name": "router1", "role": "router", "instrument": "MikroTik"
        }));
        then.status(200).json_body(json!({"ok": true}));
    });
    let record = NetRecord::new("router1", "router", "MikroTik");
    client(&server).admin().save_net(&record).unwrap();
    m.assert();
}

#[test]
fn a_listed_record_saves_back_without_live_path() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/nets/list");
        then.status(200).json_body(json!([{
            "name": "uart1", "role": "uart", "instrument": "FTDI_FT232R", "pin": 0,
            "address": "/dev/ttyUSB0", "live_path": "/dev/ttyUSB3",
            "usb_identity": {"serial": "A1"}, "mappings": [{"net": "uart1", "pin": 0}]
        }]));
    });
    let put = server.mock(|when, then| {
        when.method(PUT).path("/nets/uart1").json_body(json!({
            "name": "uart1", "role": "uart", "instrument": "FTDI_FT232R", "pin": 0,
            "address": "/dev/ttyUSB0", "usb_identity": {"serial": "A1"},
            "mappings": [{"net": "uart1", "pin": 0}],
            "params": {"baudrate": 115200}
        }));
        then.status(200).json_body(json!({"ok": true}));
    });
    let lager = client(&server);
    let record = lager.nets().unwrap().remove(0).param("baudrate", 115200);
    lager.admin().save_net(&record).unwrap();
    put.assert();
}

#[test]
fn rename_net_puts_to_the_old_name() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(PUT).path("/nets/old%20name").json_body(json!({
            "name": "gpio1", "role": "gpio", "instrument": "LabJack_T7", "pin": "FIO0"
        }));
        then.status(200).json_body(json!({"ok": true}));
    });
    let record = NetRecord::new("gpio1", "gpio", "LabJack_T7").pin("FIO0");
    client(&server)
        .admin()
        .rename_net("old name", &record)
        .unwrap();
    m.assert();
}

#[test]
fn save_net_refusal_is_a_box_error_with_its_text() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(PUT).path("/nets/x");
        then.status(400)
            .json_body(json!({"error": "name, role, and instrument are required"}));
    });
    let mut record = NetRecord::new("x", "gpio", "LabJack_T7");
    record.instrument = None;
    let err = client(&server).admin().save_net(&record).unwrap_err();
    assert_box(err, 400, "instrument are required");
}

#[test]
fn delete_net_by_name_and_by_role() {
    let server = MockServer::start();
    let any = server.mock(|when, then| {
        when.method(DELETE)
            .path("/nets/supply1")
            .matches(|req| req.query_params.as_ref().map_or(true, |q| q.is_empty()));
        then.status(200).json_body(json!({"ok": true}));
    });
    let by_role = server.mock(|when, then| {
        when.method(DELETE)
            .path("/nets/supply1")
            .query_param("role", "power-supply");
        then.status(200).json_body(json!({"ok": true}));
    });
    let admin_box = client(&server);
    let admin = admin_box.admin();
    admin.delete_net("supply1", None).unwrap();
    admin.delete_net("supply1", Some("power-supply")).unwrap();
    any.assert();
    by_role.assert();
}

#[test]
fn delete_unknown_net_is_a_404_box_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(DELETE).path("/nets/ghost");
        then.status(404)
            .json_body(json!({"error": "Net not found"}));
    });
    let err = client(&server)
        .admin()
        .delete_net("ghost", None)
        .unwrap_err();
    assert_box(err, 404, "Net not found");
}

#[test]
fn delete_all_nets() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(DELETE).path("/nets");
        then.status(200).json_body(json!({"ok": true}));
    });
    client(&server).admin().delete_all_nets().unwrap();
    m.assert();
}

// ---------------------------------------------------------------------------
// Net metadata
// ---------------------------------------------------------------------------

#[test]
fn net_metadata_reads_fields_and_timestamps() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/nets/supply1/metadata");
        then.status(200).json_body(json!({
            "name": "supply1",
            "fields": {"purpose": "VBAT", "notes": "", "tags": ["power"],
                       "dut_connection": "J3", "test_hints": []},
            "metadata_timestamps": {"purpose": "2026-11-01T10:00:00Z"},
            "shadowed_by_override": ["notes"]
        }));
    });
    let md = client(&server).admin().net_metadata("supply1").unwrap();
    assert_eq!(md.fields.purpose, "VBAT");
    assert_eq!(md.fields.tags, vec!["power".to_string()]);
    assert_eq!(md.metadata_timestamps["purpose"], "2026-11-01T10:00:00Z");
    assert_eq!(md.shadowed_by_override, vec!["notes".to_string()]);
    assert_eq!(md.records_updated, None);
}

#[test]
fn set_net_metadata_sends_only_what_changes() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(PUT)
            .path("/nets/supply1/metadata")
            .json_body(json!({
                "fields": {"purpose": "VBAT", "tags": ["power", "main"], "notes": null},
                "timestamps": {"purpose": "2026-11-03T12:00:00Z"}
            }));
        then.status(200).json_body(json!({
            "ok": true, "name": "supply1",
            "fields": {"purpose": "VBAT", "notes": "", "tags": ["power", "main"],
                       "dut_connection": "", "test_hints": []},
            "metadata_timestamps": {"purpose": "2026-11-03T12:00:00Z"},
            "records_updated": 2, "shadowed_by_override": []
        }));
    });
    let update = NetMetadataUpdate::new()
        .purpose("VBAT")
        .tags(["power", "main"])
        .clear("notes")
        .timestamp("purpose", "2026-11-03T12:00:00Z");
    let md = client(&server)
        .admin()
        .set_net_metadata("supply1", &update)
        .unwrap();
    assert_eq!(md.records_updated, Some(2));
    m.assert();
}

#[test]
fn net_metadata_on_an_unknown_net_is_a_404_box_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/nets/ghost/metadata");
        then.status(404)
            .json_body(json!({"error": "no saved net named 'ghost'"}));
    });
    let err = client(&server).admin().net_metadata("ghost").unwrap_err();
    assert_box(err, 404, "no saved net named");
}

#[test]
fn net_metadata_on_an_older_box_is_unsupported() {
    let server = MockServer::start();
    // /nets/<name> exists for PUT and DELETE only, so an older box answers
    // GET /nets/<name>/metadata with its default 404 page.
    route_missing(&server, GET, "/nets/supply1/metadata", 404);
    let err = client(&server).admin().net_metadata("supply1").unwrap_err();
    assert_unsupported(err, ">= 0.46.0");
}

// ---------------------------------------------------------------------------
// Box metadata
// ---------------------------------------------------------------------------

#[test]
fn box_metadata_read_and_replace() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/box-metadata");
        then.status(200)
            .json_body(json!({"description": null, "updated_at": null}));
    });
    let put = server.mock(|when, then| {
        when.method(PUT).path("/box-metadata").json_body(json!({
            "description": "rack 2, shelf 1", "updated_at": null
        }));
        then.status(200).json_body(json!({
            "ok": true, "description": "rack 2, shelf 1", "updated_at": null
        }));
    });
    let lager = client(&server);
    assert_eq!(lager.admin().box_metadata().unwrap().description, None);
    let md = lager
        .admin()
        .set_box_metadata(Some("rack 2, shelf 1"), None)
        .unwrap();
    assert_eq!(md.description.as_deref(), Some("rack 2, shelf 1"));
    put.assert();
}

#[test]
fn box_metadata_on_an_older_box_is_unsupported() {
    let server = MockServer::start();
    route_missing(&server, PUT, "/box-metadata", 405);
    let err = client(&server)
        .admin()
        .set_box_metadata(None, None)
        .unwrap_err();
    assert_unsupported(err, "/box-metadata");
}

// ---------------------------------------------------------------------------
// Instruments and the bench manifest
// ---------------------------------------------------------------------------

#[test]
fn instruments_decode_the_scanner_list() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/instruments/list");
        then.status(200).json_body(json!([
            {"name": "Rigol_DP832", "vid": "1ab1", "pid": "0e11", "serial": "DP8C1",
             "address": "USB0::0x1AB1::0x0E11::DP8C1::INSTR",
             "net_type": ["power-supply"],
             "channels": {"power-supply": ["1", "2", "3"]}, "model": "DP832"},
            {"name": "LabJack_T7", "vid": "0cd5", "pid": "0007", "serial": null,
             "address": "LJ-T7", "net_type": ["gpio", "adc", "dac"]}
        ]));
    });
    let list = client(&server).admin().instruments().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].channels["power-supply"], vec!["1", "2", "3"]);
    assert_eq!(list[0].extra["model"], "DP832");
    assert_eq!(list[1].serial, None);
    assert_eq!(list[1].net_type, vec!["gpio", "adc", "dac"]);
}

#[test]
fn bench_manifest_keeps_the_document() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/bench");
        then.status(200)
            .header("etag", "\"abc\"")
            .json_body(json!({"schema_version": 1, "box_id": "bench-1", "nets": []}));
    });
    let manifest = client(&server).admin().bench_manifest().unwrap();
    assert_eq!(manifest.box_id(), Some("bench-1"));
    assert_eq!(manifest.schema_version(), Some(&json!(1)));
    assert_eq!(manifest.document["nets"], json!([]));
}

#[test]
fn bench_manifest_on_an_older_box_is_unsupported() {
    let server = MockServer::start();
    route_missing(&server, GET, "/bench", 404);
    let err = client(&server).admin().bench_manifest().unwrap_err();
    assert_unsupported(err, ">= 0.50.0");
}

#[test]
fn a_501_is_unsupported_and_a_500_stays_a_box_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/instruments/list");
        then.status(500)
            .json_body(json!({"error": "scan failed: permission denied"}));
    });
    let err = client(&server).admin().instruments().unwrap_err();
    assert_box(err, 500, "permission denied");
}

// ---------------------------------------------------------------------------
// Async twins
// ---------------------------------------------------------------------------

#[cfg(feature = "async")]
mod async_parity {
    use super::*;
    use lager::AsyncLagerBox;

    #[tokio::test]
    async fn async_save_net_sends_identical_request() {
        let server = MockServer::start_async().await;
        let m = server
            .mock_async(|when, then| {
                when.method(PUT).path("/nets/adc1").json_body(json!({
                    "name": "adc1", "role": "adc", "instrument": "LabJack_T7", "pin": "AIN0"
                }));
                then.status(200).json_body(json!({"ok": true}));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        let record = NetRecord::new("adc1", "adc", "LabJack_T7").pin("AIN0");
        lager.admin().save_net(&record).await.unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn async_bench_on_an_older_box_is_unsupported() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/bench");
                then.status(404)
                    .header("content-type", "text/html")
                    .body("<!doctype html><title>404 Not Found</title>");
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        assert_unsupported(
            lager.admin().bench_manifest().await.unwrap_err(),
            ">= 0.50.0",
        );
    }
}
