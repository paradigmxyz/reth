//! Compatibility functions for rpc `Transaction` type.

use core::error;
use std::fmt;

use alloy_rpc_types_eth::{request::TransactionRequest, TransactionInfo};
use reth_primitives::{RecoveredTx, TransactionSigned};
use serde::{Deserialize, Serialize};

/// Create a new rpc transaction result for a mined transaction, using the given block hash,
/// number, and tx index fields to populate the corresponding fields in the rpc result.
///
/// The block hash, number, and tx index fields should be from the original block where the
/// transaction was mined.
pub fn from_recovered_with_block_context<Tx, T: TransactionCompat<Tx>>(
    tx: RecoveredTx<Tx>,
    tx_info: TransactionInfo,
    resp_builder: &T,
) -> Result<T::Transaction, T::Error> {
    resp_builder.fill(tx, tx_info)
}

/// Create a new rpc transaction result for a _pending_ signed transaction, setting block
/// environment related fields to `None`.
pub fn from_recovered<Tx, T: TransactionCompat<Tx>>(
    tx: RecoveredTx<Tx>,
    resp_builder: &T,
) -> Result<T::Transaction, T::Error> {
    resp_builder.fill(tx, TransactionInfo::default())
}

/// Builds RPC transaction w.r.t. network.
pub trait TransactionCompat<T = TransactionSigned>:
    Send + Sync + Unpin + Clone + fmt::Debug
{
    /// RPC transaction response type.
    type Transaction: Serialize
        + for<'de> Deserialize<'de>
        + Send
        + Sync
        + Unpin
        + Clone
        + fmt::Debug;

    /// RPC transaction error type.
    type Error: error::Error + Into<jsonrpsee_types::ErrorObject<'static>>;

    /// Create a new rpc transaction result for a _pending_ signed transaction, setting block
    /// environment related fields to `None`.
    fn fill(
        &self,
        tx: RecoveredTx<T>,
        tx_inf: TransactionInfo,
    ) -> Result<Self::Transaction, Self::Error>;

    /// Builds a fake transaction from a transaction request for inclusion into block built in
    /// `eth_simulateV1`.
    fn build_simulate_v1_transaction(&self, request: TransactionRequest) -> Result<T, Self::Error>;

    /// Truncates the input of a transaction to only the first 4 bytes.
    // todo: remove in favour of using constructor on `TransactionResponse` or similar
    // <https://github.com/alloy-rs/alloy/issues/1315>.
    fn otterscan_api_truncate_input(tx: &mut Self::Transaction);
}

/// Convert [`RecoveredTx`] to [`TransactionRequest`]
pub fn transaction_to_call_request<T: alloy_consensus::Transaction>(
    tx: RecoveredTx<T>,
) -> TransactionRequest {
    let from = tx.signer();
    TransactionRequest::from_transaction_with_sender(tx.into_signed(), from)
}
