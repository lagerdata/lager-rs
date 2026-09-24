//! Start a debug net's GDB server and talk GDB remote protocol to it over
//! `LagerBox::debug_tunnel`, which works the same on a plain box and on a
//! box behind an authenticating gateway.
//!
//! ```sh
//! LAGER_BOX_HOST=192.168.1.42 cargo run --example debug_tunnel -- debug1
//! ```
//!
//! Sends `qSupported`, reads 32 bytes at 0x20000000 (`m20000000,20`), then
//! detaches so the target runs on, and stops the GDB server.

use std::io::{Read, Write};
use std::net::TcpStream;

use lager::LagerBox;

fn main() -> lager::Result<()> {
    let net = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "debug1".to_string());
    let lager_box = LagerBox::from_env()?;
    let debug = lager_box.debug(&net);

    let connection = debug.connect()?;
    let port = connection
        .gdb_server
        .and_then(|server| server.gdb_port)
        .unwrap_or(2331) as u16;
    println!("GDB server for {net} on port {port}");

    // A server the box has only just started can take a moment to listen;
    // until it does, a tunnel to it is refused with 502.
    let mut attempts = 0;
    let mut stream = loop {
        match lager_box.debug_tunnel(port) {
            Err(lager::Error::Box { status: 502, .. }) if attempts < 10 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            other => break other?,
        }
    };
    let result = session(&mut stream);
    drop(stream);
    debug.disconnect(false)?;
    result.map_err(|e| lager::Error::Stream(format!("GDB remote protocol: {e}")))
}

fn session(stream: &mut TcpStream) -> std::io::Result<()> {
    let features = exchange(stream, "qSupported:multiprocess+;swbreak+;hwbreak+")?;
    println!("qSupported -> {features}");

    let memory = exchange(stream, "m20000000,20")?;
    println!("m20000000,20 -> {memory}");
    if let Some(text) = hex_to_text(&memory) {
        println!("  as text: {text:?}");
    }

    println!("D -> {}", exchange(stream, "D")?);
    Ok(())
}

/// Send one packet and return the reply packet's payload.
fn exchange(stream: &mut TcpStream, payload: &str) -> std::io::Result<String> {
    let checksum = payload.bytes().fold(0u8, |acc, b| acc.wrapping_add(b));
    stream.write_all(format!("${payload}#{checksum:02x}").as_bytes())?;
    let reply = read_packet(stream)?;
    stream.write_all(b"+")?;
    Ok(reply)
}

/// Read up to the next `$...#xx` packet, skipping acks.
fn read_packet(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte)?;
        if byte[0] == b'$' {
            break;
        }
    }
    let mut payload = Vec::new();
    loop {
        stream.read_exact(&mut byte)?;
        if byte[0] == b'#' {
            break;
        }
        payload.push(byte[0]);
    }
    let mut checksum = [0u8; 2];
    stream.read_exact(&mut checksum)?;
    Ok(String::from_utf8_lossy(&payload).into_owned())
}

fn hex_to_text(hex: &str) -> Option<String> {
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect();
    Some(String::from_utf8_lossy(&bytes?).into_owned())
}
