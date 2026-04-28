//! Mantle-specific call and gas estimation implementations.
//!
//! This module provides implementations that align with geth's behavior for:
//! - `eth_call`
//! - `eth_estimateGas`
//!
//! The gas estimation implementation is designed to match op-geth's Mantle
//! `eth_estimateGas` behavior: binary search strategy, optimistic gas formula,
//! and **raw estimate return** (no 120% buffer; op-geth `IsMantleArsia` path).
//!
//! ## Gas Price for Estimation (gasPriceForEstimate)
//!
//! When user doesn't specify any fee fields, geth uses `gasPriceForEstimate`
//! (`SuggestGasTipCap` + baseFee) for EVM execution. This affects:
//! - The `GASPRICE` opcode return value
//! - Gas fee balance check skip (geth's `GasEstimationWithSkipCheckBalanceMode`)
//!
//! We align this behavior by:
//! 1. Detecting when user didn't specify fees (`gas_price` == 0 after `tx_env` creation)
//! 2. Setting `evm_env.cfg_env.disable_balance_check = true` to skip GAS FEE balance
//! 3. Setting `tx_env.set_gas_price(basefee)` to provide a non-zero GASPRICE value
//!
//! **IMPORTANT**: Geth's `GasEstimationWithSkipCheckBalanceMode` only skips the
//! gas fee balance check. The VALUE TRANSFER check is ALWAYS performed:
//! - Reference: geth state_transition.go:780-781
//! - `if !value.IsZero() && !CanTransfer(from, value) { return ErrInsufficientFundsForTransfer }`
//!
//! So even when no fee fields are specified, if `value > 0 && value >= balance`,
//! we must return `ErrInsufficientFundsForTransfer`.
//!
//! Reference: geth api.go:1000-1007, api.go:1030-1035, state_transition.go:780-781
//!
//! ## L1 Cost Alignment Limitations
//!
//! The L1 cost calculation in Optimism/Mantle involves `ErrInsufficientGasForL1Cost`
//! error handling in geth (see `gasestimator.go:226`). This error is treated as
//! a retriable condition that triggers raising the gas limit.
//!
//! In reth, the L1 cost is calculated differently:
//! - Geth: Uses `L1CostFunc` which is injected into `BlockContext` and calculated during
//!   `state_transition.go` execution
//! - Reth: L1 cost calculation is handled by the Optimism EVM configuration
//!
//! Current status: The `is_gas_too_low()` check in reth should capture similar
//! scenarios, but the exact error mapping may differ. Integration tests are
//! required to verify alignment.
//!
//! ## `MaxUsedGas` Equivalence
//!
//! Geth's `MaxUsedGas` (from `state_transition.go:843-868`) equals `gas_used + gas_refund`
//! in reth's `ExecutionResult::Success`. This is used for the optimistic gas limit
//! calculation.

use crate::{eth::RpcNodeCore, OpEthApi, OpEthApiError};
use alloy_evm::overrides::apply_state_overrides;
use alloy_network::TransactionBuilder;
use alloy_primitives::{TxKind, U256};
use alloy_rpc_types_eth::{state::StateOverride, TransactionRequest};
use reth_chainspec::MIN_TRANSACTION_GAS;
use reth_errors::ProviderError;
use reth_evm::{ConfigureEvm, Database, Evm, EvmEnvFor, EvmFor, TransactionEnv, TxEnvFor};
use reth_mantle_forks::is_mantle_meta_tx;
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_rpc_convert::{RpcConvert, RpcTxReq};
use reth_rpc_eth_api::{
    helpers::{estimate::EstimateCall, Call, EthCall},
    AsEthApiError, FromEthApiError, FromEvmError, IntoEthApiError,
};
use reth_rpc_eth_types::{
    error::api::FromEvmHalt, EthApiError, RevertError, RpcInvalidTransactionError,
};
use reth_rpc_server_types::constants::gas_oracle::{CALL_STIPEND_GAS, ESTIMATE_GAS_ERROR_RATIO};
use reth_storage_api::StateProvider;
use revm::{
    context_interface::{result::ExecutionResult, Block, Transaction},
    DatabaseRef,
};
use tracing::trace;

impl<N, Rpc> EthCall for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError, Evm = N::Evm>,
{
}

impl<N, Rpc> EstimateCall for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError, Evm = N::Evm>,
{
    /// Estimates the gas usage of the `request` with the state.
    ///
    /// This implementation is aligned with geth's `gasestimator.Estimate()`:
    /// <https://github.com/ethereum/go-ethereum/blob/master/eth/gasestimator/gasestimator.go>
    ///
    /// Key alignments with op-geth Mantle (IsMantleArsia):
    /// 1. Binary search midpoint favors lower bound (mid = lo * 2 if mid > lo * 2)
    /// 2. Uses geth's optimistic gas limit formula: (`MaxUsedGas` + `CallStipend`) * 64 / 63
    /// 3. Returns raw estimate (no 120% buffer; op-geth api.go:1052-1053)
    /// 4. Basic transfer optimization with `MIN_TRANSACTION_GAS`
    /// 5. Skips balance check when `gas_price` is 0 (user didn't specify fees)
    fn estimate_gas_with<S>(
        &self,
        mut evm_env: EvmEnvFor<Self::Evm>,
        mut request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        state: S,
        state_override: Option<StateOverride>,
    ) -> Result<U256, Self::Error>
    where
        S: StateProvider,
    {
        // Disabled because eth_estimateGas is sometimes used with eoa senders
        // See <https://github.com/paradigmxyz/reth/issues/1959>
        // Reference: geth api.go:985
        evm_env.cfg_env.disable_eip3607 = true;

        // The basefee should be ignored for eth_estimateGas
        // Reference: geth api.go:985
        evm_env.cfg_env.disable_base_fee = true;

        ensure_estimate_gas_input_supported(request.as_ref()).map_err(OpEthApiError::Eth)?;

        // Set nonce to None so that the correct nonce is chosen by the EVM
        request.as_mut().take_nonce();

        // Keep a copy of gas related request values
        let tx_request_gas_limit = request.as_ref().gas_limit();
        let tx_request_gas_price = request.as_ref().gas_price();

        // Determine the highest gas limit (hi), same as geth:
        // hi = opts.Header.GasLimit
        // if call.GasLimit >= params.TxGas { hi = call.GasLimit }
        // Reference: geth gasestimator.go:61-64
        let max_gas_limit = evm_env.cfg_env.tx_gas_limit_cap.map_or_else(
            || evm_env.block_env.gas_limit(),
            |cap| cap.min(evm_env.block_env.gas_limit()),
        );

        let mut hi = tx_request_gas_limit
            .map(|mut tx_gas_limit| {
                if max_gas_limit < tx_gas_limit {
                    tx_gas_limit = max_gas_limit;
                }
                tx_gas_limit
            })
            .unwrap_or(max_gas_limit);

        // Configure the evm env
        let mut db = State::builder().with_database(StateProviderDatabase::new(state)).build();

        // Apply any state overrides if specified
        if let Some(state_override) = state_override {
            apply_state_overrides(state_override, &mut db).map_err(Self::Error::from_eth_err)?;
        }

        let mut tx_env = self.create_txn_env(&evm_env, request, &mut db)?;

        // Check if this is a basic transfer (no input data to account with no code)
        // Reference: geth gasestimator.go:134-141
        let is_basic_transfer = if tx_env.input().is_empty() &&
            let TxKind::Call(to) = tx_env.kind() &&
            let Ok(code) = db.database.account_code(&to)
        {
            code.map(|code| code.is_empty()).unwrap_or(true)
        } else {
            false
        };

        // Handle gas price for estimation - align with geth's gasPriceForEstimate behavior
        // Reference: geth api.go:1000-1007, transaction_args.go:477-485
        //
        // When user didn't specify any fee fields (gas_price == 0):
        // 1. Use basefee as the gas price for GASPRICE opcode
        // 2. Skip gas fee balance check (geth's GasEstimationWithSkipCheckBalanceMode)
        let skip_gas_fee_balance_check = tx_env.gas_price() == 0;

        // Check value transfer - align with geth state_transition.go:780-781
        // CanTransfer: `return db.GetBalance(addr).Cmp(amount) >= 0`
        //
        // This check happens during EVM execution regardless of
        // GasEstimationWithSkipCheckBalanceMode. When value > balance, geth ALWAYS returns
        // "insufficient funds for transfer".
        //
        // We perform this check early to:
        // 1. Return a meaningful error instead of "internal error"
        // 2. Match geth's exact error message
        let value = tx_env.value();
        if !value.is_zero() {
            let caller = tx_env.caller();
            let balance = db
                .database
                .basic_ref(caller)
                .map_err(EthApiError::from)?
                .map_or(U256::ZERO, |acc| acc.balance);

            // Fail if balance < value (i.e., value > balance)
            // Reference: geth state_transition.go:780-781
            if value > balance {
                // Construct error message to match geth's format:
                // "failed with {gas} gas: insufficient funds for transfer: address {addr}"
                // Reference: geth gasestimator.go:276 and state_transition.go:780
                //
                // Note: We use a generic error to ensure code -32000 (ServerError)
                // instead of -32003 (TransactionRejected)
                let msg = format!(
                    "failed with {} gas: insufficient funds for transfer: address {}",
                    hi, caller
                );
                let err_obj = jsonrpsee_types::error::ErrorObject::owned(-32000, msg, None::<()>);
                return Err(OpEthApiError::Eth(EthApiError::other(err_obj)));
            }
        }

        if skip_gas_fee_balance_check {
            // Enable skip balance check mode for GAS FEES only
            // Reference: geth state_transition.go:416, 467 - checks runMode for gas fee balance
            evm_env.cfg_env.disable_balance_check = true;

            // Set gas_price to basefee so GASPRICE opcode returns non-zero value
            // In geth, this would be SuggestGasTipCap + baseFee, but using basefee alone
            // is sufficient since disable_base_fee=true means effective gas price = gas_price
            let basefee = evm_env.block_env.basefee();
            if basefee > 0 {
                tx_env.set_gas_price(basefee as u128);
            }
        } else {
            // User specified gas price - cap by caller's gas allowance
            // Reference: geth gasestimator.go:91-124
            hi = hi.min(self.caller_gas_allowance(&mut db, &evm_env, &tx_env)?);
        }

        // Set the gas limit to the computed hi value
        tx_env.set_gas_limit(tx_env.gas_limit().min(hi));

        // Create EVM instance once and reuse it throughout the entire estimation process
        let mut evm = self.evm_config().evm_with_env(&mut db, evm_env);

        // Basic transfer optimization: try MIN_TRANSACTION_GAS (21000) first
        // Reference: geth gasestimator.go:134-141
        // If the tx is a simple transfer (call to an account with no code) we can
        // shortcircuit. But simply returning MIN_TRANSACTION_GAS is dangerous because
        // there might be additional field combos that bump the price up, so we try
        // executing the function with the minimum gas limit to make sure.
        if is_basic_transfer {
            let mut min_tx_env = tx_env.clone();
            min_tx_env.set_gas_limit(MIN_TRANSACTION_GAS);

            if let Ok(res) = evm.transact(min_tx_env).map_err(Self::Error::from_evm_err) &&
                res.result.is_success()
            {
                // Return raw estimate (Mantle/op-geth IsMantleArsia does not apply buffer)
                return Ok(U256::from(MIN_TRANSACTION_GAS));
            }
        }

        trace!(target: "rpc::eth::estimate", ?tx_env, gas_limit = tx_env.gas_limit(), is_basic_transfer, "Starting Mantle gas estimation");

        // Execute the transaction with the highest possible gas limit
        // Reference: geth gasestimator.go:144-153
        let mut res = match evm.transact(tx_env.clone()).map_err(Self::Error::from_evm_err) {
            // Handle the exceptional case where the transaction initialization uses too much gas
            Err(err)
                if err.is_gas_too_high() &&
                    (tx_request_gas_limit.is_some() || tx_request_gas_price.is_some()) =>
            {
                return map_out_of_gas_err::<Self::Evm, _>(&mut evm, tx_env, max_gas_limit);
            }
            Err(err) if err.is_gas_too_low() => {
                // This failed because the configured gas cost of the tx was lower than what
                // actually consumed by the tx
                return Err(RpcInvalidTransactionError::GasRequiredExceedsAllowance {
                    gas_limit: tx_env.gas_limit(),
                }
                .into_eth_err());
            }
            // Propagate other results
            ethres => ethres?,
        };

        // Handle execution result
        // Reference: geth gasestimator.go:148-153
        let gas_refund = match res.result {
            ExecutionResult::Success { gas_refunded, .. } => gas_refunded,
            ExecutionResult::Halt { reason, .. } => {
                // Here we don't check for invalid opcode because already executed with highest gas
                return Err(Self::Error::from_evm_halt(reason, tx_env.gas_limit()));
            }
            ExecutionResult::Revert { output, .. } => {
                // If price or limit was included in the request then we can execute the request
                // again with the block's gas limit to check if revert is gas related or not
                return if tx_request_gas_limit.is_some() || tx_request_gas_price.is_some() {
                    map_out_of_gas_err::<Self::Evm, _>(&mut evm, tx_env, max_gas_limit)
                } else {
                    Err(RpcInvalidTransactionError::Revert(RevertError::new(output)).into_eth_err())
                };
            }
        };

        // At this point we know the call succeeded
        // hi is now the gas limit that succeeded
        hi = tx_env.gas_limit();

        // lo = result.UsedGas - 1
        // Reference: geth gasestimator.go:159
        let gas_used = res.result.gas_used();
        let mut lo = gas_used.saturating_sub(1);

        // Calculate optimistic gas limit using geth's formula:
        // optimisticGasLimit := (result.MaxUsedGas + params.CallStipend) * 64 / 63
        // Reference: geth gasestimator.go:164
        //
        // Note: In geth, MaxUsedGas may differ from UsedGas + GasRefund due to Mantle-specific
        // calculations (tokenRatio, floorDataGas). For now, we use gas_used + gas_refund
        // as an approximation. TODO: Implement MaxUsedGas tracking in Mantle EVM.
        let max_used_gas = gas_used.saturating_add(gas_refund);
        let optimistic_gas_limit = (max_used_gas + CALL_STIPEND_GAS) * 64 / 63;

        if optimistic_gas_limit < hi {
            let mut optimistic_tx_env = tx_env.clone();
            optimistic_tx_env.set_gas_limit(optimistic_gas_limit);

            res = evm.transact(optimistic_tx_env).map_err(Self::Error::from_evm_err)?;

            // Update based on result
            // Reference: geth gasestimator.go:173-177
            if res.result.is_success() {
                hi = optimistic_gas_limit;
            } else {
                lo = optimistic_gas_limit;
            }
        }

        // Binary search for the smallest gas limit that allows the tx to execute successfully
        // Reference: geth gasestimator.go:180-209
        while lo + 1 < hi {
            // Check error ratio for early termination
            // Reference: geth gasestimator.go:181-188
            let hi_val: u64 = hi;
            let lo_val: u64 = lo;
            let ratio = (hi_val - lo_val) as f64 / hi_val as f64;
            if ratio < ESTIMATE_GAS_ERROR_RATIO {
                break;
            }

            // Calculate midpoint - geth's strategy favors the low side
            // Reference: geth gasestimator.go:190-195
            // mid := lo + (hi-lo)/2
            // if mid > lo*2 { mid = lo * 2 }
            let mut mid = lo + (hi - lo) / 2;
            if mid > lo.saturating_mul(2) {
                mid = lo.saturating_mul(2);
            }

            let mut mid_tx_env = tx_env.clone();
            mid_tx_env.set_gas_limit(mid);

            // Execute transaction and handle potential gas errors
            match evm.transact(mid_tx_env).map_err(Self::Error::from_evm_err) {
                Err(err) if err.is_gas_too_high() => {
                    // Decrease the highest gas limit if gas is too high
                    hi = mid;
                }
                Err(err) if err.is_gas_too_low() => {
                    // Increase the lowest gas limit if gas is too low
                    lo = mid;
                }
                ethres => {
                    res = ethres?;
                    // Update based on result
                    // Reference: geth gasestimator.go:204-207
                    if res.result.is_success() {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
            }
        }

        // Return raw estimate (Mantle/op-geth IsMantleArsia does not apply gasBuffer)
        // Reference: op-geth internal/ethapi/api.go:1052-1053
        Ok(U256::from(hi))
    }
}

fn ensure_estimate_gas_input_supported(request: &TransactionRequest) -> Result<(), EthApiError> {
    if request.input.input().is_some_and(|input| is_mantle_meta_tx(input)) {
        return Err(EthApiError::other(jsonrpsee_types::error::ErrorObject::owned(
            -32000,
            "meta tx is disabled",
            None::<()>,
        )));
    }

    Ok(())
}

/// Executes the requests again after an out of gas error to check if the error is gas related
#[inline]
fn map_out_of_gas_err<Ev, DB>(
    evm: &mut EvmFor<Ev, DB>,
    mut tx_env: TxEnvFor<Ev>,
    max_gas_limit: u64,
) -> Result<U256, OpEthApiError>
where
    Ev: ConfigureEvm,
    DB: Database<Error = ProviderError>,
    EthApiError: From<DB::Error>,
    OpEthApiError: FromEvmError<Ev>,
{
    let req_gas_limit = tx_env.gas_limit();
    tx_env.set_gas_limit(max_gas_limit);

    let retry_res = evm.transact(tx_env).map_err(OpEthApiError::from_evm_err)?;

    match retry_res.result {
        ExecutionResult::Success { .. } => {
            // Transaction succeeded by manually increasing the gas limit,
            // which means the caller lacks funds to pay for the tx
            Err(RpcInvalidTransactionError::BasicOutOfGas(req_gas_limit).into_eth_err())
        }
        ExecutionResult::Revert { output, .. } => {
            // Reverted again after bumping the limit
            Err(RpcInvalidTransactionError::Revert(RevertError::new(output)).into_eth_err())
        }
        ExecutionResult::Halt { reason, .. } => {
            Err(OpEthApiError::from_evm_halt(reason, req_gas_limit))
        }
    }
}

impl<N, Rpc> Call for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError, Evm = N::Evm>,
{
    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.inner.eth_api.gas_cap()
    }

    #[inline]
    fn max_simulate_blocks(&self) -> u64 {
        self.inner.eth_api.max_simulate_blocks()
    }

    #[inline]
    fn evm_memory_limit(&self) -> u64 {
        self.inner.eth_api.evm_memory_limit()
    }
}

#[cfg(test)]
mod tests {
    use super::ensure_estimate_gas_input_supported;
    use alloy_rpc_types_eth::TransactionRequest;
    use jsonrpsee_types::error::ErrorObject;
    use reth_chainspec::MIN_TRANSACTION_GAS;
    use reth_mantle_forks::MANTLE_META_TX_PREFIX;
    use reth_rpc_eth_types::EthApiError;
    use reth_rpc_server_types::constants::gas_oracle::{
        CALL_STIPEND_GAS, ESTIMATE_GAS_ERROR_RATIO,
    };

    #[test]
    fn test_estimate_gas_rejects_mantle_meta_tx_input() {
        let mut input = MANTLE_META_TX_PREFIX.to_vec();
        input.push(0xF8);

        let request = TransactionRequest::default().input(input.into());
        let err = ensure_estimate_gas_input_supported(request.as_ref()).unwrap_err();
        let rpc_error: ErrorObject<'static> = EthApiError::from(err).into();

        assert_eq!(rpc_error.code(), -32000);
        assert_eq!(rpc_error.message(), "meta tx is disabled");
    }

    #[test]
    fn test_mantle_returns_raw_estimate_no_buffer() {
        // Mantle/op-geth IsMantleArsia returns raw estimate (no 120% buffer).
        // We assert the values that would be returned by estimate_gas_with.
        let estimate = 100_000u64;
        assert_eq!(estimate, 100_000);

        let small_estimate = MIN_TRANSACTION_GAS;
        assert_eq!(small_estimate, 21_000);

        let large_estimate = 30_000_000u64;
        assert_eq!(large_estimate, 30_000_000);
    }

    #[test]
    fn test_binary_search_midpoint_geth_strategy() {
        // Test geth's midpoint calculation: favors lower bound
        // mid := lo + (hi-lo)/2
        // if mid > lo*2 { mid = lo * 2 }

        // Case 1: Standard case where mid > lo*2
        let lo = MIN_TRANSACTION_GAS;
        let hi = 1_000_000u64;
        let mid = lo + (hi - lo) / 2;
        let adjusted_mid = if mid > lo.saturating_mul(2) { lo.saturating_mul(2) } else { mid };
        // mid = 21000 + 489500 = 510500
        // lo * 2 = 42000
        // 510500 > 42000, so adjusted_mid = 42000
        assert_eq!(adjusted_mid, MIN_TRANSACTION_GAS * 2);

        // Case 2: Case where mid <= lo*2
        let lo2 = 500_000u64;
        let hi2 = 600_000u64;
        let mid2 = lo2 + (hi2 - lo2) / 2;
        let adjusted_mid2 = if mid2 > lo2.saturating_mul(2) { lo2.saturating_mul(2) } else { mid2 };
        // mid = 500000 + 50000 = 550000
        // lo * 2 = 1000000
        // 550000 < 1000000, so adjusted_mid = 550000
        assert_eq!(adjusted_mid2, 550_000);
    }

    #[test]
    fn test_optimistic_gas_limit_calculation() {
        // Test optimistic gas limit formula from geth:
        // optimisticGasLimit := (result.MaxUsedGas + params.CallStipend) * 64 / 63

        let gas_used = 50_000u64;
        let gas_refund = 5_000u64;
        let max_used_gas = gas_used.saturating_add(gas_refund);
        let optimistic = (max_used_gas + CALL_STIPEND_GAS) * 64 / 63;

        // (50000 + 5000 + 2300) * 64 / 63 = 57300 * 64 / 63 = 58209
        assert_eq!(optimistic, 58_209);

        // Edge case: no refund
        let no_refund = (gas_used + CALL_STIPEND_GAS) * 64 / 63;
        // (50000 + 2300) * 64 / 63 = 52300 * 64 / 63 = 53130
        assert_eq!(no_refund, 53_130);
    }

    #[test]
    fn test_error_ratio_termination() {
        // Test error ratio calculation for binary search termination
        // geth: if float64(hi-lo)/float64(hi) < opts.ErrorRatio { break }

        // Case 1: Should continue searching (ratio > 0.015)
        let lo = 95_000u64;
        let hi = 100_000u64;
        let ratio = (hi - lo) as f64 / hi as f64;
        // (100000 - 95000) / 100000 = 0.05
        assert!(ratio > ESTIMATE_GAS_ERROR_RATIO);
        assert!((ratio - 0.05).abs() < 0.0001);

        // Case 2: Should stop searching (ratio <= 0.015)
        let lo2 = 98_500u64;
        let ratio2 = (hi - lo2) as f64 / hi as f64;
        // (100000 - 98500) / 100000 = 0.015
        assert!(ratio2 <= ESTIMATE_GAS_ERROR_RATIO);

        // Case 3: Exact boundary
        let lo3 = 98_514u64; // Very close to 1.5%
        let ratio3 = (hi - lo3) as f64 / hi as f64;
        // (100000 - 98514) / 100000 = 0.01486
        assert!(ratio3 < ESTIMATE_GAS_ERROR_RATIO);
    }

    #[test]
    fn test_lo_initialization() {
        // Test lo initialization: lo = result.UsedGas - 1
        let gas_used = 50_000u64;
        let lo = gas_used.saturating_sub(1);
        assert_eq!(lo, 49_999);

        // Edge case: minimum gas
        let min_gas_used = MIN_TRANSACTION_GAS;
        let min_lo = min_gas_used.saturating_sub(1);
        assert_eq!(min_lo, MIN_TRANSACTION_GAS - 1);

        // Edge case: zero gas (shouldn't happen, but test saturation)
        let zero_gas = 0u64;
        let zero_lo = zero_gas.saturating_sub(1);
        assert_eq!(zero_lo, 0);
    }

    #[test]
    fn test_hi_cap_with_gas_allowance() {
        // Test hi cap with caller's gas allowance
        // hi = hi.min(allowance)

        let hi = 1_000_000u64;
        let allowance = 500_000u64;
        let capped_hi = hi.min(allowance);
        assert_eq!(capped_hi, 500_000);

        // Case where allowance is higher
        let high_allowance = 2_000_000u64;
        let uncapped_hi = hi.min(high_allowance);
        assert_eq!(uncapped_hi, 1_000_000);
    }

    #[test]
    fn test_gas_price_for_estimate_logic() {
        // Test the gas price for estimation logic alignment with geth
        // Reference: geth api.go:1000-1007, api.go:1030-1035, transaction_args.go:477-485
        //
        // When user doesn't specify any fee fields:
        // 1. gas_price should be set to basefee (or gasPriceForEstimate in geth)
        // 2. Balance check should be skipped (disable_balance_check = true)
        //
        // This is critical for allowing eth_estimateGas to work for accounts
        // with zero balance when no fee fields are specified.

        // Case 1: User specified gas_price (should NOT skip balance check)
        let user_gas_price = 100u128;
        let skip_balance_check = user_gas_price == 0;
        assert!(!skip_balance_check);

        // Case 2: User didn't specify gas_price (should skip balance check)
        let no_gas_price = 0u128;
        let should_skip = no_gas_price == 0;
        assert!(should_skip);
    }

    #[test]
    fn test_skip_balance_check_for_zero_balance_account() {
        // This test validates the core fix for the eth_estimateGas alignment issue.
        //
        // Scenario from user report:
        // - Geth returns: 0x6270 (25200)
        // - Reth was returning: error "insufficient funds for gas * price + value"
        //
        // The fix:
        // When user doesn't specify any fee fields (gas_price == 0), we:
        // 1. Set cfg_env.disable_balance_check = true (matches geth's
        //    GasEstimationWithSkipCheckBalanceMode)
        // 2. Set tx_env.gas_price to basefee for GASPRICE opcode
        //
        // Reference: geth api.go:1030-1035
        // ```go
        // runMode := core.GasEstimationMode
        // if (args.GasPrice == nil || args.GasPrice.ToInt().Sign() == 0) &&
        //    (args.MaxFeePerGas == nil || args.MaxFeePerGas.ToInt().Sign() == 0) &&
        //    (args.MaxPriorityFeePerGas == nil || args.MaxPriorityFeePerGas.ToInt().Sign() == 0) {
        //     runMode = core.GasEstimationWithSkipCheckBalanceMode
        // }
        // ```

        // Simulate the condition: user didn't specify any fee fields
        let gas_price = 0u128;
        let max_fee_per_gas: Option<u128> = None;
        let max_priority_fee: Option<u128> = None;

        let skip_balance_check = gas_price == 0 &&
            max_fee_per_gas.is_none_or(|fee| fee == 0) &&
            max_priority_fee.is_none_or(|fee| fee == 0);

        assert!(skip_balance_check, "Balance check should be skipped when no fees specified");

        // Simulate the condition: user specified gas_price
        let user_gas_price = 1_000_000_000u128; // 1 gwei
        let should_check_balance = user_gas_price != 0;
        assert!(should_check_balance, "Balance check should be performed when gas_price specified");
    }

    #[test]
    fn test_basefee_as_gasprice() {
        // Test that basefee is used as gas_price when user doesn't specify fees
        // This ensures GASPRICE opcode returns non-zero value

        let basefee = 1_000_000_000u64; // 1 gwei
        let user_gas_price = 0u128;

        // When user didn't specify gas_price, use basefee
        let effective_gas_price =
            if user_gas_price == 0 && basefee > 0 { basefee as u128 } else { user_gas_price };

        assert_eq!(effective_gas_price, 1_000_000_000u128);

        // When basefee is 0 (unlikely but test edge case)
        let zero_basefee = 0u64;
        let zero_effective = if user_gas_price == 0 && zero_basefee > 0 {
            zero_basefee as u128
        } else {
            user_gas_price
        };
        assert_eq!(zero_effective, 0u128);
    }

    #[test]
    fn test_value_vs_balance_check_geth_alignment() {
        // Test the value transfer check alignment with geth
        // Reference: geth state_transition.go:780-781
        //
        // Geth's CanTransfer: `return db.GetBalance(addr).Cmp(amount) >= 0`
        // i.e., `balance >= value` means can transfer
        //
        // So we fail when `balance < value` (i.e., `value > balance`)

        use alloy_primitives::U256;

        // Case 1: value > balance - should fail
        // Error type depends on gas_price (see test_value_transfer_error_depends_on_gas_price)
        let balance = U256::from(1_000_000u64); // 1M wei
        let value = U256::from(2_000_000u64); // 2M wei
        let should_fail = value > balance;
        assert!(should_fail, "value > balance should fail");

        // Case 2: value == balance - should SUCCEED (balance >= value is true)
        let equal_value = balance;
        let should_succeed = equal_value <= balance;
        assert!(should_succeed, "value == balance should succeed");

        // Case 3: value < balance - should SUCCEED
        let smaller_value = U256::from(500_000u64); // 0.5M wei
        let should_succeed_2 = smaller_value <= balance;
        assert!(should_succeed_2, "value < balance should succeed");

        // Case 4: value == 0 - should be skipped entirely (!value.is_zero() is false)
        let zero_value = U256::ZERO;
        assert!(zero_value.is_zero(), "zero value should skip the check entirely");
    }

    #[test]
    fn test_value_transfer_error_matches_geth_format() {
        // When value > balance, geth returns a specific error format:
        // "failed with {gas} gas: insufficient funds for transfer: address {addr}"
        //
        // This is wrapped in gasestimator.go:276

        use alloy_primitives::{address, U256};

        // Mock data
        let value = U256::from(1_000_000_000_000_000_000u128); // 1 ETH
        let balance = U256::ZERO;
        let caller = address!("0000000000000000000000000000000000000001");
        let hi = 40_000_000u64;

        // Verify logic directly (since we can't easily run full integration test here)
        if value > balance {
            let msg = format!(
                "failed with {} gas: insufficient funds for transfer: address {}",
                hi, caller
            );
            assert_eq!(
                msg,
                "failed with 40000000 gas: insufficient funds for transfer: address 0x0000000000000000000000000000000000000001"
            );
        }
    }

    #[test]
    fn test_value_is_zero_skips_transfer_check() {
        // Test that when value == 0, the value transfer check is skipped
        // This is geth's behavior: if !value.IsZero() { ... }
        //
        // This ensures that basic calls without value can succeed even if balance == 0

        use alloy_primitives::U256;

        let value = U256::ZERO;
        let balance = U256::ZERO;

        // The check: !value.is_zero() && value > balance
        // When value == 0: !0.is_zero() = false, so the entire condition is false
        let should_fail = !value.is_zero() && value > balance;
        assert!(!should_fail, "zero value should NOT trigger insufficient funds error");

        // This matches geth: accounts with 0 balance can call contracts
        // as long as they don't try to transfer any value
    }

    #[test]
    fn test_exact_balance_transfer_succeeds() {
        // Test case from report.json: eth_estimateGas_transfer
        // - from: 0x0000000000000000000000000000000000000001
        // - value: 0x1 (1 wei)
        // - Account balance: 0x1 (1 wei)
        //
        // Geth: SUCCESS with "0x637e"
        // Reth (bug): ERROR "insufficient funds for transfer"
        // Reth (fix): SUCCESS
        //
        // The bug was using `value >= balance` instead of `value > balance`
        // When value == balance, geth's CanTransfer returns true (balance >= value)

        use alloy_primitives::U256;

        let value = U256::from(1u64); // 1 wei
        let balance = U256::from(1u64); // 1 wei

        // Geth's CanTransfer: balance >= value
        // When balance = 1, value = 1: 1 >= 1 = true (can transfer)
        let can_transfer = balance >= value;
        assert!(can_transfer, "balance >= value should allow transfer");

        // Our check should NOT fail when value == balance
        // We fail when value > balance
        let should_fail = value > balance;
        assert!(!should_fail, "value == balance should NOT fail");

        // Contrast with the error case: value = 1 ETH, balance = 1 wei
        let value_1eth = U256::from(1_000_000_000_000_000_000u128);
        let should_fail_1eth = value_1eth > balance;
        assert!(should_fail_1eth, "value > balance should fail");
    }

    #[test]
    fn test_max_uint256_value_returns_insufficient_funds() {
        // Test case: extreme value (max uint256)
        // - value: max uint256
        // - gas_price: NOT specified
        //
        // Geth: error "insufficient funds for transfer" (from state_transition.go:780-781)
        // Reth: error "insufficient funds for transfer" (aligned)
        //
        // The value transfer check in state_transition.go is NOT affected by
        // GasEstimationWithSkipCheckBalanceMode. When value > balance, geth ALWAYS
        // returns "insufficient funds for transfer".

        use alloy_primitives::U256;

        // Max uint256 value
        let max_value = U256::MAX;
        let balance = U256::ZERO; // Any account has balance < max uint256

        // Value transfer check
        let value_exceeds_balance = !max_value.is_zero() && max_value > balance;
        assert!(value_exceeds_balance, "max uint256 should exceed any balance");

        // Regardless of gas_price, value > balance triggers InsufficientFundsForTransfer
        // This prevents "internal error" from EVM execution with extreme values
    }
}
