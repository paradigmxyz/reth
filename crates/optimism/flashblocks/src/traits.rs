//! Generic traits for flashblock payloads.
//!
//! These traits enable chain-specific flashblock implementations while sharing
//! the core flashblock infrastructure.

use alloy_consensus::crypto::RecoveryError;
use alloy_eips::eip4895::Withdrawals;
use alloy_primitives::{Bloom, Bytes, B256};
use alloy_rpc_types_engine::PayloadId;

/// Base payload information for constructing block environment.
///
/// Contains all fields needed to configure EVM execution context for the next block.
/// This is present only on the first flashblock (index 0) of a sequence.
pub trait FlashblockPayloadBase: Clone + Send + Sync + std::fmt::Debug + 'static {
    /// Parent block hash.
    fn parent_hash(&self) -> B256;
    /// Block number being built.
    fn block_number(&self) -> u64;
    /// Block timestamp.
    fn timestamp(&self) -> u64;
}

/// State diff from flashblock execution.
///
/// Contains the cumulative state changes from executing transactions in this flashblock.
pub trait FlashblockDiff: Clone + Send + Sync + std::fmt::Debug + 'static {
    /// Block hash after applying this flashblock.
    fn block_hash(&self) -> B256;
    /// State root after applying this flashblock.
    fn state_root(&self) -> B256;
    /// Cumulative gas used.
    fn gas_used(&self) -> u64;
    /// Bloom filter for logs.
    fn logs_bloom(&self) -> &Bloom;
    /// Receipts root.
    fn receipts_root(&self) -> B256;
    /// Raw encoded transactions in this flashblock.
    fn transactions_raw(&self) -> &[Bytes];

    /// Withdrawals included in this flashblock.
    fn withdrawals(&self) -> Option<&Withdrawals> {
        None
    }

    /// Withdrawals root.
    fn withdrawals_root(&self) -> Option<B256> {
        None
    }
}

/// A flashblock payload representing one slice of a block.
///
/// Flashblocks are incremental updates to block state, allowing for faster
/// pre-confirmations. A complete block is built from a sequence of flashblocks.
pub trait FlashblockPayload:
    Clone + Send + Sync + std::fmt::Debug + for<'de> serde::Deserialize<'de> + 'static
{
    /// The base payload type containing block environment configuration.
    type Base: FlashblockPayloadBase;
    /// The diff type containing state changes.
    type Diff: FlashblockDiff;
    /// The signed transaction type for this chain.
    type SignedTx: reth_primitives_traits::SignedTransaction;

    /// Sequential index of this flashblock within the current block's sequence.
    fn index(&self) -> u64;

    /// Unique identifier for the payload being built.
    fn payload_id(&self) -> PayloadId;

    /// Base payload (only present on index 0).
    fn base(&self) -> Option<Self::Base>;

    /// State diff for this flashblock.
    fn diff(&self) -> &Self::Diff;

    /// Block number this flashblock belongs to.
    fn block_number(&self) -> u64 {
        self.base().map(|b| b.block_number()).unwrap_or(0)
    }

    /// Recovers transactions from the raw transaction bytes in this flashblock.
    ///
    /// Each item is a result containing either the recovered transaction with its encoding,
    /// or an error if decoding/recovery failed.
    fn recover_transactions(
        &self,
    ) -> impl Iterator<Item = Result<alloy_eips::eip2718::WithEncoded<reth_primitives_traits::Recovered<Self::SignedTx>>, RecoveryError>>;
}
