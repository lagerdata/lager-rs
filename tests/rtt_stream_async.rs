//! The async one-way RTT stream (`AsyncDebugNet::rtt`) against a fake debug
//! service.
//!
//! As in `rtt_stream.rs`, the debug side is a raw socket server that sends
//! exactly the bytes it is given, because the box frames `/debug/rtt` as
//! chunked under an `HTTP/1.0` status line, which no HTTP library serves.
//! The async client reads it off a socket of its own for the same reason:
//! hyper refuses that head. `/nets/list` stays on httpmock.

#![cfg(feature = "async")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

use httpmock::prelude::*;
use lager::{AsyncLagerBox, Error, RttOptions};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt};

const BOX_CHUNKED_BODY: &[u8] = b"32\r\nblink 46860 period=500ms\nblink 46861 period=500ms\n\r\n\
                                  19\r\nblink 46862 period=500ms\n\r\n\
                                  0\r\n\r\n";

const PAYLOAD: &str =
    "blink 46860 period=500ms\nblink 46861 period=500ms\nblink 46862 period=500ms\n";

/// What the fake service saw: the request line, headers, and JSON body.
struct Request {
    line: String,
    headers: Vec<(String, String)>,
    body: Value,
}

/// Serve one request with `head` + `body`, then close. Returns the base URL
/// and a receiver for the request it got.
fn serve_once(head: &'static str, body: &'static [u8]) -> (String, mpsc::Receiver<Request>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let mut headers = Vec::new();
        let mut content_length = 0usize;
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).unwrap();
            if header == "\r\n" || header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.trim().parse().unwrap();
                }
                headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
            }
        }
        let mut request_body = vec![0u8; content_length];
        reader.read_exact(&mut request_body).unwrap();
        // A test that ignores the request has dropped the receiver.
        let _ = tx.send(Request {
            line: line.trim_end().to_string(),
            headers,
            body: serde_json::from_slice(&request_body).unwrap(),
        });

        let mut stream = stream;
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        stream.flush().unwrap();
    });
    (format!("http://{addr}"), rx)
}

fn client(nets: &MockServer, debug_url: String) -> AsyncLagerBox {
    nets.mock(|when, then| {
        when.method(GET).path("/nets/list");
        then.status(200).json_body(json!([{
            "name": "debug1",
            "role": "debug",
            "pin": "nrf5340_xxaa_app",
            "instrument": "jlink1",
            "address": "jlink://000440012345"
        }]));
    });
    AsyncLagerBox::builder(nets.address().to_string())
        .debug_service_url(debug_url)
        .build()
        .unwrap()
}

async fn read_all(lager: &AsyncLagerBox) -> String {
    let mut out = String::new();
    lager
        .debug("debug1")
        .rtt()
        .await
        .unwrap()
        .read_to_string(&mut out)
        .await
        .unwrap();
    out
}

#[tokio::test]
async fn box_framed_stream_yields_only_the_payload() {
    let nets = MockServer::start();
    let (url, requests) = serve_once(
        "HTTP/1.0 200 OK\r\n\
         Server: BaseHTTP/0.6 Python/3.11\r\n\
         Content-Type: application/octet-stream\r\n\
         Transfer-Encoding: chunked\r\n\r\n",
        BOX_CHUNKED_BODY,
    );
    assert_eq!(read_all(&client(&nets, url)).await, PAYLOAD);

    let request = requests.recv().unwrap();
    assert_eq!(request.line, "POST /debug/rtt HTTP/1.1");
    assert_eq!(request.body["channel"], 0);
    assert!(
        request
            .headers
            .iter()
            .all(|(name, _)| name != "authorization"),
        "a plain box gets no token"
    );
}

#[tokio::test]
async fn box_framed_stream_reads_as_clean_lines() {
    let nets = MockServer::start();
    let (url, _) = serve_once(
        "HTTP/1.0 200 OK\r\n\
         Content-Type: application/octet-stream\r\n\
         Transfer-Encoding: chunked\r\n\r\n",
        BOX_CHUNKED_BODY,
    );
    let lager = client(&nets, url);
    let rtt = lager.debug("debug1").rtt().await.unwrap();
    let mut lines = tokio::io::BufReader::new(rtt).lines();
    let mut got = Vec::new();
    while let Some(line) = lines.next_line().await.unwrap() {
        got.push(line);
    }
    assert_eq!(
        got,
        [
            "blink 46860 period=500ms",
            "blink 46861 period=500ms",
            "blink 46862 period=500ms"
        ]
    );
}

#[tokio::test]
async fn http11_chunked_stream_is_decoded_once() {
    let nets = MockServer::start();
    let (url, _) = serve_once(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/octet-stream\r\n\
         Transfer-Encoding: chunked\r\n\
         Connection: close\r\n\r\n",
        BOX_CHUNKED_BODY,
    );
    assert_eq!(read_all(&client(&nets, url)).await, PAYLOAD);
}

#[tokio::test]
async fn unframed_http10_stream_passes_through() {
    let nets = MockServer::start();
    let (url, _) = serve_once(
        "HTTP/1.0 200 OK\r\n\
         Content-Type: application/octet-stream\r\n\r\n",
        PAYLOAD.as_bytes(),
    );
    assert_eq!(read_all(&client(&nets, url)).await, PAYLOAD);
}

#[tokio::test]
async fn options_and_the_token_reach_the_service() {
    let nets = MockServer::start();
    let (url, requests) = serve_once(
        "HTTP/1.0 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
        b"0\r\n\r\n",
    );
    nets.mock(|when, then| {
        when.method(GET).path("/nets/list");
        then.status(200).json_body(json!([{
            "name": "debug1", "role": "debug", "pin": "nrf52840_xxaa",
            "instrument": "jlink1", "address": "jlink://1"
        }]));
    });
    let lager = AsyncLagerBox::builder(nets.address().to_string())
        .debug_service_url(url)
        .bearer_token("tok-3")
        .build()
        .unwrap();
    let opts = RttOptions {
        channel: 2,
        search_addr: Some(0x2000_0000),
        ..Default::default()
    };
    let mut out = Vec::new();
    lager
        .debug("debug1")
        .rtt_with(&opts)
        .await
        .unwrap()
        .read_to_end(&mut out)
        .await
        .unwrap();
    assert!(out.is_empty());

    let request = requests.recv().unwrap();
    assert_eq!(request.body["channel"], 2);
    assert_eq!(request.body["search_addr"], 0x2000_0000u64);
    assert!(request
        .headers
        .contains(&("authorization".to_string(), "Bearer tok-3".to_string())));
}

#[tokio::test]
async fn a_refusal_is_the_services_error() {
    let nets = MockServer::start();
    let (url, _) = serve_once(
        "HTTP/1.0 400 Bad Request\r\n\
         Content-Type: application/json\r\n\
         Content-Length: 52\r\n\r\n",
        br#"{"error": "No gdbserver running; connect first.   "}"#,
    );
    let lager = client(&nets, url);
    match lager.debug("debug1").rtt().await {
        Err(Error::Box { status, message }) => {
            assert_eq!(status, 400);
            assert!(message.contains("No gdbserver"), "{message}");
        }
        Err(other) => panic!("expected a box error, got {other:?}"),
        Ok(_) => panic!("expected a box error, got a stream"),
    }
}
