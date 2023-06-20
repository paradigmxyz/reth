use std::str::FromStr;

use reth_interfaces::executor::Error;
use reth_primitives::{Address, TransactionSigned, U256};
use reth_provider::StateProvider;
use reth_revm::database::SubState;
use revm::db::DatabaseRef;

const L1_FEE_RECIPIENT: &str = "0x420000000000000000000000000000000000001A";
const BASE_FEE_RECIPIENT: &str = "0x4200000000000000000000000000000000000019";
const L1_BLOCK_CONTRACT: &str = "0x4200000000000000000000000000000000000015";

const L1_BASE_FEE_SLOT: u64 = 1;
const OVERHEAD_SLOT: u64 = 5;
const SCALAR_SLOT: u64 = 6;
const ZERO_BYTE_COST: u64 = 4;
const NON_ZERO_BYTE_COST: u64 = 16;

/// Optimism Gas Cost Oracle
///
/// This is used to calculate the gas cost of a transaction on the L1 chain.
/// The L1 block contract on L2 is used to propagate changes to the gas cost over time,
/// so we need to fetch this data from the DB.
///
/// To make this slightly more efficient when executing entire blocks, the values are cached and
/// only fetched again when the block number changes.
#[derive(Default)]
pub struct L1GasCostOracle {
    /// The cached block number
    block_number: Option<u64>,
    /// The base fee of the L1 chain
    l1_base_fee: U256,
    /// The overhead value used to calculate the gas cost
    overhead: U256,
    /// The scalar value used to calculate the gas cost
    scalar: U256,
}

impl L1GasCostOracle {
    /// Calculate the gas cost of a transaction based on L1 block data posted on L2
    pub fn calculate_l1_cost<DB: DatabaseRef>(
        &mut self,
        db: &mut DB,
        block_num: u64,
        tx: &TransactionSigned,
    ) -> Result<U256, DB::Error> {
        let rollup_data_gas_cost = U256::from(tx.input().iter().fold(0, |acc, byte| {
            acc + if byte == &0x00 { ZERO_BYTE_COST } else { NON_ZERO_BYTE_COST }
        }));

        if tx.is_deposit() || rollup_data_gas_cost == U256::ZERO {
            return Ok(U256::ZERO)
        }

        if self.block_number.cmp(&Some(block_num)).is_ne() {
            let l1_block_address = Address::from_str(L1_BLOCK_CONTRACT).unwrap();

            // TODO: Check: Is the db data always up-to-date?
            self.l1_base_fee = db.storage(l1_block_address, U256::from(L1_BASE_FEE_SLOT))?;
            self.overhead = db.storage(l1_block_address, U256::from(OVERHEAD_SLOT))?;
            self.scalar = db.storage(l1_block_address, U256::from(SCALAR_SLOT))?;
        }

        Ok(rollup_data_gas_cost
            .saturating_add(self.overhead)
            .saturating_mul(self.l1_base_fee)
            .saturating_mul(self.scalar)
            .checked_div(U256::from(1_000_000))
            .unwrap_or_default())
    }
}

/// Get the L1 fee recipient address
pub fn get_l1_fee_recipient() -> Address {
    Address::from_str(L1_FEE_RECIPIENT).unwrap()
}

/// Get the base fee recipient address
pub fn get_base_fee_recipient() -> Address {
    Address::from_str(BASE_FEE_RECIPIENT).unwrap()
}

/// Route any fee to a recipient account
pub fn route_fee_to_vault<DB: StateProvider>(
    db: &mut SubState<DB>,
    fee: u64,
    gas_used: u64,
    recipient: Address,
) -> Result<(), Error> {
    let mut account = db.load_account(recipient).map_err(|_| Error::ProviderError)?;
    let amount_to_send = U256::from(fee.saturating_add(gas_used));
    account.info.balance = account.info.balance.saturating_add(amount_to_send);
    Ok(())
}
