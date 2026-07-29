use embassy_time::Duration;
use hisi_rf::{BackendTimeout, OperationTimeout, RadioConfig, WifiConfig, WorkBudget};

pub const SCAN_RESULT_DEPTH: usize = 32;
pub const RUNNER_BUDGET: WorkBudget =
    WorkBudget::try_new(8, 100_000).expect("non-zero incremental work budget");

pub const SCAN_OPERATION_TIMEOUT: OperationTimeout =
    OperationTimeout::try_from_millis(15_000).expect("non-zero scan operation timeout");
pub const CONNECT_OPERATION_TIMEOUT: OperationTimeout =
    OperationTimeout::try_from_millis(60_000).expect("non-zero connect operation timeout");

pub const INITIALIZE_WAIT_DEADLINE: Duration = Duration::from_secs(35);
pub const SCAN_WAIT_DEADLINE: Duration = Duration::from_secs(30);
pub const CONNECT_WAIT_DEADLINE: Duration = Duration::from_secs(90);
pub const EVENT_WAIT_DEADLINE: Duration = Duration::from_secs(2);

/// Public DNS targets used to prove routed UDP connectivity.
pub const PUBLIC_DNS_TARGETS: [[u8; 4]; 2] = [[223, 5, 5, 5], [180, 76, 76, 76]];

pub const TEST_SSID: &[u8] = match option_env!("WS63_WIFI_SSID") {
    Some(value) => value.as_bytes(),
    None => b"",
};
pub const TEST_PASSPHRASE: &[u8] = match option_env!("WS63_WIFI_PASSPHRASE") {
    Some(value) => value.as_bytes(),
    None => b"",
};

pub fn radio_config() -> RadioConfig {
    let mut config = RadioConfig::default();
    config.wifi = WifiConfig {
        initialize_timeout: BackendTimeout::try_from_millis(30_000)
            .expect("non-zero backend initialize timeout"),
        disconnect_timeout: BackendTimeout::try_from_millis(10_000)
            .expect("non-zero backend disconnect timeout"),
    };
    config
}
