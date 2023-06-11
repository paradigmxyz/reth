use std::str::FromStr;

use reth_primitives::{Address, TransactionSigned, U256};
use revm::db::DatabaseRef;

const L1_BLOCK_CONTRACT: &str = "0x4200000000000000000000000000000000000015";
const L1_BASE_FEE_SLOT: u64 = 1;
const OVERHEAD_SLOT: u64 = 5;
const SCALAR_SLOT: u64 = 6;
const ZERO_BYTE_COST: u64 = 4;
const NON_ZERO_BYTE_COST: u64 = 16;

/// Optimism Gas Cost Oracle
///
/// This struct is used to calculate the gas cost of a transaction on the L1 chain.
/// The L1 block contract on L2 is used to propagate changes to the gas cost over time,
/// so we need to fetch it from the Reth database.
///
/// To make this more efficient, we cache the values and only fetch them when the block
/// number changes.
#[derive(Default)]
pub struct OptimismGasCostOracle {
    /// The cached block number
    pub block_number: Option<u64>,
    /// The base fee of the L1 chain.
    pub l1_base_fee: U256,
    /// The overhead value used to calculate the gas cost.
    pub overhead: U256,
    /// The scalar value used to calculate the gas cost.
    pub scalar: U256,
}

impl OptimismGasCostOracle {
    /// Calculate the gas cost of a transaction based on L1 block data posted on L2
    pub fn calculate_l1_cost<DB: DatabaseRef>(
        &mut self,
        db: &mut DB,
        block_num: u64,
        tx: TransactionSigned,
    ) -> Result<Option<U256>, DB::Error> {
        let rollup_data_gas_cost = U256::from(tx.input().iter().fold(0, |acc, byte| {
            acc + if byte == &0x00 { ZERO_BYTE_COST } else { NON_ZERO_BYTE_COST }
        }));

        if tx.is_deposit() || rollup_data_gas_cost == U256::ZERO {
            return Ok(None)
        }

        if self.block_number.cmp(&Some(block_num)).is_ne() {
            let l1_block_address = Address::from_str(L1_BLOCK_CONTRACT).unwrap();
            self.l1_base_fee = db.storage(l1_block_address, U256::from(L1_BASE_FEE_SLOT))?;
            self.overhead = db.storage(l1_block_address, U256::from(OVERHEAD_SLOT))?;
            self.scalar = db.storage(l1_block_address, U256::from(SCALAR_SLOT))?;
        }

        Ok(rollup_data_gas_cost
            .saturating_add(self.overhead)
            .saturating_mul(self.l1_base_fee)
            .saturating_mul(self.scalar)
            .checked_div(U256::from(1_000_000)))
    }
}
