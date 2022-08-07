use atsamd_hal::eic::pin::ExtInt2;
use atsamd_hal::eic::EIC;
use atsamd_hal::gpio::{Floating, Interrupt, Output, Pin, PushPull, PA02, PA04, PB08, PB09};
use atsamd_hal::prelude::_atsamd_hal_embedded_hal_digital_v2_OutputPin;
use atsamd_hal::rtc::{Duration, Instant};
use garden_shared::{MoistureReading, MoistureSensorReport};

mod lessthan {
    struct If<const B: bool>;
    trait True {}
    impl True for If<true> {}

    pub struct LessThanCarrier<const LHS: usize>;

    pub trait LessThan<const RHS: usize> {}

    impl<const LHS: usize, const RHS: usize> LessThan<RHS> for LessThanCarrier<LHS> where
        If<{ LHS < RHS }>: True
    {
    }
}
pub struct Moisture<const PINS: usize>
where
    lessthan::LessThanCarrier<PINS>: lessthan::LessThan<8>,
{
    readings: [Option<(u16, Duration)>; PINS],
    a0: ExtInt2<Pin<PA02, Interrupt<Floating>>>,
    a1: Pin<PB08, Output<PushPull>>,
    a2: Pin<PB09, Output<PushPull>>,
    a3: Pin<PA04, Output<PushPull>>,
    state: State,
    count: u16,
}

enum State {
    Off,
    Measuring(u8, Instant),
}

impl<const PINS: usize> Moisture<PINS>
where
    lessthan::LessThanCarrier<PINS>: lessthan::LessThan<8>,
{
    pub fn new(
        a0: ExtInt2<Pin<PA02, Interrupt<Floating>>>,
        a1: Pin<PB08, Output<PushPull>>,
        a2: Pin<PB09, Output<PushPull>>,
        a3: Pin<PA04, Output<PushPull>>,
    ) -> Self {
        Self {
            readings: [None; PINS],
            a0,
            a1,
            a2,
            a3,
            state: State::Off,
            count: 0,
        }
    }

    fn set_on(&mut self, eic: &mut EIC) {
        self.a0.enable_interrupt(eic);
        self.a0.enable_interrupt_wake(eic);
        self.count = 0;
        self.readings.fill(None);
    }

    fn set_off(&mut self, eic: &mut EIC) {
        self.a0.disable_interrupt(eic);
    }

    fn set_pins_for(&mut self, n: u8) {
        let a1 = (n & 0b1) == 0b1;
        let a2 = (n & 0b10) == 0b10;
        let a3 = (n & 0b100) == 0b100;

        self.a1.set_state(a1.into()).unwrap();
        self.a2.set_state(a2.into()).unwrap();
        self.a3.set_state(a3.into()).unwrap();
    }

    pub fn is_reading_ready(&self) -> bool {
        matches!(self.state, State::Off)
    }

    pub fn format_message(&self) -> MoistureSensorReport {
        let r = self
            .readings
            .iter()
            .map(|r| {
                let (clocks, duration) = r.unwrap();

                MoistureReading {
                    clocks,
                    duration: core::time::Duration::from_millis(duration.to_millis() as u64),
                }
            })
            .collect::<heapless::Vec<_, 8>>();

        MoistureSensorReport { moisture: r }
    }

    pub fn step_state(&mut self, eic: &mut EIC, now: Instant) -> Duration {
        const BETWEEN_MEASUREMENTS_DELAY: Duration = Duration::secs(60);
        const BETWEEN_READINGS_DELAY: Duration = Duration::secs(1);

        let (new_state, next_step_delay) = match self.state {
            State::Off => {
                self.set_pins_for(0);
                self.set_on(eic);
                (State::Measuring(0, now), BETWEEN_READINGS_DELAY)
            }
            State::Measuring(n, inst) => {
                let reading = core::mem::replace(&mut self.count, 0);
                let duration = now
                    .checked_duration_since(inst)
                    .unwrap_or(Duration::from_ticks(0));
                self.readings[n as usize] = Some((reading, duration));

                if (n + 1) as usize == PINS {
                    self.set_off(eic);
                    (State::Off, BETWEEN_MEASUREMENTS_DELAY)
                } else {
                    self.set_pins_for(n + 1);
                    (State::Measuring(n + 1, now), BETWEEN_READINGS_DELAY)
                }
            }
        };

        self.state = new_state;

        next_step_delay
    }

    pub fn tick_count(&mut self) {
        if self.a0.is_interrupt() {
            self.count += 1;

            self.a0.clear_interrupt();
        }
    }
}
