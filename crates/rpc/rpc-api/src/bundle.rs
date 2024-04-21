//! Additional `eth_` functions for bundles
//!
//! See also <https://docs.flashbots.net/flashbots-auction/searchers/advanced/rpc-endpoint>

use jsonrpsee::proc_macros::rpc;
use reth_primitives::{Bytes, B256};
use reth_rpc_types::{
    CancelBundleRequest, CancelPrivateTransactionRequest, EthBundleHash, EthCallBundle,
    EthCallBundleResponse, EthSendBundle, PrivateTransactionRequest,
};

/// A subset of the [EthBundleApi] API interface that only supports `eth_callBundle`.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "eth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "eth"))]
pub trait EthCallBundleApi {
    /// `eth_callBundle` can be used to simulate a bundle against a specific block number,
    /// including simulating a bundle at the top of the next block.
    #[method(name = "callBundle")]
    async fn call_bundle(
        &self,
        request: EthCallBundle,
    ) -> jsonrpsee::core::RpcResult<EthCallBundleResponse>;
}

/// The __full__ Eth bundle rpc interface.
///
/// See also <https://docs.flashbots.net/flashbots-auction/searchers/advanced/rpc-endpoint>
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "eth"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "eth"))]
pub trait EthBundleApi {
    /// `eth_sendBundle` can be used to send your bundles to the builder.
    #[method(name = "sendBundle")]
    async fn send_bundle(&self, bundle: EthSendBundle)
        -> jsonrpsee::core::RpcResult<EthBundleHash>;

    /// `eth_callBundle` can be used to simulate a bundle against a specific block number,
    /// including simulating a bundle at the top of the next block.
    #[method(name = "callBundle")]
    async fn call_bundle(
        &self,
        request: EthCallBundle,
    ) -> jsonrpsee::core::RpcResult<EthCallBundleResponse>;

    /// `eth_cancelBundle` is used to prevent a submitted bundle from being included on-chain. See [bundle cancellations](https://docs.flashbots.net/flashbots-auction/searchers/advanced/bundle-cancellations) for more information.
    #[method(name = "cancelBundle")]
    async fn cancel_bundle(&self, request: CancelBundleRequest) -> jsonrpsee::core::RpcResult<()>;

    /// `eth_sendPrivateTransaction` is used to send a single transaction to Flashbots. Flashbots will attempt to build a block including the transaction for the next 25 blocks. See [Private Transactions](https://docs.flashbots.net/flashbots-protect/additional-documentation/eth-sendPrivateTransaction) for more info.
    #[method(name = "sendPrivateTransaction")]
    async fn send_private_transaction(
        &self,
        request: PrivateTransactionRequest,
    ) -> jsonrpsee::core::RpcResult<B256>;

    /// The `eth_sendPrivateRawTransaction` method can be used to send private transactions to
    /// the RPC endpoint. Private transactions are protected from frontrunning and kept
    /// private until included in a block. A request to this endpoint needs to follow
    /// the standard eth_sendRawTransaction
    #[method(name = "sendPrivateRawTransaction")]
    async fn send_private_raw_transaction(&self, bytes: Bytes) -> jsonrpsee::core::RpcResult<B256>;

    /// The `eth_cancelPrivateTransaction` method stops private transactions from being
    /// submitted for future blocks.
    ///
    /// A transaction can only be cancelled if the request is signed by the same key as the
    /// eth_sendPrivateTransaction call submitting the transaction in first place.
    #[method(name = "cancelPrivateTransaction")]
    async fn cancel_private_transaction(
        &self,
        request: CancelPrivateTransactionRequest,
    ) -> jsonrpsee::core::RpcResult<bool>;
}
