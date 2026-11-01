//! Bounded model-only observation; never changes timer deadlines or task policy.
use core::cell::RefCell;
use critical_section::Mutex;
use hisi_hal::{peripherals::Uart0, uart::Uart};

#[derive(Clone, Copy)]
struct Event {
    kind: u8,
    ms: u32,
    value: u32,
}
struct Trace {
    events: [Event; 256],
    len: usize,
    enabled: bool,
    lost: u32,
}
static TRACE: Mutex<RefCell<Trace>> = Mutex::new(RefCell::new(Trace {
    events: [Event {
        kind: 0,
        ms: 0,
        value: 0,
    }; 256],
    len: 0,
    enabled: true,
    lost: 0,
}));

pub fn record(kind: u8, value: u32) {
    let ms = u32::try_from(super::monotonic_ms()).expect("model timeline exceeds u32 milliseconds");
    critical_section::with(|cs| {
        let mut trace = TRACE.borrow(cs).borrow_mut();
        if !trace.enabled {
            return;
        }
        let index = trace.len;
        if index < trace.events.len() {
            trace.events[index] = Event { kind, ms, value };
            trace.len += 1;
        } else {
            trace.lost += 1;
        }
    });
}

pub fn dump(uart: &Uart<'_, Uart0<'_>>) {
    let (len, lost) = critical_section::with(|cs| {
        let mut trace = TRACE.borrow(cs).borrow_mut();
        trace.enabled = false;
        (trace.len, trace.lost)
    });
    for index in 0..len {
        let event = critical_section::with(|cs| TRACE.borrow(cs).borrow().events[index]);
        uart.write(b"TRACE ");
        uart.write(&[event.kind]);
        uart.write(b" ");
        super::write_u32(uart, event.ms);
        uart.write(b" ");
        super::write_u32(uart, event.value);
        uart.write(b"\r\n");
    }
    uart.write(b"TRACE lost=");
    super::write_u32(uart, lost);
    uart.write(b"\r\n");
}
