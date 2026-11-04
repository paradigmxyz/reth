//! Canonical-snapshot validation of public frame transactions.

use super::{
    FrameValidation, FrameValidationError, FrameValidationInspector, FrameValidationPolicy,
};
use crate::{error::Eip8141PoolTransactionError, EthPoolTransaction};
use alloy_consensus::BlockHeader;
use alloy_eips::eip8141::{nonce_slot, NONCE_MANAGER};
use alloy_primitives::U256;
use reth_evm::{
    database::StateProviderDatabase, ConfigureEvm, Evm, EvmEnv, FromRecoveredTx, TxEnvFor,
};
use reth_primitives_traits::{HeaderTy, TxTy};
use reth_storage_api::{AccountReader, BlockReaderIdExt, StateProvider, StateProviderFactory};
use std::sync::Arc;
use tracing::debug;

fn policy(reason: &'static str) -> FrameValidationError {
    FrameValidationError::Invalid(Eip8141PoolTransactionError::PublicMempoolPolicy(reason).into())
}

/// Uses the pool's configured EVM, including its active precompiles and gas schedule.
pub(super) fn validate<Client, Tx, EvmConfig>(
    client: &Client,
    evm_config: &EvmConfig,
    transaction: &Tx,
) -> Result<Arc<FrameValidation>, FrameValidationError>
where
    Client: StateProviderFactory + BlockReaderIdExt<Header = HeaderTy<EvmConfig::Primitives>>,
    Tx: EthPoolTransaction<Consensus = TxTy<EvmConfig::Primitives>>,
    EvmConfig: ConfigureEvm,
{
    let frame = transaction.frame_transaction().ok_or_else(|| policy("not a frame transaction"))?;
    let tx_hash = *transaction.hash();
    debug!(
        target: "reth::eip8141::pool",
        ?tx_hash,
        sender = ?frame.sender,
        nonce = frame.nonce,
        frame_count = frame.frames.len(),
        signature_verification_gas = frame.signature_verification_gas(),
        "validating EIP-8141 public-pool transaction"
    );
    // Bound the entire simulation before opening state or performing signature verification.
    let prefix = match FrameValidationPolicy::new(frame, frame.signature_verification_gas()) {
        Ok(prefix) => prefix,
        Err(reason) => {
            debug!(
                target: "reth::eip8141::pool",
                ?tx_hash,
                sender = ?frame.sender,
                nonce = frame.nonce,
                reason,
                "rejected EIP-8141 public-pool transaction during structural validation"
            );
            return Err(policy(reason))
        }
    };
    debug!(
        target: "reth::eip8141::pool",
        ?tx_hash,
        prefix_end = prefix.prefix_end,
        deploy_index = ?prefix.deploy_index,
        expiry_index = ?prefix.expiry_index,
        recent_root_index = ?prefix.recent_root.as_ref().map(|verifier| verifier.index),
        declared_execution_gas = prefix.declared_execution_gas,
        declared_state_gas = prefix.state_gas,
        "recognized EIP-8141 public-pool validation prefix"
    );
    let head = client
        .latest_header()?
        .ok_or(FrameValidationError::Unavailable("canonical head unavailable"))?;
    let (current_slot, recent_root_dependencies) = if let Some(verifier) = &prefix.recent_root {
        // EIP-8272 deliberately validates against the earliest possible next payload slot, not
        // against time. Empty slots therefore do not advance a public-pool root reference.
        let current_slot = head
            .slot_number()
            .ok_or(FrameValidationError::Unavailable("canonical head slot unavailable"))?
            .checked_add(1)
            .ok_or(FrameValidationError::Unavailable("canonical head slot overflows"))?;
        for reference in &verifier.references {
            let dependency = reference.dependency();
            if reference.slot >= current_slot {
                return Err(policy("recent root reference is not before current slot"))
            }
            if dependency.expires_at_slot <= current_slot {
                return Err(policy("recent root reference expired"))
            }
        }
        (Some(current_slot), verifier.dependencies())
    } else {
        (None, Vec::new())
    };
    let state = client.state_by_block_hash(head.hash())?;
    let mut env = evm_config.evm_env(&head).map_err(FrameValidationError::other)?;
    // Pre-fork frames may queue behind missing nonces. Keyed frames require every selected
    // sequence to match canonical state before admission.
    env.version_mut().features.set(evm2::EvmFeatures::NONCE_CHECK, frame.nonce_keys.is_some());
    if let Some(current_slot) = current_slot {
        env.block_env_mut().slot_num = U256::from(current_slot);
    }
    let tx = TxEnvFor::<EvmConfig>::from_recovered_tx(transaction.clone_into_consensus());
    let inspector = FrameValidationInspector::new(frame.sender, prefix.clone());
    let mut evm = evm_config
        .evm_with_env(StateProviderDatabase::new(state.as_ref().into_evm_state_provider()), env);
    let (inspector, result) =
        evm.validate_frame_prefix_with_inspector(&tx, prefix.prefix_end, inspector);
    if let Some(error) = inspector.database_error() {
        return Err(FrameValidationError::other(error.clone()))
    }
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            debug!(
                target: "reth::eip8141::pool",
                ?tx_hash,
                sender = ?frame.sender,
                nonce = frame.nonce,
                prefix_end = prefix.prefix_end,
                error = %error,
                "EIP-8141 public-pool prefix execution failed"
            );
            return Err(if error.as_internal().is_some() {
                FrameValidationError::other(error)
            } else {
                policy("validation prefix execution failed")
            })
        }
    };
    if let Some(reason) = inspector.error() {
        debug!(
            target: "reth::eip8141::pool",
            ?tx_hash,
            sender = ?frame.sender,
            nonce = frame.nonce,
            reason,
            "rejected EIP-8141 public-pool transaction after prefix inspection"
        );
        return Err(policy(reason))
    }
    if !result.sender_approved || result.prefix_end != prefix.prefix_end {
        debug!(
            target: "reth::eip8141::pool",
            ?tx_hash,
            sender = ?frame.sender,
            nonce = frame.nonce,
            sender_approved = result.sender_approved,
            expected_prefix_end = prefix.prefix_end,
            actual_prefix_end = result.prefix_end,
            "rejected EIP-8141 public-pool transaction without required approvals"
        );
        return Err(policy("validation prefix did not grant required approvals"))
    }
    if result.execution_gas > prefix.declared_execution_gas {
        debug!(
            target: "reth::eip8141::pool",
            ?tx_hash,
            execution_gas = result.execution_gas,
            declared_execution_gas = prefix.declared_execution_gas,
            "rejected EIP-8141 public-pool transaction exceeding execution-gas budget"
        );
        return Err(policy("validation prefix exceeded its declared work budget"))
    }
    if result.state_gas > prefix.state_gas {
        debug!(
            target: "reth::eip8141::pool",
            ?tx_hash,
            state_gas = result.state_gas,
            declared_state_gas = prefix.state_gas,
            "rejected EIP-8141 public-pool transaction exceeding state-gas budget"
        );
        return Err(policy("validation prefix exceeded its declared work budget"))
    }
    let expiry = inspector.expiry();
    if expiry.is_some_and(|deadline| deadline < head.timestamp()) {
        debug!(
            target: "reth::eip8141::pool",
            ?tx_hash,
            expiry = ?expiry,
            head_timestamp = head.timestamp(),
            "rejected expired EIP-8141 public-pool transaction"
        );
        return Err(policy("frame transaction expired"))
    }
    let mut dependencies = inspector.dependencies();
    dependencies.accounts.push(result.payer);
    dependencies.code.push(result.payer);
    let payer = state.basic_account(&result.payer)?.unwrap_or_default();
    let payer_balance = payer.balance;
    let payer_code_hash = payer.bytecode_hash;
    let sender = if result.payer == frame.sender {
        payer
    } else {
        state.basic_account(&frame.sender)?.unwrap_or_default()
    };
    let mut state_nonce = sender.nonce;
    if let Some(keys) = &frame.nonce_keys &&
        !keys.is_empty()
    {
        state_nonce = frame.nonce;
        for key in keys {
            let slot = nonce_slot(frame.sender, *key);
            dependencies.storage.push((NONCE_MANAGER, slot));
            let current = state.storage(NONCE_MANAGER, slot.into())?.unwrap_or_default();
            if if frame.nonce == u64::MAX {
                !current.is_zero()
            } else {
                current != U256::from(frame.nonce)
            } {
                return Err(policy("keyed nonce sequence mismatch"));
            }
        }
    }
    if result.max_cost > payer_balance {
        debug!(
            target: "reth::eip8141::pool",
            ?tx_hash,
            payer = ?result.payer,
            max_cost = ?result.max_cost,
            payer_balance = ?payer_balance,
            "rejected EIP-8141 public-pool transaction with insufficient payer balance"
        );
        return Err(policy("payer cannot cover maximum transaction cost"))
    }
    // No canonical runtime is specified by this devnet's reference. Contract pay frames
    // therefore use the non-canonical one-pending-transaction rule, never a trace exemption.
    let exclusive_payer = frame.frames[prefix.prefix_end - 1].flags == 1 &&
        payer_code_hash.is_some_and(|hash| hash != alloy_consensus::constants::KECCAK_EMPTY);
    debug!(
        target: "reth::eip8141::pool",
        ?tx_hash,
        sender = ?frame.sender,
        payer = ?result.payer,
        max_cost = ?result.max_cost,
        expiry = ?expiry,
        exclusive_payer,
        "accepted EIP-8141 public-pool transaction"
    );
    Ok(Arc::new(FrameValidation {
        sender: frame.sender,
        sender_nonce: frame.nonce,
        nonce_keys: frame.nonce_keys.clone(),
        state_nonce,
        sender_balance: sender.balance,
        sender_code_hash: sender.bytecode_hash,
        payer: result.payer,
        max_cost: result.max_cost,
        payer_balance,
        head_hash: head.hash(),
        dependencies,
        expires_at: expiry,
        recent_root_dependencies,
        exclusive_payer,
    }))
}
