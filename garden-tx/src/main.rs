#![no_std]
#![no_main]

use atsamd_hal::delay::Delay;
use atsamd_hal::gpio::{Pin, PushPullOutput, PA06, PA08};
use garden as _;

use feather_m0 as bsp;

type LoRa =
    sx127x_lora::LoRa<bsp::Spi, Pin<PA06, PushPullOutput>, Pin<PA08, PushPullOutput>, Delay>;

#[rtic::app(device = bsp::pac, peripherals = true, dispatchers = [EVSYS, USB])]
mod app {
    use core::sync::atomic::AtomicBool;

    use super::*;
    use atsamd_hal::{
        clock::{ClockGenId, ClockSource, GenericClockController},
        eic::{pin::Sense, EIC},
        pac::Peripherals,
        prelude::*,
        rtc::{Count32Mode, Duration, Rtc},
        sleeping_delay::SleepingDelay,
        timer::{TimerCounter, TimerCounter5},
    };
    use bsp::{i2c_master, periph_alias, pin_alias};
    use garden::{bme688::Bme688, moisture::Moisture};
    use garden_shared::{DevAddr, Message, Transmission};

    static TC5_FIRED: AtomicBool = AtomicBool::new(false);

    #[local]
    struct Local {
        red_led: bsp::RedLed,
        lora: LoRa,
        lora_delay: SleepingDelay<TimerCounter5>,
        eic: EIC,
        bme: Bme688,
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

        lora.set_tx_power(5, 1).unwrap();

        let mut a0 = pins.a0.into_floating_ei();
        a0.sense(&mut eic, Sense::RISE);

        let a1 = pins.a1.into_push_pull_output();
        let a2 = pins.a2.into_push_pull_output();
        let a3 = pins.a3.into_push_pull_output();

        let moisture = Moisture::<3>::new(a0, a1, a2, a3);

        let i2c = i2c_master(
            &mut clocks,
            400.khz(),
            p.SERCOM3,
            &mut p.PM,
            pins.sda,
            pins.scl,
        );
        let tc45 = clocks.tc4_tc5(&rtc_clock_src).unwrap();
        let bme = Bme688::new(i2c, p.TC4, &tc45, &mut p.PM);

        let timer = TimerCounter::tc5_(&tc45, p.TC5, &mut p.PM);
        let lora_delay = SleepingDelay::new(timer, &TC5_FIRED);

        blink::spawn().unwrap();
        moisture_ticker::spawn().unwrap();
        bme_task::spawn().unwrap();

        (
            Shared { moisture },
            Local {
                red_led,
                lora,
                lora_delay,
                eic,
                bme,
            },
            init::Monotonics(rtc),
        )
    }

    #[task(local = [lora, lora_delay], capacity = 3)]
    fn broadcast_message(cx: broadcast_message::Context, msg: Message) {
        let mut buffer = [0; 255];

        let trans = Transmission {
            src: DevAddr(0x69),
            msg,
        };

        let s = postcard::to_slice(&trans, &mut buffer).unwrap();
        let len = s.len();

        cx.local.lora.transmit_payload(buffer, len).unwrap();

        // ensure we leave a gap between transmissions
        cx.local.lora_delay.delay_ms(200u32);
    }

    #[task(local = [bme], priority = 1)]
    fn bme_task(cx: bme_task::Context) {
        if let Some(reading) = cx.local.bme.read() {
            let _ = broadcast_message::spawn(Message::BME688Report(reading));
        }

        let _ = bme_task::spawn_after(Duration::secs(10));
    }

    #[task(shared = [moisture], local = [eic], priority = 2)]
    fn moisture_ticker(mut cx: moisture_ticker::Context) {
        let (delay, report) = cx.shared.moisture.lock(|m| {
            let delay = m.step_state(cx.local.eic, monotonics::now());

            let reading = if m.is_reading_ready() {
                Some(m.format_message())
            } else {
                None
            };

            (delay, reading)
        });

        if let Some(report) = report {
            let _ = broadcast_message::spawn(Message::MoistureReport(report));
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

    #[task(priority = 3, binds = TC4)]
    fn tc4(_cx: tc4::Context) {
        garden::bme688::TC4_FIRED.store(true, core::sync::atomic::Ordering::Relaxed);

        unsafe {
            feather_m0::pac::TC4::ptr()
                .as_ref()
                .unwrap()
                .count16()
                .intflag
                .modify(|_, w| w.ovf().set_bit());
        }
    }

    #[task(priority = 3, binds = TC5)]
    fn tc5(_cx: tc5::Context) {
        TC5_FIRED.store(true, core::sync::atomic::Ordering::Relaxed);

        unsafe {
            feather_m0::pac::TC5::ptr()
                .as_ref()
                .unwrap()
                .count16()
                .intflag
                .modify(|_, w| w.ovf().set_bit());
        }
    }

    #[idle]
    fn idle(_cx: idle::Context) -> ! {
        loop {
            cortex_m::asm::wfi();
        }
    }
}
