use alloy_consensus::BlockHeader;
use alloy_eips::calc_next_block_base_fee;
use alloy_primitives::U256;
use reth_chainspec::EthChainSpec;
use reth_scroll_chainspec::{ChainConfig, ScrollChainConfig};
use reth_storage_api::{BaseFeeProvider, StorageProvider};
use scroll_alloy_evm::curie::L1_GAS_PRICE_ORACLE_ADDRESS;
use scroll_alloy_hardforks::ScrollHardforks;

/// L1 gas price oracle base fee slot.
pub const L1_BASE_FEE_SLOT: U256 = U256::from_limbs([1, 0, 0, 0]);

/// Protocol-enforced maximum L2 base fee.
pub const MAX_L2_BASE_FEE: u64 = 10_000_000_000;

/// The base fee overhead slot.
const L2_BASE_FEE_OVERHEAD_SLOT: U256 = U256::from_limbs([101, 0, 0, 0]);

/// The default base fee overhead, in case the L2 system contract isn't deployed or
/// initialized.
pub const DEFAULT_BASE_FEE_OVERHEAD: U256 = U256::from_limbs([15_680_000, 0, 0, 0]);

/// The base fee scalar slot.
const L2_BASE_FEE_SCALAR_SLOT: U256 = U256::from_limbs([102, 0, 0, 0]);

/// The default scalar applied on the L1 base fee, in case the L2 system contract isn't deployed or
/// initialized.
pub const DEFAULT_BASE_FEE_SCALAR: U256 = U256::from_limbs([34_000_000_000_000, 0, 0, 0]);

/// The precision of the L1 base fee.
pub const L1_BASE_FEE_PRECISION: U256 = U256::from_limbs([1_000_000_000_000_000_000, 0, 0, 0]);

/// The initial base fee.
const INITIAL_BASE_FEE: u64 = 10_000_000;

/// The Scroll base fee provider implementation.
#[derive(Clone, Debug, Default)]
pub struct ScrollBaseFeeProvider<ChainSpec>(ChainSpec);

impl<ChainSpec> ScrollBaseFeeProvider<ChainSpec> {
    /// Returns a new instance of a [`ScrollBaseFeeProvider`].
    pub const fn new(chain_spec: ChainSpec) -> Self {
        Self(chain_spec)
    }
}

impl<ChainSpec, P> BaseFeeProvider<P> for ScrollBaseFeeProvider<ChainSpec>
where
    ChainSpec: EthChainSpec + ScrollHardforks + ChainConfig<Config = ScrollChainConfig>,
    P: StorageProvider,
{
    fn next_block_base_fee<H: BlockHeader>(
        &self,
        provider: &mut P,
        parent_header: &H,
        ts: u64,
    ) -> Result<u64, P::Error> {
        let chain_spec = &self.0;

        // load l2 system config contract into cache.
        let system_config_contract_address =
            chain_spec.chain_config().l1_config.l2_system_config_address;
        // query scalar and overhead.
        let (mut scalar, mut overhead) = (
            provider.storage(system_config_contract_address, L2_BASE_FEE_SCALAR_SLOT)?,
            provider.storage(system_config_contract_address, L2_BASE_FEE_OVERHEAD_SLOT)?,
        );
        // if any value is 0, use the default values.
        (scalar, overhead) = (
            if scalar == U256::ZERO { DEFAULT_BASE_FEE_SCALAR } else { scalar },
            if overhead == U256::ZERO { DEFAULT_BASE_FEE_OVERHEAD } else { overhead },
        );

        let mut base_fee = if chain_spec.is_feynman_active_at_timestamp(ts) {
            feynman_base_fee(chain_spec, parent_header, ts, overhead.saturating_to())
        } else {
            let parent_l1_base_fee =
                provider.storage(L1_GAS_PRICE_ORACLE_ADDRESS, L1_BASE_FEE_SLOT)?;
            pre_feynman_base_fee(parent_l1_base_fee, scalar, overhead).saturating_to()
        };

        if base_fee > MAX_L2_BASE_FEE {
            base_fee = MAX_L2_BASE_FEE;
        }

        Ok(base_fee)
    }
}

/// Returns the Feynman base fee.
fn feynman_base_fee<H: BlockHeader, ChainSpec: EthChainSpec + ScrollHardforks>(
    chainspec: ChainSpec,
    parent_header: H,
    ts: u64,
    overhead: u64,
) -> u64 {
    let eip_1559_base_fee = if chainspec.is_feynman_active_at_timestamp(parent_header.timestamp()) {
        // extract the eip 1559 base fee from parent header by subtracting overhead from it.
        let parent_eip_1559_base_fee =
            parent_header.base_fee_per_gas().expect("Feynman active").saturating_sub(overhead);
        let base_fee_params = chainspec.base_fee_params_at_timestamp(ts);
        calc_next_block_base_fee(
            parent_header.gas_used(),
            parent_header.gas_limit(),
            parent_eip_1559_base_fee,
            base_fee_params,
        )
    } else {
        // this is the first Feynman block.
        // if the parent has a base fee, return it.
        if let Some(base_fee) = parent_header.base_fee_per_gas() {
            base_fee
        } else {
            INITIAL_BASE_FEE
        }
    };

    eip_1559_base_fee.saturating_add(overhead)
}

/// Returns the pre Feynman base fee.
fn pre_feynman_base_fee(parent_l1_base_fee: U256, scalar: U256, overhead: U256) -> U256 {
    // l1 base fee * scalar / precision + overhead.
    parent_l1_base_fee * scalar / L1_BASE_FEE_PRECISION + overhead
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::boxed::Box;

    use alloy_consensus::BlockHeader;
    use reth_scroll_chainspec::SCROLL_MAINNET;
    use revm::database::{states::plain_account::PlainStorage, EmptyDB, State};
    use scroll_alloy_hardforks::ScrollHardfork;

    const CURIE_PARAMS_TEST_CASES: [(u64, u64); 8] = [
        (0u64, 15680000u64),
        (1000000000, 15714000),
        (2000000000, 15748000),
        (100000000000, 19080000),
        (111111111111, 19457777),
        (2164000000000, 89256000),
        (644149677419355, 10000000000),
        (0x1c3c0f442u64, 15937691),
    ];

    const OVERWRITTEN_PARAMS_TEST_CASES: [(u64, u64); 7] = [
        (0, 1),
        (1000000000, 1),
        (2000000000, 1),
        (100000000000, 2),
        (111111111111, 2),
        (2164000000000, 22),
        (644149677419355, 6442),
    ];

    const EIP_1559_TEST_CASES: [(u64, u64, u64, u64); 3] = [
        (1000000000, 20000000, 10000000, 1000000000), // usage == target
        (1000000001, 20000000, 9000000, 987500001),   // usage below target
        (1000000001, 20000000, 11000000, 1012500001), // usage above target
    ];

    const CURIE_TIMESTAMP: u64 = 1719994280;

    #[test]
    fn test_should_return_correct_base_fee() -> Result<(), Box<dyn core::error::Error>> {
        // init the state db.
        let db = EmptyDB::new();
        let mut state =
            State::builder().with_database(db).with_bundle_update().without_state_clear().build();

        // init the provider and parent header.
        let base_fee_provider = ScrollBaseFeeProvider::new(SCROLL_MAINNET.clone());
        let parent_header =
            alloy_consensus::Header { timestamp: CURIE_TIMESTAMP, ..Default::default() };
        let parent_header_ts = parent_header.timestamp();

        // helper closure to insert the l1 base fee in state.
        let insert_l1_base_fee = |state: &mut State<EmptyDB>, l1_base_fee: u64| {
            let oracle_storage_pre_fork =
                PlainStorage::from_iter([(L1_BASE_FEE_SLOT, U256::from(l1_base_fee))]);
            state.insert_account_with_storage(
                L1_GAS_PRICE_ORACLE_ADDRESS,
                Default::default(),
                oracle_storage_pre_fork,
            );
        };

        // for each test case, insert the l1 base fee and check the expected value matches.
        for (l1_base_fee, expected_base_fee) in CURIE_PARAMS_TEST_CASES {
            insert_l1_base_fee(&mut state, l1_base_fee);

            // fetch base fee from db.
            let base_fee = base_fee_provider.next_block_base_fee(
                &mut state,
                &parent_header,
                parent_header_ts + 1,
            )?;
            assert_eq!(base_fee, expected_base_fee);
        }

        // insert the base fee params.
        let system_contract_storage = PlainStorage::from_iter([
            (L2_BASE_FEE_SCALAR_SLOT, U256::from(10000000)),
            (L2_BASE_FEE_OVERHEAD_SLOT, U256::ONE),
        ]);
        state.insert_account_with_storage(
            SCROLL_MAINNET.config.l1_config.l2_system_config_address,
            Default::default(),
            system_contract_storage,
        );

        // for each test case, insert the l1 base fee and check the expected value matches.
        for (l1_base_fee, expected_base_fee) in OVERWRITTEN_PARAMS_TEST_CASES {
            insert_l1_base_fee(&mut state, l1_base_fee);

            // fetch base fee from db.
            let base_fee = base_fee_provider.next_block_base_fee(
                &mut state,
                &parent_header,
                parent_header_ts + 1,
            )?;
            assert_eq!(base_fee, expected_base_fee);
        }

        // update the parent header used to activate Feynman.
        let feynman_fork_ts = SCROLL_MAINNET
            .hardforks
            .get(ScrollHardfork::Feynman)
            .unwrap()
            .as_timestamp()
            .expect("Feynman is timestamp based forked.");
        let mut parent_header =
            alloy_consensus::Header { timestamp: feynman_fork_ts + 1, ..Default::default() };
        let parent_header_ts = parent_header.timestamp();

        // for each test case, update the parent header fields and check the expected value matches.
        for (parent_base_fee, parent_gas_limit, parent_gas_used, expected_base_fee) in
            EIP_1559_TEST_CASES
        {
            parent_header.base_fee_per_gas = Some(parent_base_fee);
            parent_header.gas_limit = parent_gas_limit;
            parent_header.gas_used = parent_gas_used;

            // fetch base fee from db.
            let base_fee = base_fee_provider.next_block_base_fee(
                &mut state,
                &parent_header,
                parent_header_ts + 1,
            )?;
            assert_eq!(base_fee, expected_base_fee);
        }

        Ok(())
    }
}
