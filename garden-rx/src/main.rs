use std::collections::HashMap;

use color_eyre::Result;
use filter::kalman::kalman_filter::KalmanFilter;
use garden_shared::{BME688SensorReport, Message, Transmission};
use linux_embedded_hal as hal;

use hal::spidev::{self, SpidevOptions};
use hal::sysfs_gpio::Direction;
use hal::Delay;
use hal::{Pin, Spidev};
use nalgebra::{Matrix1, Vector1, U1};
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

    let mut exporter = Exporter::new();

    loop {
        match exporter.inner(&mut lora) {
            Ok(()) => {}
            Err(e) => {
                println!("{}", e);
            }
        }
    }
}

struct Exporter {
    temp_filter: KalmanFilter<f32, U1, U1, U1>,
    humidity_filter: KalmanFilter<f32, U1, U1, U1>,
    pressure_filter: KalmanFilter<f32, U1, U1, U1>,
    moisture_filters: [KalmanFilter<f32, U1, U1, U1>; 3],
    last_reading: Option<BME688SensorReport>,
}

impl Exporter {
    fn new() -> Self {
        let mut temp_filter = KalmanFilter::default();
        temp_filter.x = Vector1::new(19.0);
        temp_filter.H = Vector1::new(1.0);
        temp_filter.Q = Matrix1::repeat(0.01);

        let mut humidity_filter = KalmanFilter::default();
        humidity_filter.x = Vector1::new(50.0);
        humidity_filter.H = Vector1::new(1.0);
        humidity_filter.Q = Matrix1::repeat(0.01);

        let mut pressure_filter = KalmanFilter::default();
        pressure_filter.x = Vector1::new(100.0);
        pressure_filter.H = Vector1::new(1.0);
        pressure_filter.Q = Matrix1::repeat(0.01);

        let moisture_filters = [(); 3].map(|_| {
            let mut moisture_filter = KalmanFilter::default();
            moisture_filter.x = Vector1::new(17.0);
            moisture_filter.H = Vector1::new(1.0);
            moisture_filter.Q = Matrix1::repeat(0.01);

            moisture_filter
        });

        Self {
            temp_filter,
            humidity_filter,
            pressure_filter,
            moisture_filters,
            last_reading: None,
        }
    }

    fn submit(&mut self, msg: Message) -> Result<()> {
        match msg {
            Message::MoistureReport(r) => {
                for (n, r) in r.moisture.into_iter().enumerate() {
                    let level = Vector1::new(r.clocks as f32 / r.duration.as_secs_f32());
                    let filter = &mut self.moisture_filters[n];
                    filter.update(&level, None, None);
                    filter.predict(None, None, None, None);

                    MOISTURE_LEVEL.set(filter.x[0] as f64);

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
                let r = r
                    .sanity_check(self.last_reading.as_ref())
                    .ok_or_else(|| color_eyre::eyre::eyre!("Invalid reading"))?;
                self.last_reading = Some(r.clone());

                let temp = Vector1::new(r.temp.get::<degree_celsius>());
                let pressure = Vector1::new(r.pressure.get::<pascal>());
                let humidity = Vector1::new(r.humidity.get::<percent>());
                let gas_resistance = r.gas_resistance.get::<ohm>();

                self.temp_filter.update(&temp, None, None);
                self.temp_filter.predict(None, None, None, None);

                self.pressure_filter.update(&pressure, None, None);
                self.pressure_filter.predict(None, None, None, None);

                self.humidity_filter.update(&humidity, None, None);
                self.humidity_filter.predict(None, None, None, None);

                TEMPERATURE.set(self.temp_filter.x[0] as f64);
                PRESSURE.set(self.pressure_filter.x[0] as f64);
                HUMIDITY.set(self.humidity_filter.x[0] as f64);
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

    fn inner(&mut self, lora: &mut sx127x_lora::LoRa<Spidev, Pin, Pin, Delay>) -> Result<()> {
        let poll = lora.poll_irq(Some(300000000));
        match poll {
            Ok(size) => {
                println!("Got buffer of len {size}");
                let buffer = lora
                    .read_packet()
                    .map_err(|e| color_eyre::eyre::eyre!("Oops: {:?}", e))?;
                let msg: Transmission<Message> = postcard::from_bytes(&buffer[..size])?;
                println!("msg: {:?}", msg);
                self.submit(msg.msg)?;
            }
            Err(e) => println!("Timeout: {:?}", e),
        }

        Ok(())
    }
}
