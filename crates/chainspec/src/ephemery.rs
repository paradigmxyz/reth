//! Ephemery testnet support
//!
//! Ephemery is a self-resetting testnet that regenerates every 28 days.
//! Chain ID and genesis timestamp are derived from calendar time.
//! EIP-6916: <https://eips.ethereum.org/EIPS/eip-6916>
//! Genesis parameters: <https://github.com/ephemery-testnet/ephemery-genesis/blob/master/values.env>

#[cfg(feature = "std")]
use alloc::vec;
#[cfg(feature = "std")]
use alloy_eips::{eip7840::BlobParams, eip7892::BlobScheduleBlobParams};
#[cfg(feature = "std")]
use alloy_genesis::Genesis;
#[cfg(feature = "std")]
use alloy_primitives::U256;
#[cfg(feature = "std")]
use reth_ethereum_forks::{ChainHardforks, EthereumHardfork, ForkCondition, Hardfork};

/// Ephemery's anchor genesis timestamp (iteration 0)
#[cfg(feature = "std")]
pub(crate) const EPHEMERY_GENESIS_ANCHOR: u64 = 1393527600;

/// Period length in seconds (28 days)
#[cfg(feature = "std")]
pub(crate) const PERIOD_IN_SECONDS: u64 = 28 * 24 * 60 * 60;

/// Base chain ID for Ephemery (iteration 0's chain ID)
pub(crate) const EPHEMERY_BASE_CHAIN_ID: u64 = 39438000;

/// Upper bound on Ephemery iteration count (approx. 767 years)
pub(crate) const EPHEMERY_ITERATION_UPPER_BOUND: u64 = 10_000;

/// BPO1 activation offset from genesis timestamp
#[cfg(feature = "std")]
pub(crate) const BPO1_OFFSET: u64 = 787032;

/// BPO2 activation offset from genesis timestamp
#[cfg(feature = "std")]
pub(crate) const BPO2_OFFSET: u64 = 1573464;

/// Computes the Ephemery period, chain ID and genesis timestamp for a given
/// wall-clock time.
#[cfg(feature = "std")]
pub(crate) fn ephemery_period_at(now: u64) -> (u64, u64, u64) {
    let period = now.saturating_sub(EPHEMERY_GENESIS_ANCHOR) / PERIOD_IN_SECONDS;
    debug_assert!(period < EPHEMERY_ITERATION_UPPER_BOUND);

    let chain_id = EPHEMERY_BASE_CHAIN_ID + period;
    let genesis_timestamp = EPHEMERY_GENESIS_ANCHOR + (period * PERIOD_IN_SECONDS);

    (period, chain_id, genesis_timestamp)
}

/// Computes the current Ephemery period from wall-clock time.
/// Returns (period, `chain_id`, `genesis_timestamp`).
#[cfg(feature = "std")]
pub(crate) fn current_ephemery_period() -> (u64, u64, u64) {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_secs();

    ephemery_period_at(now)
}

/// Patches the genesis with the current period's timestamp.
#[cfg(feature = "std")]
pub(crate) fn ephemery_genesis(mut genesis: Genesis) -> Genesis {
    let (_, _, genesis_timestamp) = current_ephemery_period();
    genesis.timestamp = genesis_timestamp;
    genesis
}

/// Checks whether a chain ID falls within the Ephemery range.
/// Each iteration increments the chain ID by one from the base.
pub(crate) const fn is_ephemery_chain_id(chain_id: u64) -> bool {
    chain_id >= EPHEMERY_BASE_CHAIN_ID &&
        chain_id < EPHEMERY_BASE_CHAIN_ID + EPHEMERY_ITERATION_UPPER_BOUND
}

/// Builds the hardfork schedule for Ephemery.
///
/// All pre-merge forks activate at block 0, all post-merge forks
/// activate at genesis timestamp (0 offset), except bpo forks
/// which are genesis-relative offsets.
#[cfg(feature = "std")]
pub(crate) fn ephemery_hardforks(genesis_timestamp: u64) -> ChainHardforks {
    ChainHardforks::new(vec![
        (EthereumHardfork::Frontier.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Homestead.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Tangerine.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::SpuriousDragon.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Byzantium.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Constantinople.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Petersburg.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Istanbul.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::Berlin.boxed(), ForkCondition::Block(0)),
        (EthereumHardfork::London.boxed(), ForkCondition::Block(0)),
        (
            EthereumHardfork::Paris.boxed(),
            ForkCondition::TTD {
                activation_block_number: 0,
                total_difficulty: U256::ZERO,
                fork_block: None,
            },
        ),
        (EthereumHardfork::Shanghai.boxed(), ForkCondition::Timestamp(0)),
        (EthereumHardfork::Cancun.boxed(), ForkCondition::Timestamp(0)),
        (EthereumHardfork::Prague.boxed(), ForkCondition::Timestamp(0)),
        (EthereumHardfork::Osaka.boxed(), ForkCondition::Timestamp(0)),
        (EthereumHardfork::Amsterdam.boxed(), ForkCondition::Timestamp(0)),
        (EthereumHardfork::Bpo1.boxed(), ForkCondition::Timestamp(genesis_timestamp + BPO1_OFFSET)),
        (EthereumHardfork::Bpo2.boxed(), ForkCondition::Timestamp(genesis_timestamp + BPO2_OFFSET)),
    ])
}

/// Builds the blob params schedule for Ephemery.
#[cfg(feature = "std")]
pub(crate) fn ephemery_blob_params(genesis_timestamp: u64) -> BlobScheduleBlobParams {
    BlobScheduleBlobParams {
        cancun: BlobParams::cancun(),
        prague: BlobParams::prague(),
        osaka: BlobParams::osaka(),
        ..Default::default()
    }
    .with_scheduled([
        (
            genesis_timestamp + BPO1_OFFSET,
            BlobParams {
                target_blob_count: 8,
                max_blob_count: 12,
                update_fraction: 6676955,
                ..BlobParams::osaka()
            },
        ),
        (
            genesis_timestamp + BPO2_OFFSET,
            BlobParams {
                target_blob_count: 10,
                max_blob_count: 15,
                update_fraction: 8346193,
                ..BlobParams::osaka()
            },
        ),
    ])
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    fn ephemery_scheduled_at(params: &BlobScheduleBlobParams, offset: u64) -> BlobParams {
        params
            .scheduled
            .iter()
            .find(|(ts, _)| *ts == EPHEMERY_GENESIS_ANCHOR + offset)
            .map(|(_, p)| *p)
            .expect("scheduled blob params missing")
    }

    #[test]
    fn ephemery_period_zero_at_anchor() {
        let (period, chain_id, genesis_timestamp) = ephemery_period_at(EPHEMERY_GENESIS_ANCHOR);
        assert_eq!(period, 0);
        assert_eq!(chain_id, EPHEMERY_BASE_CHAIN_ID);
        assert_eq!(genesis_timestamp, EPHEMERY_GENESIS_ANCHOR);
    }

    #[test]
    fn ephemery_chain_id_increments_each_period() {
        let (period, chain_id, _) = ephemery_period_at(EPHEMERY_GENESIS_ANCHOR + PERIOD_IN_SECONDS);
        assert_eq!(period, 1);
        assert_eq!(chain_id, 39438001);
    }

    #[test]
    fn ephemery_genesis_timestamp_snaps_to_period_boundary() {
        // mid-period timestamps round down to the period's start
        let (period, _, genesis_timestamp) =
            ephemery_period_at(EPHEMERY_GENESIS_ANCHOR + PERIOD_IN_SECONDS + 12345);
        assert_eq!(period, 1);
        assert_eq!(genesis_timestamp, EPHEMERY_GENESIS_ANCHOR + PERIOD_IN_SECONDS);
    }

    #[test]
    fn ephemery_period_saturates_below_anchor() {
        let (period, chain_id, genesis_timestamp) = ephemery_period_at(0);
        assert_eq!(period, 0);
        assert_eq!(chain_id, EPHEMERY_BASE_CHAIN_ID);
        assert_eq!(genesis_timestamp, EPHEMERY_GENESIS_ANCHOR);
    }

    #[test]
    fn is_ephemery_chain_id_in_range() {
        assert!(is_ephemery_chain_id(EPHEMERY_BASE_CHAIN_ID));
        assert!(is_ephemery_chain_id(EPHEMERY_BASE_CHAIN_ID + 163));
        assert!(is_ephemery_chain_id(EPHEMERY_BASE_CHAIN_ID + 9999));
    }

    #[test]
    fn is_ephemery_chain_id_out_of_range() {
        assert!(!is_ephemery_chain_id(EPHEMERY_BASE_CHAIN_ID + EPHEMERY_ITERATION_UPPER_BOUND));
        assert!(!is_ephemery_chain_id(1));
        assert!(!is_ephemery_chain_id(0));
    }

    #[test]
    fn ephemery_all_prefork_hardforks_active_at_genesis() {
        let hardforks = ephemery_hardforks(EPHEMERY_GENESIS_ANCHOR);
        assert!(hardforks.fork(EthereumHardfork::Frontier).active_at_block(0));
        assert!(hardforks.fork(EthereumHardfork::London).active_at_block(0));
        assert!(hardforks.fork(EthereumHardfork::Shanghai).active_at_timestamp(0));
        assert!(hardforks.fork(EthereumHardfork::Cancun).active_at_timestamp(0));
        assert!(hardforks.fork(EthereumHardfork::Prague).active_at_timestamp(0));
        assert!(hardforks.fork(EthereumHardfork::Osaka).active_at_timestamp(0));
        assert!(hardforks.fork(EthereumHardfork::Amsterdam).active_at_timestamp(0));
    }

    #[test]
    fn ephemery_bpo_forks_offset_from_genesis() {
        let hardforks = ephemery_hardforks(EPHEMERY_GENESIS_ANCHOR);
        assert!(hardforks
            .fork(EthereumHardfork::Bpo1)
            .active_at_timestamp(EPHEMERY_GENESIS_ANCHOR + BPO1_OFFSET));
        assert!(!hardforks
            .fork(EthereumHardfork::Bpo1)
            .active_at_timestamp(EPHEMERY_GENESIS_ANCHOR + BPO1_OFFSET - 1));
    }

    #[test]
    fn ephemery_blob_params_per_fork_values() {
        let params = ephemery_blob_params(EPHEMERY_GENESIS_ANCHOR);

        assert_eq!(params.cancun.target_blob_count, 3);
        assert_eq!(params.cancun.max_blob_count, 6);
        assert_eq!(params.prague.target_blob_count, 6);
        assert_eq!(params.prague.max_blob_count, 9);

        let bpo1 = ephemery_scheduled_at(&params, BPO1_OFFSET);
        assert_eq!(bpo1.target_blob_count, 8);
        assert_eq!(bpo1.max_blob_count, 12);
        assert_eq!(bpo1.update_fraction, 6676955);

        let bpo2 = ephemery_scheduled_at(&params, BPO2_OFFSET);
        assert_eq!(bpo2.target_blob_count, 10);
        assert_eq!(bpo2.max_blob_count, 15);
        assert_eq!(bpo2.update_fraction, 8346193);
    }

    #[test]
    fn ephemery_bpo_params_inherit_from_osaka() {
        let params = ephemery_blob_params(EPHEMERY_GENESIS_ANCHOR);
        let osaka = BlobParams::osaka();

        for offset in [BPO1_OFFSET, BPO2_OFFSET] {
            let bpo = ephemery_scheduled_at(&params, offset);
            assert_eq!(bpo.blob_base_cost, osaka.blob_base_cost);
            assert_eq!(bpo.max_blobs_per_tx, osaka.max_blobs_per_tx);
            assert_eq!(bpo.min_blob_fee, osaka.min_blob_fee);
        }
    }
}
