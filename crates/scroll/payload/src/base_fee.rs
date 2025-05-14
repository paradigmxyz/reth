use alloy_primitives::U256;
use revm::{database::State, Database};
use scroll_alloy_evm::curie::L1_GAS_PRICE_ORACLE_ADDRESS;

/// L1 gas price oracle base fee slot.
pub const L1_BASE_FEE_SLOT: U256 = U256::from_limbs([1, 0, 0, 0]);

/// Protocol-enforced maximum L2 base fee.
pub const MAX_L2_BASE_FEE: U256 = U256::from_limbs([10_000_000_000, 0, 0, 0]);

/// The base fee overhead.
pub const L1_BASE_FEE_OVERHEAD: U256 = U256::from_limbs([15_680_000, 0, 0, 0]);

/// The scalar applied on the L1 base fee.
pub const L1_BASE_FEE_SCALAR: U256 = U256::from_limbs([34_000_000_000_000, 0, 0, 0]);

/// The precision of the L1 base fee.
pub const L1_BASE_FEE_PRECISION: U256 = U256::from_limbs([1_000_000_000_000_000_000, 0, 0, 0]);

/// An instance of the trait can return the current base fee for block building.
pub trait PayloadBuildingBaseFeeProvider {
    /// The error type.
    type Error;

    /// Returns the base fee for block building.
    fn payload_building_base_fee(&mut self) -> Result<U256, Self::Error>;
}

impl<DB> PayloadBuildingBaseFeeProvider for State<DB>
where
    DB: Database,
{
    type Error = DB::Error;

    fn payload_building_base_fee(&mut self) -> Result<U256, Self::Error> {
        // load account into cache.
        let _ = self.load_cache_account(L1_GAS_PRICE_ORACLE_ADDRESS)?;

        // query storage.
        let parent_l1_base_fee = self.storage(L1_GAS_PRICE_ORACLE_ADDRESS, L1_BASE_FEE_SLOT)?;

        // l1 base fee * scalar / precision + overhead.
        let mut base_fee =
            parent_l1_base_fee * L1_BASE_FEE_SCALAR / L1_BASE_FEE_PRECISION + L1_BASE_FEE_OVERHEAD;

        if base_fee > MAX_L2_BASE_FEE {
            base_fee = MAX_L2_BASE_FEE;
        }

        Ok(base_fee)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::boxed::Box;

    use reth_revm::db::states::plain_account::PlainStorage;
    use revm::database::EmptyDB;

    #[test]
    fn test_should_return_correct_base_fee() -> Result<(), Box<dyn core::error::Error>> {
        let db = EmptyDB::new();
        let mut state =
            State::builder().with_database(db).with_bundle_update().without_state_clear().build();

        let test_values = [
            (0u64, 15680000u64),
            (1000000000, 15714000),
            (2000000000, 15748000),
            (100000000000, 19080000),
            (111111111111, 19457777),
            (2164000000000, 89256000),
            (644149677419355, 10000000000),
            (0x1c3c0f442u64, 15937691),
        ];

        for (l1_base_fee, expected_base_fee) in test_values {
            // insert the l1 base fee.
            let oracle_storage_pre_fork =
                PlainStorage::from_iter([(U256::from(1), U256::from(l1_base_fee))]);
            state.insert_account_with_storage(
                L1_GAS_PRICE_ORACLE_ADDRESS,
                Default::default(),
                oracle_storage_pre_fork,
            );

            // fetch base fee from db.
            let base_fee = state.payload_building_base_fee()?;
            let expected_base_fee = U256::from(expected_base_fee);
            assert_eq!(base_fee, expected_base_fee);
        }

        Ok(())
    }
}
