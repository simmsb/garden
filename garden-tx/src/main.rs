#![no_std]
#![no_main]

use atsamd_hal::delay::Delay;
use atsamd_hal::gpio::{
    FloatingInput, Pin, PushPullOutput, ReadableOutput, PA06, PA08, PA09, PA16, PA18, PA19,
};
use garden as _;

use feather_m0 as bsp;
use bsp::hal::watchdog::{Watchdog, WatchdogTimeout};
use garden_shared::StatusFlags;
use radio::{Receive, Transmit};
use radio_sx127x::base::Base;

use embedded_hal_compat::{Forward, ForwardCompat};
use radio_sx127x::device::lora::{
    Bandwidth, CodingRate, FrequencyHopping, PayloadCrc, PayloadLength, SpreadingFactor,
};
use radio_sx127x::device::{Channel, Modem, PaConfig, PaSelect};
use radio_sx127x::prelude::{LoRaChannel, LoRaConfig};

type LoRa = radio_sx127x::Sx127x<
    Base<
        TransferInPlaceFwd<Forward<bsp::Spi>>,
        // cs
        Forward<Pin<PA06, ReadableOutput>>,
        // irq / busy
        Forward<Pin<PA09, FloatingInput>>,
        // d12, ready
        Forward<Pin<PA19, FloatingInput>>,
        // reset
        Forward<Pin<PA08, ReadableOutput>>,
        Forward<Delay>,
    >,
>;

pub struct TransferInPlaceFwd<T>(pub T);

impl<T> embedded_hal_compat::eh1_0::spi::ErrorType for TransferInPlaceFwd<T>
where
    T: embedded_hal_compat::eh1_0::spi::ErrorType,
{
    type Error = T::Error;
}

impl<T> embedded_hal_compat::eh1_0::spi::blocking::Write<u8> for TransferInPlaceFwd<T>
where
    T: embedded_hal_compat::eh1_0::spi::blocking::Write<u8>,
{
    fn write(&mut self, words: &[u8]) -> Result<(), Self::Error> {
        self.0.write(words)
    }
}

impl<T> embedded_hal_compat::eh1_0::spi::blocking::Transactional<u8> for TransferInPlaceFwd<T>
where
    T: embedded_hal_compat::eh1_0::spi::blocking::Transactional<u8>,
{
    fn exec<'a>(
        &mut self,
        operations: &mut [embedded_hal::spi::blocking::Operation<'a, u8>],
    ) -> Result<(), Self::Error> {
        self.0.exec(operations)
    }
}
impl<T> embedded_hal_compat::eh1_0::spi::blocking::TransferInplace<u8> for TransferInPlaceFwd<T>
where
    T: embedded_hal_compat::eh1_0::spi::blocking::Transactional<u8>,
{
    fn transfer_inplace(&mut self, words: &mut [u8]) -> Result<(), Self::Error> {
        self.0.exec(&mut [
            embedded_hal_compat::eh1_0::spi::blocking::Operation::TransferInplace(words),
        ])
    }
}

const CONFIG_CH: LoRaChannel = LoRaChannel {
    freq: 868_000_000,
    bw: Bandwidth::Bw125kHz,
    sf: SpreadingFactor::Sf7,
    cr: CodingRate::Cr4_8,
};

const CONFIG_LORA: LoRaConfig = LoRaConfig {
    preamble_len: 0x8,
    symbol_timeout: 0x64,
    payload_len: PayloadLength::Variable,
    payload_crc: PayloadCrc::Enabled,
    frequency_hop: FrequencyHopping::Disabled,
    invert_iq: false,
};

const CONFIG_PA: PaConfig = PaConfig {
    output: PaSelect::Boost,
    power: 10,
};

const CONFIG_RADIO: radio_sx127x::device::Config = radio_sx127x::device::Config {
    modem: Modem::LoRa(CONFIG_LORA),
    channel: Channel::LoRa(CONFIG_CH),
    pa_config: CONFIG_PA,
    xtal_freq: 32_000_000,
    timeout_ms: 100,
};

pub struct DeviceStatus {
    flags: StatusFlags,
    valve_pin: Pin<PA18, PushPullOutput>,
    pump_pin: Pin<PA16, PushPullOutput>,
}

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
    use garden_shared::{Command, DevAddr, Message, Transmission};

    static TC5_FIRED: AtomicBool = AtomicBool::new(false);

    #[local]
    struct Local {
        red_led: bsp::RedLed,
        lora: LoRa,
        lora_delay: SleepingDelay<TimerCounter5>,
        eic: EIC,
        bme: Bme688,
        wdt: Watchdog,
    }

    #[shared]
    struct Shared {
        moisture: Moisture<3>,
        status: DeviceStatus,
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

        let mut red_led = pin_alias!(pins.red_led).into_push_pull_output();
        red_led.set_low().unwrap();

        let mut wdt = Watchdog::new(p.WDT);
        wdt.start(WatchdogTimeout::Cycles16K as u8);

        let tc45 = clocks.tc4_tc5(&rtc_clock_src).unwrap();
        let timer = TimerCounter::tc5_(&tc45, p.TC5, &mut p.PM);
        let lora_delay = SleepingDelay::new(timer, &TC5_FIRED);
        let delay = Delay::new(core.SYST, &mut clocks);

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

        let lora = radio_sx127x::Sx127x::spi(
            TransferInPlaceFwd(spi.forward()),
            pins.rfm_cs.into_readable_output().forward(),
            pins.rfm_irq.into_floating_input().forward(),
            pins.d12.into_floating_input().forward(),
            pins.rfm_reset.into_readable_output().forward(),
            delay.forward(),
            &CONFIG_RADIO,
        )
        .unwrap();

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
        let bme = Bme688::new(i2c, p.TC4, &tc45, &mut p.PM);

        let mut valve_pin = pins.d10.into_push_pull_output();
        let mut pump_pin = pins.d11.into_push_pull_output();

        valve_pin.set_low().unwrap();
        pump_pin.set_low().unwrap();

        let status = DeviceStatus {
            flags: StatusFlags::empty(),
            valve_pin,
            pump_pin,
        };

        moisture_ticker::spawn_after(Duration::secs(3)).unwrap();
        bme_task::spawn_after(Duration::secs(5)).unwrap();
        status_task::spawn_after(Duration::secs(10)).unwrap();
        wdt_task::spawn().unwrap();

        (
            Shared { moisture, status },
            Local {
                red_led,
                lora,
                lora_delay,
                eic,
                bme,
                wdt,
            },
            init::Monotonics(rtc),
        )
    }

    #[task(local = [lora, lora_delay, red_led], capacity = 3)]
    fn broadcast_message(cx: broadcast_message::Context, msg: Message) {
        let addr = DevAddr(0x69);
        let addr_recv = DevAddr(69);

        let mut buffer = [0; 255];

        let trans = Transmission { src: addr, msg };

        let s = postcard::to_slice(&trans, &mut buffer).unwrap();

        cx.local.red_led.set_high().unwrap();
        cx.local.lora.start_transmit(s).unwrap();

        loop {
            if !matches!(cx.local.lora.check_transmit(), Ok(false)) {
                break;
            }

            cx.local.lora_delay.delay_ms(10u32);
        }

        cx.local.red_led.set_low().unwrap();

        cx.local.lora.start_receive().unwrap();

        for _ in 0..50 {
            match cx.local.lora.check_receive(true) {
                Ok(true) => {
                    if let Ok((n, _)) = cx.local.lora.get_received(&mut buffer) {
                        if let Ok(cmd) = postcard::from_bytes::<Transmission<Command>>(&buffer[..n])
                        {
                            if cmd.src == addr_recv {
                                let _ = handle_msg::spawn(cmd.msg);
                            }
                        }
                    } else {
                        break;
                    }
                }
                Ok(false) => {}
                Err(_) => {
                    break;
                }
            }

            cx.local.lora_delay.delay_ms(10u32);
        }

        let _ = cx.local.lora.reset();
        let _ = cx.local.lora.configure(&CONFIG_RADIO);

        // if let Ok(Some(msg)) = cx.local.lora.read_packet_timeout(500, cx.local.lora_delay) {
        // }

        // // ensure we leave a gap between transmissions
        cx.local.red_led.set_high().unwrap();
        cx.local.lora_delay.delay_ms(50u32);
        cx.local.red_led.set_low().unwrap();
    }

    #[task(local = [bme], priority = 1)]
    fn bme_task(cx: bme_task::Context) {
        if let Some(reading) = cx.local.bme.read() {
            let _ = broadcast_message::spawn(Message::BME688Report(reading));
        }

        let _ = bme_task::spawn_after(Duration::secs(60));
    }

    #[task(shared = [status], priority = 1)]
    fn status_task(mut cx: status_task::Context) {
        let flags = cx.shared.status.lock(|s| s.flags);

        let _ =
            broadcast_message::spawn(Message::StatusUpdate(garden_shared::DeviceStatus { flags }));

        let _ = status_task::spawn_after(Duration::secs(10));
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

    #[task(shared = [status], capacity = 3)]
    fn handle_msg(mut cx: handle_msg::Context, cmd: Command) {
        let flags = cx.shared.status.lock(|s| {
            match cmd {
                Command::SyncFlags(flags) => {
                    let pump_state = flags.contains(StatusFlags::PUMP_ON);
                    let valve_state = flags.contains(StatusFlags::VALVE_OPEN);
                    s.pump_pin.set_state(pump_state.into()).unwrap();
                    s.valve_pin.set_state(valve_state.into()).unwrap();
                    s.flags = flags;
                }
            };
            s.flags
        });

        let _ =
            broadcast_message::spawn(Message::StatusUpdate(garden_shared::DeviceStatus { flags }));
    }

    #[task(priority = 2, local = [wdt])]
    fn wdt_task(cx: wdt_task::Context) {
        cx.local.wdt.feed();

        let _ = wdt_task::spawn_after(Duration::millis(100));
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
