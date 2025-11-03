use reth_ethereum::{
    chainspec::EthereumHardforks,
    evm::revm::primitives::{
        alloy_primitives::{BlockNumber, TxNonce, TxNumber},
        bytes::{self},
        Address,
    },
    primitives::Header,
    provider::{
        db::{
            cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
            table::{Compress, Decompress},
            tables,
            transaction::{DbTx, DbTxMut},
            DatabaseError, TableSet,
        },
        providers::{ChainStorage, NodeTypesForProvider},
        ChainSpecProvider, DatabaseProvider, ExecutionOutcome, ProviderError, ProviderResult,
    },
    rpc::eth::primitives::TransactionTrait,
    storage::{
        BlockBodyIndicesProvider, ChainStorageReader, ChainStorageWriter, DBProvider, EthStorage,
        TransactionsProvider,
    },
    BlockBody, EthPrimitives, TransactionSigned,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SenderTransaction {
    /// Nonce of the sender.
    pub nonce: TxNonce,
    /// Global index of this transaction in database.
    pub global_tx_index: TxNumber,
}

impl Compress for SenderTransaction {
    type Compressed = Vec<u8>;

    fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(&self, buf: &mut B) {
        buf.put_slice(&self.nonce.to_be_bytes());
        buf.put_slice(&self.global_tx_index.to_be_bytes());
    }
}

impl Decompress for SenderTransaction {
    fn decompress(buf: &[u8]) -> Result<Self, DatabaseError> {
        let sender_tx_index = u64::from_be_bytes(buf[..8].try_into().unwrap());
        let global_tx_index = u64::from_be_bytes(buf[8..16].try_into().unwrap());

        Ok(Self { nonce: sender_tx_index, global_tx_index })
    }
}

tables! {
    /// Helper table allowing to quickly find block number where account nonce was incremented.
    table SenderTransactions {
        type Key = Address;
        type Value = SenderTransaction;
        type SubKey = TxNonce;
    }
}

/// Custom storage implementation.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct CustomStorage {
    inner: EthStorage,
}

impl<Provider> ChainStorageReader<Provider, EthPrimitives> for CustomStorage
where
    Provider: DBProvider + ChainSpecProvider<ChainSpec: EthereumHardforks>,
{
    fn read_block_bodies(
        &self,
        provider: &Provider,
        inputs: Vec<(&Header, Vec<TransactionSigned>)>,
    ) -> ProviderResult<Vec<BlockBody>> {
        ChainStorageReader::<_, EthPrimitives>::read_block_bodies(&self.inner, provider, inputs)
    }
}

impl<Provider> ChainStorageWriter<Provider, EthPrimitives> for CustomStorage
where
    Provider: DBProvider<Tx: DbTxMut>
        + ChainSpecProvider<ChainSpec: EthereumHardforks>
        + BlockBodyIndicesProvider
        + TransactionsProvider<Transaction = TransactionSigned>,
{
    fn write_block_bodies(
        &self,
        provider: &Provider,
        bodies: Vec<(u64, Option<BlockBody>)>,
    ) -> ProviderResult<()> {
        ChainStorageWriter::<_, EthPrimitives>::write_block_bodies(&self.inner, provider, bodies)
    }

    fn remove_block_bodies_above(
        &self,
        provider: &Provider,
        block: BlockNumber,
    ) -> ProviderResult<()> {
        ChainStorageWriter::<_, EthPrimitives>::remove_block_bodies_above(
            &self.inner,
            provider,
            block,
        )
    }

    fn write_custom_state(
        &self,
        provider: &Provider,
        state: &ExecutionOutcome,
    ) -> ProviderResult<()> {
        let first_block = state.first_block();
        let last_block = state.last_block();

        let block_bodies = provider.block_body_indices_range(first_block..=last_block)?;
        let first_tx = block_bodies
            .first()
            .ok_or(ProviderError::BlockBodyIndicesNotFound(first_block))?
            .first_tx_num();
        let last_tx = block_bodies
            .last()
            .ok_or(ProviderError::BlockBodyIndicesNotFound(last_block))?
            .last_tx_num();

        let db = provider.tx_ref();

        for ((sender, tx), global_tx_index) in provider
            .senders_by_tx_range(first_tx..=last_tx)?
            .into_iter()
            .zip(provider.transactions_by_tx_range(first_tx..=last_tx)?)
            .zip(first_tx..=last_tx)
        {
            db.put::<SenderTransactions>(
                sender,
                SenderTransaction { nonce: tx.nonce(), global_tx_index },
            )?;
        }

        Ok(())
    }

    fn remove_custom_state_above(
        &self,
        provider: &Provider,
        block: BlockNumber,
    ) -> ProviderResult<()> {
        let first_block = block + 1;
        let last_block = provider.last_block_number()?;

        let block_bodies = provider.block_body_indices_range(first_block..=last_block)?;
        let first_tx = block_bodies
            .first()
            .ok_or(ProviderError::BlockBodyIndicesNotFound(first_block))?
            .first_tx_num();
        let last_tx = block_bodies
            .last()
            .ok_or(ProviderError::BlockBodyIndicesNotFound(last_block))?
            .last_tx_num();

        let mut cursor = provider.tx_ref().cursor_dup_write::<SenderTransactions>()?;

        for (sender, global_tx_index) in
            provider.senders_by_tx_range(first_tx..=last_tx)?.into_iter().zip(first_tx..=last_tx)
        {
            cursor.seek_by_key_subkey(sender, TxNonce::MAX)?;
            let last_sender_tx = cursor.prev()?;

            if last_sender_tx.is_none_or(|(_, value)| value.global_tx_index != global_tx_index) {
                return Err(ProviderError::StateAtBlockPruned(first_block))
            }

            cursor.delete_current()?;
        }

        Ok(())
    }
}

impl ChainStorage<EthPrimitives> for CustomStorage {
    fn reader<TX, Types>(
        &self,
    ) -> impl ChainStorageReader<DatabaseProvider<TX, Types>, EthPrimitives>
    where
        TX: DbTx + 'static,
        Types: NodeTypesForProvider<Primitives = EthPrimitives>,
    {
        self
    }

    fn writer<TX, Types>(
        &self,
    ) -> impl ChainStorageWriter<DatabaseProvider<TX, Types>, EthPrimitives>
    where
        TX: DbTxMut + DbTx + 'static,
        Types: NodeTypesForProvider<Primitives = EthPrimitives>,
    {
        self
    }

    fn extra_tables() -> Option<impl TableSet> {
        Some(Tables::SenderTransactions)
    }
}
