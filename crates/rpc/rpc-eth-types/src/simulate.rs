//! Utilities for serving `eth_simulateV1`

use crate::{
    error::{api::FromEthApiError, FromEvmError, ToRpcError},
    EthApiError,
};
use alloy_chains::Chain;
use alloy_consensus::{transaction::TxHashRef, BlockHeader, Transaction as _};
use alloy_eips::eip2718::WithEncoded;
use alloy_evm::{
    block::TxResult,
    precompiles::{DynPrecompile, PrecompilesMap},
};
use alloy_network::{NetworkTransactionBuilder, TransactionBuilder};
use alloy_rpc_types_eth::{
    simulate::{SimBlock, SimCallResult, SimulateError, SimulatedBlock},
    state::StateOverride,
    BlockOverrides, BlockTransactionsKind,
};
use jsonrpsee_types::ErrorObject;
use reth_evm::{
    execute::{BlockBuilder, BlockBuilderOutcome, BlockExecutor},
    Evm, HaltReasonFor,
};
use reth_primitives_traits::{
    BlockBody as _, BlockTy, NodePrimitives, Recovered, RecoveredBlock, SealedHeader,
};
use reth_rpc_convert::{RpcBlock, RpcConvert, RpcTxReq};
use reth_rpc_server_types::result::rpc_err;
use reth_storage_api::{noop::NoopProvider, StateProvider};
use revm::{
    context::Block,
    context_interface::result::ExecutionResult,
    primitives::{Address, Bytes, TxKind, U256},
    Database,
};
use std::collections::HashMap;

/// Fallback seconds added between simulated block timestamps when neither the user nor the chain
/// hint provides a value.
const SIMULATE_FALLBACK_TIMESTAMP_INCREMENT: u64 = 12;

/// Error code for execution reverted in `eth_simulateV1`.
///
/// Consistent with `eth_call` revert error code.
///
/// <https://github.com/ethereum/execution-apis/pull/748>
pub const SIMULATE_REVERT_CODE: i32 = 3;

/// Error code for VM execution errors (e.g., out of gas) in `eth_simulateV1`.
///
/// <https://github.com/ethereum/execution-apis>
pub const SIMULATE_VM_ERROR_CODE: i32 = -32015;

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
    /// Attempted to move a non-precompile address.
    #[error("account {0} is not a precompile")]
    NotAPrecompile(Address),
    /// Attempted to move a precompile to its own address.
    #[error("cannot move precompile {0} to itself")]
    MovePrecompileToSelf(Address),
}

impl EthSimulateError {
    /// Returns the JSON-RPC error code for a `eth_simulateV1` error.
    pub const fn error_code(&self) -> i32 {
        match self {
            Self::NonceTooLow { .. } => -38010,
            Self::NonceTooHigh => -38011,
            Self::BaseFeePerGasTooLow => -38012,
            Self::IntrinsicGasTooLow => -38013,
            Self::InsufficientFunds { .. } => -38014,
            Self::BlockGasLimitExceeded => -38015,
            Self::BlockNumberInvalid { .. } => -38020,
            Self::BlockTimestampInvalid { .. } => -38021,
            Self::SenderNotEOA => -38024,
            Self::MaxInitCodeSizeExceeded => -38025,
            Self::TooManyBlocks | Self::GasLimitReached => -38026,
            Self::MovePrecompileToSelf(_) => -38022,
            Self::NotAPrecompile(_) => -32000,
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

/// Applies precompile move overrides from state overrides to the EVM's precompiles map.
///
/// This function processes `movePrecompileToAddress` entries from the state overrides and
/// moves precompiles from their original addresses to new addresses. The original address
/// is cleared (precompile removed) and the precompile is installed at the destination address.
pub fn apply_precompile_overrides(
    state_overrides: &StateOverride,
    precompiles: &mut PrecompilesMap,
) -> Result<(), EthSimulateError> {
    let moves: Vec<_> = state_overrides
        .iter()
        .filter_map(|(source, account_override)| {
            account_override.move_precompile_to.map(|dest| (*source, dest))
        })
        .collect();

    for (source, dest) in &moves {
        if source == dest {
            if precompiles.get(source).is_none() {
                return Err(EthSimulateError::NotAPrecompile(*source))
            }
            return Err(EthSimulateError::MovePrecompileToSelf(*source))
        }
    }

    // Validate every source before mutating the map, matching `move_precompiles`.
    for (source, _) in &moves {
        if precompiles.get(source).is_none() {
            return Err(EthSimulateError::NotAPrecompile(*source))
        }
    }

    // Extract all sources before handling destinations so swaps and chained moves retain the
    // original precompiles.
    let mut extracted = Vec::with_capacity(moves.len());
    for (source, dest) in moves {
        let mut moved_precompile = None;
        precompiles.apply_precompile(&source, |existing| {
            moved_precompile = existing;
            None
        });

        if let Some(precompile) = moved_precompile {
            extracted.push((dest, precompile));
        }
    }

    if !extracted.is_empty() {
        let mut moved_precompiles = HashMap::with_capacity(extracted.len());
        for (dest, precompile) in extracted {
            // Dynamic lookups are only consulted for addresses absent from the main map.
            precompiles.apply_precompile(&dest, |_| None);
            moved_precompiles.insert(dest, precompile);
        }

        precompiles.set_precompile_lookup(move |address: &Address| -> Option<DynPrecompile> {
            moved_precompiles.get(address).cloned()
        });
    }

    Ok(())
}

/// Converts all [`TransactionRequest`]s into [`Recovered`] transactions and applies them to the
/// given [`BlockExecutor`].
///
/// Returns all executed transactions and the result of the execution.
///
/// For each call without an explicit `gas` field, the remaining block gas is used as the default.
/// The RPC gas cap is tracked as a request-wide remaining budget and caps each call before
/// execution. This matches the spec rule `"gasLimit: blockGasLimit - soFarUsedGasInBlock"` and
/// geth's per-call `sanitizeCall` behavior.
///
/// [`TransactionRequest`]: alloy_rpc_types_eth::TransactionRequest
#[expect(clippy::type_complexity)]
pub fn execute_transactions<S, T>(
    mut builder: S,
    state_provider: impl StateProvider,
    calls: Vec<RpcTxReq<T::Network>>,
    remaining_call_gas_limit: &mut Option<u64>,
    chain_id: u64,
    compute_state_root: bool,
    converter: &T,
) -> Result<
    (
        BlockBuilderOutcome<S::Primitives>,
        Vec<ExecutionResult<<<S::Executor as BlockExecutor>::Evm as Evm>::HaltReason>>,
    ),
    EthApiError,
>
where
    S: BlockBuilder<Executor: BlockExecutor<Evm: Evm<DB: Database<Error: Into<EthApiError>>>>>,
    T: RpcConvert<Primitives = S::Primitives>,
{
    builder.apply_pre_execution_changes()?;

    let mut results = Vec::with_capacity(calls.len());
    let mut cumulative_tx_gas_used: u64 = 0;
    let mut block_regular_gas_used: u64 = 0;
    let mut block_state_gas_used: u64 = 0;
    let block_gas_limit = builder.evm().block().gas_limit();
    let is_amsterdam = builder.evm().cfg_env().enable_amsterdam_eip8037;
    let tx_gas_limit_cap = builder.evm().cfg_env().tx_gas_limit_cap.unwrap_or(u64::MAX);
    for mut call in calls {
        let block_gas_remaining = if is_amsterdam {
            block_gas_limit
                .saturating_sub(block_regular_gas_used)
                .min(block_gas_limit.saturating_sub(block_state_gas_used))
        } else {
            block_gas_limit.saturating_sub(cumulative_tx_gas_used)
        };
        let mut default_gas_limit = block_gas_remaining;

        if let Some(gas_limit) = call.as_ref().gas_limit() {
            let exceeds_gas_limit = if is_amsterdam {
                let regular_available_gas = block_gas_limit.saturating_sub(block_regular_gas_used);
                let state_available_gas = block_gas_limit.saturating_sub(block_state_gas_used);
                let regular_tx_gas_limit = gas_limit.min(tx_gas_limit_cap);

                regular_tx_gas_limit > regular_available_gas || gas_limit > state_available_gas
            } else {
                gas_limit > block_gas_remaining
            };

            if exceeds_gas_limit {
                return Err(EthApiError::other(EthSimulateError::BlockGasLimitExceeded))
            }
        }

        if let Some(remaining_call_gas_limit) = *remaining_call_gas_limit {
            if let Some(gas_limit) = call.as_ref().gas_limit() {
                if gas_limit > remaining_call_gas_limit {
                    call.as_mut().set_gas_limit(remaining_call_gas_limit);
                }
            } else {
                default_gas_limit = default_gas_limit.min(remaining_call_gas_limit);
            }
        }

        // Resolve transaction, populate missing fields and enforce calls
        // correctness.
        let tx = resolve_transaction(
            call,
            default_gas_limit,
            builder.evm().block().basefee(),
            chain_id,
            builder.evm_mut().db_mut(),
            converter,
        )?;
        // Create transaction with an empty envelope.
        // The effect for a layer-2 execution client is that it does not charge L1 cost.
        let tx = WithEncoded::new(Default::default(), tx);

        let mut tx_regular_gas_used = 0;
        let gas_output = builder.execute_transaction_with_result_closure(tx, |result| {
            tx_regular_gas_used = result.result().result.gas().block_regular_gas_used();
            results.push(result.result().result.clone())
        })?;

        let gas_used = gas_output.tx_gas_used();
        if let Some(remaining_call_gas_limit) = remaining_call_gas_limit.as_mut() {
            if gas_used > *remaining_call_gas_limit {
                return Err(EthApiError::other(EthSimulateError::GasLimitReached))
            }
            *remaining_call_gas_limit -= gas_used;
        }

        cumulative_tx_gas_used = cumulative_tx_gas_used.saturating_add(gas_used);
        block_regular_gas_used = block_regular_gas_used.saturating_add(tx_regular_gas_used);
        block_state_gas_used = block_state_gas_used.saturating_add(gas_output.state_gas_used());
    }

    let result = if compute_state_root {
        builder.finish(state_provider, None)?
    } else {
        builder.finish(NoopProvider::default(), None)?
    };

    Ok((result, results))
}

/// Goes over the list of [`TransactionRequest`]s and populates missing fields trying to resolve
/// them into primitive transactions.
///
/// This will set the defaults as defined in <https://github.com/ethereum/execution-apis/blob/e56d3208789259d0b09fa68e9d8594aa4d73c725/docs/ethsimulatev1-notes.md#default-values-for-transactions>
///
/// [`TransactionRequest`]: alloy_rpc_types_eth::TransactionRequest
pub fn resolve_transaction<DB: Database, Tx, T>(
    mut tx: RpcTxReq<T::Network>,
    default_gas_limit: u64,
    block_base_fee_per_gas: u64,
    chain_id: u64,
    db: &mut DB,
    converter: &T,
) -> Result<Recovered<Tx>, EthApiError>
where
    DB::Error: Into<EthApiError>,
    T: RpcConvert<Primitives: NodePrimitives<SignedTx = Tx>>,
{
    // If we're missing any fields we try to fill nonce, gas and
    // gas price.
    let tx_type = tx.as_ref().output_tx_type();

    let from = if let Some(from) = tx.as_ref().from() {
        from
    } else {
        tx.as_mut().set_from(Address::ZERO);
        Address::ZERO
    };

    if tx.as_ref().nonce().is_none() {
        tx.as_mut().set_nonce(
            db.basic(from).map_err(Into::into)?.map(|acc| acc.nonce).unwrap_or_default(),
        );
    }

    if tx.as_ref().gas_limit().is_none() {
        tx.as_mut().set_gas_limit(default_gas_limit);
    }

    if tx.as_ref().chain_id().is_none() {
        tx.as_mut().set_chain_id(chain_id);
    }

    if tx.as_ref().kind().is_none() {
        tx.as_mut().set_kind(TxKind::Create);
    }

    // if we can't build the _entire_ transaction yet, fill the fee fields.
    //
    // Per the eth_simulateV1 spec, unspecified fee fields default to 0 (not the block base fee),
    // matching geth's `CallDefaults` behavior. This lets simulation behave like a free-gas
    // `eth_call` when validation is off, and surfaces "max fee per gas less than block base fee"
    // errors when validation is on with a real base fee.
    let _ = block_base_fee_per_gas;
    if tx.as_ref().output_tx_type_checked().is_none() {
        if tx_type.is_legacy() || tx_type.is_eip2930() {
            if tx.as_ref().gas_price().is_none() {
                tx.as_mut().set_gas_price(0);
            }
        } else {
            if tx.as_ref().max_fee_per_gas().is_none() {
                tx.as_mut().set_max_fee_per_gas(0);
            }
            if tx.as_ref().max_priority_fee_per_gas().is_none() {
                tx.as_mut().set_max_priority_fee_per_gas(0);
            }
        }
    }

    let tx =
        converter.build_simulate_v1_transaction(tx).map_err(|e| EthApiError::other(e.into()))?;

    Ok(Recovered::new_unchecked(tx, from))
}

/// Handles outputs of the calls execution and builds a [`SimulatedBlock`].
pub fn build_simulated_block<Err, T>(
    block: RecoveredBlock<BlockTy<T::Primitives>>,
    results: Vec<ExecutionResult<HaltReasonFor<T::Evm>>>,
    txs_kind: BlockTransactionsKind,
    converter: &T,
) -> Result<SimulatedBlock<RpcBlock<T::Network>>, Err>
where
    Err: std::error::Error
        + FromEthApiError
        + FromEvmError<T::Evm>
        + From<T::Error>
        + Into<jsonrpsee_types::ErrorObject<'static>>,
    T: RpcConvert,
{
    let mut calls: Vec<SimCallResult> = Vec::with_capacity(results.len());

    let mut log_index = 0;
    for (index, (result, tx)) in results.into_iter().zip(block.body().transactions()).enumerate() {
        let call = match result {
            ExecutionResult::Halt { reason, gas, .. } => {
                let error = Err::from_evm_halt(reason, tx.gas_limit());
                SimCallResult {
                    return_data: Bytes::new(),
                    error: Some(SimulateError {
                        message: error.to_string(),
                        code: SIMULATE_VM_ERROR_CODE,
                        ..SimulateError::invalid_params()
                    }),
                    gas_used: gas.tx_gas_used(),
                    max_used_gas: Some(gas.total_gas_spent().max(gas.floor_gas())),
                    logs: Vec::new(),
                    status: false,
                }
            }
            ExecutionResult::Revert { output, gas, .. } => {
                let error = Err::from_revert(output.clone());
                SimCallResult {
                    return_data: Bytes::new(),
                    error: Some(SimulateError {
                        message: error.to_string(),
                        code: SIMULATE_REVERT_CODE,
                        data: Some(output),
                    }),
                    gas_used: gas.tx_gas_used(),
                    max_used_gas: Some(gas.total_gas_spent().max(gas.floor_gas())),
                    status: false,
                    logs: Vec::new(),
                }
            }
            ExecutionResult::Success { output, gas, logs, .. } => SimCallResult {
                return_data: output.into_data(),
                error: None,
                gas_used: gas.tx_gas_used(),
                max_used_gas: Some(gas.total_gas_spent().max(gas.floor_gas())),
                logs: logs
                    .into_iter()
                    .map(|log| {
                        log_index += 1;
                        alloy_rpc_types_eth::Log {
                            inner: log,
                            log_index: Some(log_index - 1),
                            transaction_index: Some(index as u64),
                            transaction_hash: Some(*tx.tx_hash()),
                            block_hash: Some(block.hash()),
                            block_number: Some(block.header().number()),
                            block_timestamp: Some(block.header().timestamp()),
                            ..Default::default()
                        }
                    })
                    .collect(),
                status: true,
            },
        };

        calls.push(call);
    }

    let block = block.into_rpc_block(
        txs_kind,
        |tx, tx_info| converter.fill(tx, tx_info),
        |header, size| converter.convert_header(header, size),
    )?;
    Ok(SimulatedBlock { inner: block, calls })
}

#[cfg(test)]
mod tests {
    use super::{apply_precompile_overrides, sanitize_chain, EthSimulateError};
    use crate::EthApiError;
    use alloy_chains::Chain;
    use alloy_consensus::Header;
    use alloy_evm::precompiles::{Precompile, PrecompilesMap};
    use alloy_primitives::{address, U256};
    use alloy_rpc_types_eth::{
        simulate::SimBlock,
        state::{AccountOverride, StateOverride},
        BlockOverrides, TransactionRequest,
    };
    use reth_primitives_traits::SealedHeader;
    use revm::precompile::Precompiles;

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
    fn precompile_self_move_requires_existing_precompile() {
        let address = address!("c100000000000000000000000000000000000000");
        let mut state_overrides = StateOverride::default();
        state_overrides.insert(
            address,
            AccountOverride { move_precompile_to: Some(address), ..Default::default() },
        );
        let mut precompiles = PrecompilesMap::from_static(Precompiles::prague());

        let err = apply_precompile_overrides(&state_overrides, &mut precompiles).unwrap_err();

        assert!(matches!(err, EthSimulateError::NotAPrecompile(addr) if addr == address));
    }

    #[test]
    fn precompile_self_move_errors_for_existing_precompile() {
        let address = address!("0000000000000000000000000000000000000001");
        let mut state_overrides = StateOverride::default();
        state_overrides.insert(
            address,
            AccountOverride { move_precompile_to: Some(address), ..Default::default() },
        );
        let mut precompiles = PrecompilesMap::from_static(Precompiles::prague());

        let err = apply_precompile_overrides(&state_overrides, &mut precompiles).unwrap_err();

        assert!(matches!(err, EthSimulateError::MovePrecompileToSelf(addr) if addr == address));
    }

    #[test]
    fn moved_precompile_is_callable_but_not_warm() {
        let source = address!("0000000000000000000000000000000000000001");
        let dest = address!("0000000000000000000000000000000000123456");
        let mut state_overrides = StateOverride::default();
        state_overrides.insert(
            source,
            AccountOverride { move_precompile_to: Some(dest), ..Default::default() },
        );
        let mut precompiles = PrecompilesMap::from_static(Precompiles::prague());

        apply_precompile_overrides(&state_overrides, &mut precompiles).unwrap();

        assert!(precompiles.get(&source).is_none());
        assert!(precompiles.get(&dest).is_some());
        assert!(!precompiles.addresses().any(|address| address == &dest));
    }

    #[test]
    fn invalid_precompile_move_does_not_apply_valid_moves() {
        let source = address!("0000000000000000000000000000000000000001");
        let dest = address!("0000000000000000000000000000000000123456");
        let invalid_source = address!("c100000000000000000000000000000000000000");
        let invalid_dest = address!("c200000000000000000000000000000000000000");
        let mut state_overrides = StateOverride::default();
        state_overrides.insert(
            source,
            AccountOverride { move_precompile_to: Some(dest), ..Default::default() },
        );
        state_overrides.insert(
            invalid_source,
            AccountOverride { move_precompile_to: Some(invalid_dest), ..Default::default() },
        );
        let mut precompiles = PrecompilesMap::from_static(Precompiles::prague());

        let err = apply_precompile_overrides(&state_overrides, &mut precompiles).unwrap_err();

        assert!(matches!(err, EthSimulateError::NotAPrecompile(addr) if addr == invalid_source));
        assert!(precompiles.get(&source).is_some());
        assert!(precompiles.get(&dest).is_none());
    }

    #[test]
    fn moved_precompile_replaces_existing_destination() {
        let source = address!("0000000000000000000000000000000000000001");
        let dest = address!("0000000000000000000000000000000000000004");
        let mut state_overrides = StateOverride::default();
        state_overrides.insert(
            source,
            AccountOverride { move_precompile_to: Some(dest), ..Default::default() },
        );
        let mut precompiles = PrecompilesMap::from_static(Precompiles::prague());
        let source_id = precompiles.get(&source).unwrap().precompile_id().clone();

        apply_precompile_overrides(&state_overrides, &mut precompiles).unwrap();

        assert!(precompiles.get(&source).is_none());
        assert_eq!(precompiles.get(&dest).unwrap().precompile_id(), &source_id);
        assert!(!precompiles.addresses().any(|address| address == &dest));
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
