//! Power-cycle a DUT and verify its rail with an ADC net.
//!
//! ```sh
//! LAGER_BOX_HOST=192.168.1.42 cargo run --example power_cycle -- supply1 vbat_sense
//! ```

use std::time::Duration;

use lager::LagerBox;

fn main() -> lager::Result<()> {
    let mut args = std::env::args().skip(1);
    let supply_net = args.next().unwrap_or_else(|| "supply1".to_string());
    let adc_net = args.next().unwrap_or_else(|| "adc1".to_string());

    let lager = LagerBox::from_env()?;
    println!("box: {}", lager.base_url());

    let status = lager.status()?;
    println!("box version {} with {} nets", status.version, status.nets.len());

    let supply = lager.supply(&supply_net);
    let adc = lager.adc(&adc_net);

    supply.set_voltage(3.3)?;
    supply.enable()?;
    std::thread::sleep(Duration::from_millis(200));

    let v = adc.read()?;
    println!("{adc_net} reads {v:.3} V with {supply_net} at 3.3 V");

    supply.disable()?;
    std::thread::sleep(Duration::from_millis(200));

    let v = adc.read()?;
    println!("{adc_net} reads {v:.3} V with {supply_net} off");

    Ok(())
}
