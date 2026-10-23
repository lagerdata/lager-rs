//! The one-way RTT stream (`DebugNet::rtt`) against a fake debug service.
//!
//! The box debug service answers `/debug/rtt` with an `HTTP/1.0` status
//! line plus `Transfer-Encoding: chunked`, and writes the chunk framing by
//! hand. httpmock always speaks HTTP/1.1, so the debug side here is a raw
//! socket server on 127.0.0.1 that sends exactly the bytes it is given.
//! `/nets/list` stays on httpmock.

#![cfg(feature = "blocking")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;

use httpmock::prelude::*;
use lager::LagerBox;
use serde_json::json;

/// The body the box sends for two RTT reads, framed the way
/// `service.py` frames it: hex size, CRLF, payload, CRLF, then `0\r\n\r\n`.
const BOX_CHUNKED_BODY: &[u8] = b"32\r\nblink 46860 period=500ms\nblink 46861 period=500ms\n\r\n\
                                  19\r\nblink 46862 period=500ms\n\r\n\
                                  0\r\n\r\n";

const PAYLOAD: &str =
    "blink 46860 period=500ms\nblink 46861 period=500ms\nblink 46862 period=500ms\n";

/// Serve one request with `head` + `body`, then close. Returns the base URL.
fn serve_once(head: &'static str, body: &'static [u8]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        // Drain the request so closing does not reset the connection.
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.trim().parse().unwrap();
                }
            }
        }
        let mut request_body = vec![0u8; content_length];
        reader.read_exact(&mut request_body).unwrap();

        let mut stream = stream;
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        stream.flush().unwrap();
    });
    format!("http://{addr}")
}

fn client(nets: &MockServer, debug_url: String) -> LagerBox {
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
    LagerBox::builder(nets.address().to_string())
        .debug_service_url(debug_url)
        .build()
        .unwrap()
}

fn read_all(lager: &LagerBox) -> String {
    let mut out = String::new();
    lager
        .debug("debug1")
        .rtt()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    out
}

#[test]
fn box_framed_stream_yields_only_the_payload() {
    let nets = MockServer::start();
    let url = serve_once(
        "HTTP/1.0 200 OK\r\n\
         Server: BaseHTTP/0.6 Python/3.11\r\n\
         Content-Type: application/octet-stream\r\n\
         Transfer-Encoding: chunked\r\n\r\n",
        BOX_CHUNKED_BODY,
    );
    assert_eq!(read_all(&client(&nets, url)), PAYLOAD);
}

#[test]
fn box_framed_stream_reads_as_clean_lines() {
    // The usage the docs suggest, and the one that exposed the framing:
    // chunk sizes read as lines ("32", "19") and terminators as "".
    let nets = MockServer::start();
    let url = serve_once(
        "HTTP/1.0 200 OK\r\n\
         Content-Type: application/octet-stream\r\n\
         Transfer-Encoding: chunked\r\n\r\n",
        BOX_CHUNKED_BODY,
    );
    let lager = client(&nets, url);
    let lines: Vec<String> = BufReader::new(lager.debug("debug1").rtt().unwrap())
        .lines()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        lines,
        [
            "blink 46860 period=500ms",
            "blink 46861 period=500ms",
            "blink 46862 period=500ms"
        ]
    );
}

#[test]
fn http11_chunked_stream_is_not_decoded_twice() {
    // Should the service ever answer HTTP/1.1, ureq decodes the chunks
    // itself; decoding again would corrupt the payload.
    let nets = MockServer::start();
    let url = serve_once(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/octet-stream\r\n\
         Transfer-Encoding: chunked\r\n\
         Connection: close\r\n\r\n",
        BOX_CHUNKED_BODY,
    );
    assert_eq!(read_all(&client(&nets, url)), PAYLOAD);
}

#[test]
fn unframed_http10_stream_passes_through() {
    // A service that drops the framing and streams until close.
    let nets = MockServer::start();
    let url = serve_once(
        "HTTP/1.0 200 OK\r\n\
         Content-Type: application/octet-stream\r\n\r\n",
        PAYLOAD.as_bytes(),
    );
    assert_eq!(read_all(&client(&nets, url)), PAYLOAD);
}
