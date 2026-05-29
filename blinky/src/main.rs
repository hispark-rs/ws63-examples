//! WS63 LED Blinky example.
//!
//! Blinks the onboard LED on a WS63 evaluation board.
//! The LED is typically connected to GPIO0 on most WS63 EVBs.
//!
//! This example demonstrates:
//! - Runtime initialization (ws63-rt)
//! - GPIO output control (ws63-hal)
//! - Simple busy-wait delay

#![no_std]
#![no_main]

use ws63_hal::gpio::{create_output_pin, Pin};
use ws63_hal::peripherals::Peripherals;
use ws63_hal::prelude::*;
use ws63_rt::entry;

/// Simple busy-wait delay (cycles ~240 MHz).
fn delay_ms(ms: u32) {
    // Approximate: 240 cycles = 1 µs at 240 MHz
    for _ in 0..ms {
        for _ in 0..240_000 {
            core::hint::spin_loop();
        }
    }
}

#[entry]
fn main() -> ! {
    // Take ownership of all peripherals
    let peripherals = Peripherals::take().expect("Failed to take peripherals");

    // Configure GPIO0 as output (board LED pin)
    let led_pin = Pin::new(0);
    let mut led = create_output_pin(led_pin, peripherals.GPIO0)
        .expect("Failed to create LED output pin");

    loop {
        // LED on
        led.set_high().ok();
        delay_ms(500);

        // LED off
        led.set_low().ok();
        delay_ms(500);
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
