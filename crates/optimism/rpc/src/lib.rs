//! OP-Reth RPC support.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(all(not(test), feature = "optimism"), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
// The `optimism` feature must be enabled to use this crate.
#![cfg(feature = "optimism")]

pub mod error;
pub mod eth;
pub mod sequencer;

pub use error::{OpEthApiError, OpInvalidTransactionError, SequencerClientError};
pub use eth::{OpEthApi, OpReceiptBuilder};
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use op_alloy_rpc_types_engine::{ProtocolVersion, SuperchainSignal};
pub use sequencer::SequencerClient;

/// Engine API extension for Optimism superchain signaling
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "engine"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "engine"))]
pub trait OpEngineApiExt {
    /// Signal superchain v1 message
    ///
    /// The execution engine SHOULD warn when the recommended version is newer than the current
    /// version. The execution engine SHOULD take safety precautions if it does not meet
    /// the required version.
    ///
    /// # Returns
    /// The latest supported OP-Stack protocol version of the execution engine.
    ///
    /// See: <https://specs.optimism.io/protocol/exec-engine.html#engine_signalsuperchainv1>
    #[method(name = "signalSuperchainV1")]
    async fn signal_superchain_v1(&self, signal: SuperchainSignal) -> RpcResult<ProtocolVersion>;
}
