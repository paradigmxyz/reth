//! Test network configuration

use std::time::Duration;

// L1 Configuration
/// Default L1 chain ID for testing
pub const DEFAULT_L1_CHAIN_ID: u64 = 1337;
/// Default L1 network URL for testing
pub const DEFAULT_L1_NETWORK_URL: &str = "http://localhost:8545";

// L2 Configuration
/// Default L2 chain ID for testing
pub const DEFAULT_L2_CHAIN_ID: u64 = 196;
/// Default L2 sequencer URL for testing
pub const DEFAULT_L2_SEQ_URL: &str = "http://localhost:8123";
/// Default L2 RPC node URL for testing
pub const DEFAULT_L2_NETWORK_URL: &str = "http://localhost:8124";
/// Default L2 op-rbuilder URL for testing
pub const DEFAULT_L2_BUILDER_URL: &str = "http://localhost:8125";

/// Default L2 metrics Prometheus URL for testing
pub const DEFAULT_L2_METRICS_PROMETHEUS_URL: &str =
    "http://127.0.0.1:9092/debug/metrics/prometheus";
/// Default L2 metrics URL for testing
pub const DEFAULT_L2_METRICS_URL: &str = "http://127.0.0.1:9092/debug/metrics";

/// Default Bridge address for testing
pub const BRIDGE_ADDR: &str = "0x4B24266C13AFEf2bb60e2C69A4C08A482d81e3CA";

/// Default timeout for a transaction to be mined for testing
pub const DEFAULT_TIMEOUT_TX_TO_BE_MINED: Duration = Duration::from_secs(60);

/// Default rich address for testing
pub const DEFAULT_RICH_ADDRESS: &str = "0x14dC79964da2C08b23698B3D3cc7Ca32193d9955";
/// Default rich private key for testing
pub const DEFAULT_RICH_PRIVATE_KEY: &str =
    "0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356";

/// Default L2 new account 1 address for testing
pub const DEFAULT_L2_NEW_ACC1_ADDRESS: &str = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC";
/// Default L2 new account 1 private key for testing
pub const DEFAULT_L2_NEW_ACC1_PRIVATE_KEY: &str =
    "5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";

/// Default L2 new account 2 address for testing
pub const DEFAULT_L2_NEW_ACC2_ADDRESS: &str = "0xAed6892D56AAB5DA8FBcd85b924C3bE63c74Cc29";
/// Default L2 new account 2 private key for testing
pub const DEFAULT_L2_NEW_ACC2_PRIVATE_KEY: &str =
    "bc362a16d3dedd6cdba639eb8fa91b2f6d9f929eb490ca2e5a748ba041c6a131";

/// Default OkPay sender address for testing
pub const DEFAULT_OK_PAY_SENDER_ADDRESS: &str = "0xa0Ee7A142d267C1f36714E4a8F75612F20a79720";
/// Default OkPay sender private key for testing
pub const DEFAULT_OK_PAY_SENDER_PRIVATE_KEY: &str =
    "2a871d0798f97d79848a013d4936a73bf4cc922c825d33c1cf7073dff6d409c6";
