//! Beacon consensus implementation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::{fmt::Debug, sync::Arc};
use alloy_consensus::EMPTY_OMMER_ROOT_HASH;
use alloy_eips::eip7840::BlobParams;
use alloy_primitives::{I256, U256};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_consensus::{Consensus, ConsensusError, FullConsensus, HeaderValidator};
use reth_consensus_common::validation::{
    validate_4844_header_standalone, validate_against_parent_4844,
    validate_against_parent_eip1559_base_fee, validate_against_parent_hash_number,
    validate_against_parent_timestamp, validate_block_pre_execution, validate_body_against_header,
    validate_header_base_fee, validate_header_extra_data, validate_header_gas,
};
use reth_execution_types::BlockExecutionResult;
use reth_primitives_traits::{
    constants::{GAS_LIMIT_BOUND_DIVISOR, MINIMUM_GAS_LIMIT},
    Block, BlockHeader, NodePrimitives, RecoveredBlock, SealedBlock, SealedHeader,
};

mod validation;
pub use validation::validate_block_post_execution;

/// Ethereum beacon consensus
///
/// This consensus engine does basic checks as outlined in the execution specs.
#[derive(Debug, Clone)]
pub struct EthBeaconConsensus<ChainSpec> {
    /// Configuration
    chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec: EthChainSpec + EthereumHardforks> EthBeaconConsensus<ChainSpec> {
    /// Create a new instance of [`EthBeaconConsensus`]
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }

    /// Validates the difficulty adjustment between parent and current block.
    ///
    /// This implements the Ethereum difficulty adjustment algorithm according to EIP-2 (Homestead),
    /// EIP-100 (Byzantium), and subsequent modifications for different hard forks.
    fn validate_against_parent_difficulty<H: BlockHeader>(
        &self,
        header: &SealedHeader<H>,
        parent: &SealedHeader<H>,
    ) -> Result<(), ConsensusError> {
        // Post-merge blocks should have zero difficulty
        if self.chain_spec.is_paris_active_at_block(header.number()) {
            return Ok(());
        }

        let parent_difficulty = parent.difficulty();
        let header_difficulty = header.difficulty();
        let parent_timestamp = parent.timestamp();
        let header_timestamp = header.timestamp();
        let block_number = header.number();

        // Calculate expected difficulty based on the current hard fork
        let expected_difficulty = if self.chain_spec.is_byzantium_active_at_block(block_number) {
            // Byzantium and later: EIP-100 difficulty adjustment
            // Note: While the core algorithm remains the same, different hard forks have
            // different difficulty bomb delays. For simplicity, we use a fixed delay here.
            // In a full implementation, this should be adjusted per hard fork.
            self.calculate_difficulty_byzantium(
                parent_difficulty,
                parent_timestamp,
                header_timestamp,
                block_number,
                parent.header().ommers_hash() != EMPTY_OMMER_ROOT_HASH,
            )
        } else if self.chain_spec.is_homestead_active_at_block(block_number) {
            // Homestead: EIP-2 difficulty adjustment
            self.calculate_difficulty_homestead(
                parent_difficulty,
                parent_timestamp,
                header_timestamp,
                block_number,
            )
        } else {
            // Frontier difficulty adjustment
            self.calculate_difficulty_frontier(
                parent_difficulty,
                parent_timestamp,
                header_timestamp,
                block_number,
            )
        };

        if header_difficulty != expected_difficulty {
            return Err(ConsensusError::DifficultyInvalid {
                parent_difficulty,
                child_difficulty: header_difficulty,
            });
        }

        Ok(())
    }

    /// Calculates difficulty for Frontier era
    fn calculate_difficulty_frontier(
        &self,
        parent_difficulty: U256,
        parent_timestamp: u64,
        current_timestamp: u64,
        block_number: u64,
    ) -> U256 {
        let diff = I256::try_from(current_timestamp - parent_timestamp).unwrap_or(I256::ZERO);
        let target = I256::try_from(13u64).unwrap_or(I256::ZERO); // Target 13-15 second block times

        let adjustment = if diff < target {
            I256::try_from(1u64).unwrap_or(I256::ZERO)
        } else {
            I256::try_from(-1i64).unwrap_or(I256::ZERO)
        };

        let difficulty_adjustment = I256::try_from(parent_difficulty).unwrap_or(I256::ZERO) /
            I256::try_from(2048u64).unwrap_or(I256::ZERO) *
            adjustment;
        let new_difficulty =
            I256::try_from(parent_difficulty).unwrap_or(I256::ZERO) + difficulty_adjustment;

        // Apply difficulty bomb (Ice Age)
        let period = block_number / 100000;
        if period > 2 {
            let bomb = U256::from(1) << (period - 2);
            let bomb_i256 = I256::try_from(bomb).unwrap_or(I256::ZERO);
            let final_difficulty = new_difficulty + bomb_i256;
            U256::try_from(final_difficulty.max(I256::try_from(131072u64).unwrap_or(I256::ZERO)))
                .unwrap_or_else(|_| U256::from(131072))
        } else {
            U256::try_from(new_difficulty.max(I256::try_from(131072u64).unwrap_or(I256::ZERO)))
                .unwrap_or_else(|_| U256::from(131072))
        }
    }

    /// Calculates difficulty for Homestead era (EIP-2)
    fn calculate_difficulty_homestead(
        &self,
        parent_difficulty: U256,
        parent_timestamp: u64,
        current_timestamp: u64,
        block_number: u64,
    ) -> U256 {
        let diff = I256::try_from(current_timestamp - parent_timestamp).unwrap_or(I256::ZERO);
        let target = I256::try_from(10u64).unwrap_or(I256::ZERO); // Target ~10-19 second block times

        let adjustment = if diff < target {
            I256::try_from(1u64).unwrap_or(I256::ZERO)
        } else {
            I256::try_from(-1i64).unwrap_or(I256::ZERO)
        };

        let difficulty_adjustment = I256::try_from(parent_difficulty).unwrap_or(I256::ZERO) /
            I256::try_from(2048u64).unwrap_or(I256::ZERO) *
            adjustment;
        let new_difficulty =
            I256::try_from(parent_difficulty).unwrap_or(I256::ZERO) + difficulty_adjustment;

        // Apply difficulty bomb (Ice Age) - delayed in Homestead
        let period = block_number.saturating_sub(3000000) / 100000;
        if period > 0 {
            let bomb = U256::from(1) << (period - 1);
            let bomb_i256 = I256::try_from(bomb).unwrap_or(I256::ZERO);
            let final_difficulty = new_difficulty + bomb_i256;
            U256::try_from(final_difficulty.max(I256::try_from(131072u64).unwrap_or(I256::ZERO)))
                .unwrap_or_else(|_| U256::from(131072))
        } else {
            U256::try_from(new_difficulty.max(I256::try_from(131072u64).unwrap_or(I256::ZERO)))
                .unwrap_or_else(|_| U256::from(131072))
        }
    }

    /// Calculates difficulty for Byzantium era and later (EIP-100)
    fn calculate_difficulty_byzantium(
        &self,
        parent_difficulty: U256,
        parent_timestamp: u64,
        current_timestamp: u64,
        block_number: u64,
        parent_has_ommers: bool,
    ) -> U256 {
        let diff = I256::try_from(current_timestamp - parent_timestamp).unwrap_or(I256::ZERO);

        // EIP-100: Include uncle adjustment
        let uncle_adjustment = if parent_has_ommers {
            I256::try_from(2u64).unwrap_or(I256::ZERO)
        } else {
            I256::try_from(1u64).unwrap_or(I256::ZERO)
        };
        let time_adjustment =
            uncle_adjustment - (diff / I256::try_from(9u64).unwrap_or(I256::ZERO));
        let time_adjustment = time_adjustment.max(I256::try_from(-99i64).unwrap_or(I256::ZERO));

        let difficulty_adjustment = I256::try_from(parent_difficulty).unwrap_or(I256::ZERO) /
            I256::try_from(2048u64).unwrap_or(I256::ZERO) *
            time_adjustment;
        let new_difficulty =
            I256::try_from(parent_difficulty).unwrap_or(I256::ZERO) + difficulty_adjustment;

        // Apply difficulty bomb (Ice Age) - using standard delay for Byzantium+
        // Note: Different hard forks (Constantinople, Muir Glacier, Arrow Glacier, Gray Glacier)
        // have different bomb delays, but the core algorithm remains the same
        let fake_block_number = block_number.saturating_sub(3000000);
        let period = fake_block_number / 100000;

        if period > 0 {
            let bomb = U256::from(1) << (period - 1);
            let bomb_i256 = I256::try_from(bomb).unwrap_or(I256::ZERO);
            let final_difficulty = new_difficulty + bomb_i256;
            U256::try_from(final_difficulty.max(I256::try_from(131072u64).unwrap_or(I256::ZERO)))
                .unwrap_or_else(|_| U256::from(131072))
        } else {
            U256::try_from(new_difficulty.max(I256::try_from(131072u64).unwrap_or(I256::ZERO)))
                .unwrap_or_else(|_| U256::from(131072))
        }
    }

    /// Checks the gas limit for consistency between parent and self headers.
    ///
    /// The maximum allowable difference between self and parent gas limits is determined by the
    /// parent's gas limit divided by the [`GAS_LIMIT_BOUND_DIVISOR`].
    fn validate_against_parent_gas_limit<H: BlockHeader>(
        &self,
        header: &SealedHeader<H>,
        parent: &SealedHeader<H>,
    ) -> Result<(), ConsensusError> {
        // Determine the parent gas limit, considering elasticity multiplier on the London fork.
        let parent_gas_limit = if !self.chain_spec.is_london_active_at_block(parent.number()) &&
            self.chain_spec.is_london_active_at_block(header.number())
        {
            parent.gas_limit() *
                self.chain_spec
                    .base_fee_params_at_timestamp(header.timestamp())
                    .elasticity_multiplier as u64
        } else {
            parent.gas_limit()
        };

        // Check for an increase in gas limit beyond the allowed threshold.
        if header.gas_limit() > parent_gas_limit {
            if header.gas_limit() - parent_gas_limit >= parent_gas_limit / GAS_LIMIT_BOUND_DIVISOR {
                return Err(ConsensusError::GasLimitInvalidIncrease {
                    parent_gas_limit,
                    child_gas_limit: header.gas_limit(),
                })
            }
        }
        // Check for a decrease in gas limit beyond the allowed threshold.
        else if parent_gas_limit - header.gas_limit() >=
            parent_gas_limit / GAS_LIMIT_BOUND_DIVISOR
        {
            return Err(ConsensusError::GasLimitInvalidDecrease {
                parent_gas_limit,
                child_gas_limit: header.gas_limit(),
            })
        }
        // Check if the self gas limit is below the minimum required limit.
        else if header.gas_limit() < MINIMUM_GAS_LIMIT {
            return Err(ConsensusError::GasLimitInvalidMinimum {
                child_gas_limit: header.gas_limit(),
            })
        }

        Ok(())
    }
}

impl<ChainSpec, N> FullConsensus<N> for EthBeaconConsensus<ChainSpec>
where
    ChainSpec: Send + Sync + EthChainSpec<Header = N::BlockHeader> + EthereumHardforks + Debug,
    N: NodePrimitives,
{
    fn validate_block_post_execution(
        &self,
        block: &RecoveredBlock<N::Block>,
        result: &BlockExecutionResult<N::Receipt>,
    ) -> Result<(), ConsensusError> {
        validate_block_post_execution(block, &self.chain_spec, &result.receipts, &result.requests)
    }
}

impl<B, ChainSpec> Consensus<B> for EthBeaconConsensus<ChainSpec>
where
    B: Block,
    ChainSpec: EthChainSpec<Header = B::Header> + EthereumHardforks + Debug + Send + Sync,
{
    type Error = ConsensusError;

    fn validate_body_against_header(
        &self,
        body: &B::Body,
        header: &SealedHeader<B::Header>,
    ) -> Result<(), Self::Error> {
        validate_body_against_header(body, header.header())
    }

    fn validate_block_pre_execution(&self, block: &SealedBlock<B>) -> Result<(), Self::Error> {
        validate_block_pre_execution(block, &self.chain_spec)
    }
}

impl<H, ChainSpec> HeaderValidator<H> for EthBeaconConsensus<ChainSpec>
where
    H: BlockHeader,
    ChainSpec: EthChainSpec<Header = H> + EthereumHardforks + Debug + Send + Sync,
{
    fn validate_header(&self, header: &SealedHeader<H>) -> Result<(), ConsensusError> {
        let header = header.header();
        let is_post_merge = self.chain_spec.is_paris_active_at_block(header.number());

        if is_post_merge {
            if !header.difficulty().is_zero() {
                return Err(ConsensusError::TheMergeDifficultyIsNotZero);
            }

            if !header.nonce().is_some_and(|nonce| nonce.is_zero()) {
                return Err(ConsensusError::TheMergeNonceIsNotZero);
            }

            if header.ommers_hash() != EMPTY_OMMER_ROOT_HASH {
                return Err(ConsensusError::TheMergeOmmerRootIsNotEmpty);
            }
        } else {
            #[cfg(feature = "std")]
            {
                let present_timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();

                if header.timestamp() >
                    present_timestamp + alloy_eips::merge::ALLOWED_FUTURE_BLOCK_TIME_SECONDS
                {
                    return Err(ConsensusError::TimestampIsInFuture {
                        timestamp: header.timestamp(),
                        present_timestamp,
                    });
                }
            }
        }
        validate_header_extra_data(header)?;
        validate_header_gas(header)?;
        validate_header_base_fee(header, &self.chain_spec)?;

        // EIP-4895: Beacon chain push withdrawals as operations
        if self.chain_spec.is_shanghai_active_at_timestamp(header.timestamp()) &&
            header.withdrawals_root().is_none()
        {
            return Err(ConsensusError::WithdrawalsRootMissing)
        } else if !self.chain_spec.is_shanghai_active_at_timestamp(header.timestamp()) &&
            header.withdrawals_root().is_some()
        {
            return Err(ConsensusError::WithdrawalsRootUnexpected)
        }

        // Ensures that EIP-4844 fields are valid once cancun is active.
        if self.chain_spec.is_cancun_active_at_timestamp(header.timestamp()) {
            validate_4844_header_standalone(
                header,
                self.chain_spec
                    .blob_params_at_timestamp(header.timestamp())
                    .unwrap_or_else(BlobParams::cancun),
            )?;
        } else if header.blob_gas_used().is_some() {
            return Err(ConsensusError::BlobGasUsedUnexpected)
        } else if header.excess_blob_gas().is_some() {
            return Err(ConsensusError::ExcessBlobGasUnexpected)
        } else if header.parent_beacon_block_root().is_some() {
            return Err(ConsensusError::ParentBeaconBlockRootUnexpected)
        }

        if self.chain_spec.is_prague_active_at_timestamp(header.timestamp()) {
            if header.requests_hash().is_none() {
                return Err(ConsensusError::RequestsHashMissing)
            }
        } else if header.requests_hash().is_some() {
            return Err(ConsensusError::RequestsHashUnexpected)
        }

        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        header: &SealedHeader<H>,
        parent: &SealedHeader<H>,
    ) -> Result<(), ConsensusError> {
        validate_against_parent_hash_number(header.header(), parent)?;

        validate_against_parent_timestamp(header.header(), parent.header())?;

        // Validate difficulty adjustment between parent and current block
        self.validate_against_parent_difficulty(header, parent)?;

        self.validate_against_parent_gas_limit(header, parent)?;

        validate_against_parent_eip1559_base_fee(
            header.header(),
            parent.header(),
            &self.chain_spec,
        )?;

        // ensure that the blob gas fields for this block
        if let Some(blob_params) = self.chain_spec.blob_params_at_timestamp(header.timestamp()) {
            validate_against_parent_4844(header.header(), parent.header(), blob_params)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_chainspec::{ChainSpec, ChainSpecBuilder};
    use reth_primitives_traits::proofs;

    fn header_with_gas_limit(gas_limit: u64) -> SealedHeader {
        let header = reth_primitives_traits::Header { gas_limit, ..Default::default() };
        SealedHeader::new(header, B256::ZERO)
    }

    #[test]
    fn test_valid_gas_limit_increase() {
        let parent = header_with_gas_limit(GAS_LIMIT_BOUND_DIVISOR * 10);
        let child = header_with_gas_limit((parent.gas_limit + 5) as u64);

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
            Err(ConsensusError::GasLimitInvalidMinimum { child_gas_limit: child.gas_limit as u64 })
        );
    }

    #[test]
    fn test_invalid_gas_limit_increase_exceeding_limit() {
        let parent = header_with_gas_limit(GAS_LIMIT_BOUND_DIVISOR * 10);
        let child = header_with_gas_limit(
            parent.gas_limit + parent.gas_limit / GAS_LIMIT_BOUND_DIVISOR + 1,
        );

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
        let parent = header_with_gas_limit(GAS_LIMIT_BOUND_DIVISOR * 10);
        let child = header_with_gas_limit(parent.gas_limit - 5);

        assert_eq!(
            EthBeaconConsensus::new(Arc::new(ChainSpec::default()))
                .validate_against_parent_gas_limit(&child, &parent),
            Ok(())
        );
    }

    #[test]
    fn test_invalid_gas_limit_decrease_exceeding_limit() {
        let parent = header_with_gas_limit(GAS_LIMIT_BOUND_DIVISOR * 10);
        let child = header_with_gas_limit(
            parent.gas_limit - parent.gas_limit / GAS_LIMIT_BOUND_DIVISOR - 1,
        );

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

        let header = reth_primitives_traits::Header {
            base_fee_per_gas: Some(1337),
            withdrawals_root: Some(proofs::calculate_withdrawals_root(&[])),
            ..Default::default()
        };

        assert_eq!(
            EthBeaconConsensus::new(chain_spec).validate_header(&SealedHeader::seal_slow(header,)),
            Ok(())
        );
    }

    #[test]
    fn test_difficulty_validation_frontier() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().build());
        let consensus = EthBeaconConsensus::new(chain_spec);

        // Create parent header with difficulty 1000000
        let parent_header = reth_primitives_traits::Header {
            number: 50,
            difficulty: U256::from(1000000),
            timestamp: 1000000,
            ..Default::default()
        };
        let parent = SealedHeader::new(parent_header, B256::ZERO);

        // Calculate expected difficulty for a child block with normal timing (15 seconds)
        let expected_difficulty = consensus
            .calculate_difficulty_frontier(
                U256::from(1000000),
                1000000,
                1000015, // 15 seconds later
                51,
            );

        let child_header = reth_primitives_traits::Header {
            number: 51,
            difficulty: expected_difficulty,
            timestamp: 1000015,
            parent_hash: B256::ZERO,
            ..Default::default()
        };
        let child = SealedHeader::new(child_header, B256::ZERO);

        // This should pass validation
        assert_eq!(consensus.validate_against_parent_difficulty(&child, &parent), Ok(()));
    }

    #[test]
    fn test_difficulty_validation_homestead() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().homestead_activated().build());
        let consensus = EthBeaconConsensus::new(chain_spec);

        // Create parent header with difficulty 1000000
        let parent_header = reth_primitives_traits::Header {
            number: 1150000, // Homestead era
            difficulty: U256::from(1000000),
            timestamp: 1000000,
            ..Default::default()
        };
        let parent = SealedHeader::new(parent_header, B256::ZERO);

        // Calculate expected difficulty for a child block with normal timing (15 seconds)
        let expected_difficulty = consensus
            .calculate_difficulty_homestead(
                U256::from(1000000),
                1000000,
                1000015, // 15 seconds later
                1150001,
            );

        let child_header = reth_primitives_traits::Header {
            number: 1150001,
            difficulty: expected_difficulty,
            timestamp: 1000015,
            parent_hash: B256::ZERO,
            ..Default::default()
        };
        let child = SealedHeader::new(child_header, B256::ZERO);

        // This should pass validation
        assert_eq!(consensus.validate_against_parent_difficulty(&child, &parent), Ok(()));
    }

    #[test]
    fn test_difficulty_validation_byzantium() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().byzantium_activated().build());
        let consensus = EthBeaconConsensus::new(chain_spec);

        // Create parent header with difficulty 1000000 and no ommers
        let parent_header = reth_primitives_traits::Header {
            number: 4370000, // Byzantium era
            difficulty: U256::from(1000000),
            timestamp: 1000000,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            ..Default::default()
        };
        let parent = SealedHeader::new(parent_header, B256::ZERO);

        // Calculate expected difficulty for a child block with normal timing (15 seconds)
        let expected_difficulty = consensus
            .calculate_difficulty_byzantium(
                U256::from(1000000),
                1000000,
                1000015, // 15 seconds later
                4370001,
                false, // no ommers
            );

        let child_header = reth_primitives_traits::Header {
            number: 4370001,
            difficulty: expected_difficulty,
            timestamp: 1000015,
            parent_hash: B256::ZERO,
            ..Default::default()
        };
        let child = SealedHeader::new(child_header, B256::ZERO);

        // This should pass validation
        assert_eq!(consensus.validate_against_parent_difficulty(&child, &parent), Ok(()));
    }

    #[test]
    fn test_difficulty_validation_post_merge() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().paris_activated().build());
        let consensus = EthBeaconConsensus::new(chain_spec);

        // Create parent header (post-merge)
        let parent_header = reth_primitives_traits::Header {
            number: 15537393,       // Post-merge block
            difficulty: U256::ZERO, // Should be zero post-merge
            timestamp: 1000000,
            ..Default::default()
        };
        let parent = SealedHeader::new(parent_header, B256::ZERO);

        let child_header = reth_primitives_traits::Header {
            number: 15537394,
            difficulty: U256::ZERO, // Should be zero post-merge
            timestamp: 1000012,
            parent_hash: B256::ZERO,
            ..Default::default()
        };
        let child = SealedHeader::new(child_header, B256::ZERO);

        // Post-merge blocks should always pass difficulty validation (they have zero difficulty)
        assert_eq!(consensus.validate_against_parent_difficulty(&child, &parent), Ok(()));
    }

    #[test]
    fn test_difficulty_validation_invalid() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().build());
        let consensus = EthBeaconConsensus::new(chain_spec);

        // Create parent header with difficulty 1000000
        let parent_header = reth_primitives_traits::Header {
            number: 50,
            difficulty: U256::from(1000000),
            timestamp: 1000000,
            ..Default::default()
        };
        let parent = SealedHeader::new(parent_header, B256::ZERO);

        // Create child with wrong difficulty
        let child_header = reth_primitives_traits::Header {
            number: 51,
            difficulty: U256::from(2000000), // Wrong difficulty
            timestamp: 1000015,
            parent_hash: B256::ZERO,
            ..Default::default()
        };
        let child = SealedHeader::new(child_header, B256::ZERO);

        // This should fail validation
        assert!(consensus.validate_against_parent_difficulty(&child, &parent).is_err());

        if let Err(ConsensusError::DifficultyInvalid { parent_difficulty, child_difficulty }) =
            consensus.validate_against_parent_difficulty(&child, &parent)
        {
            assert_eq!(parent_difficulty, U256::from(1000000));
            assert_eq!(child_difficulty, U256::from(2000000));
        } else {
            panic!("Expected DifficultyInvalid error");
        }
    }

    #[test]
    fn test_difficulty_bomb_is_included() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().byzantium_activated().build());
        let consensus = EthBeaconConsensus::new(chain_spec);

        // Test a block at a high number where difficulty bomb should have significant effect
        let high_block_number = 5000000; // Well into Byzantium era

        // Calculate difficulty with bomb
        let difficulty_with_bomb = consensus
            .calculate_difficulty_byzantium(
                U256::from(1000000),
                1000000,
                1000015,
                high_block_number,
                false,
            );

        // The bomb should make difficulty significantly higher than base adjustment
        // Base difficulty adjustment alone would be small, but bomb adds exponential growth
        assert!(
            difficulty_with_bomb > U256::from(1000000),
            "Difficulty bomb should increase difficulty significantly at high block numbers"
        );
    }

    #[test]
    fn test_pos_difficulty_validation_ignores_wrong_values() {
        let chain_spec = Arc::new(ChainSpecBuilder::mainnet().paris_activated().build());
        let consensus = EthBeaconConsensus::new(chain_spec);

        // Create parent header (post-merge) with zero difficulty
        let parent_header = reth_primitives_traits::Header {
            number: 15537393,
            difficulty: U256::ZERO,
            timestamp: 1000000,
            ..Default::default()
        };
        let parent = SealedHeader::new(parent_header, B256::ZERO);

        // Create child with NON-ZERO difficulty (this would be invalid in PoW, but PoS ignores it)
        let child_header = reth_primitives_traits::Header {
            number: 15537394,
            difficulty: U256::from(999999), // Non-zero difficulty in PoS era
            timestamp: 1000012,
            parent_hash: B256::ZERO,
            ..Default::default()
        };
        let child = SealedHeader::new(child_header, B256::ZERO);

        // PoS validation should STILL pass because we skip difficulty validation entirely
        assert_eq!(consensus.validate_against_parent_difficulty(&child, &parent), Ok(()));
    }
}
