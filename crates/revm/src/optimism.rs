use std::str::FromStr;

use reth_interfaces::executor;
use reth_primitives::{Address, Block, TransactionKind, TransactionSigned, U256};

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

        // The setL1BlockValues tx calldata must be exactly 184 bytes long, considering that
        // we already removed the first 4 bytes (the function selector). Detailed breakdown:
        //   8  bytes for the block number
        // + 8  bytes for the block timestamp
        // + 32 bytes for the base fee
        // + 32 bytes for the block hash
        // + 8  bytes for the block sequence number
        // + 32 bytes for the batcher hash
        // + 32 bytes for the fee overhead
        // + 32 bytes for the fee scalar
        if l1_info_tx_data.len() != 184 {
            return Err(executor::BlockExecutionError::L1BlockInfoError {
                message: "unexpected l1 block info tx calldata length found".to_string(),
            })
        }

        let l1_base_fee = U256::try_from_le_slice(&l1_info_tx_data[16..48]).ok_or(
            executor::BlockExecutionError::L1BlockInfoError {
                message: "could not convert l1 base fee".to_string(),
            },
        )?;
        let l1_fee_overhead = U256::try_from_le_slice(&l1_info_tx_data[120..152]).ok_or(
            executor::BlockExecutionError::L1BlockInfoError {
                message: "could not convert l1 fee overhead".to_string(),
            },
        )?;
        let l1_fee_scalar = U256::try_from_le_slice(&l1_info_tx_data[152..184]).ok_or(
            executor::BlockExecutionError::L1BlockInfoError {
                message: "could not convert l1 fee scalar".to_string(),
            },
        )?;

        Ok(Self { l1_base_fee, l1_fee_overhead, l1_fee_scalar })
    }
}

impl L1BlockInfo {
    /// Calculate the gas cost of a transaction based on L1 block data posted on L2
    pub fn calculate_tx_l1_cost(&mut self, tx: &TransactionSigned) -> U256 {
        let rollup_data_gas_cost = U256::from(tx.input().iter().fold(0, |acc, byte| {
            acc + if *byte == 0x00 { ZERO_BYTE_COST } else { NON_ZERO_BYTE_COST }
        }));

        if tx.is_deposit() || rollup_data_gas_cost == U256::ZERO {
            return U256::ZERO
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
