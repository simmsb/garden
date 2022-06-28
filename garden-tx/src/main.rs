#![no_std]
#![no_main]

use atsamd_hal::delay::Delay;
use atsamd_hal::gpio::{Pin, PushPullOutput, PA06, PA08};
use garden as _;

use feather_m0 as bsp;

type LoRa =
    sx127x_lora::LoRa<bsp::Spi, Pin<PA06, PushPullOutput>, Pin<PA08, PushPullOutput>, Delay>;

#[rtic::app(device = bsp::pac, peripherals = true, dispatchers = [EVSYS])]
mod app {
    use super::*;
    use atsamd_hal::{
        clock::{ClockGenId, ClockSource, GenericClockController},
        eic::{pin::Sense, EIC},
        pac::Peripherals,
        prelude::*,
        rtc::{Count32Mode, Duration, Rtc},
    };
    use bsp::{periph_alias, pin_alias};
    use garden::moisture::Moisture;
    use garden_shared::{DevAddr, Message, SensorReport, Transmission};

    #[local]
    struct Local {
        red_led: bsp::RedLed,
        lora: LoRa,
        eic: EIC,
    }

    #[shared]
    struct Shared {
        moisture: Moisture<3>,
    }

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
        let _gclk0 = clocks.gclk0();
        let gclk1 = clocks.gclk1();
        let rtc_clock_src = clocks
            .configure_gclk_divider_and_source(ClockGenId::GCLK2, 1, ClockSource::XOSC32K, false)
            .unwrap();

        clocks.configure_standby(ClockGenId::GCLK2, true);
        let rtc_clock = clocks.rtc(&rtc_clock_src).unwrap();
        let rtc = Rtc::count32_mode(p.RTC, rtc_clock.freq(), &mut p.PM);

        let eic_clock = clocks.eic(&gclk1).unwrap();
        let mut eic = feather_m0::hal::eic::EIC::init(&mut p.PM, eic_clock, p.EIC);

        core.SCB.set_sleepdeep();

        p.PM.ahbmask.modify(|_, w| {
            w.usb_().clear_bit();
            w.dmac_().clear_bit()
        });
        p.PM.apbamask.modify(|_, w| {
            w.wdt_().clear_bit();
            w.sysctrl_().clear_bit();
            w.pac0_().clear_bit()
        });
        p.PM.apbbmask.modify(|_, w| {
            w.usb_().clear_bit();
            w.dmac_().clear_bit();
            w.nvmctrl_().clear_bit();
            w.dsu_().clear_bit();
            w.pac1_().clear_bit()
        });
        p.PM.apbcmask.modify(|_, w| w.adc_().clear_bit());

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

        lora.set_tx_power(10, 1).unwrap();

        let mut a0 = pins.a0.into_floating_ei();
        a0.sense(&mut eic, Sense::RISE);

        let a1 = pins.a1.into_push_pull_output();
        let a2 = pins.a2.into_push_pull_output();
        let a3 = pins.a3.into_push_pull_output();

        let moisture = Moisture::<3>::new(a0, a1, a2, a3);

        blink::spawn().unwrap();
        moisture_ticker::spawn().unwrap();

        (
            Shared { moisture },
            Local { red_led, lora, eic },
            init::Monotonics(rtc),
        )
    }

    #[task(local = [lora], capacity = 3)]
    fn broadcast_message(cx: broadcast_message::Context, msg: Message) {
        let mut buffer = [0; 255];

        let trans = Transmission {
            src: DevAddr(0x69),
            msg,
        };

        let s = postcard::to_slice(&trans, &mut buffer).unwrap();
        let len = s.len();

        cx.local.lora.transmit_payload(buffer, len).unwrap();
    }

    #[task(shared = [moisture], local = [eic])]
    fn moisture_ticker(mut cx: moisture_ticker::Context) {
        let (delay, reading) = cx.shared.moisture.lock(|m| {
            let delay = m.step_state(cx.local.eic, monotonics::now());

            let reading = if m.is_reading_ready() {
                Some(m.format_message())
            } else {
                None
            };

            (delay, reading)
        });

        if let Some(moisture) = reading {
            let report = SensorReport { moisture };

            let _ = broadcast_message::spawn(Message::Report(report));
        }

        let _ = moisture_ticker::spawn_after(delay);
    }

    #[task(local = [red_led])]
    fn blink(cx: blink::Context) {
        cx.local.red_led.toggle().unwrap();

        let _ = blink::spawn_after(Duration::secs(1));
    }

    #[task(priority = 3, binds = EIC, shared = [moisture])]
    fn eic(mut cx: eic::Context) {
        cx.shared.moisture.lock(|m| m.tick_count());
    }

    #[idle]
    fn idle(_cx: idle::Context) -> ! {
        loop {
            cortex_m::asm::wfi();
        }
    }
}
