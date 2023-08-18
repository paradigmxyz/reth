use std::{ops::Mul, str::FromStr, sync::Arc};

use reth_interfaces::executor;
use reth_primitives::{Address, Block, Bytes, ChainSpec, TransactionKind, U256};

const L1_FEE_RECIPIENT: &str = "0x420000000000000000000000000000000000001A";
const BASE_FEE_RECIPIENT: &str = "0x4200000000000000000000000000000000000019";
const L1_BLOCK_CONTRACT: &str = "0x4200000000000000000000000000000000000015";

const ZERO_BYTE_COST: u64 = 4;
const NON_ZERO_BYTE_COST: u64 = 16;

/// L1 block info
///
/// We can extract L1 epoch data from each L2 block, by looking at the `setL1BlockValues`
/// transaction data. This data is then used to calculate the L1 cost of a transaction.
///
/// Here is the format of the `setL1BlockValues` transaction data:
///
/// setL1BlockValues(uint64 _number, uint64 _timestamp, uint256 _basefee, bytes32 _hash,
/// uint64 _sequenceNumber, bytes32 _batcherHash, uint256 _l1FeeOverhead, uint256 _l1FeeScalar)
///
/// For now, we only care about the fields necessary for L1 cost calculation.
pub struct L1BlockInfo {
    l1_base_fee: U256,
    l1_fee_overhead: U256,
    l1_fee_scalar: U256,
}

impl TryFrom<&Block> for L1BlockInfo {
    type Error = executor::BlockExecutionError;

    fn try_from(block: &Block) -> Result<Self, Self::Error> {
        let l1_block_contract = Address::from_str(L1_BLOCK_CONTRACT).unwrap();

        let l1_info_tx_data = block
            .body
            .iter()
            .find(|tx| matches!(tx.kind(), TransactionKind::Call(to) if to == &l1_block_contract))
            .ok_or(executor::BlockExecutionError::L1BlockInfoError {
                message: "could not find l1 block info tx in the L2 block".to_string(),
            })
            .and_then(|tx| {
                tx.input().get(4..).ok_or(executor::BlockExecutionError::L1BlockInfoError {
                    message: "could not get l1 block info tx calldata bytes".to_string(),
                })
            })?;

        // The setL1BlockValues tx calldata must be exactly 260 bytes long, considering that
        // we already removed the first 4 bytes (the function selector). Detailed breakdown:
        //   32 bytes for the block number
        // + 32 bytes for the block timestamp
        // + 32 bytes for the base fee
        // + 32 bytes for the block hash
        // + 32 bytes for the block sequence number
        // + 32 bytes for the batcher hash
        // + 32 bytes for the fee overhead
        // + 32 bytes for the fee scalar
        if l1_info_tx_data.len() != 256 {
            return Err(executor::BlockExecutionError::L1BlockInfoError {
                message: "unexpected l1 block info tx calldata length found".to_string(),
            })
        }

        let l1_base_fee = U256::try_from_be_slice(&l1_info_tx_data[64..96]).ok_or(
            executor::BlockExecutionError::L1BlockInfoError {
                message: "could not convert l1 base fee".to_string(),
            },
        )?;
        let l1_fee_overhead = U256::try_from_be_slice(&l1_info_tx_data[192..224]).ok_or(
            executor::BlockExecutionError::L1BlockInfoError {
                message: "could not convert l1 fee overhead".to_string(),
            },
        )?;
        let l1_fee_scalar = U256::try_from_be_slice(&l1_info_tx_data[224..256]).ok_or(
            executor::BlockExecutionError::L1BlockInfoError {
                message: "could not convert l1 fee scalar".to_string(),
            },
        )?;

        Ok(Self { l1_base_fee, l1_fee_overhead, l1_fee_scalar })
    }
}

impl L1BlockInfo {
    /// Calculate the gas cost of a transaction based on L1 block data posted on L2
    pub fn calculate_tx_l1_cost(
        &self,
        chain_spec: Arc<ChainSpec>,
        timestamp: u64,
        input: &Bytes,
        is_deposit: bool,
    ) -> U256 {
        let mut rollup_data_gas_cost = U256::from(input.iter().fold(0, |acc, byte| {
            acc + if *byte == 0x00 { ZERO_BYTE_COST } else { NON_ZERO_BYTE_COST }
        }));

        if is_deposit || rollup_data_gas_cost == U256::ZERO {
            return U256::ZERO
        }

        // Prior to regolith, an extra 68 non zero bytes were included in the rollup data costs.
        if !chain_spec.fork(reth_primitives::Hardfork::Regolith).active_at_timestamp(timestamp) {
            rollup_data_gas_cost += U256::from(NON_ZERO_BYTE_COST).mul(U256::from(68));
        }

        rollup_data_gas_cost
            .saturating_add(self.l1_fee_overhead)
            .saturating_mul(self.l1_base_fee)
            .saturating_mul(self.l1_fee_scalar)
            .checked_div(U256::from(1_000_000))
            .unwrap_or_default()
    }
}

/// Get the base fee recipient address
pub fn base_fee_recipient() -> Address {
    Address::from_str(BASE_FEE_RECIPIENT).unwrap()
}

/// Get the L1 cost recipient address
pub fn l1_cost_recipient() -> Address {
    Address::from_str(L1_FEE_RECIPIENT).unwrap()
}

#[cfg(test)]
mod test_l1_fee {
    #[cfg(feature = "optimism")]
    #[test]
    fn sanity_l1_block() {
        use super::*;
        use reth_primitives::{hex_literal::hex, Header, TransactionSigned};

        let bytes = hex!("7ef9015aa044bae9d41b8380d781187b426c6fe43df5fb2fb57bd4466ef6a701e1f01e015694deaddeaddeaddeaddeaddeaddeaddeaddead000194420000000000000000000000000000000000001580808408f0d18001b90104015d8eb900000000000000000000000000000000000000000000000000000000008057650000000000000000000000000000000000000000000000000000000063d96d10000000000000000000000000000000000000000000000000000000000009f35273d89754a1e0387b89520d989d3be9c37c1f32495a88faf1ea05c61121ab0d1900000000000000000000000000000000000000000000000000000000000000010000000000000000000000002d679b567db6187c0c8323fa982cfb88b74dbcc7000000000000000000000000000000000000000000000000000000000000083400000000000000000000000000000000000000000000000000000000000f4240");
        let l1_info_tx = TransactionSigned::decode_enveloped(Bytes::from(&bytes[..])).unwrap();
        let mock_block = Block {
            header: Header::default(),
            body: vec![l1_info_tx],
            ommers: Vec::default(),
            withdrawals: None,
        };

        let l1_info: L1BlockInfo = (&mock_block).try_into().unwrap();
        assert_eq!(l1_info.l1_base_fee, U256::from(652_114));
        assert_eq!(l1_info.l1_fee_overhead, U256::from(2100));
        assert_eq!(l1_info.l1_fee_scalar, U256::from(1_000_000));
    }
}
