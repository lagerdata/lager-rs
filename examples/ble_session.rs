//! Exercise a BLE GATT session end to end against `lager-ble-peer`, the test
//! peripheral in the lager repository (`test/assets/ble_peer/ble_peer.py`),
//! running on a second box in radio range:
//!
//! ```sh
//! lager python test/assets/ble_peer/ble_peer.py --box <PEER-BOX> --detach
//! LAGER_BOX_HOST=<BOX> cargo run --example ble_session --features ble-session -- AA:BB:CC:DD:EE:01
//! ```
//!
//! Checks the box has a radio (skipping if not) and reports the peer's
//! address type from a scan. Then opens a session, echoes a 600-byte message
//! in 500-byte writes, collects a 500-notification burst, reads a
//! characteristic, and has the peer drop the link, checking the notifications
//! already received come out before the `Disconnected` error.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use lager::{BleErrorKind, BleSessionOptions, Error, LagerBox, WriteOptions};

const ECHO: &str = "12345678-1234-5678-1234-56789abcdef1";
const RX: &str = "12345678-1234-5678-1234-56789abcdef2";
const CTRL: &str = "12345678-1234-5678-1234-56789abcdef3";
const TICK: &str = "12345678-1234-5678-1234-56789abcdef4";
const INFO: &str = "12345678-1234-5678-1234-56789abcdef5";

fn main() -> lager::Result<()> {
    let address = std::env::args()
        .nth(1)
        .expect("usage: ble_session <peer address>");
    let lager = LagerBox::from_env()?;

    // Skip cleanly on a box without a usable radio.
    let adapter = lager.ble().adapter()?;
    if !adapter.available {
        println!("skipping: {}", adapter.reason.unwrap_or_default());
        return Ok(());
    }
    for a in &adapter.adapters {
        println!("adapter: {} {:?} powered={}", a.name, a.address, a.powered);
    }
    if let Some(d) = lager
        .ble()
        .scan(5.0)?
        .into_iter()
        .find(|d| d.address.eq_ignore_ascii_case(&address))
    {
        println!(
            "scan: {} address_type={:?} random_type={:?} static_random={}",
            d.address,
            d.address_type,
            d.random_type,
            d.is_static_random()
        );
    }

    let t = Instant::now();
    let opts = BleSessionOptions {
        holder: Some("lager-net ble_session example".to_string()),
        ..BleSessionOptions::default()
    };
    let mut s = lager.ble_session(&address, opts)?;
    println!(
        "open: {:?}, MTU {} (measured: {}), max write {}",
        t.elapsed(),
        s.mtu(),
        s.mtu_is_measured(),
        s.max_write_len()
    );

    // Echo: the peer returns every write as notifications on ECHO.
    s.subscribe(ECHO)?;
    let message: Vec<u8> = (0..600u32).map(|i| (i % 256) as u8).collect();
    let t = Instant::now();
    s.write(RX, &message, WriteOptions::chunked_to(500))?;
    let mut echoed = Vec::new();
    while echoed.len() < message.len() {
        echoed.extend(s.recv(Duration::from_secs(3))?.data);
    }
    println!(
        "echo: {} bytes back in {:?}, identical: {}",
        echoed.len(),
        t.elapsed(),
        echoed == message
    );
    s.unsubscribe(ECHO)?;

    // Burst: 500 TICK notifications back to back. Counted by value, not
    // order: two BlueZ hosts negotiate EATT, which keeps order per bearer
    // only.
    s.subscribe(TICK)?;
    s.write(CTRL, &[0x02, 0x01, 0xf4], WriteOptions::default())?;
    let mut values = BTreeSet::new();
    let mut received = 0usize;
    let deadline = Instant::now() + Duration::from_secs(10);
    while received < 500 && Instant::now() < deadline {
        match s.recv(Duration::from_secs(2)) {
            Ok(n) => {
                received += 1;
                values.insert(u32::from_be_bytes(n.data[..4].try_into().unwrap()));
            }
            Err(Error::Timeout(_)) => break,
            Err(e) => return Err(e),
        }
    }
    let span = match (values.first(), values.last()) {
        (Some(a), Some(b)) => b - a + 1,
        _ => 0,
    };
    println!(
        "burst: {received} notifications, {} distinct values spanning {span}",
        values.len()
    );

    println!("read: {:?}", String::from_utf8_lossy(&s.read(INFO)?));

    // Forced disconnect: ask the peer to drop the link. Ticks already
    // received must come out first, then Disconnected.
    s.write(CTRL, &[0x01], WriteOptions::default())?;
    let mut drained = 0;
    loop {
        match s.recv(Duration::from_secs(10)) {
            Ok(_) => drained += 1,
            Err(Error::Ble {
                kind: BleErrorKind::Disconnected,
                message,
            }) => {
                println!("disconnect: {drained} notification(s) drained first, then: {message}");
                break;
            }
            Err(e) => return Err(e),
        }
    }
    println!("close reason: {:?}", s.close_reason());
    Ok(())
}
