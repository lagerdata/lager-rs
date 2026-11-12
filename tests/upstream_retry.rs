//! Riding out a box-service restart behind a gateway: HTTP 502 with
//! `X-Gateway-Error: upstream_unavailable` (the header contract pinned by
//! the Lager CLI's `test_gateway_upstream_error.py`).
//!
//! httpmock cannot script "fail twice, then succeed", so these tests run a
//! tiny scripted HTTP server: each connection gets the next canned response
//! (the last one repeats), and every request is counted.

#![cfg(feature = "blocking")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lager::{Error, LagerBox, RetryPolicy};
use serde_json::json;

const GATEWAY_BODY: &str = "The Lager service behind this box's gateway is not answering \
     (upstream lager:9000). Update or reinstall Lager on the box: lager update, or lager install.";

#[derive(Clone)]
struct Canned {
    status: u16,
    headers: Vec<(&'static str, &'static str)>,
    body: String,
}

fn upstream_down() -> Canned {
    Canned {
        status: 502,
        headers: vec![
            ("X-Gateway-Error", "upstream_unavailable"),
            ("X-Gateway-Upstream", "lager:9000"),
        ],
        body: json!({ "error": GATEWAY_BODY }).to_string(),
    }
}

fn plain_502() -> Canned {
    Canned {
        status: 502,
        headers: vec![],
        body: json!({ "error": "bad gateway" }).to_string(),
    }
}

fn usb_ok(action: &str) -> Canned {
    Canned {
        status: 200,
        headers: vec![],
        body: json!({
            "success": true, "action": action, "state": "enabled",
            "message": format!("USB port 'usb1' {action}")
        })
        .to_string(),
    }
}

/// Serve `script` in order, one response per request, repeating the last.
/// Returns the server address and the request counter.
fn serve(script: Vec<Canned>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some((k, v)) = line.split_once(':') {
                    if k.eq_ignore_ascii_case("content-length") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0; content_length];
            let _ = reader.read_exact(&mut body);

            let n = counter.fetch_add(1, Ordering::SeqCst);
            let canned = &script[n.min(script.len() - 1)];
            let mut resp = format!(
                "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                canned.status,
                canned.body.len()
            );
            for (k, v) in &canned.headers {
                resp.push_str(&format!("{k}: {v}\r\n"));
            }
            resp.push_str("\r\n");
            resp.push_str(&canned.body);
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    (addr, hits)
}

/// Same shape as the default (1s, 2s, 3s... within a budget), scaled down
/// so the suite stays fast.
fn fast_policy() -> RetryPolicy {
    RetryPolicy {
        budget: Duration::from_millis(400),
        backoff_step: Duration::from_millis(50),
    }
}

fn client(addr: &str) -> LagerBox {
    LagerBox::builder(addr)
        .upstream_retry(fast_policy())
        .build()
        .unwrap()
}

#[test]
fn rides_out_two_upstream_unavailable_responses() {
    let (addr, hits) = serve(vec![upstream_down(), upstream_down(), usb_ok("enable")]);
    client(&addr).usb("usb1").enable().unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 3);
}

#[test]
fn rides_out_a_restart_on_net_listing() {
    let nets = Canned {
        status: 200,
        headers: vec![],
        body: json!([{ "name": "usb1", "role": "usb" }]).to_string(),
    };
    let (addr, hits) = serve(vec![upstream_down(), nets]);
    let nets = client(&addr).nets().unwrap();
    assert_eq!(nets.len(), 1);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[test]
fn gives_up_after_the_budget_with_an_accurate_error() {
    let (addr, hits) = serve(vec![upstream_down()]);
    let started = Instant::now();
    let err = client(&addr).usb("usb1").enable().unwrap_err();
    let took = started.elapsed();

    let Error::BoxServiceUnavailable {
        attempts,
        elapsed,
        retry_safe,
        ref upstream,
        ref cause,
        ..
    } = err
    else {
        panic!("unexpected error: {err:?}");
    };
    assert!(retry_safe);
    assert!(attempts > 2, "attempts = {attempts}");
    assert_eq!(attempts as usize, hits.load(Ordering::SeqCst));
    assert!(elapsed >= fast_policy().budget, "elapsed = {elapsed:?}");
    assert!(took < Duration::from_secs(3), "took {took:?}");
    assert_eq!(upstream.as_deref(), Some("lager:9000"));

    let msg = err.to_string();
    assert!(msg.contains("Lager service on box 127.0.0.1"), "{msg}");
    assert!(msg.contains("not answering"), "{msg}");
    assert!(msg.contains("may be restarting"), "{msg}");
    assert!(msg.contains("lager update"), "{msg}");
    assert!(msg.contains(&format!("after {attempts} attempts")), "{msg}");

    // The gateway's own message rides along as the cause.
    assert!(
        matches!(cause.as_ref(), Error::Box { status: 502, message } if message == GATEWAY_BODY),
        "cause: {cause:?}"
    );
    let source = std::error::Error::source(&err).expect("source");
    assert!(source.to_string().contains("upstream lager:9000"));
}

#[test]
fn a_502_without_the_gateway_header_is_not_retried() {
    let (addr, hits) = serve(vec![plain_502(), usb_ok("enable")]);
    let err = client(&addr).usb("usb1").enable().unwrap_err();
    assert!(
        matches!(err, Error::Box { status: 502, ref message } if message == "bad gateway"),
        "unexpected error: {err:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn a_non_idempotent_call_is_not_retried() {
    // toggle flips the port; sending it twice would undo it.
    let (addr, hits) = serve(vec![upstream_down(), usb_ok("toggle")]);
    let err = client(&addr).usb("usb1").toggle().unwrap_err();
    assert!(
        matches!(
            err,
            Error::BoxServiceUnavailable {
                attempts: 1,
                retry_safe: false,
                ..
            }
        ),
        "unexpected error: {err:?}"
    );
    assert!(err.to_string().contains("not safe to repeat"));
    assert!(err.to_string().contains("lager update"));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn disabled_policy_fails_on_the_first_response() {
    let (addr, hits) = serve(vec![upstream_down(), usb_ok("enable")]);
    let lager = LagerBox::builder(&addr)
        .upstream_retry(RetryPolicy::disabled())
        .build()
        .unwrap();
    let err = lager.usb("usb1").enable().unwrap_err();
    assert!(
        matches!(
            err,
            Error::BoxServiceUnavailable {
                attempts: 1,
                retry_safe: true,
                ..
            }
        ),
        "unexpected error: {err:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[cfg(feature = "async")]
mod async_parity {
    use super::*;
    use lager::AsyncLagerBox;

    fn client(addr: &str) -> AsyncLagerBox {
        AsyncLagerBox::builder(addr)
            .upstream_retry(fast_policy())
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn rides_out_two_upstream_unavailable_responses() {
        let (addr, hits) = serve(vec![upstream_down(), upstream_down(), usb_ok("enable")]);
        client(&addr).usb("usb1").enable().await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn non_idempotent_and_headerless_are_not_retried() {
        let (addr, hits) = serve(vec![upstream_down(), usb_ok("toggle")]);
        let err = client(&addr).usb("usb1").toggle().await.unwrap_err();
        assert!(matches!(
            err,
            Error::BoxServiceUnavailable { attempts: 1, .. }
        ));
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        let (addr, hits) = serve(vec![plain_502(), usb_ok("enable")]);
        let err = client(&addr).usb("usb1").enable().await.unwrap_err();
        assert!(matches!(err, Error::Box { status: 502, .. }), "{err:?}");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn gives_up_after_the_budget() {
        let (addr, hits) = serve(vec![upstream_down()]);
        let err = client(&addr).usb("usb1").enable().await.unwrap_err();
        let Error::BoxServiceUnavailable { attempts, .. } = err else {
            panic!("unexpected error: {err:?}");
        };
        assert!(attempts > 2);
        assert_eq!(attempts as usize, hits.load(Ordering::SeqCst));
    }
}
