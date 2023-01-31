//! Types for broadcasting new data.
use reth_codecs::derive_arbitrary;
use reth_primitives::{Header, TransactionSigned, H256, U128};
use reth_rlp::{RlpDecodable, RlpDecodableWrapper, RlpEncodable, RlpEncodableWrapper};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// This informs peers of new blocks that have appeared on the network.
#[derive_arbitrary(rlp)]
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    RlpEncodableWrapper,
    RlpDecodableWrapper,
    Serialize,
    Deserialize,
    Default,
)]
pub struct NewBlockHashes(
    /// New block hashes and the block number for each blockhash.
    /// Clients should request blocks using a [`GetBlockBodies`](crate::GetBlockBodies) message.
    pub Vec<BlockHashNumber>,
);

// === impl NewBlockHashes ===

impl NewBlockHashes {
    /// Returns the latest block in the list of blocks.
    pub fn latest(&self) -> Option<&BlockHashNumber> {
        self.0.iter().fold(None, |latest, block| {
            if let Some(latest) = latest {
                return if latest.number > block.number { Some(latest) } else { Some(block) }
            }
            Some(block)
        })
    }
}

/// A block hash _and_ a block number.
#[derive_arbitrary(rlp)]
#[derive(
    Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Serialize, Deserialize, Default,
)]
pub struct BlockHashNumber {
    /// The block hash
    pub hash: H256,
    /// The block number
    pub number: u64,
}

impl From<Vec<BlockHashNumber>> for NewBlockHashes {
    fn from(v: Vec<BlockHashNumber>) -> Self {
        NewBlockHashes(v)
    }
}

impl From<NewBlockHashes> for Vec<BlockHashNumber> {
    fn from(v: NewBlockHashes) -> Self {
        v.0
    }
}

/// A block body, including transactions and uncle headers.
#[derive_arbitrary(rlp, 25)]
#[derive(
    Debug, Clone, PartialEq, Eq, Default, RlpEncodable, RlpDecodable, Serialize, Deserialize,
)]
pub struct RawBlockBody {
    /// This block's header
    pub header: Header,
    /// Transactions in this block.
    pub transactions: Vec<TransactionSigned>,
    /// Uncle block headers.
    pub ommers: Vec<Header>,
}

/// A new block with the current total difficulty, which includes the difficulty of the returned
/// block.
#[derive_arbitrary(rlp, 25)]
#[derive(
    Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Serialize, Deserialize, Default,
)]
pub struct NewBlock {
    /// A new block.
    pub block: RawBlockBody,
    /// The current total difficulty.
    pub td: U128,
}

/// This informs peers of transactions that have appeared on the network and are not yet included
/// in a block.
#[derive_arbitrary(rlp, 10)]
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    RlpEncodableWrapper,
    RlpDecodableWrapper,
    Serialize,
    Deserialize,
    Default,
)]
pub struct Transactions(
    /// New transactions for the peer to include in its mempool.
    pub Vec<TransactionSigned>,
);

impl From<Vec<TransactionSigned>> for Transactions {
    fn from(txs: Vec<TransactionSigned>) -> Self {
        Transactions(txs)
    }
}

impl From<Transactions> for Vec<TransactionSigned> {
    fn from(txs: Transactions) -> Self {
        txs.0
    }
}

/// Same as [`Transactions`] but this is intended as egress message send from local to _many_ peers.
///
/// The list of transactions is constructed on per-peers basis, but the underlying transaction
/// objects are shared.
#[derive_arbitrary(rlp, 20)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper)]
pub struct SharedTransactions(
    /// New transactions for the peer to include in its mempool.
    pub Vec<Arc<TransactionSigned>>,
);

/// This informs peers of transaction hashes for transactions that have appeared on the network,
/// but have not been included in a block.
#[derive_arbitrary(rlp)]
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    RlpEncodableWrapper,
    RlpDecodableWrapper,
    Serialize,
    Deserialize,
    Default,
)]
pub struct NewPooledTransactionHashes(
    /// Transaction hashes for new transactions that have appeared on the network.
    /// Clients should request the transactions with the given hashes using a
    /// [`GetPooledTransactions`](crate::GetPooledTransactions) message.
    pub Vec<H256>,
);

impl From<Vec<H256>> for NewPooledTransactionHashes {
    fn from(v: Vec<H256>) -> Self {
        NewPooledTransactionHashes(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_return_latest_block() {
        let mut blocks = NewBlockHashes(vec![BlockHashNumber { hash: H256::random(), number: 0 }]);
        let latest = blocks.latest().unwrap();
        assert_eq!(latest.number, 0);

        blocks.0.push(BlockHashNumber { hash: H256::random(), number: 100 });
        blocks.0.push(BlockHashNumber { hash: H256::random(), number: 2 });
        let latest = blocks.latest().unwrap();
        assert_eq!(latest.number, 100);
    }
}
