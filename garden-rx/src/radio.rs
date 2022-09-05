use std::sync::Mutex;

use chrono::{DateTime, Utc};
use color_eyre::Result;
use embedded_radio::EmbeddedRadio;
use garden_shared::{
    BME688SensorReport, Command, DevAddr, DeviceStatus, Message, MoistureSensorReport, StatusFlags,
    Transmission,
};
use influxdb2::{models::DataPoint, Client};
use linux_embedded_hal as hal;

use hal::spidev::{self, SpidevOptions};
use hal::sysfs_gpio::Direction;
use hal::Delay;
use hal::{Pin, Spidev};
use once_cell::sync::Lazy;
use tokio::sync::watch;
use uom::si::pressure::pascal;
use uom::si::ratio::percent;
use uom::si::thermodynamic_temperature::degree_celsius;

const LORA_CS_PIN: u64 = 26;
const LORA_RESET_PIN: u64 = 22;
const FREQUENCY: i64 = 868;

pub static DESIRED_STATE: Lazy<Mutex<StatusFlags>> = Lazy::new(|| Mutex::new(StatusFlags::empty()));
pub static RESET_WANTED: Lazy<Mutex<bool>> = Lazy::new(|| Mutex::new(false));

pub fn radio_side(status_sender: watch::Sender<Option<DeviceStatus>>) -> Result<()> {
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

    let mut lora = embedded_radio::LoRa::new(spi, cs, reset, FREQUENCY, &mut Delay)
        .expect("Failed to communicate with radio module!");
    lora.set_tx_power(17, 1).unwrap();

    let mut exporter = Exporter::new(status_sender);

    println!("Radio initialized");

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
    last_bme_reading: Option<BME688SensorReport>,
    last_moisture_reading: Option<MoistureSensorReport>,
    status_sender: watch::Sender<Option<DeviceStatus>>,
    client: influxdb2::Client,
}

impl Exporter {
    fn new(status_sender: watch::Sender<Option<DeviceStatus>>) -> Self {
        let client =
            Client::new("http://localhost:8086", "garden", "IoyGBd5jH-RScuacNjlUBSToAHtlKu270PesRi9E5Gg4M516GittWr2w5QdJPkn4X8Wh_VA7zfhxByOaviMuCQ==");

        Self {
            last_bme_reading: None,
            last_moisture_reading: None,
            status_sender,
            client,
        }
    }

    fn submit(&mut self, msg: Message) -> Result<()> {
        match msg {
            Message::MoistureReport(r) => {
                let r = match r.sanity_check(self.last_moisture_reading.as_ref()) {
                    Ok(it) => it,
                    Err(err) => {
                        self.last_moisture_reading = None;
                        return Err(err)?;
                    }
                };
                self.last_moisture_reading = Some(r.clone());

                for (n, r) in r.moisture.into_iter().enumerate() {
                    let level = r.per_second();

                    let reading = DataPoint::builder("moisture")
                        .tag("sensor", n.to_string())
                        .field("moisture", level as f64)
                        .build()?;

                    let client = self.client.clone();
                    tokio::spawn(async move {
                        client
                            .write("garden", futures::stream::iter([reading]))
                            .await
                            .unwrap();
                    });
                }
            }
            Message::BME688Report(r) => {
                let r = match r.sanity_check(self.last_bme_reading.as_ref()) {
                    Ok(it) => it,
                    Err(err) => {
                        self.last_bme_reading = None;
                        return Err(err)?;
                    }
                };
                self.last_bme_reading = Some(r.clone());

                let temp = r.temp.get::<degree_celsius>();
                let pressure = r.pressure.get::<pascal>();
                let humidity = r.humidity.get::<percent>();

                let temp_reading = DataPoint::builder("temp")
                    .field("temp", temp as f64)
                    .build()?;

                let pressure_reading = DataPoint::builder("pressure")
                    .field("pressure", pressure as f64)
                    .build()?;

                let humidity_reading = DataPoint::builder("humidity")
                    .field("humidity", humidity as f64)
                    .build()?;

                let client = self.client.clone();
                tokio::spawn(async move {
                    client
                        .write(
                            "garden",
                            futures::stream::iter([
                                temp_reading,
                                pressure_reading,
                                humidity_reading,
                            ]),
                        )
                        .await
                        .unwrap();
                });
            }
            Message::StatusUpdate(upd) => {
                self.status_sender.send(Some(upd))?;
            }
        }

        Ok(())
    }

    fn inner(&mut self, lora: &mut embedded_radio::LoRa<Spidev, Pin, Pin>) -> Result<()> {
        if let Some(buffer) = lora
            .read_packet_timeout(100000, &mut Delay)
            .map_err(|e| color_eyre::eyre::eyre!("Oops: {:?}", e))?
        {
            let msg: Transmission<Message> = postcard::from_bytes(&buffer)?;

            if msg.src != DevAddr(0x69) {
                println!("Discarding transmission (wrong src addr) {:?}", msg);
                return Ok(());
            }

            if *RESET_WANTED.lock().unwrap() {
                let t = Transmission {
                    src: DevAddr(69),
                    msg: Command::Reset,
                };

                *RESET_WANTED.lock().unwrap() = false;

                std::thread::sleep(std::time::Duration::from_millis(10));

                let ser = postcard::to_stdvec(&t).unwrap();

                println!("Transmitting command: {:?}", t);

                lora.transmit_payload(&ser)
                    .map_err(|e| color_eyre::eyre::eyre!("Opps: {:?}", e))?;
            }

            if let Message::StatusUpdate(upd) = msg.msg {
                let desired_status = *DESIRED_STATE.lock().unwrap();
                if upd.flags != desired_status {
                    let t = Transmission {
                        src: DevAddr(69),
                        msg: Command::SyncFlags(desired_status),
                    };

                    std::thread::sleep(std::time::Duration::from_millis(10));

                    let ser = postcard::to_stdvec(&t).unwrap();

                    println!("Transmitting command: {:?}", t);

                    lora.transmit_payload(&ser)
                        .map_err(|e| color_eyre::eyre::eyre!("Opps: {:?}", e))?;
                }
            }

            println!("msg: {:?}", msg);
            self.submit(msg.msg)?;
        }

        Ok(())
    }
}
