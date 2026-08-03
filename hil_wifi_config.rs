//! Non-production credentials shared by the two-board WS63 HIL fixtures.

pub const SSID: &[u8] = b"WS63-RUST-HIL";
pub const PASSPHRASE: &[u8] = b"ws63-rust-hil";
#[allow(dead_code)] // Consumed only by the AP half of the shared fixture.
pub const CHANNEL: u8 = 6;
