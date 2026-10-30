//! UART and interactive RTT sessions against a fake box Socket.IO server.
//!
//! The server speaks Engine.IO 4 long-polling, offering no websocket
//! upgrade, which is enough for the Socket.IO client to run a namespace
//! session end to end. Each test scripts the box side of one namespace: the
//! events the box emits in answer to each event the crate sends, taken from
//! `box/lager/http_handlers/uart.py` and `rtt.py`.

#![cfg(any(feature = "uart", feature = "rtt"))]
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

/// The box side of one namespace: given an event the crate sent and its
/// data, the events to emit back.
type Script = dyn Fn(&str, &Value) -> Vec<(&'static str, Value)> + Send + Sync;

#[derive(Default)]
struct State {
    /// Engine.IO packets waiting for the client's next poll.
    outbox: VecDeque<String>,
    /// `(event, data)` for every event the crate emitted, in order.
    received: Vec<(String, Value)>,
    /// The `Authorization` header on the Engine.IO handshake.
    authorization: Option<String>,
    disconnected: bool,
}

struct FakeBox {
    url: String,
    state: Arc<(Mutex<State>, Condvar)>,
}

impl FakeBox {
    fn received(&self) -> Vec<(String, Value)> {
        self.state.0.lock().unwrap().received.clone()
    }

    fn events(&self) -> Vec<String> {
        self.received()
            .into_iter()
            .map(|(event, _)| event)
            .collect()
    }

    fn authorization(&self) -> Option<String> {
        self.state.0.lock().unwrap().authorization.clone()
    }

    /// Wait until the crate has sent `event`, for checks after a drop.
    fn wait_for_event(&self, event: &str) -> bool {
        for _ in 0..100 {
            if self.events().iter().any(|e| e == event) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }
}

fn fake_box(namespace: &'static str, script: Box<Script>) -> FakeBox {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state: Arc<(Mutex<State>, Condvar)> = Arc::default();
    let script: Arc<Script> = Arc::from(script);
    let shared = state.clone();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let state = shared.clone();
            let script = script.clone();
            thread::spawn(move || serve(stream, namespace, &state, &*script));
        }
    });
    FakeBox { url, state }
}

fn serve(stream: TcpStream, namespace: &str, state: &(Mutex<State>, Condvar), script: &Script) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let mut content_length = 0;
    let mut authorization = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap();
            } else if name.eq_ignore_ascii_case("authorization") {
                authorization = Some(value.trim().to_string());
            }
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).unwrap();

    let reply = if !request_line.contains("sid=") {
        state.0.lock().unwrap().authorization = authorization;
        r#"0{"sid":"s1","upgrades":[],"pingInterval":25000,"pingTimeout":20000,"maxPayload":1000000}"#
            .to_string()
    } else if request_line.starts_with("POST") {
        let body = String::from_utf8(body).unwrap();
        let mut guard = state.0.lock().unwrap();
        for packet in body.split('\u{1e}') {
            receive(packet, namespace, &mut guard, script);
        }
        state.1.notify_all();
        "ok".to_string()
    } else {
        // A long poll: answer with what is queued, or a noop after a while.
        let guard = state.0.lock().unwrap();
        let (mut guard, _) = state
            .1
            .wait_timeout_while(guard, Duration::from_millis(300), |s| s.outbox.is_empty())
            .unwrap();
        if guard.outbox.is_empty() {
            "6".to_string()
        } else {
            guard.outbox.drain(..).collect::<Vec<_>>().join("\u{1e}")
        }
    };
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=UTF-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
}

fn receive(packet: &str, namespace: &str, state: &mut State, script: &Script) {
    let connect = format!("40{namespace},");
    let event = format!("42{namespace},");
    if packet.starts_with(&connect) {
        state
            .outbox
            .push_back(format!(r#"40{namespace},{{"sid":"n1"}}"#));
    } else if let Some(args) = packet.strip_prefix(&event) {
        let args: Vec<Value> = serde_json::from_str(args).unwrap();
        let name = args[0].as_str().unwrap().to_string();
        let data = args.get(1).cloned().unwrap_or(Value::Null);
        for (reply, payload) in script(&name, &data) {
            state
                .outbox
                .push_back(format!("42{namespace},{}", json!([reply, payload])));
        }
        state.received.push((name, data));
    } else if packet.starts_with(&format!("41{namespace}")) || packet == "1" {
        state.disconnected = true;
    }
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// A UART box: opens the port, sends a boot banner, and echoes writes.
fn uart_box() -> FakeBox {
    fake_box(
        "/uart",
        Box::new(|event, data| match event {
            "start_uart" => vec![
                (
                    "uart_connected",
                    json!({"device_path": "/dev/ttyUSB0", "baudrate": 115200}),
                ),
                ("uart_data", json!({"data": hex(b"boot ok\n")})),
            ],
            "uart_write" => vec![("uart_data", json!({"data": data["data"]}))],
            "stop_uart" => vec![("uart_stopped", json!({}))],
            _ => vec![],
        }),
    )
}

/// A UART box whose net is held by another session.
fn busy_uart_box() -> FakeBox {
    fake_box(
        "/uart",
        Box::new(|event, _| match event {
            "start_uart" => vec![(
                "error",
                json!({
                    "message": "UART net 'uart1' is already in use",
                    "code": "net_in_use",
                    "held_by": "session-7"
                }),
            )],
            _ => vec![],
        }),
    )
}

/// An RTT box: attaches, sends a prompt, and answers `ping` with `pong`.
fn rtt_box() -> FakeBox {
    fake_box(
        "/rtt",
        Box::new(|event, data| match event {
            "start_rtt" => vec![
                ("rtt_connected", json!({"backend": "jlink"})),
                ("rtt_data", json!({"data": hex(b"> ")})),
            ],
            "rtt_write" => {
                let written = unhex(data["data"].as_str().unwrap());
                if written == b"ping\n" {
                    vec![("rtt_data", json!({"data": hex(b"pong\n> ")}))]
                } else {
                    vec![]
                }
            }
            "stop_rtt" => vec![("rtt_stopped", json!({}))],
            _ => vec![],
        }),
    )
}

/// An RTT box with no gdbserver running for the net.
fn refusing_rtt_box() -> FakeBox {
    fake_box(
        "/rtt",
        Box::new(|event, _| match event {
            "start_rtt" => vec![(
                "error",
                json!({"message": "No gdbserver running for 'debug1'; connect first"}),
            )],
            _ => vec![],
        }),
    )
}

const WAIT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Blocking sessions
// ---------------------------------------------------------------------------

#[cfg(feature = "uart")]
mod blocking_uart {
    use super::*;
    use lager::{Error, LagerBox, Parity, UartConfig};

    #[test]
    fn opens_reads_writes_and_stops() {
        let fake = uart_box();
        let lager = LagerBox::connect(&fake.url).unwrap();
        let mut uart = lager.uart("uart1").unwrap();
        assert_eq!(uart.device_path(), "/dev/ttyUSB0");
        assert_eq!(uart.baudrate(), 115200);
        assert_eq!(uart.wait_for(b"ok\n", WAIT).unwrap(), b"boot ok\n");

        uart.write_str("hello\n").unwrap();
        assert_eq!(uart.wait_for(b"hello\n", WAIT).unwrap(), b"hello\n");
        uart.stop().unwrap();

        assert!(fake.wait_for_event("stop_uart"));
        let received = fake.received();
        assert_eq!(
            received[0],
            (
                "start_uart".to_string(),
                json!({"netname": "uart1", "overrides": {}})
            )
        );
        assert_eq!(
            received[1],
            ("uart_write".to_string(), json!({"data": hex(b"hello\n")}))
        );
    }

    #[test]
    fn sends_the_config_overrides() {
        let fake = uart_box();
        let lager = LagerBox::connect(&fake.url).unwrap();
        let config = UartConfig {
            baudrate: Some(9600),
            parity: Some(Parity::Even),
            ..Default::default()
        };
        drop(lager.uart_with("uart1", &config).unwrap());
        assert_eq!(
            fake.received()[0].1,
            json!({"netname": "uart1", "overrides": {"baudrate": 9600, "parity": "even"}})
        );
    }

    #[test]
    fn a_held_net_is_uart_in_use() {
        let fake = busy_uart_box();
        let lager = LagerBox::connect(&fake.url).unwrap();
        match lager.uart("uart1") {
            Err(Error::UartInUse { held_by, .. }) => {
                assert_eq!(held_by.as_deref(), Some("session-7"))
            }
            Err(other) => panic!("expected UartInUse, got {other:?}"),
            Ok(_) => panic!("expected UartInUse, got a session"),
        }
    }

    #[test]
    fn the_handshake_carries_the_bearer_token() {
        let fake = uart_box();
        let lager = LagerBox::builder(&fake.url)
            .bearer_token("tok-1")
            .build()
            .unwrap();
        drop(lager.uart("uart1").unwrap());
        assert_eq!(fake.authorization().as_deref(), Some("Bearer tok-1"));
    }
}

#[cfg(feature = "rtt")]
mod blocking_rtt {
    use super::*;
    use lager::{Error, LagerBox};

    #[test]
    fn writes_down_and_reads_up() {
        let fake = rtt_box();
        let lager = LagerBox::connect(&fake.url).unwrap();
        let mut rtt = lager.debug("debug1").rtt_interactive().unwrap();
        assert_eq!(rtt.backend(), "jlink");
        rtt.write_str("ping\n").unwrap();
        assert_eq!(rtt.wait_for(b"pong\n", WAIT).unwrap(), b"> pong\n");
        rtt.stop().unwrap();
        assert!(fake.wait_for_event("stop_rtt"));
        assert_eq!(
            fake.received()[0],
            (
                "start_rtt".to_string(),
                json!({"netname": "debug1", "channel": 0})
            )
        );
    }

    #[test]
    fn a_refused_attach_is_a_stream_error() {
        let fake = refusing_rtt_box();
        let lager = LagerBox::connect(&fake.url).unwrap();
        match lager.debug("debug1").rtt_interactive() {
            Err(Error::Stream(message)) => assert!(message.contains("No gdbserver")),
            Err(other) => panic!("expected a stream error, got {other:?}"),
            Ok(_) => panic!("expected a stream error, got a session"),
        }
    }
}

// ---------------------------------------------------------------------------
// Async sessions
// ---------------------------------------------------------------------------

#[cfg(all(feature = "uart", feature = "async"))]
mod async_uart {
    use super::*;
    use lager::{AsyncLagerBox, Error, StopBits, UartConfig};

    #[tokio::test]
    async fn opens_reads_writes_and_stops() {
        let fake = uart_box();
        let lager = AsyncLagerBox::connect(&fake.url).unwrap();
        let mut uart = lager.uart("uart1").await.unwrap();
        assert_eq!(uart.device_path(), "/dev/ttyUSB0");
        assert_eq!(uart.baudrate(), 115200);
        assert_eq!(uart.wait_for(b"ok\n", WAIT).await.unwrap(), b"boot ok\n");

        uart.write_str("hello\n").await.unwrap();
        assert_eq!(uart.wait_for(b"hello\n", WAIT).await.unwrap(), b"hello\n");
        assert!(uart.try_read().unwrap().is_empty());
        uart.stop().await.unwrap();

        assert!(fake.wait_for_event("stop_uart"));
        assert_eq!(fake.events(), ["start_uart", "uart_write", "stop_uart"]);
    }

    #[tokio::test]
    async fn read_returns_what_arrives_and_an_idle_read_is_empty() {
        let fake = uart_box();
        let lager = AsyncLagerBox::connect(&fake.url).unwrap();
        let mut uart = lager.uart("uart1").await.unwrap();
        let mut got = Vec::new();
        while !got.ends_with(b"\n") {
            got.extend(uart.read(WAIT).await.unwrap());
        }
        assert_eq!(got, b"boot ok\n");
        let idle = uart.read(Duration::from_millis(100)).await.unwrap();
        assert!(idle.is_empty());
    }

    #[tokio::test]
    async fn sends_the_config_overrides() {
        let fake = uart_box();
        let lager = AsyncLagerBox::connect(&fake.url).unwrap();
        let config = UartConfig {
            stopbits: Some(StopBits::Two),
            ..Default::default()
        };
        lager
            .uart_with("uart1", &config)
            .await
            .unwrap()
            .stop()
            .await
            .unwrap();
        assert_eq!(
            fake.received()[0].1,
            json!({"netname": "uart1", "overrides": {"stopbits": "2"}})
        );
    }

    #[tokio::test]
    async fn a_held_net_is_uart_in_use() {
        let fake = busy_uart_box();
        let lager = AsyncLagerBox::connect(&fake.url).unwrap();
        match lager.uart("uart1").await {
            Err(Error::UartInUse { held_by, .. }) => {
                assert_eq!(held_by.as_deref(), Some("session-7"))
            }
            Err(other) => panic!("expected UartInUse, got {other:?}"),
            Ok(_) => panic!("expected UartInUse, got a session"),
        }
    }

    #[tokio::test]
    async fn a_dropped_session_still_hangs_up() {
        let fake = uart_box();
        let lager = AsyncLagerBox::builder(&fake.url)
            .bearer_token("tok-2")
            .build()
            .unwrap();
        drop(lager.uart("uart1").await.unwrap());
        assert!(fake.wait_for_event("stop_uart"));
        assert_eq!(fake.authorization().as_deref(), Some("Bearer tok-2"));
    }
}

#[cfg(all(feature = "rtt", feature = "async"))]
mod async_rtt {
    use super::*;
    use lager::{AsyncLagerBox, Error, RttOptions};

    #[tokio::test]
    async fn writes_down_and_reads_up() {
        let fake = rtt_box();
        let lager = AsyncLagerBox::connect(&fake.url).unwrap();
        let opts = RttOptions {
            channel: 1,
            ..Default::default()
        };
        let mut rtt = lager
            .debug("debug1")
            .rtt_interactive_with(&opts)
            .await
            .unwrap();
        assert_eq!(rtt.backend(), "jlink");
        assert_eq!(rtt.channel(), 1);
        rtt.write_str("ping\n").await.unwrap();
        assert_eq!(rtt.wait_for(b"pong\n", WAIT).await.unwrap(), b"> pong\n");
        rtt.stop().await.unwrap();
        assert!(fake.wait_for_event("stop_rtt"));
        assert_eq!(
            fake.received()[0],
            (
                "start_rtt".to_string(),
                json!({"netname": "debug1", "channel": 1})
            )
        );
    }

    #[tokio::test]
    async fn a_refused_attach_is_a_stream_error() {
        let fake = refusing_rtt_box();
        let lager = AsyncLagerBox::connect(&fake.url).unwrap();
        match lager.debug("debug1").rtt_interactive().await {
            Err(Error::Stream(message)) => assert!(message.contains("No gdbserver")),
            Err(other) => panic!("expected a stream error, got {other:?}"),
            Ok(_) => panic!("expected a stream error, got a session"),
        }
    }

    #[tokio::test]
    async fn a_timed_out_wait_names_the_needle() {
        let fake = rtt_box();
        let lager = AsyncLagerBox::connect(&fake.url).unwrap();
        let mut rtt = lager.debug("debug1").rtt_interactive().await.unwrap();
        match rtt.wait_for(b"never", Duration::from_millis(150)).await {
            Err(Error::Timeout(message)) => assert!(message.contains("never")),
            other => panic!("expected a timeout, got {other:?}"),
        }
    }
}
