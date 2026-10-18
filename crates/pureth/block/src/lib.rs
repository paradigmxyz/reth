#![allow(missing_docs, rustdoc::missing_crate_level_docs)]
#![forbid(unsafe_code)]

use alloy_primitives::{Address, Bytes, B256, U256};
use reth_pureth_ssz::{merkleize_progressive, mix_in_length, RetainedNode, TreeConstructionError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GasAmounts {
    pub regular: u64,
    pub blob: u64,
}

impl GasAmounts {
    pub fn to_ssz_bytes(self) -> [u8; 16] {
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&self.regular.to_le_bytes());
        bytes[8..].copy_from_slice(&self.blob.to_le_bytes());
        bytes
    }

    pub fn tree(self) -> Result<RetainedNode, TreeConstructionError> {
        let Self { regular, blob } = self;
        active_container(vec![uint64(regular), uint64(blob)], 0x03)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlobFeesPerGas {
    pub regular: U256,
    pub blob: U256,
}

impl BlobFeesPerGas {
    pub fn to_ssz_bytes(self) -> [u8; 64] {
        let mut bytes = [0; 64];
        bytes[..32].copy_from_slice(&self.regular.to_le_bytes::<32>());
        bytes[32..].copy_from_slice(&self.blob.to_le_bytes::<32>());
        bytes
    }

    pub fn tree(self) -> Result<RetainedNode, TreeConstructionError> {
        let Self { regular, blob } = self;
        active_container(
            vec![
                RetainedNode::leaf(B256::from(regular.to_le_bytes::<32>())),
                RetainedNode::leaf(B256::from(blob.to_le_bytes::<32>())),
            ],
            0x03,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionSummary {
    pub parent_hash: B256,
    pub miner: Address,
    pub state_root: B256,
    pub transactions_root: B256,
    pub receipts_root: B256,
    pub number: u64,
    pub gas_limits: GasAmounts,
    pub gas_used: GasAmounts,
    pub timestamp: u64,
    pub extra_data: Bytes,
    pub mix_hash: B256,
    pub base_fees_per_gas: BlobFeesPerGas,
    pub withdrawals_root: B256,
    pub excess_gas: GasAmounts,
    pub parent_beacon_block_root: B256,
    pub requests_root: B256,
    pub block_access_list_root: B256,
    pub slot_number: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BlockCommitmentError {
    ExtraDataTooLong,
    GasUsedExceedsLimit,
    UnsupportedRegularExcess,
    Tree(TreeConstructionError),
}

impl From<TreeConstructionError> for BlockCommitmentError {
    fn from(error: TreeConstructionError) -> Self {
        Self::Tree(error)
    }
}

impl ExecutionSummary {
    pub fn tree(&self) -> Result<RetainedNode, BlockCommitmentError> {
        let Self {
            parent_hash,
            miner,
            state_root,
            transactions_root,
            receipts_root,
            number,
            gas_limits,
            gas_used,
            timestamp,
            extra_data,
            mix_hash,
            base_fees_per_gas,
            withdrawals_root,
            excess_gas,
            parent_beacon_block_root,
            requests_root,
            block_access_list_root,
            slot_number,
        } = self;
        if extra_data.len() > 32 {
            return Err(BlockCommitmentError::ExtraDataTooLong);
        }
        if gas_used.regular > gas_limits.regular || gas_used.blob > gas_limits.blob {
            return Err(BlockCommitmentError::GasUsedExceedsLimit);
        }
        if excess_gas.regular != 0 {
            return Err(BlockCommitmentError::UnsupportedRegularExcess);
        }
        let extra = mix_in_length(
            RetainedNode::leaf(B256::right_padding_from(extra_data)),
            extra_data.len(),
        );
        Ok(active_container(
            vec![
                RetainedNode::leaf(*parent_hash),
                RetainedNode::leaf(B256::right_padding_from(miner.as_slice())),
                RetainedNode::leaf(*state_root),
                RetainedNode::leaf(*transactions_root),
                RetainedNode::leaf(*receipts_root),
                uint64(*number),
                gas_limits.tree()?,
                gas_used.tree()?,
                uint64(*timestamp),
                extra,
                RetainedNode::leaf(*mix_hash),
                base_fees_per_gas.tree()?,
                RetainedNode::leaf(*withdrawals_root),
                excess_gas.tree()?,
                RetainedNode::leaf(*parent_beacon_block_root),
                RetainedNode::leaf(*requests_root),
                RetainedNode::leaf(*block_access_list_root),
                uint64(*slot_number),
            ],
            0x03ffff,
        )?)
    }
}

fn uint64(value: u64) -> RetainedNode {
    RetainedNode::leaf(B256::right_padding_from(&value.to_le_bytes()))
}

fn active_container(
    fields: Vec<RetainedNode>,
    active_fields: u32,
) -> Result<RetainedNode, TreeConstructionError> {
    Ok(RetainedNode::pair(
        merkleize_progressive(fields)?,
        RetainedNode::leaf(B256::right_padding_from(&active_fields.to_le_bytes())),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::b256;

    fn summary() -> ExecutionSummary {
        ExecutionSummary {
            parent_hash: B256::repeat_byte(1),
            miner: Address::repeat_byte(2),
            state_root: B256::repeat_byte(3),
            transactions_root: B256::repeat_byte(4),
            receipts_root: B256::repeat_byte(5),
            number: 6,
            gas_limits: GasAmounts { regular: 30_000_000, blob: 786_432 },
            gas_used: GasAmounts { regular: 21_000, blob: 131_072 },
            timestamp: 9,
            extra_data: Bytes::from_static(&[10, 11]),
            mix_hash: B256::repeat_byte(11),
            base_fees_per_gas: BlobFeesPerGas { regular: U256::MAX, blob: U256::from(12) },
            withdrawals_root: B256::repeat_byte(13),
            excess_gas: GasAmounts { regular: 0, blob: 14 },
            parent_beacon_block_root: B256::repeat_byte(15),
            requests_root: B256::repeat_byte(16),
            block_access_list_root: B256::repeat_byte(17),
            slot_number: 18,
        }
    }

    #[test]
    fn gas_and_fees_preserve_full_width_little_endian_bytes() {
        let gas = GasAmounts { regular: 1, blob: u64::MAX };
        assert_eq!(&gas.to_ssz_bytes()[..8], &1_u64.to_le_bytes());
        assert_eq!(&gas.to_ssz_bytes()[8..], &[0xff; 8]);
        let fees = BlobFeesPerGas { regular: U256::MAX, blob: U256::from(1) << 255 };
        assert_eq!(&fees.to_ssz_bytes()[..32], &[0xff; 32]);
        assert_eq!(fees.to_ssz_bytes()[63], 0x80);
        assert!(fees.to_ssz_bytes()[32..63].iter().all(|byte| *byte == 0));
        for tree in [gas.tree().unwrap(), fees.tree().unwrap()] {
            assert_eq!(tree.children().unwrap()[1].root(), B256::right_padding_from(&[3]));
        }
        assert_eq!(
            gas.tree().unwrap().root(),
            b256!("9f9a1cf07aff54f3197a2cf6f38a6c9d4f96dc1edd352b69d00cdb66e9c7b7e4")
        );
        assert_eq!(
            fees.tree().unwrap().root(),
            b256!("e1e98c0ea24eb951bece89b1deb89c40bf1d574cb58f23c80293731c35decb16")
        );
        assert_ne!(
            gas.tree().unwrap().root(),
            GasAmounts { regular: u64::MAX, blob: 1 }.tree().unwrap().root()
        );
    }

    #[test]
    fn summary_keeps_all_eighteen_positions_and_active_bits() {
        let summary = summary();
        let tree = summary.tree().unwrap();
        assert_eq!(
            tree.root(),
            b256!("77cd9751aa6a2ed05e728b9a96037f4f3c57eb1946900fb06b7140bc9c60d35b")
        );
        let mut mask = [0; 32];
        mask[..3].copy_from_slice(&[0xff, 0xff, 3]);
        assert_eq!(tree.children().unwrap()[1].root(), B256::from(mask));
        let changes: [fn(&mut ExecutionSummary); 18] = [
            |value| value.parent_hash[0] ^= 1,
            |value| value.miner[0] ^= 1,
            |value| value.state_root[0] ^= 1,
            |value| value.transactions_root[0] ^= 1,
            |value| value.receipts_root[0] ^= 1,
            |value| value.number += 1,
            |value| value.gas_limits.regular += 1,
            |value| value.gas_used.blob += 1,
            |value| value.timestamp += 1,
            |value| value.extra_data = Bytes::from_static(&[10, 11, 0]),
            |value| value.mix_hash[0] ^= 1,
            |value| value.base_fees_per_gas.blob += U256::from(1),
            |value| value.withdrawals_root[0] ^= 1,
            |value| value.excess_gas.blob += 1,
            |value| value.parent_beacon_block_root[0] ^= 1,
            |value| value.requests_root[0] ^= 1,
            |value| value.block_access_list_root[0] ^= 1,
            |value| value.slot_number += 1,
        ];
        for (field, change) in changes.into_iter().enumerate() {
            let mut changed = summary.clone();
            change(&mut changed);
            assert_ne!(tree.root(), changed.tree().unwrap().root(), "field {field}");
        }
        for length in [0, 1, 31, 32] {
            let mut changed = summary.clone();
            changed.extra_data = Bytes::from(vec![0; length]);
            assert_ne!(tree.root(), changed.tree().unwrap().root());
        }
    }

    #[test]
    fn summary_rejects_invalid_experimental_inputs() {
        let mut value = summary();
        value.extra_data = Bytes::from(vec![0; 33]);
        assert_eq!(value.tree().unwrap_err(), BlockCommitmentError::ExtraDataTooLong);
        value = summary();
        value.gas_used.regular = value.gas_limits.regular + 1;
        assert_eq!(value.tree().unwrap_err(), BlockCommitmentError::GasUsedExceedsLimit);
        value = summary();
        value.gas_used.blob = value.gas_limits.blob + 1;
        assert_eq!(value.tree().unwrap_err(), BlockCommitmentError::GasUsedExceedsLimit);
        value = summary();
        value.excess_gas.regular = 1;
        assert_eq!(value.tree().unwrap_err(), BlockCommitmentError::UnsupportedRegularExcess);
        value = summary();
        value.gas_used = value.gas_limits;
        assert!(value.tree().is_ok());
    }
}
