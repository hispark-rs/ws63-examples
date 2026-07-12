//! WS63 LED Blinky example.
//!
//! Blinks the onboard LED (GPIO0 on most WS63 EVBs).
//!
//! Demonstrates the HAL's **modern GPIO path** — the [`OutputConfig`] builder +
//! the type-erased [`Output`](hisi_hal::gpio::Output) driver — plus a simple
//! busy-wait delay. For an interrupt- or async-timed delay (using the corrected
//! 24 MHz timer clock) see the `timer_irq` / `async_delay` examples.

#![no_std]
#![no_main]

use hisi_hal::gpio::{AnyPin, OutputConfig};
use hisi_riscv_rt::entry;

/// Approximate busy-wait delay (~240 cycles ≈ 1 µs at the 240 MHz CPU clock).
fn delay_ms(ms: u32) {
    for _ in 0..ms {
        for _ in 0..240_000 {
            core::hint::spin_loop();
        }
    }
}

#[entry]
fn main() -> ! {
    // GPIO0 as a push-pull output starting low, built via the OutputConfig builder.
    // SAFETY: GPIO0 is a valid WS63 pin (0..=18) and this example owns it exclusively.
    let mut led = unsafe { AnyPin::steal(0) }.init_output(OutputConfig::new().with_initial(false));

    loop {
        led.set_high();
        delay_ms(500);
        led.set_low();
        delay_ms(500);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
