//! Compatibility functions for rpc `Transaction` type.

use core::error;
use std::fmt;

use alloy_consensus::{BlockHeader, Sealable};
use alloy_primitives::U256;
use alloy_rpc_types_eth::{
    request::TransactionRequest, Block, BlockTransactions, BlockTransactionsKind, Header,
    TransactionInfo,
};
use reth_primitives::{NodePrimitives, Recovered, RecoveredBlock, TransactionSigned};
use reth_primitives_traits::{Block as BlockTrait, BlockBody, SealedHeader, SignedTransaction};
use serde::{Deserialize, Serialize};

/// Builds RPC transaction w.r.t. network.
pub trait TransactionCompat<
    N: NodePrimitives,
    //N: NodePrimitives<SignedTx = T>,
    //T: SignedTransaction + Clone = TransactionSigned,
>: Send + Sync + Unpin + Clone + fmt::Debug
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

    /// Wrapper for `fill()` with default `TransactionInfo`
    /// Create a new rpc transaction result for a _pending_ signed transaction, setting block
    /// environment related fields to `None`.
    fn fill_pending(&self, tx: Recovered<N::SignedTx>) -> Result<Self::Transaction, Self::Error> {
        self.fill(tx, TransactionInfo::default())
    }

    /// Create a new rpc transaction result for a mined transaction, using the given block hash,
    /// number, and tx index fields to populate the corresponding fields in the rpc result.
    ///
    /// The block hash, number, and tx index fields should be from the original block where the
    /// transaction was mined.
    fn fill(
        &self,
        tx: Recovered<N::SignedTx>,
        tx_inf: TransactionInfo,
    ) -> Result<Self::Transaction, Self::Error>;

    /// Builds a fake transaction from a transaction request for inclusion into block built in
    /// `eth_simulateV1`.
    fn build_simulate_v1_transaction(&self, request: TransactionRequest) -> Result<N::SignedTx, Self::Error>;

    /// Truncates the input of a transaction to only the first 4 bytes.
    // todo: remove in favour of using constructor on `TransactionResponse` or similar
    // <https://github.com/alloy-rs/alloy/issues/1315>.
    fn otterscan_api_truncate_input(tx: &mut Self::Transaction);

    /// Converts the given primitive block into a [`Block`] response with the given
    /// [`BlockTransactionsKind`]
    ///
    /// If a `block_hash` is provided, then this is used, otherwise the block hash is computed.
    #[expect(clippy::type_complexity)]
    fn from_block(
        &self,
        block: RecoveredBlock<N::Block>,
        kind: BlockTransactionsKind,
    ) -> Result<Block<Self::Transaction, Header<<N::Block as BlockTrait>::Header>>, Self::Error>
    {
        match kind {
            BlockTransactionsKind::Hashes => Ok(Self::from_block_with_tx_hashes(block)),
            BlockTransactionsKind::Full => self.from_block_full(block),
        }
    }

    /// Create a new [`Block`] response from a [primitive block](reth_primitives::Block), using the
    /// total difficulty to populate its field in the rpc response.
    ///
    /// This will populate the `transactions` field with only the hashes of the transactions in the
    /// block: [`BlockTransactions::Hashes`]
    fn from_block_with_tx_hashes(
        block: RecoveredBlock<N::Block>,
    ) -> Block<Self::Transaction, Header<<N::Block as BlockTrait>::Header>> {
        let transactions = block.body().transaction_hashes_iter().copied().collect();
        let rlp_length = block.rlp_length();
        let (header, body) = block.into_sealed_block().split_sealed_header_body();
        Self::from_block_with_transactions::<N::Block>(
            rlp_length,
            header,
            body,
            BlockTransactions::Hashes(transactions),
        )
    }

    /// Create a new [`Block`] response from a [primitive block](reth_primitives::Block), using the
    /// total difficulty to populate its field in the rpc response.
    ///
    /// This will populate the `transactions` field with the _full_
    /// [`TransactionCompat::Transaction`] objects: [`BlockTransactions::Full`]
    #[expect(clippy::type_complexity)]
    fn from_block_full(
        &self,
        block: RecoveredBlock<N::Block>,
    ) -> Result<Block<Self::Transaction, Header<<N::Block as BlockTrait>::Header>>, Self::Error>
    {
        let block_number = block.header().number();
        let base_fee = block.header().base_fee_per_gas();
        let block_length = block.rlp_length();
        let block_hash = Some(block.hash());

        let transactions = block
            .transactions_recovered()
            .enumerate()
            .map(|(idx, tx)| {
                let tx_info = TransactionInfo {
                    hash: Some(*tx.tx_hash()),
                    block_hash,
                    block_number: Some(block_number),
                    base_fee,
                    index: Some(idx as u64),
                };

                self.fill(tx.cloned(), tx_info)
            })
            .collect::<Result<Vec<_>, Self::Error>>()?;

        let (header, body) = block.into_sealed_block().split_sealed_header_body();
        Ok(Self::from_block_with_transactions::<N::Block>(
            block_length,
            header,
            body,
            BlockTransactions::Full(transactions),
        ))
    }

    #[inline]
    fn from_block_with_transactions<B: BlockTrait>(
        block_length: usize,
        header: SealedHeader<B::Header>,
        body: B::Body,
        transactions: BlockTransactions<Self::Transaction>,
    ) -> Block<Self::Transaction, Header<B::Header>> {
        let withdrawals =
            header.withdrawals_root().is_some().then(|| body.withdrawals().cloned()).flatten();

        let uncles =
            body.ommers().map(|o| o.iter().map(|h| h.hash_slow()).collect()).unwrap_or_default();
        let header = Header::from_consensus(header.into(), None, Some(U256::from(block_length)));

        Block { header, uncles, transactions, withdrawals }
    }
}
