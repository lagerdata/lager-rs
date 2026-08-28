//! Contract tests against a mock box HTTP server.
//!
//! Each test pins down the exact request JSON this crate sends (netname /
//! role / action / params) and that it parses the real response shapes the
//! box emits — captured from the handlers in
//! `box/lager/http_handlers/{net_command,supply,battery,usb}.py`.

#![cfg(feature = "blocking")]

use httpmock::prelude::*;
use lager::{
    BatteryMode, EloadMode, Error, LagerBox, Level, SafetyLimits, SpiConfig, SpiOptions,
    WaitForLevelOptions,
};
use serde_json::json;

fn client(server: &MockServer) -> LagerBox {
    LagerBox::connect(server.address().to_string()).unwrap()
}

// ---------------------------------------------------------------------------
// GPIO
// ---------------------------------------------------------------------------

#[test]
fn gpio_input() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "gpi1", "role": "gpio", "action": "input", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "input", "message": "HIGH (1)", "value": 1
        }));
    });
    let lager = client(&server);
    assert_eq!(lager.gpio("gpi1").input().unwrap(), Level::High);
    m.assert();
}

#[test]
fn gpio_output_and_toggle() {
    let server = MockServer::start();
    let out = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "gpo1", "role": "gpio", "action": "output",
            "params": {"level": "low"}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "output", "message": "Output set LOW", "value": 0}));
    });
    let toggle = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "gpo1", "role": "gpio", "action": "output",
            "params": {"level": "toggle"}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "output", "message": "Toggled to HIGH", "value": 1}));
    });
    let lager = client(&server);
    let gpio = lager.gpio("gpo1");
    gpio.output(Level::Low).unwrap();
    assert_eq!(gpio.toggle().unwrap(), Level::High);
    out.assert();
    toggle.assert();
}

#[test]
fn gpio_wait_for_level_sends_wait_params() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "gpi1", "role": "gpio", "action": "wait_for_level",
            "params": {"level": "high", "timeout": 5.0, "scan_rate": 10000}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "wait_for_level",
            "message": "Reached level high in 0.1234s", "value": 0.1234
        }));
    });
    let lager = client(&server);
    let elapsed = lager
        .gpio("gpi1")
        .wait_for_level_with(
            Level::High,
            &WaitForLevelOptions {
                timeout: Some(5.0),
                scan_rate: Some(10_000),
                ..Default::default()
            },
        )
        .unwrap();
    assert!((elapsed - 0.1234).abs() < 1e-9);
    m.assert();
}

// ---------------------------------------------------------------------------
// ADC / DAC / thermocouple
// ---------------------------------------------------------------------------

#[test]
fn adc_read() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "adc1", "role": "adc", "action": "read", "params": {}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "read", "message": "1.5 V", "value": 1.5}));
    });
    let lager = client(&server);
    assert_eq!(lager.adc("adc1").read().unwrap(), 1.5);
    m.assert();
}

#[test]
fn dac_set_and_read() {
    let server = MockServer::start();
    let set = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "dac1", "role": "dac", "action": "set", "params": {"value": 2.5}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "set", "message": "Set to 2.500000 V", "value": 2.5}));
    });
    let read = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "dac1", "role": "dac", "action": "read", "params": {}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "read", "message": "2.5 V", "value": 2.5}));
    });
    let lager = client(&server);
    let dac = lager.dac("dac1");
    dac.set(2.5).unwrap();
    assert_eq!(dac.read().unwrap(), 2.5);
    set.assert();
    read.assert();
}

#[test]
fn thermocouple_read() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "tc1", "role": "thermocouple", "action": "read", "params": {}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "read", "message": "24.8 °C", "value": 24.8}));
    });
    let lager = client(&server);
    assert_eq!(lager.thermocouple("tc1").read().unwrap(), 24.8);
    m.assert();
}

// ---------------------------------------------------------------------------
// Watt meter / energy analyzer
// ---------------------------------------------------------------------------

#[test]
fn watt_power_and_all() {
    let server = MockServer::start();
    let power = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "watt1", "role": "watt-meter", "action": "power",
            "params": {"duration": 0.1}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "power", "message": "0.0523 W", "value": 0.0523}));
    });
    let all = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "watt1", "role": "watt-meter", "action": "all",
            "params": {"duration": 0.5}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "all",
            "message": "I 0.010000 A, V 3.300 V, P 0.033000 W (0.5s)",
            "value": {"current": 0.01, "voltage": 3.3, "power": 0.033, "duration_s": 0.5}
        }));
    });
    let lager = client(&server);
    let watt = lager.watt_meter("watt1");
    assert_eq!(watt.power(0.1).unwrap(), 0.0523);
    let reading = watt.all(0.5).unwrap();
    assert_eq!(reading.current, Some(0.01));
    assert_eq!(reading.voltage, Some(3.3));
    assert_eq!(reading.power, Some(0.033));
    power.assert();
    all.assert();
}

#[test]
fn energy_read_energy_and_stats() {
    let server = MockServer::start();
    let energy = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "en1", "role": "energy-analyzer", "action": "read_energy",
            "params": {"duration": 10.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "read_energy",
            "message": "1.2345 J (0.3741 C) over 10.0 s",
            "value": {"energy_j": 1.2345, "charge_c": 0.3741, "duration_s": 10.0}
        }));
    });
    let stats = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "en1", "role": "energy-analyzer", "action": "read_stats",
            "params": {"duration": 1.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "read_stats", "message": "...",
            "value": {
                "current": {"mean": 0.01, "min": 0.001, "max": 0.09},
                "voltage": {"mean": 3.3},
                "power": {"mean": 0.033}
            }
        }));
    });
    let lager = client(&server);
    let analyzer = lager.energy_analyzer("en1");
    let reading = analyzer.read_energy(10.0).unwrap();
    assert_eq!(reading.energy_j, Some(1.2345));
    assert_eq!(reading.charge_c, Some(0.3741));
    let s = analyzer.read_stats(1.0).unwrap();
    assert_eq!(s.current.unwrap().mean, Some(0.01));
    assert_eq!(s.voltage.unwrap().mean, Some(3.3));
    energy.assert();
    stats.assert();
}

// ---------------------------------------------------------------------------
// E-load
// ---------------------------------------------------------------------------

#[test]
fn eload_set_state_and_setpoint() {
    let server = MockServer::start();
    let set = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "eload1", "role": "eload", "action": "cc", "params": {"value": 0.5}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "cc", "message": "0.5", "value": 0.5}));
    });
    let state = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "eload1", "role": "eload", "action": "state", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "state",
            "message": "Mode cc, Enabled, V 3.290, I 0.499, P 1.642",
            "value": {
                "mode": "cc", "input_enabled": true,
                "measured_voltage": 3.29, "measured_current": 0.499, "measured_power": 1.642
            }
        }));
    });
    let setpoint = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "eload1", "role": "eload", "action": "cv", "params": {}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "cv", "message": "3.3", "value": 3.3}));
    });
    let lager = client(&server);
    let eload = lager.eload("eload1");
    eload.set(EloadMode::Cc, 0.5).unwrap();
    let st = eload.state().unwrap();
    assert_eq!(st.mode.as_deref(), Some("cc"));
    assert_eq!(st.input_enabled, Some(true));
    assert_eq!(st.measured_current, Some(0.499));
    assert_eq!(eload.setpoint(EloadMode::Cv).unwrap(), 3.3);
    set.assert();
    state.assert();
    setpoint.assert();
}

// ---------------------------------------------------------------------------
// Solar simulator
// ---------------------------------------------------------------------------

#[test]
fn solar_set_and_stop() {
    let server = MockServer::start();
    let set = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "solar1", "role": "solar", "action": "set", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "set",
            "message": "Solar simulator 'solar1' initialized and started in PV simulation mode"
        }));
    });
    let stop = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "solar1", "role": "solar", "action": "stop", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "stop",
            "message": "Solar simulator 'solar1' stopped"
        }));
    });
    let lager = client(&server);
    let solar = lager.solar("solar1");
    solar.set().unwrap();
    solar.stop().unwrap();
    set.assert();
    stop.assert();
}

#[test]
fn solar_irradiance_read_and_set() {
    let server = MockServer::start();
    let read = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "solar1", "role": "solar", "action": "irradiance", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "irradiance", "message": "1000.0", "value": 1000.0
        }));
    });
    let set = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "solar1", "role": "solar", "action": "irradiance",
            "params": {"value": 800.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "irradiance", "message": "800.0", "value": 800.0
        }));
    });
    let lager = client(&server);
    let solar = lager.solar("solar1");
    assert_eq!(solar.irradiance().unwrap(), 1000.0);
    assert_eq!(solar.set_irradiance(800.0).unwrap(), 800.0);
    read.assert();
    set.assert();
}

#[test]
fn solar_reads_parse_numeric_values() {
    let server = MockServer::start();
    let mocks: Vec<_> = [
        ("mpp_current", "1.234 A", 1.234),
        ("mpp_voltage", "12.345 V", 12.345),
        ("temperature", "25.0°C", 25.0),
        ("voc", "21.980 V", 21.980),
    ]
    .into_iter()
    .map(|(action, message, value)| {
        server.mock(move |when, then| {
            when.method(POST).path("/net/command").json_body(json!({
                "netname": "solar1", "role": "solar", "action": action, "params": {}
            }));
            then.status(200).json_body(json!({
                "success": true, "action": action, "message": message, "value": value
            }));
        })
    })
    .collect();

    let lager = client(&server);
    let solar = lager.solar("solar1");
    assert_eq!(solar.mpp_current().unwrap(), 1.234);
    assert_eq!(solar.mpp_voltage().unwrap(), 12.345);
    assert_eq!(solar.temperature().unwrap(), 25.0);
    assert_eq!(solar.voc().unwrap(), 21.980);
    for m in mocks {
        m.assert();
    }
}

#[test]
fn solar_resistance_read_and_set() {
    let server = MockServer::start();
    let read = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "solar1", "role": "solar", "action": "resistance", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "resistance", "message": "4.00", "value": 4.0
        }));
    });
    let set = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "solar1", "role": "solar", "action": "resistance",
            "params": {"value": 2.5}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "resistance", "message": "2.50", "value": 2.5
        }));
    });
    let lager = client(&server);
    let solar = lager.solar("solar1");
    assert_eq!(solar.resistance().unwrap(), 4.0);
    assert_eq!(solar.set_resistance(2.5).unwrap(), 2.5);
    read.assert();
    set.assert();
}

// ---------------------------------------------------------------------------
// SPI
// ---------------------------------------------------------------------------

#[test]
fn spi_configure_and_transactions() {
    let server = MockServer::start();
    let config = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "spi1", "role": "spi", "action": "config",
            "params": {"mode": 0, "frequency_hz": 1_000_000, "bit_order": "msb"}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "config",
            "message": "SPI configured: ...",
            "value": {
                "mode": 0, "frequency_hz": 1_000_000, "word_size": 8,
                "bit_order": "msb", "cs_active": "low", "cs_mode": "auto"
            }
        }));
    });
    let read = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "spi1", "role": "spi", "action": "read",
            "params": {"n_words": 3, "fill": 255, "keep_cs": false}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "read", "message": "9F 25 16",
            "value": [0x9F, 0x25, 0x16], "word_size": 8
        }));
    });
    let write = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "spi1", "role": "spi", "action": "write",
            "params": {"data": [0x9F], "keep_cs": true}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "write", "message": "FF",
            "value": [0xFF], "word_size": 8
        }));
    });
    let lager = client(&server);
    let spi = lager.spi("spi1");

    let effective = spi
        .configure(&SpiConfig {
            mode: Some(0),
            frequency_hz: Some(1_000_000),
            bit_order: Some(lager::BitOrder::Msb),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(effective.frequency_hz, Some(1_000_000));
    assert_eq!(effective.cs_mode.as_deref(), Some("auto"));

    let t = spi.read(3, &SpiOptions::default()).unwrap();
    assert_eq!(t.words, vec![0x9F, 0x25, 0x16]);
    assert_eq!(t.word_size, 8);

    let t = spi
        .write(
            &[0x9F],
            &SpiOptions {
                keep_cs: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(t.words, vec![0xFF]);

    config.assert();
    read.assert();
    write.assert();
}

// ---------------------------------------------------------------------------
// I2C
// ---------------------------------------------------------------------------

#[test]
fn i2c_scan_read_write_transfer() {
    let server = MockServer::start();
    let scan = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "i2c1", "role": "i2c", "action": "scan", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "scan",
            "message": "Found 2 device(s): 0x1d, 0x68", "value": [0x1D, 0x68]
        }));
    });
    let read = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "i2c1", "role": "i2c", "action": "read",
            "params": {"address": 0x68, "num_bytes": 2}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "read", "message": "75 00", "value": [0x75, 0x00]}));
    });
    let write = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "i2c1", "role": "i2c", "action": "write",
            "params": {"address": 0x68, "data": [0x6B, 0x00]}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "write", "message": "Wrote 2 byte(s) to 0x68"}));
    });
    let transfer = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "i2c1", "role": "i2c", "action": "transfer",
            "params": {"address": 0x68, "data": [0x75], "num_bytes": 1}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "transfer", "message": "68", "value": [0x68]}));
    });
    let lager = client(&server);
    let i2c = lager.i2c("i2c1");
    assert_eq!(i2c.scan().unwrap(), vec![0x1D, 0x68]);
    assert_eq!(i2c.read(0x68, 2).unwrap(), vec![0x75, 0x00]);
    i2c.write(0x68, &[0x6B, 0x00]).unwrap();
    assert_eq!(i2c.write_read(0x68, &[0x75], 1).unwrap(), vec![0x68]);
    scan.assert();
    read.assert();
    write.assert();
    transfer.assert();
}

#[test]
fn i2c_configure() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "i2c1", "role": "i2c", "action": "config",
            "params": {"frequency_hz": 400_000, "pull_ups": true}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "config",
            "message": "I2C configured: freq=400000Hz, pull_ups=on",
            "value": {"frequency_hz": 400_000, "pull_ups": true}
        }));
    });
    let lager = client(&server);
    let cfg = lager.i2c("i2c1").configure(Some(400_000), Some(true)).unwrap();
    assert_eq!(cfg.frequency_hz, Some(400_000));
    assert_eq!(cfg.pull_ups, Some(true));
    m.assert();
}

// ---------------------------------------------------------------------------
// Power supply
// ---------------------------------------------------------------------------

#[test]
fn supply_set_enable_disable() {
    let server = MockServer::start();
    let volts = server.mock(|when, then| {
        when.method(POST).path("/supply/command").json_body(json!({
            "netname": "supply1", "action": "voltage", "params": {"value": 3.3}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "voltage", "message": "Voltage set to 3.3V"}));
    });
    let enable = server.mock(|when, then| {
        when.method(POST).path("/supply/command").json_body(json!({
            "netname": "supply1", "action": "enable", "params": {}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "enable", "message": "Supply output enabled"}));
    });
    let disable = server.mock(|when, then| {
        when.method(POST).path("/supply/command").json_body(json!({
            "netname": "supply1", "action": "disable", "params": {}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "disable", "message": "Supply output disabled"}));
    });
    let lager = client(&server);
    let supply = lager.supply("supply1");
    supply.set_voltage(3.3).unwrap();
    supply.enable().unwrap();
    supply.disable().unwrap();
    volts.assert();
    enable.assert();
    disable.assert();
}

#[test]
fn supply_state_full_shape_with_degraded_fields() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/supply/command").json_body(json!({
            "netname": "supply1", "action": "state", "params": {}
        }));
        // Degraded read: some fields null, per build_supply_state.
        then.status(200).json_body(json!({
            "success": true, "action": "state", "message": "Channel 1: ON, ...",
            "state": {
                "netname": "supply1", "channel": 1, "error": null,
                "voltage": 3.295, "current": 0.012, "power": null,
                "enabled": true, "mode": "CV",
                "voltage_set": 3.3, "current_set": 1.0,
                "voltage_max": 32.0, "current_max": 3.2,
                "ocp_limit": null, "ocp_tripped": false,
                "ovp_limit": 5.0, "ovp_tripped": false
            }
        }));
    });
    let lager = client(&server);
    let state = lager.supply("supply1").state().unwrap();
    assert_eq!(state.voltage, Some(3.295));
    assert_eq!(state.power, None);
    assert_eq!(state.enabled, Some(true));
    assert_eq!(state.voltage_set, Some(3.3));
    assert_eq!(state.ocp_limit, None);
    assert_eq!(state.ovp_limit, Some(5.0));
    m.assert();
}

// ---------------------------------------------------------------------------
// Battery
// ---------------------------------------------------------------------------

#[test]
fn battery_setters_and_state() {
    let server = MockServer::start();
    let soc = server.mock(|when, then| {
        when.method(POST).path("/battery/command").json_body(json!({
            "netname": "battery1", "action": "set_soc", "params": {"value": 80.0}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "set_soc", "message": "SOC set to 80%"}));
    });
    let mode = server.mock(|when, then| {
        when.method(POST).path("/battery/command").json_body(json!({
            "netname": "battery1", "action": "set_mode", "params": {"mode_type": "dynamic"}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "set_mode", "message": "Mode set to dynamic"}));
    });
    let enable = server.mock(|when, then| {
        when.method(POST).path("/battery/command").json_body(json!({
            "netname": "battery1", "action": "enable_battery", "params": {}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "enable_battery", "message": "Battery output enabled"}));
    });
    let state = server.mock(|when, then| {
        when.method(POST).path("/battery/command").json_body(json!({
            "netname": "battery1", "action": "state", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "state", "message": "Channel 1: ON, ...",
            "state": {
                "netname": "battery1", "channel": 1, "error": null,
                "terminal_voltage": 3.82, "current": -0.15, "esr": 0.05,
                "soc": 80.0, "voc": 3.9, "enabled": true,
                "mode": "dynamic", "model": "Custom", "capacity": 2.5,
                "current_limit": 1.0, "ocp_limit": 2.0, "ovp_limit": 4.5,
                "volt_full": 4.2, "volt_empty": 3.0,
                "ocp_tripped": false, "ovp_tripped": false
            }
        }));
    });
    let lager = client(&server);
    let battery = lager.battery("battery1");
    battery.set_soc(80.0).unwrap();
    battery.set_mode(BatteryMode::Dynamic).unwrap();
    battery.enable().unwrap();
    let st = battery.state().unwrap();
    assert_eq!(st.soc, Some(80.0));
    assert_eq!(st.terminal_voltage, Some(3.82));
    assert_eq!(st.mode.as_deref(), Some("dynamic"));
    soc.assert();
    mode.assert();
    enable.assert();
    state.assert();
}

// ---------------------------------------------------------------------------
// USB hub port
// ---------------------------------------------------------------------------

#[test]
fn usb_toggle_and_state() {
    let server = MockServer::start();
    let toggle = server.mock(|when, then| {
        when.method(POST).path("/usb/command").json_body(json!({
            "netname": "usb1", "action": "toggle"
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "toggle", "state": "disabled",
            "message": "USB port 'usb1' toggled → disabled"
        }));
    });
    let state = server.mock(|when, then| {
        when.method(POST).path("/usb/command").json_body(json!({
            "netname": "usb1", "action": "state"
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "state", "state": "enabled",
            "message": "USB port 'usb1' is enabled"
        }));
    });
    let lager = client(&server);
    let usb = lager.usb("usb1");
    assert!(!usb.toggle().unwrap());
    assert!(usb.state().unwrap());
    toggle.assert();
    state.assert();
}

#[test]
fn usb_state_maps_pre_0_29_rejection_to_unsupported() {
    // Box images before 0.29.0 don't know the `state` action; their 400
    // enumerates only enable|disable|toggle. That must surface as
    // UnsupportedByBox, not a generic box error.
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/usb/command").json_body(json!({
            "netname": "usb1", "action": "state"
        }));
        then.status(400).json_body(json!({
            "success": false,
            "error": "netname and action (enable|disable|toggle) are required"
        }));
    });
    let lager = client(&server);
    let err = lager.usb("usb1").state().unwrap_err();
    assert!(
        matches!(err, Error::UnsupportedByBox { .. }),
        "unexpected error: {err:?}"
    );
    assert!(err.to_string().contains("0.29.0"));
    m.assert();
}

#[test]
fn usb_state_bad_request_stays_a_box_error() {
    // A current box's validation message includes `state`; that 400 is a
    // genuine bad request and must NOT be reclassified.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/usb/command");
        then.status(400).json_body(json!({
            "success": false,
            "error": "netname and action (enable|disable|toggle|state) are required"
        }));
    });
    let lager = client(&server);
    let err = lager.usb("usb1").state().unwrap_err();
    assert!(matches!(err, Error::Box { status: 400, .. }));
}

#[test]
fn usb_cycle_default_omits_off_time_and_reads_reconnected() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/usb/command").json_body(json!({
            "netname": "usb1", "action": "cycle"
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "cycle", "state": "enabled",
            "message": "USB port 'usb1' power-cycled; device re-enumerated",
            "reconnected": true
        }));
    });
    let lager = client(&server);
    assert_eq!(lager.usb("usb1").cycle().unwrap(), Some(true));
    m.assert();
}

#[test]
fn usb_cycle_with_off_time_sends_it_and_handles_unobserved() {
    // A port with nothing on it (or a hub that cannot observe
    // re-enumeration) omits `reconnected`; that is "unconfirmed", not
    // "did not come back".
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/usb/command").json_body(json!({
            "netname": "usb1", "action": "cycle", "off_time": 2.5
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "cycle", "state": "enabled",
            "message": "USB port 'usb1' power-cycled; no device on this port"
        }));
    });
    let lager = client(&server);
    assert_eq!(lager.usb("usb1").cycle_with_off_time(2.5).unwrap(), None);
    m.assert();
}

#[test]
fn usb_recover_sends_plain_action() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/usb/command").json_body(json!({
            "netname": "usb1", "action": "recover"
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "recover", "state": "enabled",
            "message": "USB port 'usb1': power restored on port(s) 1, 3"
        }));
    });
    let lager = client(&server);
    lager.usb("usb1").recover().unwrap();
    m.assert();
}

#[test]
fn usb_cycle_maps_pre_0_39_rejection_to_unsupported() {
    // A 0.29-0.38 box's action list stops at `state`; that 400 means the
    // box is too old, not that the request was malformed.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/usb/command");
        then.status(400).json_body(json!({
            "success": false,
            "error": "netname and action (enable|disable|toggle|state) are required"
        }));
    });
    let lager = client(&server);
    let err = lager.usb("usb1").cycle().unwrap_err();
    assert!(
        matches!(err, Error::UnsupportedByBox { .. }),
        "unexpected error: {err:?}"
    );
    assert!(err.to_string().contains("0.39.0"));

    let err = lager.usb("usb1").recover().unwrap_err();
    assert!(matches!(err, Error::UnsupportedByBox { .. }));
}

#[test]
fn usb_cycle_bad_request_on_current_box_stays_a_box_error() {
    // A current box enumerates cycle/recover; its 400 is a genuine bad
    // request (e.g. off_time out of range) and must NOT be reclassified.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/usb/command");
        then.status(400).json_body(json!({
            "success": false,
            "error": "netname and action (enable|disable|toggle|state|cycle|recover) \
                      are required"
        }));
    });
    let lager = client(&server);
    let err = lager.usb("usb1").cycle().unwrap_err();
    assert!(matches!(err, Error::Box { status: 400, .. }));
}

// ---------------------------------------------------------------------------
// USB bus enumeration (GET /usb/devices)
// ---------------------------------------------------------------------------

#[test]
fn usb_devices_lists_bus() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET).path("/usb/devices");
        then.status(200).json_body(json!({
            "success": true,
            "devices": [
                {"sysfs_name": "1-1.4", "vid": "0483", "pid": "df11",
                 "serial": "STM32-DUT-01", "product": "STM32 BOOTLOADER",
                 "manufacturer": "STMicroelectronics", "busnum": "1",
                 "devnum": "42", "devpath": "1.4", "device_class": "00",
                 "speed": "12"},
                {"sysfs_name": "1-1.2", "vid": "0403", "pid": "6001",
                 "serial": null, "product": "FT232R USB UART",
                 "manufacturer": null, "busnum": "1", "devnum": "7",
                 "devpath": "1.2", "device_class": "00", "speed": "12"}
            ]
        }));
    });
    let lager = client(&server);
    let devices = lager.usb_devices().unwrap();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].vid.as_deref(), Some("0483"));
    assert_eq!(devices[0].serial.as_deref(), Some("STM32-DUT-01"));
    assert_eq!(devices[1].serial, None);
    m.assert();
}

#[test]
fn usb_devices_sends_filters_as_query_params() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/usb/devices")
            .query_param("vid", "0483")
            .query_param("serial", "STM32-DUT-01");
        then.status(200)
            .json_body(json!({"success": true, "devices": []}));
    });
    let lager = client(&server);
    let devices = lager
        .usb_devices_matching(&lager::UsbDeviceFilter {
            vid: Some("0483".into()),
            serial: Some("STM32-DUT-01".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(devices.is_empty());
    m.assert();
}

#[test]
fn usb_devices_missing_route_is_unsupported() {
    // Old boxes 404 the route with Flask's HTML error page (non-JSON).
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/usb/devices");
        then.status(404)
            .header("content-type", "text/html")
            .body("<!doctype html><title>404 Not Found</title>");
    });
    let lager = client(&server);
    let err = lager.usb_devices().unwrap_err();
    assert!(
        matches!(err, Error::UnsupportedByBox { .. }),
        "unexpected error: {err:?}"
    );
    assert!(err.to_string().contains("0.33.0"));
}

// ---------------------------------------------------------------------------
// DFU (POST /usb/dfu)
// ---------------------------------------------------------------------------

#[test]
fn dfu_list_parses_devices() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/usb/dfu").json_body(json!({
            "action": "list", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "list",
            "message": "Found 2 DFU device(s)",
            "value": {
                "exit_code": 0, "stdout": "...", "stderr": "",
                "devices": [
                    {"mode": "DFU", "vid": "0483", "pid": "df11",
                     "devnum": 42, "cfg": 1, "intf": 0, "alt": 0,
                     "name": "@Internal Flash  /0x08000000/256*0002Kg",
                     "serial": "STM32-DUT-01", "path": "1-1.4"},
                    {"mode": "Runtime", "vid": "0483", "pid": "374b",
                     "devnum": 9, "cfg": 1, "intf": 3, "alt": 0,
                     "name": "UNKNOWN", "serial": "066FFF38", "path": "1-1.2"}
                ]
            }
        }));
    });
    let lager = client(&server);
    let devices = lager.dfu().list().unwrap();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].mode, "DFU");
    assert_eq!(devices[0].alt, Some(0));
    assert_eq!(devices[0].serial.as_deref(), Some("STM32-DUT-01"));
    m.assert();
}

#[test]
fn dfu_download_sends_base64_firmware_and_options() {
    let server = MockServer::start();
    // "foobar" -> base64 "Zm9vYmFy" (same reference vector as debug flash).
    let m = server.mock(|when, then| {
        when.method(POST).path("/usb/dfu").json_body(json!({
            "action": "download",
            "params": {
                "vid_pid": "0483:df11",
                "alt": 0,
                "dfuse_address": "0x08000000:leave",
                "reset": true,
                "firmware": "Zm9vYmFy"
            }
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "download",
            "message": "dfu-util download completed",
            "value": {"exit_code": 0, "stdout": "Download done.", "stderr": ""}
        }));
    });
    let lager = client(&server);
    let out = lager
        .dfu()
        .download(
            b"foobar",
            &lager::DfuOptions {
                vid_pid: Some("0483:df11".into()),
                alt: Some(0),
                dfuse_address: Some("0x08000000:leave".into()),
                reset: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(out.exit_code, Some(0));
    assert_eq!(out.stdout, "Download done.");
    m.assert();
}

#[test]
fn dfu_failure_surfaces_box_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/usb/dfu");
        then.status(502).json_body(json!({
            "success": false, "action": "detach",
            "value": {"exit_code": 74, "stdout": "", "stderr": "..."},
            "error": "dfu-util exited with code 74: No DFU capable USB device available"
        }));
    });
    let lager = client(&server);
    let err = lager.dfu().detach(&Default::default()).unwrap_err();
    match err {
        Error::Box { status, message } => {
            assert_eq!(status, 502);
            assert!(message.contains("No DFU capable USB device"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn dfu_missing_route_is_unsupported() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/usb/dfu");
        then.status(404)
            .header("content-type", "text/html")
            .body("<!doctype html><title>404 Not Found</title>");
    });
    let lager = client(&server);
    let err = lager.dfu().list().unwrap_err();
    assert!(
        matches!(err, Error::UnsupportedByBox { .. }),
        "unexpected error: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Box lock / reservation
// ---------------------------------------------------------------------------

#[test]
fn lock_acquire_matches_cli_payload() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/lock").json_body(json!({
            "user": "ci-bot", "holder_type": "user", "ttl_seconds": null
        }));
        then.status(200).json_body(json!({
            "locked": true, "user": "ci-bot", "holder_type": "user",
            "locked_at": "2026-07-28T12:00:00Z",
            "last_heartbeat": "2026-07-28T12:00:00Z",
            "ttl_seconds": null, "previous_user": null
        }));
    });
    let lager = client(&server);
    let lock = lager.lock("ci-bot").unwrap();
    assert!(lock.locked);
    assert_eq!(lock.user.as_deref(), Some("ci-bot"));
    assert_eq!(lock.ttl_seconds, None);
    assert_eq!(lock.previous_user, None);
    m.assert();
}

#[test]
fn lock_with_ttl_and_heartbeat() {
    let server = MockServer::start();
    let acquire = server.mock(|when, then| {
        when.method(POST).path("/lock").json_body(json!({
            "user": "ci-bot", "holder_type": "ci", "ttl_seconds": 1800
        }));
        then.status(200).json_body(json!({
            "locked": true, "user": "ci-bot", "holder_type": "ci",
            "ttl_seconds": 1800, "previous_user": null
        }));
    });
    let heartbeat = server.mock(|when, then| {
        when.method(POST)
            .path("/lock/heartbeat")
            .json_body(json!({"user": "ci-bot"}));
        then.status(200).json_body(json!({
            "locked": true, "user": "ci-bot", "holder_type": "ci",
            "ttl_seconds": 1800,
            "last_heartbeat": "2026-07-28T12:05:00Z"
        }));
    });
    let lager = client(&server);
    lager.lock_with("ci-bot", "ci", Some(1800)).unwrap();
    let lock = lager.lock_heartbeat("ci-bot").unwrap();
    assert_eq!(lock.last_heartbeat.as_deref(), Some("2026-07-28T12:05:00Z"));
    acquire.assert();
    heartbeat.assert();
}

#[test]
fn lock_contention_is_a_409_box_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/lock");
        then.status(409).json_body(json!({
            "error": "Box is locked by alice",
            "lock": {"locked": true, "user": "alice"}
        }));
    });
    let lager = client(&server);
    let err = lager.lock("bob").unwrap_err();
    match err {
        Error::Box { status, message } => {
            assert_eq!(status, 409);
            assert!(message.contains("alice"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn lock_status_and_unlock() {
    let server = MockServer::start();
    let status = server.mock(|when, then| {
        when.method(GET).path("/lock");
        then.status(200).json_body(json!({"locked": false}));
    });
    let unlock = server.mock(|when, then| {
        when.method(POST).path("/unlock").json_body(json!({
            "user": "ci-bot", "force": false
        }));
        then.status(200)
            .json_body(json!({"locked": false, "message": "Box unlocked"}));
    });
    let lager = client(&server);
    assert!(!lager.lock_status().unwrap().locked);
    lager.unlock("ci-bot").unwrap();
    status.assert();
    unlock.assert();
}

#[test]
fn lock_guard_releases_on_drop() {
    let server = MockServer::start();
    let acquire = server.mock(|when, then| {
        when.method(POST).path("/lock");
        then.status(200)
            .json_body(json!({"locked": true, "user": "ci-bot"}));
    });
    let unlock = server.mock(|when, then| {
        when.method(POST).path("/unlock").json_body(json!({
            "user": "ci-bot", "force": false
        }));
        then.status(200)
            .json_body(json!({"locked": false, "message": "Box unlocked"}));
    });
    let lager = client(&server);
    {
        let _guard = lager.lock_guard("ci-bot").unwrap();
    }
    acquire.assert();
    unlock.assert();
}

// ---------------------------------------------------------------------------
// Discovery / health / status
// ---------------------------------------------------------------------------

#[test]
fn nets_list_bare_array() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET).path("/nets/list");
        then.status(200).json_body(json!([
            {"name": "supply1", "role": "power-supply", "instrument": "Rigol DP832",
             "address": "USB0::0x1AB1::0x0E11::DP8XXXXX::INSTR", "pin": 1},
            {"name": "adc1", "role": "adc", "instrument": "LabJack T7", "pin": 0}
        ]));
    });
    let lager = client(&server);
    let nets = lager.nets().unwrap();
    assert_eq!(nets.len(), 2);
    assert_eq!(nets[0].name, "supply1");
    assert_eq!(nets[0].role, "power-supply");
    assert_eq!(nets[1].instrument.as_deref(), Some("LabJack T7"));
    m.assert();
}

#[test]
fn nets_list_falls_back_to_uart_shape() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/nets/list");
        then.status(404).json_body(json!({"error": "not found"}));
    });
    let fallback = server.mock(|when, then| {
        when.method(GET).path("/uart/nets/list");
        then.status(200)
            .json_body(json!({"nets": [{"name": "uart1", "role": "uart"}]}));
    });
    let lager = client(&server);
    let nets = lager.nets().unwrap();
    assert_eq!(nets.len(), 1);
    assert_eq!(nets[0].name, "uart1");
    fallback.assert();
}

#[test]
fn health_and_status() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/health");
        then.status(200).json_body(json!({
            "status": "healthy", "service": "lager-python-box",
            "version": "1.0.0", "websocket": "enabled"
        }));
    });
    server.mock(|when, then| {
        when.method(GET).path("/status");
        then.status(200).json_body(json!({
            "healthy": true, "version": "0.17.2",
            "nets": [{"name": "supply1", "type": "PowerSupply"}],
            "capabilities": {
                "netCommand": true,
                "netCommandRoles": ["gpio", "adc", "arm", "webcam", "router"],
                "bleCommand": true, "wifiCommand": true, "blufiCommand": false,
                "customDevices": true, "binaries": true, "safetyLimits": true
            }
        }));
    });
    let lager = client(&server);
    assert_eq!(lager.health().unwrap().status, "healthy");
    let status = lager.status().unwrap();
    assert!(status.healthy);
    assert!(status.capabilities.net_command);
    assert!(status.capabilities.ble_command);
    assert!(status.capabilities.wifi_command);
    assert!(!status.capabilities.blufi_command);
    assert!(status.capabilities.custom_devices);
    assert!(status.capabilities.binaries);
    assert!(status.capabilities.safety_limits);
    assert!(status
        .capabilities
        .net_command_roles
        .iter()
        .any(|r| r == "arm"));
    assert_eq!(status.nets[0].net_type, "PowerSupply");
}

// ---------------------------------------------------------------------------
// Arm
// ---------------------------------------------------------------------------

#[test]
fn arm_position_and_moves() {
    let server = MockServer::start();
    let pos = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "arm1", "role": "arm", "action": "position", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "position",
            "message": "X: 0.0 Y: 300.0 Z: 0.0", "value": [0.0, 300.0, 0.0]
        }));
    });
    let mv = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "arm1", "role": "arm", "action": "move",
            "params": {"x": 50.0, "y": 250.0, "z": -20.0, "timeout": 15.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "move",
            "message": "X: 50.0 Y: 250.0 Z: -20.0", "value": [50.0, 250.0, -20.0]
        }));
    });
    let mv_by = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "arm1", "role": "arm", "action": "move_by",
            "params": {"dx": 0.0, "dy": -10.0, "dz": 5.0, "timeout": 15.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "move_by",
            "message": "X: 50.0 Y: 240.0 Z: -15.0", "value": [50.0, 240.0, -15.0]
        }));
    });
    let slow_mv = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "arm1", "role": "arm", "action": "move",
            "params": {"x": 0.0, "y": 300.0, "z": 0.0, "timeout": 60.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "move",
            "message": "X: 0.0 Y: 300.0 Z: 0.0", "value": [0.0, 300.0, 0.0]
        }));
    });
    let slow_mv_by = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "arm1", "role": "arm", "action": "move_by",
            "params": {"dx": 1.0, "dy": 0.0, "dz": 0.0, "timeout": 45.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "move_by",
            "message": "X: 1.0 Y: 300.0 Z: 0.0", "value": [1.0, 300.0, 0.0]
        }));
    });
    let lager = client(&server);
    let arm = lager.arm("arm1");
    let p = arm.position().unwrap();
    assert_eq!((p.x, p.y, p.z), (0.0, 300.0, 0.0));
    let p = arm.move_to(50.0, 250.0, -20.0).unwrap();
    assert_eq!((p.x, p.y, p.z), (50.0, 250.0, -20.0));
    let p = arm.move_by(0.0, -10.0, 5.0).unwrap();
    assert_eq!(p.y, 240.0);
    let p = arm.move_to_with_timeout(0.0, 300.0, 0.0, 60.0).unwrap();
    assert_eq!(p.y, 300.0);
    let p = arm.move_by_with_timeout(1.0, 0.0, 0.0, 45.0).unwrap();
    assert_eq!(p.x, 1.0);
    pos.assert();
    mv.assert();
    mv_by.assert();
    slow_mv.assert();
    slow_mv_by.assert();
}

#[test]
fn arm_motor_home_save_and_acceleration() {
    let server = MockServer::start();
    let home = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "arm1", "role": "arm", "action": "go_home", "params": {}
        }));
        then.status(200).json_body(
            json!({"success": true, "action": "go_home", "message": "Arm moving to home position (X0 Y300 Z0)"}),
        );
    });
    let enable = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "arm1", "role": "arm", "action": "enable_motor", "params": {}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "enable_motor", "message": "Arm motors enabled"}));
    });
    let disable = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "arm1", "role": "arm", "action": "disable_motor", "params": {}
        }));
        then.status(200)
            .json_body(json!({"success": true, "action": "disable_motor", "message": "Arm motors disabled"}));
    });
    let save = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "arm1", "role": "arm", "action": "read_and_save_position", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "read_and_save_position",
            "message": "Saved position X: 1.0 Y: 2.0 Z: 3.0", "value": [1.0, 2.0, 3.0]
        }));
    });
    let accel = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "arm1", "role": "arm", "action": "set_acceleration",
            "params": {"acceleration": 30, "travel_acceleration": 30, "retract_acceleration": 60}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "set_acceleration",
            "message": "Acceleration set (M204): print=30 travel=30 retract=60"
        }));
    });
    let lager = client(&server);
    let arm = lager.arm("arm1");
    arm.go_home().unwrap();
    arm.enable_motor().unwrap();
    arm.disable_motor().unwrap();
    let p = arm.read_and_save_position().unwrap();
    assert_eq!((p.x, p.y, p.z), (1.0, 2.0, 3.0));
    arm.set_acceleration(30, 30, 60).unwrap();
    home.assert();
    enable.assert();
    disable.assert();
    save.assert();
    accel.assert();
}

// ---------------------------------------------------------------------------
// Webcam
// ---------------------------------------------------------------------------

#[test]
fn webcam_start_stop_status_url() {
    let server = MockServer::start();
    let start = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "cam1", "role": "webcam", "action": "start", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "start",
            "message": "Stream started at http://192.168.1.42:8090/stream.mjpg",
            "value": {"url": "http://192.168.1.42:8090/stream.mjpg", "port": 8090,
                      "already_running": false}
        }));
    });
    let status = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "cam1", "role": "webcam", "action": "status", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "status",
            "message": "Streaming at http://192.168.1.42:8090/stream.mjpg",
            "value": {"running": true, "url": "http://192.168.1.42:8090/stream.mjpg",
                      "port": 8090, "video_device": "/dev/video0"}
        }));
    });
    let url = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "cam1", "role": "webcam", "action": "url", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "url",
            "message": "Streaming at http://192.168.1.42:8090/stream.mjpg",
            "value": {"running": true, "url": "http://192.168.1.42:8090/stream.mjpg",
                      "port": 8090, "video_device": "/dev/video0"}
        }));
    });
    let stop = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "cam1", "role": "webcam", "action": "stop", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "stop", "message": "Stream stopped",
            "value": {"stopped": true}
        }));
    });
    let lager = client(&server);
    let cam = lager.webcam("cam1");
    let stream = cam.start().unwrap();
    assert_eq!(stream.url, "http://192.168.1.42:8090/stream.mjpg");
    assert_eq!(stream.port, Some(8090));
    assert!(!stream.already_running);
    let s = cam.status().unwrap();
    assert!(s.running);
    assert_eq!(s.video_device.as_deref(), Some("/dev/video0"));
    assert_eq!(
        cam.url().unwrap().as_deref(),
        Some("http://192.168.1.42:8090/stream.mjpg")
    );
    assert!(cam.stop().unwrap());
    start.assert();
    status.assert();
    url.assert();
    stop.assert();
}

#[test]
fn webcam_url_none_when_not_running() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "cam1", "role": "webcam", "action": "url", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "url",
            "message": "No active stream for net 'cam1'", "value": {"running": false}
        }));
    });
    let lager = client(&server);
    assert_eq!(lager.webcam("cam1").url().unwrap(), None);
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

#[test]
fn router_reads() {
    let server = MockServer::start();
    let connect = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "router1", "action": "connect", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "connect", "message": "router connect ok",
            "value": {"connected": true, "identity": "MikroTik", "version": "7.14",
                      "board": "hAP ac^2", "uptime": "1w2d"}
        }));
    });
    let sysinfo = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "router1", "action": "system_info", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "system_info", "message": "router system_info ok",
            "value": {"name": "MikroTik", "version": "7.14", "board": "hAP ac^2",
                      "architecture": "arm", "uptime": "1w2d", "cpu_load": "3",
                      "free_memory": 191840256, "total_memory": 268435456,
                      "free_hdd_space": 2542592}
        }));
    });
    let interfaces = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "router1", "action": "interfaces", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "interfaces", "message": "router interfaces ok",
            "value": [{"name": "ether1", "type": "ether", "disabled": "false"}]
        }));
    });
    let wireless = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "router1", "action": "wireless_interfaces", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "wireless_interfaces",
            "message": "router wireless_interfaces ok",
            "value": [{"name": "wlan1", "ssid": "testnet"}]
        }));
    });
    let clients = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "router1", "action": "wireless_clients", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "wireless_clients",
            "message": "router wireless_clients ok",
            "value": [{"mac-address": "AA:BB:CC:DD:EE:FF", "interface": "wlan1"}]
        }));
    });
    let leases = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "router1", "action": "dhcp_leases", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "dhcp_leases", "message": "router dhcp_leases ok",
            "value": [{"address": "192.168.88.10", "mac-address": "AA:BB:CC:DD:EE:FF"}]
        }));
    });
    let lager = client(&server);
    let router = lager.router("router1");
    assert_eq!(router.connect().unwrap()["identity"], "MikroTik");
    let info = router.system_info().unwrap();
    assert_eq!(info.board.as_deref(), Some("hAP ac^2"));
    assert_eq!(info.cpu_load, Some(3)); // numeric string tolerated
    assert_eq!(router.interfaces().unwrap()[0]["name"], "ether1");
    assert_eq!(router.wireless_interfaces().unwrap()[0]["ssid"], "testnet");
    assert_eq!(
        router.wireless_clients().unwrap()[0]["mac-address"],
        "AA:BB:CC:DD:EE:FF"
    );
    assert_eq!(router.dhcp_leases().unwrap()[0]["address"], "192.168.88.10");
    connect.assert();
    sysinfo.assert();
    interfaces.assert();
    wireless.assert();
    clients.assert();
    leases.assert();
}

#[test]
fn router_control_actions() {
    let server = MockServer::start();
    let reboot = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "router1", "action": "reboot", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "reboot", "message": "router reboot ok",
            "value": {"rebooting": true}
        }));
    });
    let disable = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "router1", "action": "disable_interface",
            "params": {"interface": "wlan1"}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "disable_interface",
            "message": "router disable_interface ok",
            "value": {"interface": "wlan1", "disabled": true}
        }));
    });
    let enable = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "router1", "action": "enable_interface",
            "params": {"interface": "wlan1"}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "enable_interface",
            "message": "router enable_interface ok",
            "value": {"interface": "wlan1", "disabled": false}
        }));
    });
    let ssid = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "router1", "action": "set_wireless_ssid",
            "params": {"interface": "wlan1", "ssid": "newnet"}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "set_wireless_ssid",
            "message": "router set_wireless_ssid ok", "value": {"ssid": "newnet"}
        }));
    });
    let block = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "router1", "action": "block_internet", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "block_internet",
            "message": "router block_internet ok", "value": {"blocked": "internet"}
        }));
    });
    let unblock = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "router1", "action": "remove_firewall_rules",
            "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "remove_firewall_rules",
            "message": "router remove_firewall_rules ok", "value": {"removed": true}
        }));
    });
    let lager = client(&server);
    let router = lager.router("router1");
    router.reboot().unwrap();
    router.disable_interface("wlan1").unwrap();
    router.enable_interface("wlan1").unwrap();
    router.set_wireless_ssid("wlan1", "newnet").unwrap();
    router.block_internet().unwrap();
    router.remove_firewall_rules().unwrap();
    reboot.assert();
    disable.assert();
    enable.assert();
    ssid.assert();
    block.assert();
    unblock.assert();
}

#[test]
fn router_generic_command_escape_hatch() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/net/command").json_body(json!({
            "netname": "router1", "action": "add_bandwidth_limit",
            "params": {"target": "192.168.88.10", "max_limit": "1M/1M"}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "add_bandwidth_limit",
            "message": "router add_bandwidth_limit ok",
            "value": {"name": "limit-192.168.88.10", "max-limit": "1M/1M"}
        }));
    });
    let lager = client(&server);
    let result = lager
        .router("router1")
        .command(
            "add_bandwidth_limit",
            json!({"target": "192.168.88.10", "max_limit": "1M/1M"}),
        )
        .unwrap();
    assert_eq!(result["max-limit"], "1M/1M");
    m.assert();
}

// ---------------------------------------------------------------------------
// BLE (box-level)
// ---------------------------------------------------------------------------

#[test]
fn ble_scan() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/ble/command").json_body(json!({
            "action": "scan", "params": {"timeout": 5.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "scan", "message": "Found 2 device(s)",
            "value": {"devices": [
                {"name": "MyDevice", "address": "AA:BB:CC:DD:EE:FF", "rssi": -55,
                 "uuids": ["0000180f-0000-1000-8000-00805f9b34fb"]},
                {"name": "11:22:33:44:55:66", "address": "11:22:33:44:55:66",
                 "rssi": -80, "uuids": []}
            ]}
        }));
    });
    let lager = client(&server);
    let devices = lager.ble().scan(5.0).unwrap();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].name, "MyDevice");
    assert_eq!(devices[0].rssi, Some(-55));
    assert_eq!(devices[0].uuids.len(), 1);
    m.assert();
}

#[test]
fn ble_scan_named_sends_filter() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST).path("/ble/command").json_body(json!({
            "action": "scan", "params": {"timeout": 5.0, "name_contains": "My"}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "scan", "message": "Found 1 device(s)",
            "value": {"devices": [
                {"name": "MyDevice", "address": "AA:BB:CC:DD:EE:FF", "rssi": -55, "uuids": []}
            ]}
        }));
    });
    let lager = client(&server);
    let devices = lager.ble().scan_named(5.0, "My").unwrap();
    assert_eq!(devices.len(), 1);
    m.assert();
}

#[test]
fn ble_info_connect_disconnect() {
    let server = MockServer::start();
    let info_body = json!({
        "success": true, "action": "info",
        "message": "Connected to AA:BB:CC:DD:EE:FF: 1 service(s)",
        "value": {"address": "AA:BB:CC:DD:EE:FF", "connected": true, "services": [
            {"uuid": "0000180f-0000-1000-8000-00805f9b34fb",
             "description": "Battery Service",
             "characteristics": [
                {"uuid": "00002a19-0000-1000-8000-00805f9b34fb",
                 "description": "Battery Level", "properties": ["read", "notify"]}
             ]}
        ]}
    });
    let info = server.mock(|when, then| {
        when.method(POST).path("/ble/command").json_body(json!({
            "action": "info", "params": {"address": "AA:BB:CC:DD:EE:FF", "timeout": 10.0}
        }));
        then.status(200).json_body(info_body.clone());
    });
    let connect = server.mock(|when, then| {
        when.method(POST).path("/ble/command").json_body(json!({
            "action": "connect", "params": {"address": "AA:BB:CC:DD:EE:FF", "timeout": 10.0}
        }));
        then.status(200).json_body(info_body.clone());
    });
    let disconnect = server.mock(|when, then| {
        when.method(POST).path("/ble/command").json_body(json!({
            "action": "disconnect", "params": {"address": "AA:BB:CC:DD:EE:FF"}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "disconnect",
            "message": "Disconnected from AA:BB:CC:DD:EE:FF",
            "value": {"address": "AA:BB:CC:DD:EE:FF", "disconnected": true}
        }));
    });
    let lager = client(&server);
    let ble = lager.ble();
    let device = ble.info("AA:BB:CC:DD:EE:FF").unwrap();
    assert!(device.connected);
    assert_eq!(device.services[0].description.as_deref(), Some("Battery Service"));
    assert_eq!(device.services[0].characteristics[0].properties, ["read", "notify"]);
    ble.connect("AA:BB:CC:DD:EE:FF").unwrap();
    ble.disconnect("AA:BB:CC:DD:EE:FF").unwrap();
    info.assert();
    connect.assert();
    disconnect.assert();
}

// ---------------------------------------------------------------------------
// WiFi (box-level)
// ---------------------------------------------------------------------------

#[test]
fn wifi_status_scan_connect_delete() {
    let server = MockServer::start();
    let status = server.mock(|when, then| {
        when.method(POST).path("/wifi/command").json_body(json!({
            "action": "status", "params": {}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "status",
            "message": "Connected to labnet on wlan0",
            "value": {"interfaces": [
                {"interface": "wlan0", "ssid": "labnet", "state": "Connected"}
            ]}
        }));
    });
    let scan = server.mock(|when, then| {
        when.method(POST).path("/wifi/command").json_body(json!({
            "action": "scan", "params": {"interface": "wlan0"}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "scan", "message": "Found 2 network(s)",
            "value": {"access_points": [
                {"ssid": "labnet", "address": "AA:BB:CC:DD:EE:FF", "strength": 84,
                 "security": "Secured"},
                {"ssid": "guest", "address": "11:22:33:44:55:66", "strength": 40,
                 "security": "Open"}
            ]}
        }));
    });
    let connect = server.mock(|when, then| {
        when.method(POST).path("/wifi/command").json_body(json!({
            "action": "connect", "params": {"ssid": "labnet", "password": "hunter22"}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "connect",
            "message": "Successfully connected to labnet",
            "value": {"ssid": "labnet", "connected": true, "interface": "wlan0",
                      "method": "nmcli"}
        }));
    });
    let delete = server.mock(|when, then| {
        when.method(POST).path("/wifi/command").json_body(json!({
            "action": "delete", "params": {"ssid": "labnet"}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "delete",
            "message": "Deleted connection 'labnet'",
            "value": {"deleted": true, "connection": "labnet"}
        }));
    });
    let lager = client(&server);
    let wifi = lager.wifi();
    let interfaces = wifi.status().unwrap();
    assert_eq!(interfaces[0].state, "Connected");
    let aps = wifi.scan("wlan0").unwrap();
    assert_eq!(aps[0].strength, Some(84));
    assert_eq!(aps[1].security.as_deref(), Some("Open"));
    let conn = wifi.connect("labnet", "hunter22").unwrap();
    assert!(conn.connected);
    assert_eq!(conn.method.as_deref(), Some("nmcli"));
    wifi.delete("labnet").unwrap();
    status.assert();
    scan.assert();
    connect.assert();
    delete.assert();
}

#[test]
fn wifi_error_maps_to_box_error() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/wifi/command");
        then.status(502).json_body(json!({
            "success": false, "error": "WiFi error: Failed to connect: no such SSID"
        }));
    });
    let lager = client(&server);
    match lager.wifi().connect("nope", "pw").unwrap_err() {
        Error::Box { status, message } => {
            assert_eq!(status, 502);
            assert!(message.contains("no such SSID"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// BluFi (box-level)
// ---------------------------------------------------------------------------

#[test]
fn blufi_scan_and_connect() {
    let server = MockServer::start();
    let scan = server.mock(|when, then| {
        when.method(POST).path("/blufi/command").json_body(json!({
            "action": "scan", "params": {"timeout": 10.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "scan", "message": "Found 1 BluFi device(s)",
            "value": {"devices": [
                {"name": "BLUFI_DEVICE", "address": "AA:BB:CC:DD:EE:FF", "rssi": -60,
                 "uuids": ["0000ffff-0000-1000-8000-00805f9b34fb"]}
            ]}
        }));
    });
    let connect = server.mock(|when, then| {
        when.method(POST).path("/blufi/command").json_body(json!({
            "action": "connect", "params": {"device_name": "BLUFI_DEVICE", "timeout": 20.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "connect",
            "message": "Connected to BLUFI_DEVICE (version 1.0, STA: Connected)",
            "value": {"device_name": "BLUFI_DEVICE", "version": "1.0",
                      "opMode": 1, "opModeName": "STA",
                      "staConn": 0, "staConnName": "Connected", "softAPConn": 0}
        }));
    });
    let lager = client(&server);
    let blufi = lager.blufi();
    let devices = blufi.scan(10.0).unwrap();
    assert_eq!(devices[0].name, "BLUFI_DEVICE");
    let info = blufi.connect("BLUFI_DEVICE").unwrap();
    assert_eq!(info.version.as_deref(), Some("1.0"));
    assert_eq!(info.status.sta_conn_name.as_deref(), Some("Connected"));
    scan.assert();
    connect.assert();
}

#[test]
fn blufi_provision_wifi_scan_status_version() {
    let server = MockServer::start();
    let provision = server.mock(|when, then| {
        when.method(POST).path("/blufi/command").json_body(json!({
            "action": "provision",
            "params": {"device_name": "BLUFI_DEVICE", "ssid": "labnet",
                       "password": "hunter22", "timeout": 20.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "provision",
            "message": "Device connected to 'labnet' successfully",
            "value": {"device_name": "BLUFI_DEVICE", "ssid": "labnet",
                      "staConn": 0, "staConnName": "Connected", "success": true}
        }));
    });
    let wifi_scan = server.mock(|when, then| {
        when.method(POST).path("/blufi/command").json_body(json!({
            "action": "wifi_scan", "params": {"device_name": "BLUFI_DEVICE", "timeout": 20.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "wifi_scan", "message": "Found 2 network(s)",
            "value": {"device_name": "BLUFI_DEVICE", "networks": [
                {"ssid": "labnet", "rssi": -45}, {"ssid": "guest", "rssi": -70}
            ]}
        }));
    });
    let status = server.mock(|when, then| {
        when.method(POST).path("/blufi/command").json_body(json!({
            "action": "status", "params": {"device_name": "BLUFI_DEVICE", "timeout": 20.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "status",
            "message": "Op Mode: STA, STA: Connected, SoftAP: 0",
            "value": {"device_name": "BLUFI_DEVICE", "opMode": 1, "opModeName": "STA",
                      "staConn": 0, "staConnName": "Connected", "softAPConn": 0}
        }));
    });
    let version = server.mock(|when, then| {
        when.method(POST).path("/blufi/command").json_body(json!({
            "action": "version", "params": {"device_name": "BLUFI_DEVICE", "timeout": 20.0}
        }));
        then.status(200).json_body(json!({
            "success": true, "action": "version", "message": "Firmware version: 1.0",
            "value": {"device_name": "BLUFI_DEVICE", "version": "1.0"}
        }));
    });
    let lager = client(&server);
    let blufi = lager.blufi();
    let result = blufi.provision("BLUFI_DEVICE", "labnet", "hunter22").unwrap();
    assert_eq!(result.sta_conn, Some(0));
    let networks = blufi.wifi_scan("BLUFI_DEVICE").unwrap();
    assert_eq!(networks[0].ssid, "labnet");
    assert_eq!(networks[0].rssi, Some(-45));
    let state = blufi.status("BLUFI_DEVICE").unwrap();
    assert_eq!(state.op_mode_name.as_deref(), Some("STA"));
    assert_eq!(blufi.version("BLUFI_DEVICE").unwrap().as_deref(), Some("1.0"));
    provision.assert();
    wifi_scan.assert();
    status.assert();
    version.assert();
}

#[test]
fn blufi_provision_failure_is_box_error() {
    // The box reports a target that never joined WiFi as success=false/502,
    // so callers see it as an Err without inspecting `value`.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/blufi/command");
        then.status(502).json_body(json!({
            "success": false, "error": "BluFi error: Device connection status: Failed"
        }));
    });
    let lager = client(&server);
    match lager
        .blufi()
        .provision("BLUFI_DEVICE", "labnet", "wrongpw")
        .unwrap_err()
    {
        Error::Box { status, message } => {
            assert_eq!(status, 502);
            assert!(message.contains("Failed"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Error conventions
// ---------------------------------------------------------------------------

#[test]
fn unknown_net_is_box_error_404() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/net/command");
        then.status(404)
            .json_body(json!({"success": false, "error": "Net 'nope' not found"}));
    });
    let lager = client(&server);
    match lager.adc("nope").read().unwrap_err() {
        Error::Box { status, message } => {
            assert_eq!(status, 404);
            assert_eq!(message, "Net 'nope' not found");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn old_box_501_is_unsupported() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/net/command");
        then.status(501).json_body(json!({
            "success": false,
            "error": "Role 'gpio' is not supported by /net/command"
        }));
    });
    let lager = client(&server);
    assert!(matches!(
        lager.gpio("gpi1").input().unwrap_err(),
        Error::UnsupportedByBox { .. }
    ));
}

#[test]
fn cross_role_conflict_success_false_http_200() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/supply/command");
        then.status(200).json_body(json!({
            "success": false,
            "error": "battery TUI is monitoring the same instrument"
        }));
    });
    let lager = client(&server);
    match lager.supply("supply1").enable().unwrap_err() {
        Error::Box { status, message } => {
            assert_eq!(status, 200);
            assert!(message.contains("battery TUI"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn hardware_error_502() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/net/command");
        then.status(502).json_body(json!({
            "success": false,
            "error": "Hardware error: Function call failed: [Errno 16] Resource busy"
        }));
    });
    let lager = client(&server);
    match lager.adc("adc1").read().unwrap_err() {
        Error::Box { status, .. } => assert_eq!(status, 502),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn unreachable_box_is_connection_error() {
    // Port 9 (discard) on localhost is almost certainly closed.
    let lager = LagerBox::connect("127.0.0.1:9").unwrap();
    assert!(matches!(
        lager.adc("adc1").read().unwrap_err(),
        Error::Connection(_)
    ));
}

#[test]
fn scope_is_a_stub() {
    let server = MockServer::start();
    let lager = client(&server);
    assert!(matches!(
        lager.scope("scope1").capture().unwrap_err(),
        Error::NotSupportedByBox { feature: "scope", .. }
    ));
}

// ---------------------------------------------------------------------------
// Per-net safety limits (PUT /nets/<name>/safety-limits, box >= 0.35.0)
// ---------------------------------------------------------------------------

#[test]
fn set_safety_limits_sends_only_set_fields() {
    // The box's key set is closed and an explicit null means "do not store
    // this key", so unset fields must be absent from the body, not null.
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(PUT)
            .path("/nets/supply1/safety-limits")
            .json_body(json!({"max_voltage": 5.0, "allow_destructive": false}));
        then.status(200).json_body(json!({
            "ok": true, "name": "supply1",
            "safety_limits": {"max_voltage": 5.0, "allow_destructive": false}
        }));
    });
    let lager = client(&server);
    let applied = lager
        .set_safety_limits(
            "supply1",
            &SafetyLimits {
                max_voltage: Some(5.0),
                allow_destructive: Some(false),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(applied.max_voltage, Some(5.0));
    assert_eq!(applied.max_current, None);
    assert_eq!(applied.allow_destructive, Some(false));
    m.assert();
}

#[test]
fn clear_safety_limits_sends_empty_object() {
    // `{}` is the box's documented "back to unrestricted" body; it echoes
    // `safety_limits: null` back.
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(PUT)
            .path("/nets/supply1/safety-limits")
            .json_body(json!({}));
        then.status(200)
            .json_body(json!({"ok": true, "name": "supply1", "safety_limits": null}));
    });
    let lager = client(&server);
    lager.clear_safety_limits("supply1").unwrap();
    m.assert();
}

#[test]
fn safety_limits_validation_refusal_stays_a_box_error() {
    // The box refuses max_power with its reason rather than storing a limit
    // nothing enforces. That 400 is a genuine refusal, not a missing route,
    // and must surface verbatim.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(PUT).path("/nets/supply1/safety-limits");
        then.status(400).json_body(json!({
            "error": "max_power is not supported: one setter call establishes \
                      either voltage or current, never both, so a power ceiling \
                      could not be evaluated honestly. Use max_voltage and max_current."
        }));
    });
    let lager = client(&server);
    let err = lager
        .set_safety_limits("supply1", &SafetyLimits::default())
        .unwrap_err();
    match err {
        Error::Box { status, message } => {
            assert_eq!(status, 400);
            assert!(message.contains("max_power"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn safety_limits_unknown_net_stays_a_box_404() {
    // The route exists but the net does not: a JSON 404 from the handler,
    // which must NOT be reclassified as an unsupported box.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(PUT).path("/nets/ghost/safety-limits");
        then.status(404)
            .json_body(json!({"error": "no saved net named 'ghost'"}));
    });
    let lager = client(&server);
    let err = lager
        .set_safety_limits("ghost", &SafetyLimits::default())
        .unwrap_err();
    match err {
        Error::Box { status, message } => {
            assert_eq!(status, 404);
            assert!(message.contains("ghost"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn safety_limits_missing_route_maps_to_unsupported() {
    // A pre-0.35.0 box has no such rule, so Flask answers with its HTML 404
    // page. That is a box-too-old condition, not a request problem.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(PUT).path("/nets/supply1/safety-limits");
        then.status(404)
            .header("content-type", "text/html")
            .body("<!doctype html><title>404 Not Found</title>");
    });
    let lager = client(&server);
    let err = lager
        .set_safety_limits("supply1", &SafetyLimits::default())
        .unwrap_err();
    assert!(
        matches!(err, Error::UnsupportedByBox { .. }),
        "unexpected error: {err:?}"
    );
    assert!(err.to_string().contains("0.35.0"));
}

#[test]
fn safety_limits_read_back_from_nets_list() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/nets/list");
        then.status(200).json_body(json!([
            {"name": "supply1", "role": "power-supply",
             "safety_limits": {"max_voltage": 12.0, "max_current": 2.5}},
            {"name": "gpio1", "role": "gpio"},
        ]));
    });
    let lager = client(&server);

    let limits = lager.safety_limits("supply1").unwrap().unwrap();
    assert_eq!(limits.max_voltage, Some(12.0));
    assert_eq!(limits.max_current, Some(2.5));
    assert_eq!(limits.allow_destructive, None);

    // A net with no limits key is unrestricted, not an error...
    assert!(lager.safety_limits("gpio1").unwrap().is_none());

    // ...while a net that does not exist is a 404.
    match lager.safety_limits("ghost").unwrap_err() {
        Error::Box { status, .. } => assert_eq!(status, 404),
        other => panic!("unexpected error: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Live net state (GET /nets/state, box >= 0.34.0)
// ---------------------------------------------------------------------------

#[test]
fn nets_state_parses_states_and_null_reasons() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET).path("/nets/state");
        then.status(200).json_body(json!([
            {"name": "usb1", "role": "usb", "state": "enabled"},
            {"name": "supply1", "role": "power-supply", "state": "3.300V, ON"},
            {"name": "uart1", "role": "uart", "state": null,
             "reason": "no probe for role"},
            {"name": "usb2", "role": "usb", "state": null,
             "reason": "not probed: slower instruments consumed the state budget",
             "reason_code": "hub-skipped"},
        ]));
    });
    let lager = client(&server);
    let states = lager.nets_state().unwrap();
    assert_eq!(states.len(), 4);
    assert_eq!(states[0].state.as_deref(), Some("enabled"));
    assert!(states[0].reason.is_none());
    assert_eq!(states[2].state, None);
    assert_eq!(states[2].reason.as_deref(), Some("no probe for role"));
    assert_eq!(states[3].reason_code.as_deref(), Some("hub-skipped"));
    m.assert();
}

#[test]
fn nets_state_missing_route_maps_to_unsupported() {
    // A pre-0.34.0 box has no /nets/state rule; Flask answers with its HTML
    // 404 page.
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/nets/state");
        then.status(404)
            .header("content-type", "text/html")
            .body("<!doctype html><title>404 Not Found</title>");
    });
    let lager = client(&server);
    let err = lager.nets_state().unwrap_err();
    assert!(
        matches!(err, Error::UnsupportedByBox { .. }),
        "unexpected error: {err:?}"
    );
    assert!(err.to_string().contains("0.34.0"));
}

// ---------------------------------------------------------------------------
// Async client parity (same wire layer, so a spot check suffices)
// ---------------------------------------------------------------------------

#[cfg(feature = "async")]
mod async_parity {
    use super::*;
    use lager::AsyncLagerBox;

    #[tokio::test]
    async fn async_adc_read_sends_identical_request() {
        let server = MockServer::start_async().await;
        let m = server
            .mock_async(|when, then| {
                when.method(POST).path("/net/command").json_body(json!({
                    "netname": "adc1", "role": "adc", "action": "read", "params": {}
                }));
                then.status(200)
                    .json_body(json!({"success": true, "action": "read", "message": "1.5 V", "value": 1.5}));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        assert_eq!(lager.adc("adc1").read().await.unwrap(), 1.5);
        m.assert_async().await;
    }

    #[tokio::test]
    async fn async_supply_state() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST).path("/supply/command").json_body(json!({
                    "netname": "supply1", "action": "state", "params": {}
                }));
                then.status(200).json_body(json!({
                    "success": true, "action": "state", "message": "...",
                    "state": {"netname": "supply1", "channel": 1, "voltage": 3.3, "enabled": true}
                }));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        let state = lager.supply("supply1").state().await.unwrap();
        assert_eq!(state.voltage, Some(3.3));
        assert_eq!(state.enabled, Some(true));
    }

    #[tokio::test]
    async fn async_error_mapping() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(POST).path("/net/command");
                then.status(404)
                    .json_body(json!({"success": false, "error": "Net 'nope' not found"}));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        assert!(matches!(
            lager.adc("nope").read().await.unwrap_err(),
            Error::Box { status: 404, .. }
        ));
    }

    #[tokio::test]
    async fn async_set_safety_limits_sends_identical_request() {
        let server = MockServer::start_async().await;
        let m = server
            .mock_async(|when, then| {
                when.method(PUT)
                    .path("/nets/supply1/safety-limits")
                    .json_body(json!({"max_current": 1.5}));
                then.status(200).json_body(json!({
                    "ok": true, "name": "supply1",
                    "safety_limits": {"max_current": 1.5}
                }));
            })
            .await;
        let lager = AsyncLagerBox::connect(server.address().to_string()).unwrap();
        let applied = lager
            .set_safety_limits(
                "supply1",
                &SafetyLimits {
                    max_current: Some(1.5),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(applied.max_current, Some(1.5));
        m.assert_async().await;
    }
}
