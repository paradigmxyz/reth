//! Config traits for various node components.

use alloy_eips::eip1559::{ETHEREUM_BLOCK_GAS_LIMIT_30M, ETHEREUM_BLOCK_GAS_LIMIT_36M};
use alloy_primitives::Bytes;
use reth_chainspec::{Chain, ChainKind, NamedChain};
use reth_network::{protocol::IntoRlpxSubProtocol, NetworkPrimitives};
use reth_transaction_pool::PoolConfig;
use std::{borrow::Cow, time::Duration};

/// A trait that provides payload builder settings.
///
/// This provides all basic payload builder settings and is implemented by the
/// [`PayloadBuilderArgs`](crate::args::PayloadBuilderArgs) type.
pub trait PayloadBuilderConfig {
    /// Block extra data set by the payload builder.
    fn extra_data(&self) -> Cow<'_, str>;

    /// Returns the extra data as bytes.
    fn extra_data_bytes(&self) -> Bytes {
        self.extra_data().as_bytes().to_vec().into()
    }

    /// The interval at which the job should build a new payload after the last.
    fn interval(&self) -> Duration;

    /// The deadline for when the payload builder job should resolve.
    fn deadline(&self) -> Duration;

    /// Target gas limit for built blocks.
    fn gas_limit(&self) -> Option<u64>;

    /// Maximum number of tasks to spawn for building a payload.
    fn max_payload_tasks(&self) -> usize;

    /// Returns the configured gas limit if set, or a chain-specific default.
    fn gas_limit_for(&self, chain: Chain) -> u64 {
        if let Some(limit) = self.gas_limit() {
            return limit;
        }

        match chain.kind() {
            ChainKind::Named(NamedChain::Sepolia | NamedChain::Holesky | NamedChain::Hoodi) => {
                ETHEREUM_BLOCK_GAS_LIMIT_30M
            }
            ChainKind::Named(NamedChain::Mainnet) => ETHEREUM_BLOCK_GAS_LIMIT_30M,
            _ => ETHEREUM_BLOCK_GAS_LIMIT_36M,
        }
    }
}

/// A trait that represents the configured network and can be used to apply additional configuration
/// to the network.
pub trait RethNetworkConfig {
    /// Adds a new additional protocol to the `RLPx` sub-protocol list.
    ///
    /// These additional protocols are negotiated during the `RLPx` handshake.
    /// If both peers share the same protocol, the corresponding handler will be included alongside
    /// the `eth` protocol.
    ///
    /// See also [`ProtocolHandler`](reth_network::protocol::ProtocolHandler)
    fn add_rlpx_sub_protocol(&mut self, protocol: impl IntoRlpxSubProtocol);

    /// Returns the secret key used for authenticating sessions.
    fn secret_key(&self) -> secp256k1::SecretKey;

    // TODO add more network config methods here
}

impl<N: NetworkPrimitives> RethNetworkConfig for reth_network::NetworkManager<N> {
    fn add_rlpx_sub_protocol(&mut self, protocol: impl IntoRlpxSubProtocol) {
        Self::add_rlpx_sub_protocol(self, protocol);
    }

    fn secret_key(&self) -> secp256k1::SecretKey {
        Self::secret_key(self)
    }
}

/// A trait that provides all basic config values for the transaction pool and is implemented by the
/// [`TxPoolArgs`](crate::args::TxPoolArgs) type.
pub trait RethTransactionPoolConfig {
    /// Returns transaction pool configuration.
    fn pool_config(&self) -> PoolConfig;

    /// Returns max batch size for transaction batch insertion.
    fn max_batch_size(&self) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{borrow::Cow, time::Duration};

    #[derive(Default)]
    struct DummyConfig;

    impl PayloadBuilderConfig for DummyConfig {
        fn extra_data(&self) -> Cow<'_, str> {
            Cow::Borrowed("")
        }

        fn interval(&self) -> Duration {
            Duration::from_secs(1)
        }

        fn deadline(&self) -> Duration {
            Duration::from_secs(1)
        }

        fn gas_limit(&self) -> Option<u64> {
            None
        }

        fn max_payload_tasks(&self) -> usize {
            1
        }
    }

    #[derive(Default)]
    struct OverrideConfig(u64);

    impl PayloadBuilderConfig for OverrideConfig {
        fn extra_data(&self) -> Cow<'_, str> {
            Cow::Borrowed("")
        }

        fn interval(&self) -> Duration {
            Duration::from_secs(1)
        }

        fn deadline(&self) -> Duration {
            Duration::from_secs(1)
        }

        fn gas_limit(&self) -> Option<u64> {
            Some(self.0)
        }

        fn max_payload_tasks(&self) -> usize {
            1
        }
    }

    #[test]
    fn default_gas_limits_match_chain_expectations() {
        let cfg = DummyConfig;

        assert_eq!(
            cfg.gas_limit_for(Chain::from_named(NamedChain::Mainnet)),
            ETHEREUM_BLOCK_GAS_LIMIT_30M
        );
        assert_eq!(
            cfg.gas_limit_for(Chain::from_named(NamedChain::Sepolia)),
            ETHEREUM_BLOCK_GAS_LIMIT_30M
        );
        assert_eq!(
            cfg.gas_limit_for(Chain::from_named(NamedChain::Holesky)),
            ETHEREUM_BLOCK_GAS_LIMIT_30M
        );
        assert_eq!(
            cfg.gas_limit_for(Chain::from_named(NamedChain::Hoodi)),
            ETHEREUM_BLOCK_GAS_LIMIT_30M
        );
        assert_eq!(cfg.gas_limit_for(Chain::from(1337u64)), ETHEREUM_BLOCK_GAS_LIMIT_36M);
    }

    #[test]
    fn explicit_gas_limit_is_respected() {
        let cfg = OverrideConfig(42);
        assert_eq!(cfg.gas_limit_for(Chain::from_named(NamedChain::Mainnet)), 42);
    }
}
