//! Beacon consensus implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use reth_chainspec::{ChainSpec, EthereumHardfork, EthereumHardforks};
use reth_consensus::{Consensus, ConsensusError, PostExecutionInput};
use reth_consensus_common::{
    difficulty::{
        calc_difficulty_frontier, calc_difficulty_generic, calc_difficulty_homestead, BombDelay,
    },
    validation::{
        validate_4844_header_standalone, validate_against_parent_4844,
        validate_against_parent_eip1559_base_fee, validate_against_parent_hash_number,
        validate_against_parent_timestamp, validate_block_pre_execution, validate_header_base_fee,
        validate_header_extradata, validate_header_gas,
    },
};
use reth_primitives::{
    constants::MINIMUM_GAS_LIMIT, BlockWithSenders, GotExpected, Header, SealedBlock, SealedHeader,
    EMPTY_OMMER_ROOT_HASH, U256,
};
use std::{cmp::Ordering, sync::Arc, time::SystemTime};

mod validation;
pub use validation::validate_block_post_execution;

/// Ethereum beacon consensus
///
/// This consensus engine does basic checks as outlined in the execution specs.
#[derive(Debug)]
pub struct EthBeaconConsensus {
    /// Configuration
    chain_spec: Arc<ChainSpec>,
}

impl EthBeaconConsensus {
    /// Create a new instance of [`EthBeaconConsensus`]
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }

    /// Checks the gas limit for consistency between parent and self headers.
    ///
    /// The maximum allowable difference between self and parent gas limits is determined by the
    /// parent's gas limit divided by the elasticity multiplier (1024).
    fn validate_against_parent_gas_limit(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        // Determine the parent gas limit, considering elasticity multiplier on the London fork.
        let parent_gas_limit =
            if self.chain_spec.fork(EthereumHardfork::London).transitions_at_block(header.number) {
                parent.gas_limit *
                    self.chain_spec
                        .base_fee_params_at_timestamp(header.timestamp)
                        .elasticity_multiplier as u64
            } else {
                parent.gas_limit
            };

        // Check for an increase in gas limit beyond the allowed threshold.
        if header.gas_limit > parent_gas_limit {
            if header.gas_limit - parent_gas_limit >= parent_gas_limit / 1024 {
                return Err(ConsensusError::GasLimitInvalidIncrease {
                    parent_gas_limit,
                    child_gas_limit: header.gas_limit,
                })
            }
        }
        // Check for a decrease in gas limit beyond the allowed threshold.
        else if parent_gas_limit - header.gas_limit >= parent_gas_limit / 1024 {
            return Err(ConsensusError::GasLimitInvalidDecrease {
                parent_gas_limit,
                child_gas_limit: header.gas_limit,
            })
        }
        // Check if the self gas limit is below the minimum required limit.
        else if header.gas_limit < MINIMUM_GAS_LIMIT {
            return Err(ConsensusError::GasLimitInvalidMinimum { child_gas_limit: header.gas_limit })
        }

        Ok(())
    }

    /// Checks difficulty change between blocks based on difficulty formulas
    fn validate_difficulty_increment(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        // Check difficulty for the block base on the current hardfork
        let calculated_difficulty;

        if self.chain_spec.fork(EthereumHardfork::GrayGlacier).active_at_block(header.number) {
            calculated_difficulty =
                calc_difficulty_generic(header.timestamp, parent, BombDelay::Eip5133)
                    .map_err(|_| ConsensusError::DifficultyCalculationError)?;
        } else if self
            .chain_spec
            .fork(EthereumHardfork::ArrowGlacier)
            .active_at_block(header.number)
        {
            calculated_difficulty =
                calc_difficulty_generic(header.timestamp, parent, BombDelay::Eip4345)
                    .map_err(|_| ConsensusError::DifficultyCalculationError)?;
        } else if self.chain_spec.fork(EthereumHardfork::London).active_at_block(header.number) {
            calculated_difficulty =
                calc_difficulty_generic(header.timestamp, parent, BombDelay::Eip3554)
                    .map_err(|_| ConsensusError::DifficultyCalculationError)?;
        } else if self.chain_spec.fork(EthereumHardfork::MuirGlacier).active_at_block(header.number)
        {
            calculated_difficulty =
                calc_difficulty_generic(header.timestamp, parent, BombDelay::Eip2384)
                    .map_err(|_| ConsensusError::DifficultyCalculationError)?;
        } else if self
            .chain_spec
            .fork(EthereumHardfork::Constantinople)
            .active_at_block(header.number)
        {
            calculated_difficulty =
                calc_difficulty_generic(header.timestamp, parent, BombDelay::Constantinople)
                    .map_err(|_| ConsensusError::DifficultyCalculationError)?;
        } else if self.chain_spec.fork(EthereumHardfork::Byzantium).active_at_block(header.number) {
            calculated_difficulty =
                calc_difficulty_generic(header.timestamp, parent, BombDelay::Byzantium)
                    .map_err(|_| ConsensusError::DifficultyCalculationError)?;
        } else if self.chain_spec.fork(EthereumHardfork::Homestead).active_at_block(header.number) {
            calculated_difficulty = calc_difficulty_homestead(header.timestamp, parent)
                .map_err(|_| ConsensusError::DifficultyCalculationError)?;
        } else {
            // Frontier default
            calculated_difficulty = calc_difficulty_frontier(header.timestamp, parent)
                .map_err(|_| ConsensusError::DifficultyCalculationError)?;
        }

        if header.difficulty.cmp(&calculated_difficulty) != Ordering::Equal {
            return Err(ConsensusError::DifficultyDiff(GotExpected::new(
                header.difficulty,
                calculated_difficulty,
            )));
        }

        Ok(())
    }
}

impl Consensus for EthBeaconConsensus {
    fn validate_header(&self, header: &SealedHeader) -> Result<(), ConsensusError> {
        validate_header_gas(header)?;
        validate_header_base_fee(header, &self.chain_spec)?;

        // EIP-4895: Beacon chain push withdrawals as operations
        if self.chain_spec.is_shanghai_active_at_timestamp(header.timestamp) &&
            header.withdrawals_root.is_none()
        {
            return Err(ConsensusError::WithdrawalsRootMissing)
        } else if !self.chain_spec.is_shanghai_active_at_timestamp(header.timestamp) &&
            header.withdrawals_root.is_some()
        {
            return Err(ConsensusError::WithdrawalsRootUnexpected)
        }

        // Ensures that EIP-4844 fields are valid once cancun is active.
        if self.chain_spec.is_cancun_active_at_timestamp(header.timestamp) {
            validate_4844_header_standalone(header)?;
        } else if header.blob_gas_used.is_some() {
            return Err(ConsensusError::BlobGasUsedUnexpected)
        } else if header.excess_blob_gas.is_some() {
            return Err(ConsensusError::ExcessBlobGasUnexpected)
        } else if header.parent_beacon_block_root.is_some() {
            return Err(ConsensusError::ParentBeaconBlockRootUnexpected)
        }

        if self.chain_spec.is_prague_active_at_timestamp(header.timestamp) {
            if header.requests_root.is_none() {
                return Err(ConsensusError::RequestsRootMissing)
            }
        } else if header.requests_root.is_some() {
            return Err(ConsensusError::RequestsRootUnexpected)
        }

        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader,
        parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        let is_post_merge =
            self.chain_spec.fork(EthereumHardfork::Paris).active_at_block(header.number);

        validate_against_parent_hash_number(header, parent)?;

        validate_against_parent_timestamp(header, parent)?;

        if !is_post_merge {
            self.validate_difficulty_increment(header, parent)?;
        }

        self.validate_against_parent_gas_limit(header, parent)?;

        validate_against_parent_eip1559_base_fee(header, parent, &self.chain_spec)?;

        // ensure that the blob gas fields for this block
        if self.chain_spec.is_cancun_active_at_timestamp(header.timestamp) {
            validate_against_parent_4844(header, parent)?;
        }

        Ok(())
    }

    fn validate_header_with_total_difficulty(
        &self,
        header: &Header,
        total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        let is_post_merge = self
            .chain_spec
            .fork(EthereumHardfork::Paris)
            .active_at_ttd(total_difficulty, header.difficulty);

        if is_post_merge {
            if !header.is_zero_difficulty() {
                return Err(ConsensusError::TheMergeDifficultyIsNotZero)
            }

            if header.nonce != 0 {
                return Err(ConsensusError::TheMergeNonceIsNotZero)
            }

            if header.ommers_hash != EMPTY_OMMER_ROOT_HASH {
                return Err(ConsensusError::TheMergeOmmerRootIsNotEmpty)
            }

            // Post-merge, the consensus layer is expected to perform checks such that the block
            // timestamp is a function of the slot. This is different from pre-merge, where blocks
            // are only allowed to be in the future (compared to the system's clock) by a certain
            // threshold.
            //
            // Block validation with respect to the parent should ensure that the block timestamp
            // is greater than its parent timestamp.

            // validate header extradata for all networks post merge
            validate_header_extradata(header)?;

            // mixHash is used instead of difficulty inside EVM
            // https://eips.ethereum.org/EIPS/eip-4399#using-mixhash-field-instead-of-difficulty
        } else {
            // TODO Consensus checks for old blocks:
            //  * difficulty, mix_hash & nonce aka PoW stuff
            // low priority as syncing is done in reverse order

            // Check if timestamp is in the future. Clock can drift but this can be consensus issue.
            let present_timestamp =
                SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();

            if header.exceeds_allowed_future_timestamp(present_timestamp) {
                return Err(ConsensusError::TimestampIsInFuture {
                    timestamp: header.timestamp,
                    present_timestamp,
                })
            }

            validate_header_extradata(header)?;
        }

        Ok(())
    }

    fn validate_block_pre_execution(&self, block: &SealedBlock) -> Result<(), ConsensusError> {
        validate_block_pre_execution(block, &self.chain_spec)
    }

    fn validate_block_post_execution(
        &self,
        block: &BlockWithSenders,
        input: PostExecutionInput<'_>,
    ) -> Result<(), ConsensusError> {
        validate_block_post_execution(block, &self.chain_spec, input.receipts, input.requests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::ChainSpecBuilder;
    use reth_primitives::{proofs, BlockNumber, B256};

    fn header_with_gas_limit(gas_limit: u64) -> SealedHeader {
        let header = Header { gas_limit, ..Default::default() };
        header.seal(B256::ZERO)
    }

    fn header_with_difficulty_block_number_and_timestamp(
        difficulty: U256,
        number: BlockNumber,
        timestamp: u64,
    ) -> SealedHeader {
        let header = Header { difficulty, number, timestamp, ..Default::default() };
        header.seal(B256::ZERO)
    }

    fn header_with_difficulty_block_number_timestamp_and_ommers(
        difficulty: U256,
        number: BlockNumber,
        timestamp: u64,
        ommers_hash: B256,
    ) -> SealedHeader {
        let header = Header { difficulty, number, timestamp, ommers_hash, ..Default::default() };
        header.seal(B256::ZERO)
    }

    #[test]
    fn test_valid_gas_limit_increase() {
        let parent = header_with_gas_limit(1024 * 10);
        let child = header_with_gas_limit(parent.gas_limit + 5);

        assert_eq!(
            EthBeaconConsensus::new(Arc::new(ChainSpec::default()))
                .validate_against_parent_gas_limit(&child, &parent),
            Ok(())
        );
    }

    #[test]
    fn test_gas_limit_below_minimum() {
        let parent = header_with_gas_limit(MINIMUM_GAS_LIMIT);
        let child = header_with_gas_limit(MINIMUM_GAS_LIMIT - 1);

        assert_eq!(
            EthBeaconConsensus::new(Arc::new(ChainSpec::default()))
                .validate_against_parent_gas_limit(&child, &parent),
            Err(ConsensusError::GasLimitInvalidMinimum { child_gas_limit: child.gas_limit })
        );
    }

    #[test]
    fn test_invalid_gas_limit_increase_exceeding_limit() {
        let parent = header_with_gas_limit(1024 * 10);
        let child = header_with_gas_limit(parent.gas_limit + parent.gas_limit / 1024 + 1);

        assert_eq!(
            EthBeaconConsensus::new(Arc::new(ChainSpec::default()))
                .validate_against_parent_gas_limit(&child, &parent),
            Err(ConsensusError::GasLimitInvalidIncrease {
                parent_gas_limit: parent.gas_limit,
                child_gas_limit: child.gas_limit,
            })
        );
    }

    #[test]
    fn test_valid_gas_limit_decrease_within_limit() {
        let parent = header_with_gas_limit(1024 * 10);
        let child = header_with_gas_limit(parent.gas_limit - 5);

        assert_eq!(
            EthBeaconConsensus::new(Arc::new(ChainSpec::default()))
                .validate_against_parent_gas_limit(&child, &parent),
            Ok(())
        );
    }

    #[test]
    fn test_invalid_gas_limit_decrease_exceeding_limit() {
        let parent = header_with_gas_limit(1024 * 10);
        let child = header_with_gas_limit(parent.gas_limit - parent.gas_limit / 1024 - 1);

        assert_eq!(
            EthBeaconConsensus::new(Arc::new(ChainSpec::default()))
                .validate_against_parent_gas_limit(&child, &parent),
            Err(ConsensusError::GasLimitInvalidDecrease {
                parent_gas_limit: parent.gas_limit,
                child_gas_limit: child.gas_limit,
            })
        );
    }

    #[test]
    fn shanghai_block_zero_withdrawals() {
        // ensures that if shanghai is activated, and we include a block with a withdrawals root,
        // that the header is valid
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().shanghai_activated().build());

        let header = Header {
            base_fee_per_gas: Some(1337u64),
            withdrawals_root: Some(proofs::calculate_withdrawals_root(&[])),
            ..Default::default()
        }
        .seal_slow();

        assert_eq!(EthBeaconConsensus::new(chain_spec).validate_header(&header), Ok(()));
    }

    /// Test blocks difficulty for blocks 0 and 1
    #[test]
    fn test_difficulty_genesis_block() {
        // Ensures that Frontier is activated
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().frontier_activated().build());

        let parent =
            header_with_difficulty_block_number_and_timestamp(U256::from(17179869184u64), 0, 0);
        let child = header_with_difficulty_block_number_and_timestamp(
            U256::from(17171480576u64),
            1,
            1438269988,
        );

        assert_eq!(
            EthBeaconConsensus::new(chain_spec).validate_difficulty_increment(&child, &parent),
            Ok(())
        );
    }

    /// Test blocks difficulty for blocks 500000 and 500001
    #[test]
    fn test_difficulty_frontier() {
        // Ensures that Frontier is activated
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().frontier_activated().build());

        let parent = header_with_difficulty_block_number_and_timestamp(
            U256::from(7545189127997u64),
            500000,
            1446832865,
        );
        let child = header_with_difficulty_block_number_and_timestamp(
            U256::from(7541504953627u64),
            500001,
            1446832888,
        );

        assert_eq!(
            EthBeaconConsensus::new(chain_spec).validate_difficulty_increment(&child, &parent),
            Ok(())
        );
    }

    /// Test blocks difficulty for blocks 1149999 (`Frontier`) and 1150000 (`Homestead`)
    #[test]
    fn test_difficulty_frontier_homestead() {
        // Ensures that Homestead is activated
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().homestead_activated().build());

        let parent = header_with_difficulty_block_number_and_timestamp(
            U256::from(20513164863791u64),
            1149999,
            1457981342,
        );
        let child = header_with_difficulty_block_number_and_timestamp(
            U256::from(20473100089179u64),
            1150000,
            1457981393,
        );

        assert_eq!(
            EthBeaconConsensus::new(chain_spec).validate_difficulty_increment(&child, &parent),
            Ok(())
        );
    }

    /// No changes of difficulty algo between `Homestead` and `Byzantium`, so we skip plenty of
    /// hardforks
    ///
    /// Test blocks difficulty for blocks 3000000 and 3000001
    #[test]
    fn test_difficulty_spurious_dragon() {
        // Ensures that SpuriousDragon is activated
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().spurious_dragon_activated().build());

        let parent = header_with_difficulty_block_number_and_timestamp(
            U256::from(103975266902792u64),
            3000000,
            1484475035,
        );
        let child = header_with_difficulty_block_number_and_timestamp(
            U256::from(103924766164956u64),
            3000001,
            1484475055,
        );

        assert_eq!(
            EthBeaconConsensus::new(chain_spec).validate_difficulty_increment(&child, &parent),
            Ok(())
        );
    }

    /// Test blocks difficulty for blocks 4369999 (`SpuriousDragon`) and 4370000 (`Byzantium`)
    #[test]
    fn test_difficulty_spurious_dragon_byzantium() {
        // Ensures that SpuriousDragon is activated
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().byzantium_activated().build());

        let parent = header_with_difficulty_block_number_and_timestamp(
            U256::from(2997274096101735u64),
            4369999,
            1508131303,
        );
        let child = header_with_difficulty_block_number_and_timestamp(
            U256::from(2994347070619309u64),
            4370000,
            1508131331,
        );

        assert_eq!(
            EthBeaconConsensus::new(chain_spec).validate_difficulty_increment(&child, &parent),
            Ok(())
        );
    }

    /// Test blocks difficulty for blocks 6000000 and 6000001
    #[test]
    fn test_difficulty_byzantium() {
        // Ensures that Byzantium is activated
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().byzantium_activated().build());

        let parent = header_with_difficulty_block_number_and_timestamp(
            U256::from(3483739548912554u64),
            6000000,
            1532118564,
        );
        let child = header_with_difficulty_block_number_and_timestamp(
            U256::from(3482038772646393u64),
            6000001,
            1532118585,
        );

        assert_eq!(
            EthBeaconConsensus::new(chain_spec).validate_difficulty_increment(&child, &parent),
            Ok(())
        );
    }

    /// Test blocks difficulty for blocks 7279999 (`Byzantium`) and 7280000 (`Constantinople`)
    #[test]
    fn test_difficulty_byzantium_constantinople() {
        // Ensures that Constantinople is activated
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().constantinople_activated().build());

        let parent = header_with_difficulty_block_number_and_timestamp(
            U256::from(2958546502099724u64),
            7279999,
            1551383501,
        );
        let child = header_with_difficulty_block_number_and_timestamp(
            U256::from(2957101900364072u64),
            7280000,
            1551383524,
        );

        assert_eq!(
            EthBeaconConsensus::new(chain_spec).validate_difficulty_increment(&child, &parent),
            Ok(())
        );
    }

    /// Test blocks difficulty for blocks 9100000 and 9100001
    #[test]
    fn test_difficulty_istanbul() {
        // Ensures that Istanbul is activated
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().istanbul_activated().build());

        let parent = header_with_difficulty_block_number_and_timestamp(
            U256::from(2475385893975275u64),
            9100000,
            1576239700,
        );
        let child = header_with_difficulty_block_number_and_timestamp(
            U256::from(2475935649789163u64),
            9100001,
            1576239714,
        );

        assert_eq!(
            EthBeaconConsensus::new(chain_spec).validate_difficulty_increment(&child, &parent),
            Ok(())
        );
    }

    /// Test blocks difficulty for blocks 9199999 (`Istanbul`) and 9200000 (`MuirGlacier`)
    #[test]
    fn test_difficulty_istanbul_muir_glacier() {
        // Ensures that Muir Glacier is activated
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().muir_glacier_activated().build());

        let parent = header_with_difficulty_block_number_and_timestamp(
            U256::from(2462196499244987u64),
            9199999,
            1577953806,
        );
        let child = header_with_difficulty_block_number_and_timestamp(
            U256::from(2458589766091800u64),
            9200000,
            1577953849,
        );

        assert_eq!(
            EthBeaconConsensus::new(chain_spec).validate_difficulty_increment(&child, &parent),
            Ok(())
        );
    }

    /// Test blocks difficulty for blocks 12244000 and 12244001
    #[test]
    fn test_difficulty_berlin() {
        // Ensures that Berlin is activated
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().berlin_activated().build());

        let parent = header_with_difficulty_block_number_and_timestamp(
            U256::from(6696239334037736u64),
            12244000,
            1618481223,
        );
        let child = header_with_difficulty_block_number_and_timestamp(
            U256::from(6699510055891883u64),
            12244001,
            1618481230,
        );

        assert_eq!(
            EthBeaconConsensus::new(chain_spec).validate_difficulty_increment(&child, &parent),
            Ok(())
        );
    }

    /// Test blocks difficulty for blocks 12964999 (`Berlin`) and 12965000 (`London`)
    #[test]
    fn test_difficulty_berlin_london() {
        // Ensures that London is activated
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().london_activated().build());

        let parent = header_with_difficulty_block_number_and_timestamp(
            U256::from(7742493487903256u64),
            12964999,
            1628166812,
        );
        let child = header_with_difficulty_block_number_and_timestamp(
            U256::from(7742494561645080u64),
            12965000,
            1628166822,
        );

        assert_eq!(
            EthBeaconConsensus::new(chain_spec).validate_difficulty_increment(&child, &parent),
            Ok(())
        );
    }

    /// Test blocks difficulty for blocks 13772999 (`London`) and 13773000 (`ArrowGlacier`)
    #[test]
    fn test_difficulty_london_arrow_glacier() {
        // Ensures that ArrowGlacier is activated
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().arrow_glacier_activated().build());

        let parent = header_with_difficulty_block_number_and_timestamp(
            U256::from(11869818951734368u64),
            13772999,
            1639079715,
        );
        let child = header_with_difficulty_block_number_and_timestamp(
            U256::from(11875615030204850u64),
            13773000,
            1639079723,
        );

        assert_eq!(
            EthBeaconConsensus::new(chain_spec).validate_difficulty_increment(&child, &parent),
            Ok(())
        );
    }

    /// Test blocks difficulty for blocks 15049999 (`ArrowGlacier`) and 15050000 (`GrayGlacier`)
    #[test]
    fn test_difficulty_arrow_glacier_gray_glacier() {
        // Ensures that GrayGlacier is activated
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().gray_glacier_activated().build());

        let parent = header_with_difficulty_block_number_timestamp_and_ommers(
            U256::from(14296525396309425u64),
            15049999,
            1656586434,
            B256::try_random().unwrap(),
        );

        let child = header_with_difficulty_block_number_and_timestamp(
            U256::from(14303523301469775u64),
            15050000,
            1656586444,
        );

        assert_eq!(
            EthBeaconConsensus::new(chain_spec).validate_difficulty_increment(&child, &parent),
            Ok(())
        );
    }
}
