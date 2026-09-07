//! Canonical-snapshot validation of public frame transactions.

use alloy_consensus::BlockHeader;
use reth_ethereum_primitives::TransactionSigned;
use reth_evm::{ConfigureEvm, Evm, FromRecoveredTx, TxEnvFor};
use reth_node_api::{FullNodeTypes, NodePrimitives, NodeTypes, PrimitivesTy};
use reth_revm::database::StateProviderDatabase;
use reth_storage_api::{AccountReader, BlockReaderIdExt, StateProviderFactory};
use reth_transaction_pool::{
    error::{Eip8141PoolTransactionError, InvalidPoolTransactionError},
    validate::{FrameValidation, FrameValidationInspector, FrameValidationPolicy},
    EthPooledTransaction,
};
use std::sync::Arc;

fn policy(reason: &'static str) -> InvalidPoolTransactionError {
    Eip8141PoolTransactionError::PublicMempoolPolicy(reason).into()
}

/// Uses the node's configured EVM, including its active precompiles and gas schedule.
pub(super) fn validate<Node, EvmConfig>(
    client: &Node::Provider,
    evm_config: &EvmConfig,
    transaction: &EthPooledTransaction,
) -> Result<Arc<FrameValidation>, InvalidPoolTransactionError>
where
    Node: FullNodeTypes<Types: NodeTypes<Primitives: NodePrimitives<SignedTx = TransactionSigned>>>,
    EvmConfig: ConfigureEvm<Primitives = PrimitivesTy<Node::Types>>,
{
    let frame =
        transaction.transaction.as_eip8141().ok_or_else(|| policy("not a frame transaction"))?;
    // Bound the entire simulation before opening state or performing signature verification.
    let prefix =
        FrameValidationPolicy::new(frame, frame.signature_verification_gas()).map_err(policy)?;
    let head = client
        .latest_header()
        .map_err(|_| policy("cannot read canonical head"))?
        .ok_or_else(|| policy("canonical head unavailable"))?;
    let state = client
        .state_by_block_hash(head.hash())
        .map_err(|_| policy("canonical state unavailable"))?;
    let env =
        evm_config.evm_env(&head).map_err(|_| policy("cannot configure frame validation EVM"))?;
    let tx = TxEnvFor::<EvmConfig>::from_recovered_tx_with_gas_params(
        transaction.transaction.inner(),
        frame.sender,
        &env.cfg_env.gas_params,
    );
    let inspector = FrameValidationInspector::new(frame.sender, prefix);
    let mut evm =
        evm_config.evm_with_env_and_inspector(StateProviderDatabase::new(&state), env, inspector);
    let result = evm
        .validate_frame_transaction(tx, prefix.prefix_end)
        .ok_or(Eip8141PoolTransactionError::PublicMempoolValidationUnavailable)?
        .map_err(|_| policy("validation prefix execution failed"))?;
    let inspector = evm.components().1;
    if let Some(reason) = inspector.error() {
        return Err(policy(reason))
    }
    if !result.sender_approved || result.prefix_end != prefix.prefix_end {
        return Err(policy("validation prefix did not grant required approvals"))
    }
    if (result.execution_gas > prefix.declared_execution_gas) ||
        (result.state_gas > prefix.state_gas)
    {
        return Err(policy("validation prefix exceeded its declared work budget"))
    }
    let expiry = inspector.expiry();
    if expiry.is_some_and(|deadline| deadline < head.timestamp()) {
        return Err(policy("frame transaction expired"))
    }
    let mut dependencies = inspector.dependencies();
    dependencies.accounts.push(result.payer);
    dependencies.code.push(result.payer);
    let payer = state
        .basic_account(&result.payer)
        .map_err(|_| policy("cannot read payer"))?
        .unwrap_or_default();
    let sender = state
        .basic_account(&frame.sender)
        .map_err(|_| policy("cannot read sender"))?
        .unwrap_or_default();
    if result.max_cost > payer.balance {
        return Err(policy("payer cannot cover maximum transaction cost"))
    }
    // No canonical runtime is specified by this devnet's reference. Contract pay frames
    // therefore use the non-canonical one-pending-transaction rule, never a trace exemption.
    let exclusive_payer = frame.frames[prefix.prefix_end - 1].flags == 1 &&
        payer.bytecode_hash.is_some_and(|hash| hash != alloy_consensus::constants::KECCAK_EMPTY);
    Ok(Arc::new(FrameValidation {
        sender: frame.sender,
        sender_nonce: sender.nonce,
        sender_balance: sender.balance,
        sender_code_hash: sender.bytecode_hash,
        payer: result.payer,
        max_cost: result.max_cost,
        payer_balance: payer.balance,
        head_hash: head.hash(),
        dependencies,
        expires_at: expiry,
        exclusive_payer,
    }))
}
