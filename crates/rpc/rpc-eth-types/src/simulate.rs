//! Utilities for serving `eth_simulateV1`

use crate::{error::ToRpcError, EthApiError};
use alloy_chains::Chain;
use alloy_consensus::BlockHeader;
use alloy_primitives::U256;
use alloy_rpc_types_eth::{simulate::SimBlock, BlockId, BlockOverrides};
use jsonrpsee_types::{error::INTERNAL_ERROR_CODE, ErrorObject};
use reth_primitives_traits::SealedHeader;
use reth_rpc_server_types::result::{block_id_to_str, rpc_err};

/// Fallback seconds added between simulated block timestamps when neither the user nor the chain
/// hint provides a value.
const SIMULATE_FALLBACK_TIMESTAMP_INCREMENT: u64 = 12;

/// Errors which may occur during `eth_simulateV1` execution.
#[derive(Debug, thiserror::Error)]
pub enum EthSimulateError {
    /// Total gas limit of transactions for the block exceeds the block gas limit.
    #[error("Block gas limit exceeded by the block's transactions")]
    BlockGasLimitExceeded,
    /// Number of simulated blocks exceeds the configured client limit.
    #[error("too many blocks")]
    TooManyBlocks,
    /// Max gas limit for entire operation exceeded.
    #[error("Client adjustable limit reached")]
    GasLimitReached,
    /// Base block for the simulation was not found.
    #[error("block not found: {}", block_id_to_str(*block))]
    BlockNotFound {
        /// The block id that was requested.
        block: BlockId,
    },
    /// Block number in sequence did not increase.
    #[error("block numbers must be in order: {got} <= {parent}")]
    BlockNumberInvalid {
        /// The block number that was provided.
        got: u64,
        /// The parent block number.
        parent: u64,
    },
    /// Block timestamp in sequence did not increase.
    #[error("block timestamps must be in order: {got} <= {parent}")]
    BlockTimestampInvalid {
        /// The block timestamp that was provided.
        got: u64,
        /// The parent block timestamp.
        parent: u64,
    },
    /// Transaction nonce is too low.
    #[error("nonce too low: next nonce {state}, tx nonce {tx}")]
    NonceTooLow {
        /// Transaction nonce.
        tx: u64,
        /// Current state nonce.
        state: u64,
    },
    /// Transaction nonce is too high.
    #[error("nonce too high")]
    NonceTooHigh,
    /// Transaction nonce cannot be incremented.
    #[error("nonce has max value")]
    NonceMaxValue,
    /// Transaction's baseFeePerGas is too low.
    #[error("max fee per gas less than block base fee")]
    BaseFeePerGasTooLow,
    /// Not enough gas provided to pay for intrinsic gas.
    #[error("intrinsic gas too low")]
    IntrinsicGasTooLow,
    /// Insufficient funds to pay for gas fees and value.
    #[error("insufficient funds for gas * price + value: have {balance} want {cost}")]
    InsufficientFunds {
        /// Transaction cost.
        cost: U256,
        /// Sender balance.
        balance: U256,
    },
    /// Sender is not an EOA.
    #[error("sender is not an EOA")]
    SenderNotEOA,
    /// Max init code size exceeded.
    #[error("max initcode size exceeded")]
    MaxInitCodeSizeExceeded,
}

impl EthSimulateError {
    /// Returns the JSON-RPC error code for a `eth_simulateV1` error.
    pub const fn error_code(&self) -> i32 {
        match self {
            Self::NonceTooLow { .. } => -38010,
            Self::NonceTooHigh => -38011,
            Self::NonceMaxValue => INTERNAL_ERROR_CODE,
            Self::BaseFeePerGasTooLow => -38012,
            Self::IntrinsicGasTooLow => -38013,
            Self::InsufficientFunds { .. } => -38014,
            Self::BlockGasLimitExceeded => -38015,
            Self::BlockNumberInvalid { .. } => -38020,
            Self::BlockTimestampInvalid { .. } => -38021,
            Self::SenderNotEOA => -38024,
            Self::MaxInitCodeSizeExceeded => -38025,
            Self::TooManyBlocks | Self::GasLimitReached => -38026,
            Self::BlockNotFound { .. } => -32000,
        }
    }
}

impl ToRpcError for EthSimulateError {
    fn to_rpc_error(&self) -> ErrorObject<'static> {
        rpc_err(self.error_code(), self.to_string(), None)
    }
}

/// Sanitizes and gap-fills the chain of [`SimBlock`]s for `eth_simulateV1`.
///
/// Walks the provided block-state calls in order and:
/// - validates that each block number and timestamp strictly increases relative to the parent and
///   prior simulated block;
/// - inserts empty filler blocks for every gap in block numbers, so a request like `[block at
///   number N + k]` over a parent at `N` expands to `k - 1` empty blocks followed by the requested
///   one (per the execution-apis spec: "If the block number is increased more than `1` compared to
///   the previous block, new empty blocks are generated in between.");
/// - assigns default block numbers (`prev_number + 1`) and timestamps (`prev_timestamp + chain
///   block time`) when missing, so every returned entry has explicit `number` and `time` overrides.
///   The block time defaults to [`Chain::average_blocktime_hint`] for the chain id, falling back to
///   `SIMULATE_FALLBACK_TIMESTAMP_INCREMENT` when no hint is registered. Sub-second chain hints are
///   rounded up because block timestamps are second-granular;
/// - enforces the global `max_simulate_blocks` cap on the total number of blocks (including
///   generated fillers).
pub fn sanitize_chain<TxReq, H>(
    blocks: Vec<SimBlock<TxReq>>,
    parent: &SealedHeader<H>,
    chain_id: u64,
    max_simulate_blocks: u64,
) -> Result<Vec<SimBlock<TxReq>>, EthApiError>
where
    H: BlockHeader,
{
    let timestamp_increment = Chain::from(chain_id)
        .average_blocktime_hint()
        .map(|d| d.as_secs().saturating_add(u64::from(d.subsec_nanos() > 0)))
        .filter(|&s| s > 0)
        .unwrap_or(SIMULATE_FALLBACK_TIMESTAMP_INCREMENT);

    let mut out = Vec::with_capacity(blocks.len());
    let base_number = parent.number();
    let mut prev_number = base_number;
    let mut prev_timestamp = parent.timestamp();

    for mut block in blocks {
        let overrides = block.block_overrides.get_or_insert_with(BlockOverrides::default);

        // Default block number to prev + 1 if not specified.
        let target_number = if let Some(n) = overrides.number {
            u64::try_from(n).unwrap_or(u64::MAX)
        } else {
            let n = prev_number.saturating_add(1);
            overrides.number = Some(U256::from(n));
            n
        };

        if target_number <= prev_number {
            return Err(EthApiError::other(EthSimulateError::BlockNumberInvalid {
                got: target_number,
                parent: prev_number,
            }));
        }

        if target_number.saturating_sub(base_number) > max_simulate_blocks {
            return Err(EthApiError::other(EthSimulateError::TooManyBlocks));
        }

        // Insert empty filler blocks for any gap between prev_number and target_number.
        let gap = target_number - prev_number;
        if gap > 1 {
            for i in 1..gap {
                let filler_number = prev_number + i;
                let filler_time = prev_timestamp + timestamp_increment;
                out.push(SimBlock {
                    block_overrides: Some(BlockOverrides {
                        number: Some(U256::from(filler_number)),
                        time: Some(filler_time),
                        ..Default::default()
                    }),
                    state_overrides: None,
                    calls: Vec::new(),
                });
                prev_timestamp = filler_time;
            }
        }

        prev_number = target_number;
        // Default timestamp to prev + increment if not specified, otherwise validate ordering.
        let block_time = if let Some(t) = overrides.time {
            if t <= prev_timestamp {
                return Err(EthApiError::other(EthSimulateError::BlockTimestampInvalid {
                    got: t,
                    parent: prev_timestamp,
                }));
            }
            t
        } else {
            let t = prev_timestamp + timestamp_increment;
            overrides.time = Some(t);
            t
        };
        prev_timestamp = block_time;

        out.push(block);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{sanitize_chain, EthSimulateError, INTERNAL_ERROR_CODE};
    use crate::{error::ToRpcError, EthApiError};
    use alloy_chains::Chain;
    use alloy_consensus::Header;
    use alloy_primitives::U256;
    use alloy_rpc_types_eth::{simulate::SimBlock, BlockOverrides, TransactionRequest};
    use reth_primitives_traits::SealedHeader;

    #[test]
    fn nonce_max_value_error_uses_internal_error_code() {
        let err = EthSimulateError::NonceMaxValue.to_rpc_error();

        assert_eq!(err.code(), INTERNAL_ERROR_CODE);
        assert_eq!(err.message(), "nonce has max value");
    }

    #[test]
    fn block_not_found_error_uses_simulate_code() {
        let err = EthSimulateError::BlockNotFound { block: 100000.into() }.to_rpc_error();

        assert_eq!(err.code(), -32000);
        assert_eq!(err.message(), "block not found: 0x186a0");
    }

    fn parent_at(number: u64, timestamp: u64) -> SealedHeader<Header> {
        SealedHeader::seal_slow(Header { number, timestamp, ..Default::default() })
    }

    fn block_with_number(number: u64) -> SimBlock<TransactionRequest> {
        SimBlock {
            block_overrides: Some(BlockOverrides {
                number: Some(U256::from(number)),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn sanitize_chain_fills_gaps_with_empty_blocks() {
        // parent at block 5; user requests one block at 8 — sanitize should insert fillers at 6
        // and 7 before the requested block.
        let parent = parent_at(5, 100);
        let blocks = vec![block_with_number(8)];

        let out = sanitize_chain(blocks, &parent, Chain::mainnet().id(), 256).unwrap();
        assert_eq!(out.len(), 3);

        let numbers: Vec<u64> = out
            .iter()
            .map(|b| b.block_overrides.as_ref().unwrap().number.unwrap().try_into().unwrap())
            .collect();
        assert_eq!(numbers, vec![6, 7, 8]);

        assert!(out[0].calls.is_empty());
        assert!(out[1].calls.is_empty());

        // Mainnet hint is 12s, so timestamps should auto-increment from 100 by 12.
        let times: Vec<u64> =
            out.iter().map(|b| b.block_overrides.as_ref().unwrap().time.unwrap()).collect();
        assert_eq!(times, vec![112, 124, 136]);
    }

    #[test]
    fn sanitize_chain_defaults_missing_number_and_time() {
        let parent = parent_at(10, 1000);
        let blocks: Vec<SimBlock<TransactionRequest>> =
            vec![SimBlock::default(), SimBlock::default()];

        let out = sanitize_chain(blocks, &parent, Chain::mainnet().id(), 256).unwrap();
        assert_eq!(out.len(), 2);

        let overrides = out[0].block_overrides.as_ref().unwrap();
        assert_eq!(overrides.number.unwrap(), U256::from(11));
        assert_eq!(overrides.time, Some(1012));

        let overrides = out[1].block_overrides.as_ref().unwrap();
        assert_eq!(overrides.number.unwrap(), U256::from(12));
        assert_eq!(overrides.time, Some(1024));
    }

    #[test]
    fn sanitize_chain_uses_chain_blocktime_hint() {
        // Optimism has a 2s blocktime hint; filler/auto timestamps should reflect that.
        let parent = parent_at(0, 0);
        let blocks = vec![block_with_number(3)];

        let out = sanitize_chain(blocks, &parent, Chain::optimism_mainnet().id(), 256).unwrap();
        let times: Vec<u64> =
            out.iter().map(|b| b.block_overrides.as_ref().unwrap().time.unwrap()).collect();
        assert_eq!(times, vec![2, 4, 6]);
    }

    #[test]
    fn sanitize_chain_rounds_subsecond_blocktime_hint_up() {
        // Arbitrum has a 260ms blocktime hint. Simulated timestamps are second-granular, so this
        // rounds up to a 1s increment instead of falling back to the default.
        let parent = parent_at(0, 0);
        let blocks = vec![block_with_number(3)];

        let out = sanitize_chain(blocks, &parent, Chain::arbitrum_mainnet().id(), 256).unwrap();
        let times: Vec<u64> =
            out.iter().map(|b| b.block_overrides.as_ref().unwrap().time.unwrap()).collect();
        assert_eq!(times, vec![1, 2, 3]);
    }

    #[test]
    fn sanitize_chain_falls_back_when_chain_has_no_hint() {
        // An unknown chain id has no blocktime hint — fall back to the 12s default.
        let parent = parent_at(0, 0);
        let blocks = vec![block_with_number(2)];

        let out = sanitize_chain(blocks, &parent, Chain::from_id(123_456_789).id(), 256).unwrap();
        let times: Vec<u64> =
            out.iter().map(|b| b.block_overrides.as_ref().unwrap().time.unwrap()).collect();
        assert_eq!(times, vec![12, 24]);
    }

    #[test]
    fn sanitize_chain_rejects_non_increasing_number() {
        let parent = parent_at(10, 100);
        let err = sanitize_chain(vec![block_with_number(10)], &parent, Chain::mainnet().id(), 256)
            .unwrap_err();
        assert!(matches!(err, EthApiError::Other(_)));
    }

    #[test]
    fn sanitize_chain_enforces_max_blocks() {
        let parent = parent_at(0, 0);
        let err = sanitize_chain(vec![block_with_number(257)], &parent, Chain::mainnet().id(), 256)
            .unwrap_err();
        assert!(matches!(err, EthApiError::Other(_)));
    }
}
