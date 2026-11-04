//! Estimate gas needed implementation

use super::{call::request_gas_price, Call, LoadPendingBlock};
use crate::{AsEthApiError, FromEthApiError, FromEvmError, IntoEthApiError};
use alloy_consensus::Transaction;
use alloy_network::TransactionBuilder;
use alloy_primitives::{TxKind, U256};
use alloy_rpc_types_eth::{state::EvmOverrides, BlockId};
use evm2::{
    ethereum::eip8141::FrameGas,
    evm::{CacheDB, Db, DynDatabase},
    interpreter::InstrStop,
    EvmFeatures, TxResult,
};
use futures::Future;
use reth_chainspec::MIN_TRANSACTION_GAS;
use reth_evm::{
    database::StateProviderDatabase, ConfigureEvm, Evm, EvmEnv, EvmEnvFor, EvmTypesFor,
    TransactionEnvMut,
};
use reth_rpc_convert::{RpcConvert, RpcTxReq};
use reth_rpc_eth_types::{
    cache::db::{apply_block_overrides, apply_state_overrides},
    error::api::{FromEvmHalt, FromRevert},
    EthApiError, RpcInvalidTransactionError,
};
use reth_rpc_server_types::constants::gas_oracle::{CALL_STIPEND_GAS, ESTIMATE_GAS_ERROR_RATIO};
use reth_storage_api::EvmStateProvider;
use reth_tasks::cancel::is_cancelled;
use tracing::trace;

/// Gas execution estimates
pub trait EstimateCall: Call {
    /// Estimates the gas usage of the `request` with the state.
    ///
    /// This will execute the [`RpcTxReq`] and find the best gas limit via binary search.
    ///
    /// ## EVM settings
    ///
    /// This modifies certain EVM settings to mirror geth's `SkipAccountChecks` when transacting requests, see also: <https://github.com/ethereum/go-ethereum/blob/380688c636a654becc8f114438c2a5d93d2db032/core/state_transition.go#L145-L148>:
    ///
    ///  - `disable_eip3607` is set to `true`
    ///  - `disable_base_fee` is set to `true`
    ///  - `disable_fee_charge` is set to `true`
    ///  - `nonce` is set to `None`
    fn estimate_gas_with<S>(
        &self,
        evm_env: EvmEnvFor<Self::Evm>,
        request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        state: S,
        overrides: EvmOverrides,
    ) -> Result<U256, Self::Error>
    where
        S: EvmStateProvider,
    {
        let db = CacheDB::new(Db::new(StateProviderDatabase::new(state)));
        self.estimate_gas_with_cache(evm_env, request, db, overrides)
    }

    /// Estimates execution using a cache that already contains pre-execution state changes.
    fn estimate_gas_with_cache<DB: DynDatabase>(
        &self,
        mut evm_env: EvmEnvFor<Self::Evm>,
        mut request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        mut db: CacheDB<DB>,
        overrides: EvmOverrides,
    ) -> Result<U256, Self::Error> {
        // Disabled because eth_estimateGas is sometimes used with eoa senders
        // See <https://github.com/paradigmxyz/reth/issues/1959>
        evm_env.version_mut().features.remove(EvmFeatures::EIP3607);

        // The basefee should be ignored for eth_estimateGas and similar
        // See:
        // <https://github.com/ethereum/go-ethereum/blob/ee8e83fa5f6cb261dad2ed0a7bbcde4930c41e6c/internal/ethapi/api.go#L985>
        evm_env.version_mut().features.remove(EvmFeatures::BASE_FEE_CHECK);

        // Disable additional fee charges (e.g. L2 operator fees) for gas estimation,
        // consistent with `prepare_call_env` for `eth_call`.
        evm_env.version_mut().features.remove(
            EvmFeatures::FEE_CHARGE | EvmFeatures::BALANCE_CHECK | EvmFeatures::BALANCE_TOP_UP,
        );

        let is_frame = request.as_ref().minimal_tx_type().is_eip8141();
        evm_env.version_mut().features.set(EvmFeatures::FRAME_SIGNATURE_PLACEHOLDERS, is_frame);
        if is_frame {
            request.as_mut().gas = None;
            request.as_mut().signatures.get_or_insert_with(Vec::new);
            if request.as_ref().eip8141_fees.is_none() {
                request.as_mut().max_fee.get_or_insert(U256::MAX);
                request.as_mut().max_priority_fee_per_gas.get_or_insert(0);
            }
        } else {
            request.as_mut().take_nonce();
        }

        // Keep a copy of gas related request values
        let tx_request_gas_limit = request.as_ref().gas_limit();
        let tx_request_gas_price = request.as_ref().gas_price();

        // Apply any block overrides before deriving block-derived limits and the tx env so
        // overrides for `gasLimit`, `baseFee` and `blobBaseFee` are visible to estimation.
        // Mirrors geth's behavior, see:
        // <https://github.com/ethereum/go-ethereum/pull/30695>
        if let Some(block_overrides) = overrides.block {
            apply_block_overrides(*block_overrides, &mut db, &mut evm_env);
        }

        // Apply any state overrides if specified.
        if let Some(state_override) = overrides.state {
            apply_state_overrides(state_override, &mut db).map_err(Self::Error::from_eth_err)?;
        }

        // the gas limit of the corresponding block
        let block_gas_limit = evm_env.block_env().gas_limit.to::<u64>();
        // If EIP-8037 is enabled, the transaction gas limit cap is not applicable
        let mut max_gas_limit = evm_env.version().tx_gas_caps().0.min(block_gas_limit);

        // Also bound diagnostic retries by the RPC gas cap. Zero means unlimited.
        let gas_cap = self.call_gas_limit();
        if gas_cap != 0 {
            max_gas_limit = max_gas_limit.min(gas_cap);
        }

        let mut highest_gas_limit =
            tx_request_gas_limit.unwrap_or(max_gas_limit).min(max_gas_limit);

        // Check if this is a basic transfer (no input data to account with no code)
        let is_basic_transfer = if request.as_ref().input().is_none_or(|input| input.is_empty()) &&
            let Some(TxKind::Call(to)) = request.as_ref().kind()
        {
            // Fetch the account through the DB so the state overrides applied above are visible.
            match db.get_account(&to) {
                Ok(Some(account)) => account.is_empty_code_hash(),
                Ok(None) => true,
                Err(_) => false,
            }
        } else {
            false
        };

        let request_gas_price = request_gas_price(request.as_ref(), evm_env.block_base_fee());
        let mut tx_env = self.create_txn_env(&evm_env, request, &mut db)?;

        // Check funds of the sender (only useful to check if transaction gas price is more than 0).
        //
        // The caller allowance reserves the value and maximum blob fee before funding execution
        // gas at the transaction's gas price.
        if !is_frame && request_gas_price > 0 {
            let allowance = self.caller_gas_allowance(&mut db, &evm_env, &tx_env)?;
            highest_gas_limit = highest_gas_limit.min(allowance);
        }

        if let Some(frame) = tx_env.frame_transaction() {
            let gas = FrameGas::calculate(frame, evm_env.version())
                .map_err(reth_evm::execute::map_handler_error)
                .map_err(Self::Error::from_evm_err)?;
            if self.call_gas_limit() != 0 && gas.maximum > self.call_gas_limit() {
                return Err(EthApiError::InvalidParams(
                    "frame transaction exceeds the RPC gas cap".into(),
                )
                .into());
            }
            let mut evm = self.evm_config().evm_with_env(&mut db, evm_env);
            let result = evm.transact_result(&tx_env).map_err(Self::Error::from_evm_err)?;
            if result.frame.is_none() {
                return Err(EthApiError::EvmCustom(
                    "frame transaction execution returned a non-frame result".into(),
                )
                .into());
            }
            return Ok(U256::from(gas.maximum));
        }

        trace!(target: "rpc::eth::estimate", ?tx_env, gas_limit = highest_gas_limit, is_basic_transfer, "Starting gas estimation");

        let mut evm = self.evm_config().evm_with_env(&mut db, evm_env);
        let mut execute = move |gas_limit| {
            tx_env.inner_mut().set_gas_limit(gas_limit);
            evm.transact_result(&tx_env).map_err(Self::Error::from_evm_err)
        };

        // For basic transfers, try using minimum gas before running full binary search
        if is_basic_transfer &&
            let Ok(res) = execute(MIN_TRANSACTION_GAS.min(max_gas_limit)) &&
            res.status
        {
            return Ok(U256::from(res.tx_gas_used()))
        }

        // Execute the transaction with the highest possible gas limit.
        let mut res = match execute(highest_gas_limit) {
            // Handle the exceptional case where the transaction initialization uses too much
            // gas. If the gas price or gas limit was specified in the request,
            // retry the transaction with the block's gas limit to determine if
            // the failure was due to insufficient gas.
            Err(err)
                if err.is_gas_too_high() &&
                    (tx_request_gas_limit.is_some() || tx_request_gas_price.is_some()) =>
            {
                let retry = execute(max_gas_limit)?;
                return map_out_of_gas_result::<_, EvmTypesFor<Self::Evm>>(retry, highest_gas_limit)
            }
            Err(err) if err.is_gas_too_low() => {
                // This failed because the configured gas cost of the tx was lower than what
                // actually consumed by the tx This can happen if the
                // request provided fee values manually and the resulting gas cost exceeds the
                // sender's allowance, so we return the appropriate error here
                return Err(RpcInvalidTransactionError::GasRequiredExceedsAllowance {
                    gas_limit: highest_gas_limit,
                }
                .into_eth_err());
            }
            // Propagate other results (successful or other errors).
            ethres => ethres?,
        };

        let gas_refund = if res.status {
            res.final_refunded()
        } else if res.stop == InstrStop::Revert {
            // if price or limit was included in the request then we can execute the request
            // again with the block's gas limit to check if revert is gas related or not
            return if tx_request_gas_limit.is_some() || tx_request_gas_price.is_some() {
                let retry = execute(max_gas_limit)?;
                map_out_of_gas_result::<_, EvmTypesFor<Self::Evm>>(retry, highest_gas_limit)
            } else {
                // the transaction did revert
                Err(Self::Error::from_revert(res.output))
            }
        } else {
            // here we don't check for invalid opcode because already executed with highest gas
            // limit
            return Err(Self::Error::from_evm_halt(res.stop, highest_gas_limit))
        };

        // At this point we know the call succeeded but want to find the _best_ (lowest) gas the
        // transaction succeeds with. We find this by doing a binary search over the possible range.

        // NOTE: this is the gas the transaction used, which is less than the
        // transaction requires to succeed.
        let mut gas_used = res.tx_gas_used();
        // the lowest value is capped by the gas used by the unconstrained transaction
        let mut lowest_gas_limit = gas_used.saturating_sub(1);

        // As stated in Geth, there is a good chance that the transaction will pass if we set the
        // gas limit to the execution gas used plus the gas refund, so we check this first
        // <https://github.com/ethereum/go-ethereum/blob/a5a4fa7032bb248f5a7c40f4e8df2b131c4186a4/eth/gasestimator/gasestimator.go#L135
        //
        // Calculate the optimistic gas limit by adding gas used and gas refund,
        // then applying a 64/63 multiplier to account for gas forwarding rules.
        let optimistic_gas_limit = (gas_used + gas_refund + CALL_STIPEND_GAS) * 64 / 63;
        if optimistic_gas_limit < highest_gas_limit {
            // Re-execute the transaction with the new gas limit and update the result and
            // environment.
            res = execute(optimistic_gas_limit)?;

            // Update the gas used based on the new result.
            gas_used = res.tx_gas_used();
            // Update the gas limit estimates (highest and lowest) based on the execution result.
            update_estimated_gas_range::<EvmTypesFor<Self::Evm>>(
                res,
                optimistic_gas_limit,
                &mut highest_gas_limit,
                &mut lowest_gas_limit,
            )?;
        };

        // Pick a point that's close to the estimated gas
        let mut mid_gas_limit = std::cmp::min(
            gas_used * 3,
            ((highest_gas_limit as u128 + lowest_gas_limit as u128) / 2) as u64,
        );

        trace!(target: "rpc::eth::estimate", ?highest_gas_limit, ?lowest_gas_limit, ?mid_gas_limit, "Starting binary search for gas");

        // Binary search narrows the range to find the minimum gas limit needed for the transaction
        // to succeed.
        while lowest_gas_limit + 1 < highest_gas_limit {
            // An estimation error is allowed once the current gas limit range used in the binary
            // search is small enough (less than 1.5% of the highest gas limit)
            // <https://github.com/ethereum/go-ethereum/blob/a5a4fa7032bb248f5a7c40f4e8df2b131c4186a4/eth/gasestimator/gasestimator.go#L152
            let ratio = (highest_gas_limit - lowest_gas_limit) as f64 / (highest_gas_limit as f64);
            if ratio < ESTIMATE_GAS_ERROR_RATIO {
                break
            };

            if is_cancelled() {
                return Err(EthApiError::InternalEthError.into())
            }
            // Execute transaction and handle potential gas errors, adjusting limits accordingly.
            match execute(mid_gas_limit) {
                Err(err) if err.is_gas_too_high() => {
                    // Decrease the highest gas limit if gas is too high
                    highest_gas_limit = mid_gas_limit;
                }
                Err(err) if err.is_gas_too_low() => {
                    // Increase the lowest gas limit if gas is too low
                    lowest_gas_limit = mid_gas_limit;
                }
                // Handle other cases, including successful transactions.
                ethres => {
                    // Update the estimated gas range based on the execution result.
                    update_estimated_gas_range::<EvmTypesFor<Self::Evm>>(
                        ethres?,
                        mid_gas_limit,
                        &mut highest_gas_limit,
                        &mut lowest_gas_limit,
                    )?;
                }
            }

            // New midpoint
            mid_gas_limit = ((highest_gas_limit as u128 + lowest_gas_limit as u128) / 2) as u64;
        }

        Ok(U256::from(highest_gas_limit))
    }

    /// Defaults the nonce keys and fills omitted frame limits at one block.
    fn fill_frame_gas_at(
        &self,
        mut request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        at: BlockId,
        overrides: EvmOverrides,
    ) -> impl Future<Output = Result<RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>, Self::Error>>
           + Send
    where
        Self: LoadPendingBlock,
    {
        async move {
            let invalid = |message: &str| {
                Self::Error::from_eth_err(EthApiError::InvalidParams(message.into()))
            };
            let requested_block = at;
            let (evm_env, at) = self.evm_env_at(requested_block).await?;
            let derived_pending = requested_block.is_pending() && !at.is_pending();
            if evm_env.version().feature(EvmFeatures::EIP8250) &&
                request.as_ref().nonce_keys.is_none()
            {
                if request.as_ref().signatures.as_ref().is_some_and(|signatures| {
                    signatures.iter().any(|signature| {
                        u8::from(signature.scheme) != 0 && !signature.signature.is_empty()
                    })
                }) {
                    return Err(invalid("signed frame transactions must include nonceKeys"));
                }
                request.as_mut().nonce_keys = Some(vec![]);
            }
            let frames =
                request.as_ref().frames.as_ref().ok_or_else(|| invalid("missing frames"))?;
            if frames.is_empty() || frames.len() > alloy_eips::eip8141::MAX_FRAMES {
                return Err(invalid("expected between 1 and 64 frames"));
            }
            let missing: Vec<_> = frames
                .iter()
                .enumerate()
                .flat_map(|(index, frame)| {
                    [(index, false, frame.execution_gas), (index, true, frame.state_gas)]
                        .into_iter()
                        .filter_map(|(index, state, gas)| gas.is_none().then_some((index, state)))
                })
                .collect();
            if missing.is_empty() {
                return Ok(request);
            }
            if request.as_ref().signatures.as_ref().is_some_and(|signatures| {
                signatures.iter().any(|signature| {
                    u8::from(signature.scheme) != 0 && !signature.signature.is_empty()
                })
            }) {
                return Err(invalid("signed frame transactions must include both frame gas limits"));
            }

            let block_limit = overrides
                .block
                .as_ref()
                .and_then(|block| block.gas_limit)
                .unwrap_or_else(|| evm_env.block_env().gas_limit.to::<u64>());
            let mut combined_cap =
                if self.call_gas_limit() == 0 { u64::MAX } else { self.call_gas_limit() };
            // Initial probes must fit the signed fee budget before gas limits can be refined.
            if let Some(budget) = request
                .as_ref()
                .max_fee
                .or_else(|| request.as_ref().eip8141_fees.map(|fees| fees.max_fee))
            {
                let block_overrides = overrides.block.as_ref();
                let base_fee = block_overrides
                    .and_then(|block| block.base_fee)
                    .unwrap_or_else(|| U256::from(evm_env.block_env().basefee));
                let blob_fee = block_overrides
                    .and_then(|block| block.blob_base_fee)
                    .unwrap_or_else(|| U256::from(evm_env.block_env().blob_basefee));
                let blob_gas =
                    request.as_ref().blob_versioned_hashes.as_ref().map_or(0, |hashes| {
                        hashes.len() as u64 * alloy_eips::eip4844::DATA_GAS_PER_BLOB
                    });
                let affordable_gas = frame_fee_gas_limit(budget, base_fee, blob_fee, blob_gas)
                    .ok_or_else(|| invalid("frame fee budget does not cover blob base fees"))?;
                combined_cap = combined_cap.min(affordable_gas);
            }
            for frame in
                request.as_mut().frames.as_mut().ok_or_else(|| invalid("missing frames"))?
            {
                frame.execution_gas.get_or_insert(0);
                frame.state_gas.get_or_insert(0);
            }
            // Reserve transaction overhead and every explicit allocation before assigning defaults.
            let tx_env = self.converter().tx_env(request.clone(), &evm_env)?;
            let gas_params = &evm_env.version().gas_params;
            let signature_bytes = request
                .as_ref()
                .signatures
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter(|signature| signature.signature.is_empty())
                .filter_map(|signature| signature.scheme.signature_length())
                .try_fold(0u64, |sum, length| sum.checked_add(length as u64))
                .ok_or_else(|| invalid("signature gas overflows"))?;
            let signature_gas = signature_bytes
                .checked_mul(gas_params.get(evm2::version::GasId::TxTokenNonZeroByteMultiplier))
                .and_then(|tokens| {
                    tokens.checked_mul(
                        gas_params
                            .get(evm2::version::GasId::TxTokenCost)
                            .max(gas_params.get(evm2::version::GasId::TxFloorCostPerToken)),
                    )
                })
                .ok_or_else(|| invalid("signature gas overflows"))?;
            let gas = FrameGas::calculate(
                tx_env.frame_transaction().ok_or_else(|| invalid("missing frame transaction"))?,
                evm_env.version(),
            )
            .map_err(reth_evm::execute::map_handler_error)
            .map_err(Self::Error::from_evm_err)?;
            let reserved = gas
                .maximum
                .checked_add(signature_gas)
                .ok_or_else(|| invalid("frame gas overflows"))?;
            let allowance = combined_cap
                .checked_sub(reserved)
                .ok_or_else(|| invalid("frame transaction exceeds the gas cap"))?;
            let execution_reserved = gas
                .execution
                .checked_add(signature_gas)
                .ok_or_else(|| invalid("frame gas overflows"))?;
            let execution_allowance = evm_env
                .version()
                .tx_gas_limit_cap
                .min(block_limit)
                .checked_sub(execution_reserved)
                .ok_or_else(|| invalid("frame transaction exceeds the execution gas cap"))?;
            let state_allowance = block_limit
                .checked_sub(gas.state)
                .ok_or_else(|| invalid("frame transaction exceeds the state gas cap"))?;
            let execution_count = missing.iter().filter(|(_, state)| !state).count() as u64;
            let state_count = missing.len() as u64 - execution_count;
            let initial = allowance / missing.len() as u64;
            let execution_initial =
                initial.min(execution_allowance.checked_div(execution_count).unwrap_or(0));
            let state_initial = initial.min(state_allowance.checked_div(state_count).unwrap_or(0));
            for &(index, state) in &missing {
                let frame = request
                    .as_mut()
                    .frames
                    .as_mut()
                    .and_then(|frames| frames.get_mut(index))
                    .ok_or(EthApiError::InternalEthError)?;
                if state {
                    frame.state_gas = Some(state_initial);
                } else {
                    frame.execution_gas = Some(execution_initial);
                }
            }
            let probe = async |candidate| {
                let permit = self
                    .acquire_owned_blocking_io()
                    .await
                    .map_err(|_| EthApiError::InternalEthError)?;
                let env = evm_env.clone();
                let overrides = overrides.clone();
                let result = self
                    .spawn_with_state_at_block(at, move |this, mut db| {
                        let _permit = permit;
                        if derived_pending {
                            this.apply_pending_block_state_changes(&mut db, &env, at)?;
                        }
                        let (env, tx) =
                            this.prepare_call_env(env, candidate, &mut db, overrides)?;
                        this.transact(&mut db, env, tx)
                    })
                    .await?;
                Ok::<_, Self::Error>(result.result.frame.as_ref().is_some_and(|frame| {
                    frame
                        .receipts
                        .iter()
                        .all(|receipt| receipt.status == alloy_eips::eip8141::FrameStatus::Success)
                }))
            };
            if !probe(request.clone()).await? {
                return Err(invalid("frame transaction does not succeed within the gas cap"));
            }
            let mut probes = 1;
            for (index, state) in missing {
                let mut low = 0;
                let mut high = if state { state_initial } else { execution_initial };
                while low < high && probes < 511 {
                    let middle = low + (high - low) / 2;
                    let mut candidate = request.clone();
                    let frame = candidate
                        .as_mut()
                        .frames
                        .as_mut()
                        .and_then(|frames| frames.get_mut(index))
                        .ok_or(EthApiError::InternalEthError)?;
                    if state {
                        frame.state_gas = Some(middle);
                    } else {
                        frame.execution_gas = Some(middle);
                    }
                    probes += 1;
                    // VERIFY out-of-gas is a transaction error, so an unsuccessful probe keeps the
                    // known-good bound.
                    let succeeds = match probe(candidate).await {
                        Ok(success) => success,
                        Err(error)
                            if error.is_gas_too_low() ||
                                error.is_gas_too_high() ||
                                matches!(
                                    error.as_err(),
                                    Some(EthApiError::InvalidTransaction(
                                        RpcInvalidTransactionError::FrameTransaction(
                                            evm2::registry::HandlerError::InvalidFrameTransaction(
                                                "EIP-8141 VERIFY frame failed"
                                            )
                                        )
                                    ))
                                ) =>
                        {
                            false
                        }
                        Err(error) => return Err(error),
                    };
                    if succeeds {
                        high = middle;
                    } else {
                        low = middle + 1;
                    }
                }
                let frame = request
                    .as_mut()
                    .frames
                    .as_mut()
                    .and_then(|frames| frames.get_mut(index))
                    .ok_or(EthApiError::InternalEthError)?;
                if state {
                    frame.state_gas = Some(high);
                } else {
                    frame.execution_gas = Some(high);
                }
            }
            if !probe(request.clone()).await? {
                return Err(invalid("filled frame gas limits failed validation"));
            }
            Ok(request)
        }
    }

    /// Estimate gas needed for execution of the `request` at the [`BlockId`].
    fn estimate_gas_at(
        &self,
        request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        at: BlockId,
        overrides: EvmOverrides,
    ) -> impl Future<Output = Result<U256, Self::Error>> + Send
    where
        Self: LoadPendingBlock,
    {
        async move {
            let request = if request.as_ref().frames.as_ref().is_some_and(|frames| {
                frames
                    .iter()
                    .any(|frame| frame.execution_gas.is_none() || frame.state_gas.is_none())
            }) {
                self.fill_frame_gas_at(request, at, overrides.clone()).await?
            } else {
                request
            };
            let requested_block = at;
            let (evm_env, at) = self.evm_env_at(requested_block).await?;
            let derived_pending = requested_block.is_pending() && !at.is_pending();

            self.spawn_with_state_at_block(at, move |this, mut db| {
                if derived_pending && request.as_ref().minimal_tx_type().is_eip8141() {
                    this.apply_pending_block_state_changes(&mut db, &evm_env, at)?;
                }
                EstimateCall::estimate_gas_with_cache(&this, evm_env, request, db, overrides)
            })
            .await
        }
    }
}

fn frame_fee_gas_limit(budget: U256, base_fee: U256, blob_fee: U256, blob_gas: u64) -> Option<u64> {
    let remaining = budget.checked_sub(blob_fee.checked_mul(U256::from(blob_gas))?)?;
    Some(if base_fee.is_zero() { u64::MAX } else { (remaining / base_fee).saturating_to() })
}

fn map_out_of_gas_result<Error, EvmTypes>(
    result: TxResult<EvmTypes>,
    requested_gas_limit: u64,
) -> Result<U256, Error>
where
    Error: FromEvmHalt<evm2::interpreter::InstrStop> + FromRevert + FromEthApiError,
    EvmTypes: evm2::EvmTypes,
{
    if result.status {
        // Transaction succeeded by manually increasing the gas limit,
        // which means the caller lacks funds to pay for the tx
        Err(RpcInvalidTransactionError::BasicOutOfGas(requested_gas_limit).into_eth_err())
    } else if result.stop == InstrStop::Revert {
        // reverted again after bumping the limit
        Err(Error::from_revert(result.output))
    } else {
        Err(Error::from_evm_halt(result.stop, requested_gas_limit))
    }
}

/// Updates the highest and lowest gas limits for binary search based on the execution result.
///
/// This function refines the gas limit estimates used in a binary search to find the optimal
/// gas limit for a transaction. It adjusts the highest or lowest gas limits depending on
/// whether the execution succeeded, reverted, or halted due to specific reasons.
#[inline]
pub fn update_estimated_gas_range<EvmTypes: evm2::EvmTypes>(
    result: TxResult<EvmTypes>,
    tx_gas_limit: u64,
    highest_gas_limit: &mut u64,
    lowest_gas_limit: &mut u64,
) -> Result<(), EthApiError> {
    if result.status {
        // Cap the highest gas limit with the succeeding gas limit.
        *highest_gas_limit = tx_gas_limit;
    } else {
        // We know that transaction succeeded with a higher gas limit before, so any failure
        // means that we need to increase it.
        //
        // We are ignoring all halts here, and not just OOG errors because there are cases when
        // non-OOG halt might flag insufficient gas limit as well.
        //
        // Common usage of invalid opcode in OpenZeppelin:
        // <https://github.com/OpenZeppelin/openzeppelin-contracts/blob/94697be8a3f0dfcd95dfb13ffbd39b5973f5c65d/contracts/metatx/ERC2771Forwarder.sol#L360-L367>
        *lowest_gas_limit = tx_gas_limit;
    }

    Ok(())
}
