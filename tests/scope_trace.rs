//! Scope trace capture against a fake box: the stream ticket over HTTP, then
//! the daemon's WebSocket protocol as `box/lager/measurement/scope/
//! daemon_client.py` speaks it, with LSCP frames in the daemon's encoding.

#![cfg(feature = "scope-trace")]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use httpmock::prelude::*;
use lager::{Error, LagerBox};
use serde_json::{json, Value};
use tungstenite::handshake::server::{Request, Response};
use tungstenite::{Message, WebSocket};

/// The daemon's canonical frame (`protocol/examples/lscp_fixture.rs`), seq 7.
const FIXTURE: &str = concat!(
    "4c534350010001000700000000000000081a99be1c000000000000000000304002",
    "00000002000000040000000208020000000000000000000000000000000000000700",
    "000ad7a33a0000000000000000010401000ad7233b000080be000000000000e80318",
    "fcff7f00800500fbff0000"
);

fn frame(seq: u64) -> Vec<u8> {
    let mut bytes: Vec<u8> = (0..FIXTURE.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&FIXTURE[i..i + 2], 16).unwrap())
        .collect();
    bytes[8..16].copy_from_slice(&seq.to_le_bytes());
    bytes
}

fn reply(body: Value) -> Message {
    Message::Text(json!({ "Response": body }).to_string())
}

fn command(ws: &mut WebSocket<TcpStream>) -> Value {
    loop {
        match ws.read().unwrap() {
            Message::Text(text) => return serde_json::from_str(&text).unwrap(),
            Message::Close(_) => return Value::Null,
            _ => {}
        }
    }
}

/// What the fake box saw: the `Authorization` header on the WebSocket
/// upgrade, and every command the client sent.
struct Seen {
    authorization: Option<String>,
    commands: Vec<Value>,
}

/// A box that issues ticket `t1` for net `scope1` and runs `daemon` on the
/// WebSocket. Returns its address and a receiver for what it saw.
// tungstenite's handshake callback returns its own large error response.
#[allow(clippy::result_large_err)]
fn fake_box<F>(daemon: F) -> (String, mpsc::Receiver<Seen>)
where
    F: FnOnce(&mut WebSocket<TcpStream>, &mut Vec<Value>) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut daemon = Some(daemon);
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut peek = [0u8; 64];
            let n = stream.peek(&mut peek).unwrap();
            let line = String::from_utf8_lossy(&peek[..n]).to_string();
            if line.starts_with("GET /scope/scope1/stream") {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    head.push(byte[0]);
                }
                let body = json!({
                    "success": true, "net": "scope1", "instrument": "picoscope",
                    "ws_path": "/scope/scope1/ws?token=t1", "token": "t1",
                    "expires_in": 120, "capabilities": null, "capability_error": null,
                })
                .to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                continue;
            }
            let mut authorization = None;
            let mut ws = tungstenite::accept_hdr(stream, |req: &Request, resp: Response| {
                assert_eq!(req.uri().path(), "/scope/scope1/ws");
                assert_eq!(req.uri().query(), Some("token=t1"));
                authorization = req
                    .headers()
                    .get("Authorization")
                    .map(|v| v.to_str().unwrap().to_string());
                Ok(resp)
            })
            .unwrap();
            let mut commands = Vec::new();
            (daemon.take().unwrap())(&mut ws, &mut commands);
            tx.send(Seen {
                authorization,
                commands,
            })
            .unwrap();
            return;
        }
    });
    (addr, rx)
}

#[test]
fn capture_returns_the_acknowledged_frame() {
    let (addr, seen) = fake_box(|ws, commands| {
        commands.push(command(ws));
        // A capture pushed before the reply, and one between the reply and
        // the frame it names: both must be skipped.
        ws.send(Message::Binary(frame(5))).unwrap();
        ws.send(reply(json!({"response": "TriggeredDataFollows", "seq": 7})))
            .unwrap();
        ws.send(Message::Binary(frame(6))).unwrap();
        ws.send(Message::Binary(frame(7))).unwrap();
        let _ = command(ws);
    });
    let lager = LagerBox::connect(addr).unwrap();
    let capture = lager.scope("scope1").capture().unwrap();
    assert_eq!(capture.seq, 7);
    assert_eq!(capture.counts('A').unwrap(), [0, 1000, -1000, 32767]);
    assert!(capture.triggered());

    let seen = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(seen.commands, [json!({"command": "GetTriggeredData"})]);
    assert_eq!(seen.authorization, None, "a plain box gets no token");
}

#[test]
fn stream_subscribes_with_credit_and_returns_a_credit_per_capture() {
    let (addr, seen) = fake_box(|ws, commands| {
        commands.push(command(ws));
        ws.send(reply(json!({"response": "Subscribed"}))).unwrap();
        ws.send(Message::Binary(frame(11))).unwrap();
        commands.push(command(ws));
        ws.send(Message::Binary(frame(12))).unwrap();
        commands.push(command(ws));
        let _ = command(ws);
    });
    let lager = LagerBox::connect(addr).unwrap();
    let mut stream = lager.scope("scope1").stream().unwrap();
    let wait = Duration::from_secs(5);
    assert_eq!(stream.next_capture(wait).unwrap().seq, 11);
    assert_eq!(stream.next_capture(wait).unwrap().seq, 12);
    stream.close();

    let seen = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        seen.commands,
        [
            json!({"command": "Subscribe", "credits": 4}),
            json!({"command": "Credit", "count": 1}),
            json!({"command": "Credit", "count": 1}),
        ]
    );
}

#[test]
fn a_daemon_error_is_a_stream_error_with_its_message() {
    let (addr, _seen) = fake_box(|ws, commands| {
        commands.push(command(ws));
        ws.send(reply(json!({
            "response": "Error",
            "message": "opening the oscilloscope; try again in a few seconds"
        })))
        .unwrap();
        let _ = command(ws);
    });
    let lager = LagerBox::connect(addr).unwrap();
    match lager.scope("scope1").capture() {
        Err(Error::Stream(message)) => assert!(message.contains("opening the oscilloscope")),
        other => panic!("expected a stream error, got {other:?}"),
    }
}

#[test]
fn a_silent_daemon_times_out() {
    let (addr, _seen) = fake_box(|ws, commands| {
        commands.push(command(ws));
        thread::sleep(Duration::from_millis(600));
    });
    let lager = LagerBox::connect(addr).unwrap();
    let result = lager
        .scope("scope1")
        .capture_timeout(Duration::from_millis(200));
    assert!(matches!(result, Err(Error::Timeout(_))), "{result:?}");
}

#[test]
fn the_websocket_upgrade_carries_the_bearer_token() {
    let (addr, seen) = fake_box(|ws, commands| {
        commands.push(command(ws));
        ws.send(reply(json!({"response": "TriggeredDataFollows", "seq": 7})))
            .unwrap();
        ws.send(Message::Binary(frame(7))).unwrap();
        let _ = command(ws);
    });
    let lager = LagerBox::builder(addr)
        .bearer_token("tok-1")
        .build()
        .unwrap();
    lager.scope("scope1").capture().unwrap();
    let seen = seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(seen.authorization.as_deref(), Some("Bearer tok-1"));
}

#[test]
fn a_refused_ticket_is_a_box_error_with_the_reason() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET).path("/scope/rigol1/stream");
        then.status(400).json_body(json!({
            "success": false,
            "error": "Net 'rigol1' has no HDMI capture card paired with its scope, so it has no live view"
        }));
    });
    let lager = LagerBox::connect(server.address().to_string()).unwrap();
    match lager.scope("rigol1").capture() {
        Err(Error::Box { status, message }) => {
            assert_eq!(status, 400);
            assert!(message.contains("HDMI capture card"));
        }
        other => panic!("expected a box error, got {other:?}"),
    }
    m.assert();
}

#[test]
fn a_box_without_the_stream_route_is_unsupported() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/scope/scope1/stream");
        then.status(404).body("<html>Not Found</html>");
    });
    let lager = LagerBox::connect(server.address().to_string()).unwrap();
    let result = lager.scope("scope1").capture();
    assert!(
        matches!(result, Err(Error::UnsupportedByBox { .. })),
        "{result:?}"
    );
}
