//! Real-hardware smoke tests against a live Lager box.
//!
//! All tests are `#[ignore]`d so `cargo test` stays hermetic. To run them,
//! point `LAGER_BOX_HOST` at a box and opt in:
//!
//! ```sh
//! LAGER_BOX_HOST=192.168.1.42 cargo test --test hardware -- --ignored
//! ```
//!
//! Net-specific tests additionally need the net name in an env var (e.g.
//! `LAGER_TEST_SUPPLY_NET=supply1`) and are skipped otherwise, so you can
//! run the suite against whatever your box has configured.

#![cfg(feature = "blocking")]

use lager::{LagerBox, Level};

fn lager() -> LagerBox {
    LagerBox::from_env().expect("set LAGER_BOX_HOST to a reachable box")
}

/// Returns the net name in `var`, or None (test prints a skip note).
fn net_from_env(var: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(name) if !name.is_empty() => Some(name),
        _ => {
            eprintln!("skipping: set {var} to a configured net name to run this test");
            None
        }
    }
}

#[test]
#[ignore = "requires a live box (LAGER_BOX_HOST)"]
fn box_is_healthy() {
    let lager = lager();
    let health = lager.health().unwrap();
    assert_eq!(health.status, "healthy");

    let status = lager.status().unwrap();
    assert!(status.healthy);
    assert!(
        status.capabilities.net_command,
        "box {} does not serve /net/command; update the box",
        status.version
    );
}

#[test]
#[ignore = "requires a live box (LAGER_BOX_HOST)"]
fn lists_nets() {
    let nets = lager().nets().unwrap();
    eprintln!("box has {} nets:", nets.len());
    for net in &nets {
        eprintln!("  {} ({})", net.name, net.role);
    }
}

#[test]
#[ignore = "requires a live box + LAGER_TEST_SUPPLY_NET"]
fn supply_cycle() {
    let Some(name) = net_from_env("LAGER_TEST_SUPPLY_NET") else {
        return;
    };
    let lager = lager();
    let supply = lager.supply(&name);

    supply.set_voltage(3.3).unwrap();
    supply.enable().unwrap();
    let state = supply.state().unwrap();
    assert_eq!(state.enabled, Some(true));
    assert!(state.voltage_set.map(|v| (v - 3.3).abs() < 0.05).unwrap_or(false));
    supply.disable().unwrap();
    let state = supply.state().unwrap();
    assert_eq!(state.enabled, Some(false));
}

#[test]
#[ignore = "requires a live box + LAGER_TEST_SOLAR_NET (an EA PSB solar simulator)"]
fn solar_cycle() {
    let Some(name) = net_from_env("LAGER_TEST_SOLAR_NET") else {
        return;
    };
    let lager = lager();
    let solar = lager.solar(&name);

    solar.set().unwrap();
    let applied = solar.set_irradiance(800.0).unwrap();
    assert!((applied - 800.0).abs() < 1.0);
    let irr = solar.irradiance().unwrap();
    eprintln!("{name}: irradiance {irr} W/m²");
    assert!(irr.is_finite());
    let voc = solar.voc().unwrap();
    let mpp_v = solar.mpp_voltage().unwrap();
    let mpp_i = solar.mpp_current().unwrap();
    eprintln!("{name}: Voc {voc} V, MPP {mpp_v} V / {mpp_i} A");
    assert!(voc.is_finite() && mpp_v.is_finite() && mpp_i.is_finite());
    solar.stop().unwrap();
}

#[test]
#[ignore = "requires a live box + LAGER_TEST_ADC_NET"]
fn adc_reads_a_voltage() {
    let Some(name) = net_from_env("LAGER_TEST_ADC_NET") else {
        return;
    };
    let v = lager().adc(&name).read().unwrap();
    eprintln!("{name} reads {v} V");
    assert!(v.is_finite());
}

#[test]
#[ignore = "requires a live box + LAGER_TEST_GPIO_NET (a safe-to-toggle output)"]
fn gpio_toggles() {
    let Some(name) = net_from_env("LAGER_TEST_GPIO_NET") else {
        return;
    };
    let lager = lager();
    let gpio = lager.gpio(&name);
    gpio.output(Level::Low).unwrap();
    let level = gpio.toggle().unwrap();
    assert_eq!(level, Level::High);
    gpio.output(Level::Low).unwrap();
}

#[test]
#[ignore = "requires a live box + LAGER_TEST_DEBUG_NET (a configured debug probe)"]
fn debug_status_and_info() {
    let Some(name) = net_from_env("LAGER_TEST_DEBUG_NET") else {
        return;
    };
    let lager = lager();
    let debug = lager.debug(&name);

    // info/status are read-only and safe to run without touching flash.
    let info = debug.info().unwrap();
    eprintln!(
        "{name}: device={:?} arch={:?} backend={:?} connected={}",
        info.device, info.arch, info.backend, info.connected
    );

    let status = debug.status().unwrap();
    eprintln!("{name}: connected={} pid={:?}", status.connected, status.pid);
}

#[test]
#[ignore = "DESTRUCTIVE: erases/reflashes. Needs LAGER_TEST_DEBUG_NET + LAGER_TEST_FIRMWARE"]
fn debug_flash_reset_read() {
    let Some(name) = net_from_env("LAGER_TEST_DEBUG_NET") else {
        return;
    };
    let Some(firmware) = net_from_env("LAGER_TEST_FIRMWARE") else {
        return;
    };
    let lager = lager();
    let debug = lager.debug(&name);

    debug.connect().unwrap();
    debug.flash(&firmware).unwrap();
    debug.reset(false).unwrap();
    let head = debug.read_memory(0x0800_0000, 16).unwrap();
    eprintln!("first 16 bytes of flash: {head:02x?}");
    assert_eq!(head.len(), 16);
    debug.disconnect(false).unwrap();
}

#[test]
#[ignore = "requires a live box + LAGER_TEST_ARM_NET (clear the arm's workspace first)"]
fn arm_position_and_home() {
    let Some(name) = net_from_env("LAGER_TEST_ARM_NET") else {
        return;
    };
    let lager = lager();
    let arm = lager.arm(&name);

    let pos = arm.position().unwrap();
    eprintln!("{name} at X{} Y{} Z{}", pos.x, pos.y, pos.z);
    arm.go_home().unwrap();
    let pos = arm.position().unwrap();
    eprintln!("{name} homed to X{} Y{} Z{}", pos.x, pos.y, pos.z);
}

#[test]
#[ignore = "requires a live box + LAGER_TEST_WEBCAM_NET"]
fn webcam_stream_cycle() {
    let Some(name) = net_from_env("LAGER_TEST_WEBCAM_NET") else {
        return;
    };
    let lager = lager();
    let cam = lager.webcam(&name);

    let stream = cam.start().unwrap();
    eprintln!("{name} streaming at {}", stream.url);
    let status = cam.status().unwrap();
    assert!(status.running);
    assert_eq!(cam.url().unwrap(), Some(stream.url));
    cam.stop().unwrap();
    assert!(!cam.status().unwrap().running);
}

#[test]
#[ignore = "requires a live box + LAGER_TEST_ROUTER_NET"]
fn router_connect_and_reads() {
    let Some(name) = net_from_env("LAGER_TEST_ROUTER_NET") else {
        return;
    };
    let lager = lager();
    let router = lager.router(&name);

    let conn = router.connect().unwrap();
    eprintln!("{name}: {conn}");
    let info = router.system_info().unwrap();
    eprintln!(
        "{name}: board={:?} version={:?} uptime={:?}",
        info.board, info.version, info.uptime
    );
    let interfaces = router.interfaces().unwrap();
    eprintln!(
        "{name} has {} interface(s)",
        interfaces.as_array().map(Vec::len).unwrap_or(0)
    );
}

#[test]
#[ignore = "requires a live box with a Bluetooth adapter (bleCommand capability)"]
fn ble_scan_finds_advertisers() {
    let lager = lager();
    let devices = lager.ble().scan(5.0).unwrap();
    eprintln!("found {} BLE device(s):", devices.len());
    for d in &devices {
        eprintln!("  {} {} ({:?} dBm)", d.address, d.name, d.rssi);
    }
}

#[test]
#[ignore = "requires a live box with a wlan interface (wifiCommand capability)"]
fn wifi_status_and_scan() {
    let lager = lager();
    let wifi = lager.wifi();
    for i in wifi.status().unwrap() {
        eprintln!("{}: {} ({})", i.interface, i.ssid, i.state);
    }
    let aps = wifi.scan("wlan0").unwrap();
    eprintln!("found {} access point(s)", aps.len());
}

#[test]
#[ignore = "requires a live box + LAGER_TEST_BLUFI_DEVICE (an advertising BluFi target)"]
fn blufi_connect_and_status() {
    let Some(device) = net_from_env("LAGER_TEST_BLUFI_DEVICE") else {
        return;
    };
    let lager = lager();
    let blufi = lager.blufi();
    let info = blufi.connect(&device).unwrap();
    eprintln!(
        "{device}: version={:?} sta={:?}",
        info.version, info.status.sta_conn_name
    );
    let state = blufi.status(&device).unwrap();
    eprintln!("{device}: op mode {:?}", state.op_mode_name);
}

#[cfg(feature = "uart")]
#[test]
#[ignore = "requires a live box + LAGER_TEST_UART_NET"]
fn uart_streams() {
    use std::time::Duration;

    let Some(name) = net_from_env("LAGER_TEST_UART_NET") else {
        return;
    };
    let lager = lager();
    let mut uart = lager.uart(name).unwrap();
    eprintln!(
        "uart open: {} at {} baud",
        uart.device_path(),
        uart.baudrate()
    );
    let out = uart.read(Duration::from_secs(2)).unwrap();
    eprintln!("received {} bytes", out.len());
    uart.stop().unwrap();
}
