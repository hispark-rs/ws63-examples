//! WS63 embassy **async-I/O capstone**.
//!
//! Three async drivers cooperating under `embassy-executor`:
//! * `embassy-time` `Timer` (ws63-hal embassy-time `Driver`) paces the blinker,
//! * `embedded_hal_async::digital::Wait` (ws63-hal GPIO) awaits a real GPIO edge
//!   IRQ — demoed here because embassy gives the concurrency a single-task
//!   `block_on` lacks (one task drives the edge, another awaits it),
//! * `embedded_io_async::Write` (ws63-hal UART) prints the result asynchronously.
//!
//! `blinker` toggles GPIO0 pin0 every 20 ms (QEMU loops output→input, raising the
//! edge); `waiter` `wait_for_rising_edge().await`s it and async-writes a line.
//! The trap routes the embassy alarm (TIMER_INT0=26) and GPIO0 (GPIO_INT0=33) to
//! their HAL `on_interrupt` hooks.

#![no_std]
#![no_main]

use embassy_executor::{Executor, Spawner};
use embassy_time::Timer;
use embedded_hal_async::digital::Wait;
use embedded_io_async::Write;
use static_cell::StaticCell;
use ws63_hal::gpio::{AnyPin, InputConfig};
use ws63_hal::interrupt;
use ws63_hal::peripherals::Uart0;
use ws63_hal::uart::{Config as UartConfig, Uart};
use ws63_rt::entry;

const GPIO0: usize = 0x4402_8000;
const GPIO_OEN: *mut u32 = (GPIO0 + 0x04) as *mut u32;
const GPIO_DATA_SET: *mut u32 = (GPIO0 + 0x30) as *mut u32;
const GPIO_DATA_CLR: *mut u32 = (GPIO0 + 0x34) as *mut u32;

// ── trap: embassy alarm (26) + GPIO0 (33) ──
core::arch::global_asm!(
    ".section .text.atrap, \"ax\"",
    ".balign 4",
    ".global atrap",
    "atrap:",
    "    addi sp, sp, -64",
    "    sw ra,0(sp)\n sw t0,4(sp)\n sw t1,8(sp)\n sw t2,12(sp)\n sw t3,16(sp)",
    "    sw t4,20(sp)\n sw t5,24(sp)\n sw t6,28(sp)\n sw a0,32(sp)\n sw a1,36(sp)",
    "    sw a2,40(sp)\n sw a3,44(sp)\n sw a4,48(sp)\n sw a5,52(sp)\n sw a6,56(sp)\n sw a7,60(sp)",
    "    call atrap_handle",
    "    lw ra,0(sp)\n lw t0,4(sp)\n lw t1,8(sp)\n lw t2,12(sp)\n lw t3,16(sp)",
    "    lw t4,20(sp)\n lw t5,24(sp)\n lw t6,28(sp)\n lw a0,32(sp)\n lw a1,36(sp)",
    "    lw a2,40(sp)\n lw a3,44(sp)\n lw a4,48(sp)\n lw a5,52(sp)\n lw a6,56(sp)\n lw a7,60(sp)",
    "    addi sp, sp, 64",
    "    mret",
);

unsafe extern "C" {
    fn atrap();
}

#[unsafe(no_mangle)]
extern "C" fn atrap_handle() {
    let mcause: u32;
    unsafe { core::arch::asm!("csrr {0}, mcause", out(reg) mcause) };
    if (mcause & 0x8000_0000) != 0 {
        match mcause & 0xFFF {
            26 => ws63_hal::embassy::on_alarm_interrupt(), // embassy-time alarm
            33 => ws63_hal::gpio::on_interrupt(0),         // GPIO0 edge
            _ => {}
        }
    }
}

#[embassy_executor::task]
async fn blinker() {
    loop {
        Timer::after_millis(20).await;
        unsafe {
            core::ptr::write_volatile(GPIO_DATA_CLR, 1); // pin0 low
            core::ptr::write_volatile(GPIO_DATA_SET, 1); // pin0 high -> rising edge
        }
    }
}

#[embassy_executor::task]
async fn waiter() {
    // Steal pin0 as input (senses the loopback edge) and UART0 (async output).
    let mut input = unsafe { AnyPin::steal(0) }.init_input(InputConfig::default());
    let mut uart = Uart::new_uart0(unsafe { Uart0::steal() }, UartConfig::default());
    let mut n = 0u32;
    loop {
        input.wait_for_rising_edge().await.ok(); // GPIO IRQ -> waker
        n += 1;
        let _ = uart.write_all(b"[gpio] async rising edge\r\n").await;
        let _ = uart.flush().await;
        if n == 5 {
            let _ = uart.write_all(b"EMBASSY ASYNC IO: PASS\r\n").await;
        }
    }
}

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

#[entry]
fn main() -> ! {
    // Start the TCXO counter (embassy-time `now()` source); banner via blocking UART.
    let mut tcxo = ws63_hal::tcxo::TcxoDriver::new(unsafe { ws63_hal::peripherals::Tcxo::steal() });
    tcxo.enable();
    let banner = Uart::new_uart0(unsafe { Uart0::steal() }, UartConfig::default());
    banner.write(
        0,
        b"\r\nWS63 embassy async-IO (GPIO Wait + async UART, timed by embassy-time)\r\n",
    );

    unsafe {
        core::ptr::write_volatile(GPIO_OEN, 0); // GPIO0 pin0 = output (drives loopback)
        core::arch::asm!("csrw mtvec, {0}", in(reg) atrap as *const () as usize);
        interrupt::init();
        interrupt::enable_global();
    }

    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner: Spawner| {
        spawner.spawn(blinker().unwrap());
        spawner.spawn(waiter().unwrap());
    });
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
