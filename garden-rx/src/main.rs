use color_eyre::Result;
use garden_shared::{Message, Transmission};
use linux_embedded_hal as hal;

use hal::spidev::{self, SpidevOptions};
use hal::sysfs_gpio::Direction;
use hal::Delay;
use hal::{Pin, Spidev};

const LORA_CS_PIN: u64 = 26;
const LORA_RESET_PIN: u64 = 22;
const FREQUENCY: i64 = 868;

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
        }
        Err(e) => println!("Timeout: {:?}", e),
    }

    Ok(())
}
