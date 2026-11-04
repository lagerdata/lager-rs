//! Contract tests for the `admin` feature's drivers, binaries and file
//! download: the exact request each method sends, including the multipart
//! bodies, and the response shapes from
//! `box/lager/http_handlers/{drivers_handler,binaries_handler}.py`.

#![cfg(all(feature = "admin", feature = "blocking"))]

use httpmock::prelude::*;
use lager::{Error, LagerBox};
use serde_json::json;

fn client(server: &MockServer) -> LagerBox {
    LagerBox::connect(server.address().to_string()).unwrap()
}

/// What the box's global 404 handler sends for a path it does not serve.
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

fn assert_unsupported(err: Error, needle: &str) {
    match err {
        Error::UnsupportedByBox { message } => assert!(message.contains(needle), "{message}"),
        other => panic!("expected UnsupportedByBox, got {other:?}"),
    }
}

const BOUNDARY: &str = "lager-net-boundary-0";
const ZIP: &[u8] = b"PK\x03\x04\x00\xffbundle";
const ELF: &[u8] = b"\x7fELF\x02\x01\x00\xfe";

fn driver_bundle_body() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    b.extend_from_slice(
        b"Content-Disposition: form-data; name=\"bundle\"; filename=\"thermal.zip\"\r\n",
    );
    b.extend_from_slice(b"Content-Type: application/zip\r\n\r\n");
    b.extend_from_slice(ZIP);
    b.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    b
}

fn binary_upload_body() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    b.extend_from_slice(
        b"Content-Disposition: form-data; name=\"binary\"; filename=\"fw-tool\"\r\n",
    );
    b.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    b.extend_from_slice(ELF);
    b.extend_from_slice(format!("\r\n--{BOUNDARY}\r\n").as_bytes());
    b.extend_from_slice(b"Content-Disposition: form-data; name=\"name\"\r\n\r\n");
    b.extend_from_slice(b"fw-tool");
    b.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    b
}

// ---------------------------------------------------------------------------
// Custom drivers
// ---------------------------------------------------------------------------

#[test]
fn drivers_list_decodes_entries() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/drivers");
        then.status(200).json_body(json!({"drivers": [
            {"type": "thermal", "sha256": "ab12", "status": "ok", "version": "1.2.0",
             "description": "Thermal chamber", "manifest": {"type": "thermal"},
             "nets": ["chamber1"],
             "deps": {"status": "ok", "packages": ["pyserial==3.5"], "python": "cp312",
                      "machine": "x86_64"}},
            {"type": "broken", "sha256": null, "status": "error", "version": null,
             "description": "", "manifest": null, "nets": [], "deps": null,
             "error": "driver.py: SyntaxError"}
        ]}));
    });
    let drivers = client(&server).admin().drivers().unwrap();
    assert_eq!(drivers.len(), 2);
    assert_eq!(drivers[0].driver_type, "thermal");
    let deps = drivers[0].deps.as_ref().unwrap();
    assert_eq!(deps.packages, vec!["pyserial==3.5"]);
    assert_eq!(drivers[0].nets, vec!["chamber1"]);
    assert_eq!(drivers[1].status, "error");
    assert_eq!(drivers[1].error.as_deref(), Some("driver.py: SyntaxError"));
}

#[test]
fn unknown_driver_is_a_404_box_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/drivers/ghost");
        then.status(404)
            .json_body(json!({"error": "No custom driver 'ghost' is installed"}));
    });
    let err = client(&server).admin().driver("ghost").unwrap_err();
    assert_box(err, 404, "No custom driver");
}

#[test]
fn drivers_on_a_box_without_them_is_unsupported() {
    let server = MockServer::start();
    route_missing(&server, GET, "/drivers");
    let err = client(&server).admin().drivers().unwrap_err();
    assert_unsupported(err, ">= 0.59.0");
}

#[test]
fn install_driver_uploads_the_bundle_as_multipart() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(PUT)
            .path("/drivers/thermal")
            .header(
                "content-type",
                "multipart/form-data; boundary=lager-net-boundary-0",
            )
            .matches(|req| req.body.as_deref() == Some(&driver_bundle_body()[..]));
        then.status(200).json_body(json!({
            "type": "thermal", "version": "1.2.0", "sha256": "ab12", "replaced": true,
            "packages": ["pyserial==3.5"]
        }));
    });
    let result = client(&server)
        .admin()
        .install_driver("thermal", ZIP)
        .unwrap();
    assert!(result.replaced);
    assert_eq!(result.packages, vec!["pyserial==3.5"]);
    m.assert();
}

#[test]
fn install_driver_refusals_keep_their_status_and_text() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(PUT).path("/drivers/big");
        then.status(413)
            .json_body(json!({"error": "bundle is larger than 5 MB"}));
    });
    server.mock(|when, then| {
        when.method(PUT).path("/drivers/slow");
        then.status(504)
            .json_body(json!({"error": "pip did not finish within 300 s"}));
    });
    let lager = client(&server);
    assert_box(
        lager.admin().install_driver("big", ZIP).unwrap_err(),
        413,
        "5 MB",
    );
    assert_box(
        lager.admin().install_driver("slow", ZIP).unwrap_err(),
        504,
        "300 s",
    );
}

#[test]
fn reinstall_driver_deps_returns_the_packages() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/drivers/thermal/deps");
        then.status(200)
            .json_body(json!({"type": "thermal", "packages": ["pyserial==3.5"]}));
    });
    let packages = client(&server)
        .admin()
        .reinstall_driver_deps("thermal")
        .unwrap();
    assert_eq!(packages, vec!["pyserial==3.5"]);
    m.assert();
}

#[test]
fn remove_driver_sends_force_only_when_asked() {
    let server = MockServer::start();
    let refused = server.mock(|when, then| {
        when.method(DELETE)
            .path("/drivers/thermal")
            .matches(|req| req.query_params.as_ref().map_or(true, |q| q.is_empty()));
        then.status(409).json_body(json!({
            "error": "Driver 'thermal' is used by nets: chamber1"
        }));
    });
    let forced = server.mock(|when, then| {
        when.method(DELETE)
            .path("/drivers/thermal")
            .query_param("force", "1");
        then.status(200).json_body(json!({
            "type": "thermal", "removed": true, "orphaned_nets": ["chamber1"]
        }));
    });
    let lager = client(&server);
    assert_box(
        lager.admin().remove_driver("thermal", false).unwrap_err(),
        409,
        "chamber1",
    );
    let removal = lager.admin().remove_driver("thermal", true).unwrap();
    assert!(removal.removed);
    assert_eq!(removal.orphaned_nets, vec!["chamber1"]);
    refused.assert();
    forced.assert();
}

// ---------------------------------------------------------------------------
// Binaries
// ---------------------------------------------------------------------------

#[test]
fn binaries_list_decodes() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/binaries/list");
        then.status(200).json_body(json!({
            "binaries": [{"name": "fw-tool", "size": 8, "executable": true}],
            "host_path": "/etc/lager/binaries", "container_path": "/home/www-data/binaries",
            "mounted": true
        }));
    });
    let list = client(&server).admin().binaries().unwrap();
    assert_eq!(list.binaries[0].name, "fw-tool");
    assert_eq!(list.binaries[0].size, 8);
    assert!(list.mounted);
}

#[test]
fn add_binary_uploads_the_file_and_its_name() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/binaries/add")
            .header(
                "content-type",
                "multipart/form-data; boundary=lager-net-boundary-0",
            )
            .matches(|req| req.body.as_deref() == Some(&binary_upload_body()[..]));
        then.status(200).json_body(json!({
            "success": true, "name": "fw-tool", "path": "/etc/lager/binaries/fw-tool",
            "size": 8, "restart_required": false
        }));
    });
    let added = client(&server).admin().add_binary("fw-tool", ELF).unwrap();
    assert_eq!(added.path, "/etc/lager/binaries/fw-tool");
    assert!(!added.restart_required);
    m.assert();
}

#[test]
fn remove_binary_sends_its_name() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/binaries/remove")
            .json_body(json!({"name": "fw-tool"}));
        then.status(200)
            .json_body(json!({"success": true, "name": "fw-tool"}));
    });
    server.mock(|when, then| {
        when.method(POST)
            .path("/binaries/remove")
            .json_body(json!({"name": "ghost"}));
        then.status(404)
            .json_body(json!({"error": "Binary 'ghost' not found"}));
    });
    let lager = client(&server);
    lager.admin().remove_binary("fw-tool").unwrap();
    assert_box(
        lager.admin().remove_binary("ghost").unwrap_err(),
        404,
        "Binary 'ghost' not found",
    );
    m.assert();
}

// ---------------------------------------------------------------------------
// File download
// ---------------------------------------------------------------------------

#[test]
fn download_file_returns_the_exact_bytes() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/download-file")
            .query_param("filename", "/tmp/lager-output/run 1/log.bin");
        then.status(200)
            .header("content-type", "application/octet-stream")
            .body(ELF);
    });
    let bytes = client(&server)
        .admin()
        .download_file("/tmp/lager-output/run 1/log.bin")
        .unwrap();
    assert_eq!(bytes, ELF);
    m.assert();
}

#[test]
fn download_file_refusals_stay_box_errors() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/download-file")
            .query_param("filename", "/etc/shadow");
        then.status(403)
            .json_body(json!({"error": "Access denied"}));
    });
    server.mock(|when, then| {
        when.method(GET)
            .path("/download-file")
            .query_param("filename", "/tmp/lager-output/gone");
        then.status(404)
            .json_body(json!({"error": "File not found: /tmp/lager-output/gone"}));
    });
    let lager = client(&server);
    assert_box(
        lager.admin().download_file("/etc/shadow").unwrap_err(),
        403,
        "Access denied",
    );
    assert_box(
        lager
            .admin()
            .download_file("/tmp/lager-output/gone")
            .unwrap_err(),
        404,
        "File not found",
    );
}

#[test]
fn download_file_on_a_box_without_the_route_is_unsupported() {
    let server = MockServer::start();
    route_missing(&server, GET, "/download-file");
    let err = client(&server)
        .admin()
        .download_file("/tmp/lager-output/x")
        .unwrap_err();
    assert_unsupported(err, "/download-file");
}

#[test]
fn download_file_with_timeout_times_out() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/download-file");
        then.status(200)
            .delay(std::time::Duration::from_millis(1500))
            .body(ELF);
    });
    let err = client(&server)
        .admin()
        .download_file_with_timeout(
            "/tmp/lager-output/x",
            Some(std::time::Duration::from_millis(200)),
        )
        .unwrap_err();
    assert!(matches!(err, Error::Timeout(_)), "{err:?}");
}

// ---------------------------------------------------------------------------
// Async twins
// ---------------------------------------------------------------------------

#[cfg(feature = "async")]
mod async_parity {
    use super::*;
    use lager::AsyncLagerBox;

    #[tokio::test]
    async fn async_add_binary_sends_the_identical_body() {
        let server = MockServer::start_async().await;
        let m = server
            .mock_async(|when, then| {
                when.method(POST)
                    .path("/binaries/add")
                    .header(
                        "content-type",
                        "multipart/form-data; boundary=lager-net-boundary-0",
                    )
                    .matches(|req| req.body.as_deref() == Some(&binary_upload_body()[..]));
                then.status(200).json_body(json!({
                    "success": true, "name": "fw-tool", "path": "/etc/lager/binaries/fw-tool",
                    "size": 8, "restart_required": false
                }));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        lager.admin().add_binary("fw-tool", ELF).await.unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn async_download_file_returns_the_exact_bytes() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET)
                    .path("/download-file")
                    .query_param("filename", "/tmp/lager-output/log.bin");
                then.status(200).body(ELF);
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        let bytes = lager
            .admin()
            .download_file("/tmp/lager-output/log.bin")
            .await
            .unwrap();
        assert_eq!(bytes, ELF);
    }

    #[tokio::test]
    async fn async_drivers_on_a_box_without_them_is_unsupported() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET).path("/drivers");
                then.status(404).json_body(json!({
                    "error": "Not found", "message": "The requested endpoint does not exist"
                }));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        assert_unsupported(lager.admin().drivers().await.unwrap_err(), ">= 0.59.0");
    }
}
