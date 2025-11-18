//! Default configuration constants for testing

use alloy_primitives::Address;
use std::time::Duration;

// L1 Configuration

/// Default L1 network URL for testing
pub const DEFAULT_L1_NETWORK_URL: &str = "http://localhost:8545";
/// Default L1 chain ID for testing
pub const DEFAULT_L1_CHAIN_ID: u64 = 1337;
/// Default L1 admin address for testing
pub const DEFAULT_L1_ADMIN_ADDRESS: &str = "0x8f8E2d6cF621f30e9a11309D6A56A876281Fd534";
/// Default L1 admin private key for testing
pub const DEFAULT_L1_ADMIN_PRIVATE_KEY: &str =
    "0x815405dddb0e2a99b12af775fd2929e526704e1d1aea6a0b4e74dc33e2f7fcd2";

// L2 Configuration

/// Default L2 network URL for testing
pub const DEFAULT_L2_NETWORK_URL: &str = "http://localhost:8124";
/// Default L2 sequencer URL for testing
pub const DEFAULT_L2_SEQ_URL: &str = "http://localhost:8123";
/// Default L2 chain ID for testing
pub const DEFAULT_L2_CHAIN_ID: u64 = 196;

// L2 Metrics

/// Default L2 Prometheus metrics URL
pub const DEFAULT_L2_METRICS_PROMETHEUS_URL: &str =
    "http://127.0.0.1:9092/debug/metrics/prometheus";
/// Default L2 metrics URL
pub const DEFAULT_L2_METRICS_URL: &str = "http://127.0.0.1:9092/debug/metrics";

// Bridge Address

/// Bridge contract address
pub const BRIDGE_ADDR: &str = "0x4B24266C13AFEf2bb60e2C69A4C08A482d81e3CA";

// Timeouts

/// Default timeout for waiting for transactions to be mined
pub const DEFAULT_TIMEOUT_TX_TO_BE_MINED: Duration = Duration::from_secs(60);

// L2 Admin Account

/// Default L2 admin address
pub const DEFAULT_L2_ADMIN_ADDRESS: &str = "0x8f8E2d6cF621f30e9a11309D6A56A876281Fd534";
/// Default L2 admin private key
pub const DEFAULT_L2_ADMIN_PRIVATE_KEY: &str =
    "0x815405dddb0e2a99b12af775fd2929e526704e1d1aea6a0b4e74dc33e2f7fcd2";

// Rich Account (for testing)

/// Default rich test account address
pub const DEFAULT_RICH_ADDRESS: &str = "0x14dC79964da2C08b23698B3D3cc7Ca32193d9955";
/// Default rich test account private key
pub const DEFAULT_RICH_PRIVATE_KEY: &str =
    "0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356";

// Test Account 1

/// Default L2 test account 1 address
pub const DEFAULT_L2_NEW_ACC1_ADDRESS: &str = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC";
/// Default L2 test account 1 private key
pub const DEFAULT_L2_NEW_ACC1_PRIVATE_KEY: &str =
    "5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";

// Test Account 2

/// Default L2 test account 2 address
pub const DEFAULT_L2_NEW_ACC2_ADDRESS: &str = "0xAed6892D56AAB5DA8FBcd85b924C3bE63c74Cc29";
/// Default L2 test account 2 private key
pub const DEFAULT_L2_NEW_ACC2_PRIVATE_KEY: &str =
    "bc362a16d3dedd6cdba639eb8fa91b2f6d9f929eb490ca2e5a748ba041c6a131";

// OkPay Sender Account

/// Default OkPay sender address
pub const DEFAULT_OK_PAY_SENDER_ADDRESS: &str = "0xa0Ee7A142d267C1f36714E4a8F75612F20a79720";
/// Default OkPay sender private key
pub const DEFAULT_OK_PAY_SENDER_PRIVATE_KEY: &str =
    "2a871d0798f97d79848a013d4936a73bf4cc922c825d33c1cf7073dff6d409c6";

/// Helper function to parse address from string constant
pub fn parse_address(addr: &str) -> Address {
    addr.parse().expect("Invalid address constant")
}
