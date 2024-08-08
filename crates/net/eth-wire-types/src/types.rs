//! Types and traits for networking types abstraction

use alloy_rlp::{Decodable, Encodable};
use reth_consensus::ConsensusError;
use reth_primitives::{
    alloy_primitives::{Sealable, Sealed},
    GotExpected, Header, SealedBlock,
};
use std::{fmt::Debug, hash::Hash};

/// A trait defining types used in networking.
pub trait NetworkTypes: Send + Sync + Debug + Clone + Unpin + 'static {
    /// The block type. Expected to hold header and body.
    type Block: Block<Header = Self::BlockHeader>;
    /// The block body type.
    type BlockBody: BlockBody;
    /// The block header type.
    type BlockHeader: BlockHeader;

    /// Helper conversion method building [`SealedBlock`] from sealed header and body.
    fn build_sealed_block(header: Sealed<Self::BlockHeader>, body: Self::BlockBody) -> SealedBlock;

    /// Helper conversion method splitting [`SealedBlock`] into sealed header and body.
    fn split_sealed_block(block: SealedBlock) -> (Sealed<Self::BlockHeader>, Self::BlockBody);

    /// Validates block body against header.
    fn validate_block(
        header: &Self::BlockHeader,
        body: &Self::BlockBody,
    ) -> Result<(), ConsensusError>;
}

/// Block
pub trait Block:
    Encodable + Decodable + Debug + PartialEq + Eq + Clone + Unpin + Send + Sync
{
    /// Block header type.
    type Header: BlockHeader;

    /// Returns block header
    fn header(&self) -> &Self::Header;
}

/// Block header
pub trait BlockHeader:
    Send
    + Sync
    + Encodable
    + Decodable
    + Debug
    + Sealable
    + Unpin
    + Clone
    + PartialEq
    + Eq
    + Hash
    + Into<Header>
    + From<Header>
{
    /// Block number.
    fn number(&self) -> u64;

    /// Returns whether the block is empty.
    fn is_empty(&self) -> bool;

    /// Size of the header in memory
    fn size(&self) -> usize;
}

/// Block body
pub trait BlockBody:
    Encodable
    + Decodable
    + Debug
    + Unpin
    + Send
    + Sync
    + PartialEq
    + Eq
    + From<reth_primitives::BlockBody>
    + Into<reth_primitives::BlockBody>
{
    /// Body size used for collecting metrics.
    fn size(&self) -> usize;
}

/// Default networking types
#[derive(Debug, Clone)]
pub struct PrimitiveNetworkTypes;

impl NetworkTypes for PrimitiveNetworkTypes {
    type Block = reth_primitives::Block;
    type BlockBody = reth_primitives::BlockBody;
    type BlockHeader = reth_primitives::Header;

    fn build_sealed_block(header: Sealed<Self::BlockHeader>, body: Self::BlockBody) -> SealedBlock {
        SealedBlock::new(header.into(), body)
    }

    fn split_sealed_block(block: SealedBlock) -> (Sealed<Self::BlockHeader>, Self::BlockBody) {
        let (header, body) = block.split_header_body();
        (header.into(), body)
    }

    fn validate_block(
        header: &Self::BlockHeader,
        body: &Self::BlockBody,
    ) -> Result<(), ConsensusError> {
        let ommers_hash = body.calculate_ommers_root();
        if header.ommers_hash != ommers_hash {
            return Err(ConsensusError::BodyOmmersHashDiff(
                GotExpected { got: ommers_hash, expected: header.ommers_hash }.into(),
            ))
        }

        let tx_root = body.calculate_tx_root();
        if header.transactions_root != tx_root {
            return Err(ConsensusError::BodyTransactionRootDiff(
                GotExpected { got: tx_root, expected: header.transactions_root }.into(),
            ))
        }

        match (header.withdrawals_root, &body.withdrawals) {
            (Some(header_withdrawals_root), Some(withdrawals)) => {
                let withdrawals = withdrawals.as_slice();
                let withdrawals_root =
                    reth_primitives::proofs::calculate_withdrawals_root(withdrawals);
                if withdrawals_root != header_withdrawals_root {
                    return Err(ConsensusError::BodyWithdrawalsRootDiff(
                        GotExpected { got: withdrawals_root, expected: header_withdrawals_root }
                            .into(),
                    ))
                }
            }
            (None, None) => {
                // this is ok because we assume the fork is not active in this case
            }
            _ => return Err(ConsensusError::WithdrawalsRootUnexpected),
        }

        match (header.requests_root, &body.requests) {
            (Some(header_requests_root), Some(requests)) => {
                let requests = requests.0.as_slice();
                let requests_root = reth_primitives::proofs::calculate_requests_root(requests);
                if requests_root != header_requests_root {
                    return Err(ConsensusError::BodyRequestsRootDiff(
                        GotExpected { got: requests_root, expected: header_requests_root }.into(),
                    ))
                }
            }
            (None, None) => {
                // this is ok because we assume the fork is not active in this case
            }
            _ => return Err(ConsensusError::RequestsRootUnexpected),
        }

        Ok(())
    }
}

impl BlockHeader for reth_primitives::Header {
    fn number(&self) -> u64 {
        self.number
    }

    fn is_empty(&self) -> bool {
        self.is_empty()
    }

    fn size(&self) -> usize {
        self.size()
    }
}

impl Block for reth_primitives::Block {
    type Header = reth_primitives::Header;

    fn header(&self) -> &Self::Header {
        &self.header
    }
}

impl BlockBody for reth_primitives::BlockBody {
    fn size(&self) -> usize {
        self.size()
    }
}
