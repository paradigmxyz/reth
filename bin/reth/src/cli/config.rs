//! Config traits for various node components.

use reth_revm::primitives::bytes::BytesMut;
use reth_rlp::Encodable;
use reth_rpc_builder::EthConfig;
use std::{borrow::Cow, time::Duration};

/// A trait that provides configured RPC server.
///
/// This provides all basic config values for the RPC server and is implemented by the
/// [RpcServerArgs](crate::args::RpcServerArgs) type.
pub trait RethRpcConfig {
    /// Returns whether ipc is enabled.
    fn is_ipc_enabled(&self) -> bool;

    /// The configured ethereum RPC settings.
    fn eth_config(&self) -> EthConfig;

    // TODO extract more functions from RpcServerArgs
}

/// A trait that provides payload builder settings.
///
/// This provides all basic payload builder settings and is implemented by the
/// [PayloadBuilderArgs](crate::args::PayloadBuilderArgs) type.
pub trait PayloadBuilderConfig {
    /// Block extra data set by the payload builder.
    fn extradata(&self) -> Cow<'_, str>;

    /// Returns the rlp-encoded extradata bytes.
    fn extradata_rlp_bytes(&self) -> reth_primitives::bytes::Bytes {
        let mut extradata = BytesMut::new();
        self.extradata().as_bytes().encode(&mut extradata);
        extradata.freeze()
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
