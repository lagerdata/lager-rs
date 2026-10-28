//! Start a debug net's GDB server and talk GDB remote protocol to it over
//! `LagerBox::debug_tunnel`, which works the same on a plain box and on a
//! box behind an authenticating gateway.
//!
//! ```sh
//! LAGER_BOX_HOST=192.168.1.42 cargo run --example debug_tunnel -- debug1
//! LAGER_BOX_HOST=192.168.1.42 cargo run --example debug_tunnel -- debug1 --write
//! ```
//!
//! Sends `qSupported`, reads the core registers (`g`) and 32 bytes at
//! 0x20000000 (`m20000000,20`), then detaches so the target runs on, and
//! stops the GDB server.
//!
//! With `--write` it also writes four bytes to RAM at 0x20000000 (`M`),
//! reads them back, and writes the original bytes back. That address is
//! the start of RAM on most Cortex-M parts; on a running target it can hold
//! live data, so use `--write` only on a target you can reset.
//!
//! This is how a Rust test reaches what the typed API does not cover:
//! register access, memory writes, breakpoints, or any other GDB remote
//! protocol packet.

use std::io::{Read, Write};
use std::net::TcpStream;

use lager::LagerBox;

fn main() -> lager::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let write = args.iter().any(|a| a == "--write");
    let net = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "debug1".to_string());
    let lager_box = LagerBox::from_env()?;
    let debug = lager_box.debug(&net);

    let connection = debug.connect()?;
    let port = connection
        .gdb_server
        .and_then(|server| server.gdb_port)
        .unwrap_or(2331) as u16;
    println!("GDB server for {net} on port {port}");

    // debug_tunnel waits out the moment a just-started server takes to
    // open its port.
    let mut stream = lager_box.debug_tunnel(port)?;
    let result = session(&mut stream, write);
    drop(stream);
    debug.disconnect(false)?;
    result.map_err(|e| lager::Error::Stream(format!("GDB remote protocol: {e}")))
}

fn session(stream: &mut TcpStream, write: bool) -> std::io::Result<()> {
    let features = exchange(stream, "qSupported:multiprocess+;swbreak+;hwbreak+")?;
    println!("qSupported -> {features}");

    // All core registers as one hex string, in the target's register order
    // (r0-r15 first on Cortex-M).
    let registers = exchange(stream, "g")?;
    println!("g -> {registers}");

    let memory = exchange(stream, "m20000000,20")?;
    println!("m20000000,20 -> {memory}");
    if let Some(text) = hex_to_text(&memory) {
        println!("  as text: {text:?}");
    }

    if write {
        let original = exchange(stream, "m20000000,4")?;
        // An error reply (`E01`) means the target refused the read; writing
        // it back as data would corrupt RAM, so stop here.
        if original.len() != 8 || !original.bytes().all(|b| b.is_ascii_hexdigit()) {
            println!("m20000000,4 -> {original}: not writing");
        } else {
            let wrote = exchange(stream, "M20000000,4:deadbeef")?;
            println!("M20000000,4:deadbeef -> {wrote}");
            println!("m20000000,4 -> {}", exchange(stream, "m20000000,4")?);
            let restore = format!("M20000000,4:{original}");
            println!("{restore} -> {}", exchange(stream, &restore)?);
        }
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
