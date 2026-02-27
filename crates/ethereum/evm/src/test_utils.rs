use crate::EthEvmConfig;
use reth_evm::noop::NoopEvmConfig;

/// A helper type alias for mocked block executor provider.
pub type MockExecutorProvider = MockEvmConfig;

/// Mock for EVM config.
pub type MockEvmConfig = NoopEvmConfig<EthEvmConfig>;
