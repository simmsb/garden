#![no_main]
#![no_std]
#![feature(generic_const_exprs)]

pub mod moisture;
pub mod bme688;

#[cfg(feature = "debugger")]
use defmt_rtt as _;

// #[cfg(feature = "debugger")]
// use panic_probe as _;

use panic_reset as _;

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
