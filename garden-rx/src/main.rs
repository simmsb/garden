use std::collections::HashMap;

use color_eyre::Result;
use garden_shared::{Message, Transmission};
use linux_embedded_hal as hal;

use hal::spidev::{self, SpidevOptions};
use hal::sysfs_gpio::Direction;
use hal::Delay;
use hal::{Pin, Spidev};
use once_cell::sync::Lazy;
use prometheus::{register_gauge_with_registry, Gauge, Registry};
use uom::si::electrical_resistance::ohm;
use uom::si::pressure::pascal;
use uom::si::ratio::percent;
use uom::si::thermodynamic_temperature::degree_celsius;

const LORA_CS_PIN: u64 = 26;
const LORA_RESET_PIN: u64 = 22;
const FREQUENCY: i64 = 868;

static MOISTURE_REGISTRY: Lazy<Registry> = Lazy::new(|| Registry::new());

static WEATHER_REGISTRY: Lazy<Registry> = Lazy::new(|| Registry::new());

static MOISTURE_LEVEL: Lazy<Gauge> = Lazy::new(|| {
    register_gauge_with_registry!(
        "moisture_level",
        "Arbitrary moisture level",
        MOISTURE_REGISTRY
    )
    .unwrap()
});

static TEMPERATURE: Lazy<Gauge> = Lazy::new(|| {
    register_gauge_with_registry!("temperature", "Temperature in Celcius", WEATHER_REGISTRY)
        .unwrap()
});

static PRESSURE: Lazy<Gauge> = Lazy::new(|| {
    register_gauge_with_registry!("pressure", "Pressure in Pascals", WEATHER_REGISTRY).unwrap()
});

static HUMIDITY: Lazy<Gauge> = Lazy::new(|| {
    register_gauge_with_registry!("humidity", "Humidity in Percent (0-100)", WEATHER_REGISTRY)
        .unwrap()
});

static GAS_RESISTANCE: Lazy<Gauge> = Lazy::new(|| {
    register_gauge_with_registry!("gas_resistance", "Gas resistance in Ohms", WEATHER_REGISTRY)
        .unwrap()
});

fn main() -> Result<()> {
    color_eyre::install()?;

    let mut spi = Spidev::open("/dev/spidev0.1").unwrap();
    let options = SpidevOptions::new()
        .bits_per_word(8)
        .max_speed_hz(20_000)
        .mode(spidev::SpiModeFlags::SPI_MODE_0)
        .build();

    spi.configure(&options).unwrap();

    let cs = Pin::new(LORA_CS_PIN);
    cs.export().unwrap();
    cs.set_direction(Direction::Out).unwrap();

    let reset = Pin::new(LORA_RESET_PIN);
    reset.export().unwrap();
    reset.set_direction(Direction::Out).unwrap();

    let mut lora = sx127x_lora::LoRa::new(spi, cs, reset, FREQUENCY, Delay)
        .expect("Failed to communicate with radio module!");

    loop {
        match inner(&mut lora) {
            Ok(()) => {}
            Err(e) => {
                println!("{}", e);
            }
        }
    }
}

fn submit(msg: Message) -> Result<()> {
    match msg {
        Message::MoistureReport(r) => {
            for (n, r) in r.moisture.into_iter().enumerate() {
                let level = r.clocks as f32 / r.duration.as_secs_f32();

                MOISTURE_LEVEL.set(level as f64);

                let families = MOISTURE_REGISTRY.gather();

                let tags = [("moisture_sensor".to_owned(), n.to_string())]
                    .into_iter()
                    .collect::<HashMap<_, _>>();

                prometheus::push_metrics(
                    "reporter",
                    tags,
                    "http://localhost:9091",
                    families,
                    None,
                )?;
            }
        }
        Message::BME688Report(r) => {
            let temp = r.temp.get::<degree_celsius>();
            let pressure = r.pressure.get::<pascal>();
            let humidity = r.humidity.get::<percent>();
            let gas_resistance = r.gas_resistance.get::<ohm>();
            TEMPERATURE.set(temp as f64);
            PRESSURE.set(pressure as f64);
            HUMIDITY.set(humidity as f64);
            GAS_RESISTANCE.set(gas_resistance as f64);

            let families = WEATHER_REGISTRY.gather();

            prometheus::push_metrics(
                "reporter",
                HashMap::new(),
                "http://localhost:9091",
                families,
                None,
            )?;
        }
    }

    Ok(())
}

fn inner(lora: &mut sx127x_lora::LoRa<Spidev, Pin, Pin, Delay>) -> Result<()> {
    let poll = lora.poll_irq(Some(300000000));
    match poll {
        Ok(size) => {
            println!("Got buffer of len {size}");
            let buffer = lora
                .read_packet()
                .map_err(|e| color_eyre::eyre::eyre!("Oops: {:?}", e))?;
            let msg: Transmission<Message> = postcard::from_bytes(&buffer[..size])?;
            println!("msg: {:?}", msg);
            submit(msg.msg)?;
        }
        Err(e) => println!("Timeout: {:?}", e),
    }

    Ok(())
}
