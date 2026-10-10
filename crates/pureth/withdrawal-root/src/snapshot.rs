use crate::{
    decode_withdrawals, encode_withdrawals, WithdrawalsCodecError, WITHDRAWAL_ACTIVE_FIELDS,
};
use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::{Bytes, B256};
use reth_pureth_ssz::{merkleize_progressive, mix_in_length, RetainedNode, TreeConstructionError};
use std::fmt;
use tree_hash::TreeHash;

#[derive(Debug)]
pub struct WithdrawalSnapshot {
    withdrawals: Vec<Withdrawal>,
    serialized: Bytes,
    tree: RetainedNode,
}

impl WithdrawalSnapshot {
    pub fn build(withdrawals: Vec<Withdrawal>) -> Result<Self, WithdrawalSnapshotError> {
        let serialized =
            encode_withdrawals(&withdrawals).map_err(WithdrawalSnapshotError::Codec)?;

        let tree =
            withdrawals_tree(&withdrawals).map_err(WithdrawalSnapshotError::TreeConstruction)?;

        Ok(Self { withdrawals, serialized: Bytes::from(serialized), tree })
    }

    pub fn from_ssz(bytes: &[u8]) -> Result<Self, WithdrawalSnapshotError> {
        let withdrawals = decode_withdrawals(bytes).map_err(WithdrawalSnapshotError::Codec)?;

        Self::build(withdrawals)
    }

    pub fn withdrawals(&self) -> &[Withdrawal] {
        &self.withdrawals
    }

    pub fn serialized(&self) -> &[u8] {
        self.serialized.as_ref()
    }

    pub const fn tree(&self) -> &RetainedNode {
        &self.tree
    }

    pub const fn root(&self) -> B256 {
        self.tree.root()
    }
}

#[derive(Debug)]
pub enum WithdrawalSnapshotError {
    Codec(WithdrawalsCodecError),
    TreeConstruction(TreeConstructionError),
}

impl fmt::Display for WithdrawalSnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codec(error) => {
                write!(formatter, "withdrawal snapshot codec failed: {error}")
            }
            Self::TreeConstruction(error) => {
                write!(formatter, "withdrawal snapshot tree construction failed: {error}")
            }
        }
    }
}

impl std::error::Error for WithdrawalSnapshotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Codec(error) => Some(error),
            Self::TreeConstruction(error) => Some(error),
        }
    }
}

fn withdrawals_tree(withdrawals: &[Withdrawal]) -> Result<RetainedNode, TreeConstructionError> {
    let nodes = withdrawals.iter().map(withdrawal_tree).collect::<Result<Vec<_>, _>>()?;

    let contents = merkleize_progressive(nodes)?;

    Ok(mix_in_length(contents, withdrawals.len()))
}

fn withdrawal_tree(withdrawal: &Withdrawal) -> Result<RetainedNode, TreeConstructionError> {
    let Withdrawal { index, validator_index, address, amount } = withdrawal;

    let fields = vec![
        RetainedNode::leaf(index.tree_hash_root()),
        RetainedNode::leaf(validator_index.tree_hash_root()),
        RetainedNode::leaf(address.tree_hash_root()),
        RetainedNode::leaf(amount.tree_hash_root()),
    ];

    let contents = merkleize_progressive(fields)?;

    Ok(RetainedNode::pair(contents, RetainedNode::leaf(WITHDRAWAL_ACTIVE_FIELDS)))
}
