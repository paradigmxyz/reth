use alloy_consensus::Transaction;
use alloy_consensus::{transaction::TxHashRef, TxReceipt};
use alloy_evm::block::BlockExecutor;
use alloy_rlp::encode;
use eyre::Result;
use futures::TryStreamExt;
use reth_ethereum::{
    exex::{ExExContext, ExExEvent, ExExNotification},
    node::api::FullNodeComponents,
    primitives::AlloyBlockHeader,
    provider::StateProviderFactory,
};
use reth_evm::ConfigureEvm;
use reth_primitives_traits::{NodePrimitives, RecoveredBlock};
use reth_revm::{database::StateProviderDatabase, primitives::alloy_primitives::TxHash};
use reth_tracing::tracing::{error, info};
use revm_database::states::State;
use xlayer_db::{
    internal_transaction_inspector::TraceCollector,
    structs::{BlockTable, TxTable},
    utils::{
        delete_single, rw_batch_delete, rw_batch_end, rw_batch_start, rw_batch_write, write_single,
    },
};

fn replay_and_index_block<P, E, N>(
    provider: P,
    evm_config: E,
    block: RecoveredBlock<<N as NodePrimitives>::Block>,
) -> Result<()>
where
    P: StateProviderFactory + Send + Sync + 'static,
    E: ConfigureEvm<Primitives = N> + Send + Sync + 'static,
    N: NodePrimitives + 'static,
{
    let state_provider = provider.history_by_block_hash(block.parent_hash())?;

    let mut db = State::builder()
        .with_database(StateProviderDatabase::new(&state_provider))
        .with_bundle_update()
        .without_state_clear()
        .build();

    let mut inspector = TraceCollector::default();
    let evm_env = evm_config.evm_env(block.header())?;
    let evm = evm_config.evm_with_env_and_inspector(&mut db, evm_env, &mut inspector);
    let block_ctx = evm_config.context_for_block(&block)?;
    let mut executor = evm_config.create_executor(evm, block_ctx);

    executor.set_state_hook(None);
    let output = executor.execute_block(block.transactions_recovered())?;

    let mut internal_transactions = inspector.get();
    let mut tx_hashes = Vec::<TxHash>::default();

    let (rw_tx, rw_db) = rw_batch_start::<TxTable>()?;

    let mut prev_cumulative_gas = 0u64;
    for (index, tx) in block.transactions_recovered().enumerate() {
        let success = output.receipts[index].status();

        let current_cumulative_gas = output.receipts[index].cumulative_gas_used();
        let tx_gas_used = current_cumulative_gas - prev_cumulative_gas;
        prev_cumulative_gas = current_cumulative_gas;

        if !success || (!internal_transactions.is_empty() && internal_transactions[index].len() > 0)
        {
            if !internal_transactions.is_empty() && !internal_transactions[index].is_empty() {
                if let Some(first_inner_tx) = internal_transactions[index].first_mut() {
                    first_inner_tx.set_transaction_gas(tx.gas_limit(), tx_gas_used);
                }
            }

            tx_hashes.push(*tx.tx_hash());
            rw_batch_write::<TxTable>(
                &rw_tx,
                &rw_db,
                tx.tx_hash().to_vec(),
                encode(internal_transactions[index].clone()),
            )?;
        }
    }

    rw_batch_end::<TxTable>(rw_tx)?;

    write_single::<BlockTable, Vec<TxHash>>(block.hash().to_vec(), tx_hashes)?;

    Ok(())
}

fn remove_block<E, N>(_: E, block: RecoveredBlock<<N as NodePrimitives>::Block>) -> Result<()>
where
    E: ConfigureEvm<Primitives = N> + Send + Sync + 'static,
    N: NodePrimitives + 'static,
{
    let (rw_tx, rw_db) = rw_batch_start::<TxTable>()?;

    for tx in block.transactions_recovered() {
        rw_batch_delete::<TxTable>(&rw_tx, &rw_db, tx.tx_hash().to_vec())?;
    }

    rw_batch_end::<TxTable>(rw_tx)?;

    delete_single::<BlockTable>(block.hash().to_vec())?;

    Ok(())
}

pub async fn post_exec_exex<Node: FullNodeComponents>(mut ctx: ExExContext<Node>) -> Result<()> {
    while let Some(notif) = ctx.notifications.try_next().await? {
        match &notif {
            ExExNotification::ChainCommitted { new } => {
                info!(target:"reth::cli", "xlayer exex chainCommitted new range {:#?}", new.range());

                for block in new.blocks_iter() {
                    info!(target:"reth::cli", "xlayer exex chainCommitted new block {:#?}", block.hash());

                    let provider = ctx.provider().clone();
                    let evm_config = ctx.evm_config().clone();

                    if let Err(err) = replay_and_index_block(provider, evm_config, block.clone()) {
                        error!(target:"reth::cli", "xlayer exex chainCommitted failed to process new block {:#?} with error {:#?}", block.hash(), err);
                    }
                }

                // Tell Reth “I’m done up to this height” (unblocks pruning & WAL growth):
                ctx.events.send(ExExEvent::FinishedHeight(new.tip().num_hash()))?;
            }
            ExExNotification::ChainReorged { old, new } => {
                info!(target:"reth::cli", "xlayer exex chainReorged old range {:#?} new range {:#?}", old.range(), new.range());

                for block in old.blocks_iter() {
                    info!(target:"reth::cli", "xlayer exex chainReorged old block {:#?}", block.hash());

                    let evm_config = ctx.evm_config().clone();

                    if let Err(err) = remove_block(evm_config, block.clone()) {
                        error!(target:"reth::cli", "xlayer exex chainReorged failed to reorg old block {:#?} with error {:#?}", block.hash(), err);
                    }
                }

                for block in new.blocks_iter() {
                    info!(target:"reth::cli", "xlayer exex chainReorged new block {:#?}", block.hash());

                    let provider = ctx.provider().clone();
                    let evm_config = ctx.evm_config().clone();

                    if let Err(err) = replay_and_index_block(provider, evm_config, block.clone()) {
                        error!(target:"reth::cli", "xlayer exex chainReorged failed to process new block {:#?} with error {:#?}", block.hash(), err);
                    }
                }

                ctx.events.send(ExExEvent::FinishedHeight(new.tip().num_hash()))?;
            }
            ExExNotification::ChainReverted { old } => {
                info!(target:"reth::cli", "xlayer exex chainReverted old range {:#?}", old.range());

                for block in old.blocks_iter() {
                    info!(target:"reth::cli", "xlayer exex chainReverted old block {:#?}", block.hash());

                    let evm_config = ctx.evm_config().clone();

                    if let Err(err) = remove_block(evm_config, block.clone()) {
                        error!(target:"reth::cli", "xlayer exex chainReverted failed to revert old block {:#?} with error {:#?}", block.hash(), err);
                    }
                }
            }
        }
    }
    Ok(())
}
