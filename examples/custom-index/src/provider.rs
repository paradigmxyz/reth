use reth_ethereum::{
    evm::revm::primitives::{alloy_primitives::TxNumber, Address},
    node::api::NodeTypesWithDBAdapter,
    provider::{
        db::{
            cursor::{DbCursorRO, DbDupCursorRO},
            database_metrics::DatabaseMetrics,
            transaction::DbTx,
            Database,
        },
        providers::BlockchainProvider,
        BlockNumReader, ProviderResult,
    },
    storage::{DatabaseProviderFactory, TransactionsProvider},
};

use crate::{storage::SenderTransactions, CustomNode};

pub trait SenderTxReader: TransactionsProvider {
    /// Get a transaction by sender and transaction index.
    fn transaction_by_sender_and_idx(
        &self,
        sender: Address,
        tx_index: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>>;
}

impl<DB: Database + Clone + Unpin + DatabaseMetrics + 'static> SenderTxReader
    for BlockchainProvider<NodeTypesWithDBAdapter<CustomNode, DB>>
{
    fn transaction_by_sender_and_idx(
        &self,
        sender: Address,
        tx_index: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>> {
        let database = self.database_provider_ro()?;
        let mut cursor = database.tx_ref().cursor_dup_read::<SenderTransactions>()?;

        cursor.seek_by_key_subkey(sender, tx_index)?;
        if let Some((s, value)) = cursor.current()? &&
            s == sender
        {
            return database.transaction_by_id(value.global_tx_index)
        }

        let mut next_sender_tx = cursor
            .prev()?
            .filter(|(s, _)| *s == sender)
            .map(|(_, value)| value.sender_tx_index + 1)
            .unwrap_or(0);

        let highest_db_block = database.last_block_number()?;

        let state = self.canonical_in_memory_state().head_state();
        let chain = state.as_ref().map(|state| state.chain().collect::<Vec<_>>()).unwrap_or_default();

        for block in chain {
            if block.number() <= highest_db_block {
                continue;
            }

            for tx in block.block_ref().recovered_block().transactions_recovered() {
                if tx.signer() == sender {
                    if next_sender_tx == tx_index {
                        return Ok(Some((*tx.inner()).clone()));
                    }
                    next_sender_tx += 1;
                }
            }
        }

        Ok(None)
    }
}
