//! Model-only register observations with global IRQs disabled. Afterwards main
//! enables IRQs and proves five real trap-driven waits on the same adapter.
use core::{
    future::Future,
    task::{Context, Waker},
};
use embedded_hal_async::delay::DelayNs;
use hisi_hal::{
    interrupt::{self},
    peripherals::{Timer, Uart0},
    timer::{self, AsyncDelay},
    uart::Uart,
};

pub fn check(delay: &mut AsyncDelay<'_>, uart: &Uart<'_, Uart0<'_>>) -> bool {
    // SAFETY: fixture only observes the singleton TIMER registers; AsyncDelay
    // owns writes. No other driver runs, and global IRQs are disabled by main.
    let regs = unsafe { &*Timer::ptr() };
    uart.write(b"TIMER CHANNEL: ");
    super::put_u32(uart, super::CHANNEL.index() as u32);
    uart.write(b"\r\n");
    macro_rules! require {
        ($ok:expr, $name:literal) => {
            if !$ok {
                uart.write(concat!("TIMER LIFECYCLE FAIL ", $name, "\r\n").as_bytes());
                return false;
            }
        };
    }
    let mut cx = Context::from_waker(Waker::noop());
    let stopped = || {
        regs.timer0_control(super::CHANNEL.index()).read().bits() & 9 == 8
            && regs.timer0_raw_intr(super::CHANNEL.index()).read().bits() & 1 == 0
            && !interrupt::is_enabled(super::TIMER_IRQ)
            && !interrupt::is_pending(super::TIMER_IRQ)
    };

    {
        let future = delay.delay_ms(1_000);
        let mut future = core::pin::pin!(future);
        require!(future.as_mut().poll(&mut cx).is_pending(), "initial poll");
        require!(
            regs.timer0_control(super::CHANNEL.index()).read().bits() & 9 == 1,
            "armed"
        );
    } // Drop the actual pinned Future, not just a Pin<&mut _>.
    require!(stopped(), "cancel leaves timer or IRQ active");

    let before = regs.timer0_load_count(super::CHANNEL.index()).read().bits();
    drop(delay.delay_ms(1));
    require!(
        stopped() && regs.timer0_load_count(super::CHANNEL.index()).read().bits() == before,
        "unpolled wait changed hardware"
    );
    {
        let mut zero = core::pin::pin!(delay.delay_ns(0));
        require!(
            zero.as_mut().poll(&mut cx).is_ready(),
            "zero duration armed"
        );
    }
    require!(
        stopped() && regs.timer0_load_count(super::CHANNEL.index()).read().bits() == before,
        "zero changed registers"
    );

    {
        let mut future = core::pin::pin!(delay.delay_ms(1_000));
        require!(future.as_mut().poll(&mut cx).is_pending(), "reuse poll");
        require!(
            regs.timer0_raw_intr(super::CHANNEL.index()).read().bits() & 1 == 0,
            "long period unexpectedly expired"
        );
        timer::on_interrupt(super::CHANNEL); // deliberate empty IRQ hook
        require!(
            regs.timer0_control(super::CHANNEL.index()).read().bits() & 1 != 0,
            "empty IRQ stopped timer"
        );
        require!(
            future.as_mut().poll(&mut cx).is_pending(),
            "empty IRQ completed wait"
        );
    }
    require!(stopped(), "second cancellation");
    {
        let mut tiny = core::pin::pin!(delay.delay_ns(43));
        require!(tiny.as_mut().poll(&mut cx).is_pending(), "tiny wait");
        require!(
            regs.timer0_load_count(super::CHANNEL.index()).read().bits() == 2,
            "nanosecond rounding shortened delay"
        );
    }
    require!(stopped(), "expired-before-ISR cancellation");
    {
        let mut long = core::pin::pin!(delay.delay_ms(u32::MAX));
        require!(long.as_mut().poll(&mut cx).is_pending(), "long wait");
        require!(
            regs.timer0_load_count(super::CHANNEL.index()).read().bits() == u32::MAX,
            "long period not chunked"
        );
    }
    require!(stopped(), "long cancellation");
    uart.write(
        b"TIMER LIFECYCLE: PASS cancel=4 zero=1 unpolled=1 spurious=1 rounding=1 long=1\r\n",
    );
    true
}
