//! The blocking client talks to the box directly even when the process has
//! proxy environment variables set: a bench machine behind a corporate
//! proxy must not route LAN box traffic through it.
//!
//! This is its own test binary because it sets process-global proxy
//! variables that would leak into any other test running alongside it.

#![cfg(feature = "blocking")]

use std::net::TcpListener;

use httpmock::prelude::*;
use lager::LagerBox;
use serde_json::json;

#[test]
fn proxy_environment_variables_are_not_used() {
    // A proxy that refuses every connection: if the client used it, the
    // call below would fail.
    let dead = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let proxy = format!("http://127.0.0.1:{dead}");
    for key in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        std::env::set_var(key, &proxy);
    }
    for key in ["NO_PROXY", "no_proxy"] {
        std::env::remove_var(key);
    }

    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/net/command");
        then.status(200).json_body(
            json!({"success": true, "action": "read", "message": "1.5 V", "value": 1.5}),
        );
    });
    let lager = LagerBox::connect(server.address().to_string()).unwrap();
    assert_eq!(lager.adc("adc1").read().unwrap(), 1.5);
    m.assert();
}
