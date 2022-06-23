#![no_main]
#![no_std]

#[cfg(feature = "debugger")]
use defmt_rtt as _;

#[cfg(feature = "debugger")]
use panic_probe as _;

#[cfg(feature = "debugger")]
use feather_m0 as _;

#[defmt::panic_handler]
fn panic() -> ! {
    cortex_m::asm::udf()
}

pub fn exit() -> ! {
    loop {
        cortex_m::asm::bkpt();
    }
}
