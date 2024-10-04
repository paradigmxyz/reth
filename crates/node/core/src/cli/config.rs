//! Config traits for various node components.

use alloy_primitives::Bytes;
use reth_network::protocol::IntoRlpxSubProtocol;
use reth_transaction_pool::PoolConfig;
use std::{borrow::Cow, time::Duration};

/// A trait that provides payload builder settings.
///
/// This provides all basic payload builder settings and is implemented by the
/// [`PayloadBuilderArgs`](crate::args::PayloadBuilderArgs) type.
pub trait PayloadBuilderConfig {
    /// Block extra data set by the payload builder.
    fn extradata(&self) -> Cow<'_, str>;

    /// Returns the extradata as bytes.
    fn extradata_bytes(&self) -> Bytes {
        self.extradata().as_bytes().to_vec().into()
    }

    /// The interval at which the job should build a new payload after the last.
    fn interval(&self) -> Duration;

    /// The deadline for when the payload builder job should resolve.
    fn deadline(&self) -> Duration;

    /// Target gas ceiling for built blocks.
    fn max_gas_limit(&self) -> u64;

    /// Maximum number of tasks to spawn for building a payload.
    fn max_payload_tasks(&self) -> usize;
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

impl RethNetworkConfig for reth_network::NetworkManager {
    fn add_rlpx_sub_protocol(&mut self, protocol: impl IntoRlpxSubProtocol) {
        Self::add_rlpx_sub_protocol(self, protocol);
    }

    fn secret_key(&self) -> secp256k1::SecretKey {
        self.secret_key()
    }
}

/// A trait that provides all basic config values for the transaction pool and is implemented by the
/// [`TxPoolArgs`](crate::args::TxPoolArgs) type.
pub trait RethTransactionPoolConfig {
    /// Returns transaction pool configuration.
    fn pool_config(&self) -> PoolConfig;
}
