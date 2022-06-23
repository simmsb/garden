#![no_std]
#![no_main]

use atsamd_hal::delay::Delay;
use atsamd_hal::gpio::{Pin, PushPullOutput, PA06, PA08};
use garden as _;

use feather_m0 as bsp;

type LoRa =
    sx127x_lora::LoRa<bsp::Spi, Pin<PA06, PushPullOutput>, Pin<PA08, PushPullOutput>, Delay>;

#[derive(serde::Serialize, serde::Deserialize)]
struct DevAddr(u16);

#[derive(serde::Serialize)]
struct Message<'a> {
    msg: &'a str,
}

#[derive(serde::Serialize)]
struct Transmission<T> {
    src: DevAddr,
    msg: T,
}

#[rtic::app(device = bsp::pac, peripherals = true, dispatchers = [EVSYS])]
mod app {
    use super::*;
    use atsamd_hal::{
        clock::{ClockGenId, ClockSource, GenericClockController},
        pac::Peripherals,
        prelude::*,
        rtc::{Count32Mode, Duration, Rtc},
    };
    use bsp::{periph_alias, pin_alias};

    #[local]
    struct Local {
        red_led: bsp::RedLed,
        lora: LoRa,
    }

    #[shared]
    struct Shared {}

    #[monotonic(binds = RTC, default = true)]
    type RtcMonotonic = Rtc<Count32Mode>;

    #[init]
    fn init(cx: init::Context) -> (Shared, Local, init::Monotonics) {
        let mut p: Peripherals = cx.device;
        let pins = bsp::Pins::new(p.PORT);
        let mut core = cx.core;
        let mut clocks = GenericClockController::with_external_32kosc(
            p.GCLK,
            &mut p.PM,
            &mut p.SYSCTRL,
            &mut p.NVMCTRL,
        );
        let _gclk = clocks.gclk0();
        let rtc_clock_src = clocks
            .configure_gclk_divider_and_source(ClockGenId::GCLK2, 1, ClockSource::XOSC32K, false)
            .unwrap();

        clocks.configure_standby(ClockGenId::GCLK2, true);
        let rtc_clock = clocks.rtc(&rtc_clock_src).unwrap();
        let rtc = Rtc::count32_mode(p.RTC, rtc_clock.freq(), &mut p.PM);

        core.SCB.set_sleepdeep();

        let red_led = pin_alias!(pins.red_led).into_push_pull_output();

        let spi_sercom = periph_alias!(p.spi_sercom);
        let spi = bsp::spi_master(
            &mut clocks,
            8u32.mhz(),
            spi_sercom,
            &mut p.PM,
            pins.sclk,
            pins.mosi,
            pins.miso,
        );

        let mut lora = LoRa::new(
            spi,
            pins.rfm_cs.into_push_pull_output(),
            pins.rfm_reset.into_push_pull_output(),
            868,
            Delay::new(core.SYST, &mut clocks),
        )
        .unwrap();

        lora.set_tx_power(5, 1).unwrap();

        blink::spawn().unwrap();
        radio::spawn().unwrap();

        (Shared {}, Local { red_led, lora }, init::Monotonics(rtc))
    }

    #[task(local = [lora])]
    fn radio(cx: radio::Context) {
        let mut buffer = [0; 255];

        let msg = Message { msg: "poo" };

        let s = postcard::to_slice(&msg, &mut buffer).unwrap();
        let len = s.len();

        cx.local.lora.transmit_payload(buffer, len).unwrap();

        let _ = radio::spawn_after(Duration::secs(1));
    }

    #[task(local = [red_led])]
    fn blink(cx: blink::Context) {
        cx.local.red_led.toggle().unwrap();

        let _ = blink::spawn_after(Duration::secs(1));
    }
}
