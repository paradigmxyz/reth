//! Types for broadcasting new data.
use crate::{EthMessage, EthVersion};
use reth_codecs::derive_arbitrary;
use reth_primitives::{Block, TransactionSigned, H256, U128};
use reth_rlp::{RlpDecodable, RlpDecodableWrapper, RlpEncodable, RlpEncodableWrapper};
use std::sync::Arc;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// This informs peers of new blocks that have appeared on the network.
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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

/// A new block with the current total difficulty, which includes the difficulty of the returned
/// block.
#[derive_arbitrary(rlp, 25)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct NewBlock {
    /// A new block.
    pub block: Block,
    /// The current total difficulty.
    pub td: U128,
}

/// This informs peers of transactions that have appeared on the network and are not yet included
/// in a block.
#[derive_arbitrary(rlp, 10)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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

/// A wrapper type for all different new pooled transaction types
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewPooledTransactionHashes {
    /// A list of transaction hashes valid for [66-68)
    Eth66(NewPooledTransactionHashes66),
    /// A list of transaction hashes valid from [68..]
    ///
    /// Note: it is assumed that the payload is valid (all vectors have the same length)
    Eth68(NewPooledTransactionHashes68),
}

// === impl NewPooledTransactionHashes ===

impl NewPooledTransactionHashes {
    /// Returns `true` if the payload is valid for the given version
    pub fn is_valid_for_version(&self, version: EthVersion) -> bool {
        match self {
            NewPooledTransactionHashes::Eth66(_) => {
                matches!(version, EthVersion::Eth67 | EthVersion::Eth66)
            }
            NewPooledTransactionHashes::Eth68(_) => {
                matches!(version, EthVersion::Eth68)
            }
        }
    }

    /// Returns an iterator over all transaction hashes.
    pub fn iter_hashes(&self) -> impl Iterator<Item = &H256> + '_ {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => msg.0.iter(),
            NewPooledTransactionHashes::Eth68(msg) => msg.hashes.iter(),
        }
    }

    /// Consumes the type and returns all hashes
    pub fn into_hashes(self) -> Vec<H256> {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => msg.0,
            NewPooledTransactionHashes::Eth68(msg) => msg.hashes,
        }
    }

    /// Returns an iterator over all transaction hashes.
    pub fn into_iter_hashes(self) -> impl Iterator<Item = H256> {
        match self {
            NewPooledTransactionHashes::Eth66(msg) => msg.0.into_iter(),
            NewPooledTransactionHashes::Eth68(msg) => msg.hashes.into_iter(),
        }
    }
}

impl From<NewPooledTransactionHashes> for EthMessage {
    fn from(value: NewPooledTransactionHashes) -> Self {
        match value {
            NewPooledTransactionHashes::Eth66(msg) => EthMessage::NewPooledTransactionHashes66(msg),
            NewPooledTransactionHashes::Eth68(msg) => EthMessage::NewPooledTransactionHashes68(msg),
        }
    }
}

impl From<NewPooledTransactionHashes66> for NewPooledTransactionHashes {
    fn from(hashes: NewPooledTransactionHashes66) -> Self {
        Self::Eth66(hashes)
    }
}

impl From<NewPooledTransactionHashes68> for NewPooledTransactionHashes {
    fn from(hashes: NewPooledTransactionHashes68) -> Self {
        Self::Eth68(hashes)
    }
}

/// This informs peers of transaction hashes for transactions that have appeared on the network,
/// but have not been included in a block.
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct NewPooledTransactionHashes66(
    /// Transaction hashes for new transactions that have appeared on the network.
    /// Clients should request the transactions with the given hashes using a
    /// [`GetPooledTransactions`](crate::GetPooledTransactions) message.
    pub Vec<H256>,
);

impl From<Vec<H256>> for NewPooledTransactionHashes66 {
    fn from(v: Vec<H256>) -> Self {
        NewPooledTransactionHashes66(v)
    }
}

/// Same as [`NewPooledTransactionHashes66`] but extends that that beside the transaction hashes,
/// the node sends the transaction types and their sizes (as defined in EIP-2718) as well.
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct NewPooledTransactionHashes68 {
    /// Transaction types for new transactions that have appeared on the network.
    pub types: Vec<u8>,
    /// Transaction sizes for new transactions that have appeared on the network.
    pub sizes: Vec<usize>,
    /// Transaction hashes for new transactions that have appeared on the network.
    pub hashes: Vec<H256>,
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
