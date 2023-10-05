use crate::{
    bundle_state::BundleStateWithReceipts,
    traits::{BlockSource, ReceiptProvider},
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, BlockReaderIdExt,
    ChainSpecProvider, ChangeSetReader, EvmEnvProvider, HeaderProvider, PruneCheckpointReader,
    ReceiptProviderIdExt, StageCheckpointReader, StateProvider, StateProviderBox,
    StateProviderFactory, StateRootProvider, TransactionsProvider, WithdrawalsProvider,
};
use reth_db::models::{AccountBeforeTx, StoredBlockBodyIndices};
use reth_interfaces::RethResult;
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    Account, Address, Block, BlockHash, BlockHashOrNumber, BlockId, BlockNumber, Bytecode, Bytes,
    ChainInfo, ChainSpec, Header, PruneCheckpoint, PruneSegment, Receipt, SealedBlock,
    SealedHeader, StorageKey, StorageValue, TransactionMeta, TransactionSigned,
    TransactionSignedNoHash, TxHash, TxNumber, B256, KECCAK_EMPTY, MAINNET, U256,
};
use revm::primitives::{BlockEnv, CfgEnv};
use std::{
    ops::{RangeBounds, RangeInclusive},
    sync::Arc,
};

/// Supports various api interfaces for testing purposes.
#[derive(Debug, Clone, Default, Copy)]
#[non_exhaustive]
pub struct NoopProvider;

impl ChainSpecProvider for NoopProvider {
    fn chain_spec(&self) -> Arc<ChainSpec> {
        MAINNET.clone()
    }
}

/// Noop implementation for testing purposes
impl BlockHashReader for NoopProvider {
    fn block_hash(&self, _number: u64) -> RethResult<Option<B256>> {
        Ok(None)
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> RethResult<Vec<B256>> {
        Ok(vec![])
    }
}

impl BlockNumReader for NoopProvider {
    fn chain_info(&self) -> RethResult<ChainInfo> {
        Ok(ChainInfo::default())
    }

    fn best_block_number(&self) -> RethResult<BlockNumber> {
        Ok(0)
    }

    fn last_block_number(&self) -> RethResult<BlockNumber> {
        Ok(0)
    }

    fn block_number(&self, _hash: B256) -> RethResult<Option<BlockNumber>> {
        Ok(None)
    }
}

impl BlockReader for NoopProvider {
    fn find_block_by_hash(&self, hash: B256, _source: BlockSource) -> RethResult<Option<Block>> {
        self.block(hash.into())
    }

    fn block(&self, _id: BlockHashOrNumber) -> RethResult<Option<Block>> {
        Ok(None)
    }

    fn pending_block(&self) -> RethResult<Option<SealedBlock>> {
        Ok(None)
    }

    fn pending_block_and_receipts(&self) -> RethResult<Option<(SealedBlock, Vec<Receipt>)>> {
        Ok(None)
    }

    fn ommers(&self, _id: BlockHashOrNumber) -> RethResult<Option<Vec<Header>>> {
        Ok(None)
    }

    fn block_body_indices(&self, _num: u64) -> RethResult<Option<StoredBlockBodyIndices>> {
        Ok(None)
    }

    fn block_with_senders(
        &self,
        _number: BlockNumber,
    ) -> RethResult<Option<reth_primitives::BlockWithSenders>> {
        Ok(None)
    }

    fn block_range(&self, _range: RangeInclusive<BlockNumber>) -> RethResult<Vec<Block>> {
        Ok(vec![])
    }
}

impl BlockReaderIdExt for NoopProvider {
    fn block_by_id(&self, _id: BlockId) -> RethResult<Option<Block>> {
        Ok(None)
    }

    fn sealed_header_by_id(&self, _id: BlockId) -> RethResult<Option<SealedHeader>> {
        Ok(None)
    }

    fn header_by_id(&self, _id: BlockId) -> RethResult<Option<Header>> {
        Ok(None)
    }

    fn ommers_by_id(&self, _id: BlockId) -> RethResult<Option<Vec<Header>>> {
        Ok(None)
    }
}

impl BlockIdReader for NoopProvider {
    fn pending_block_num_hash(&self) -> RethResult<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }

    fn safe_block_num_hash(&self) -> RethResult<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }

    fn finalized_block_num_hash(&self) -> RethResult<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }
}

impl TransactionsProvider for NoopProvider {
    fn transaction_id(&self, _tx_hash: TxHash) -> RethResult<Option<TxNumber>> {
        Ok(None)
    }

    fn transaction_by_id(&self, _id: TxNumber) -> RethResult<Option<TransactionSigned>> {
        Ok(None)
    }

    fn transaction_by_id_no_hash(
        &self,
        _id: TxNumber,
    ) -> RethResult<Option<TransactionSignedNoHash>> {
        Ok(None)
    }

    fn transaction_by_hash(&self, _hash: TxHash) -> RethResult<Option<TransactionSigned>> {
        Ok(None)
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: TxHash,
    ) -> RethResult<Option<(TransactionSigned, TransactionMeta)>> {
        Ok(None)
    }

    fn transaction_block(&self, _id: TxNumber) -> RethResult<Option<BlockNumber>> {
        todo!()
    }

    fn transactions_by_block(
        &self,
        _block_id: BlockHashOrNumber,
    ) -> RethResult<Option<Vec<TransactionSigned>>> {
        Ok(None)
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> RethResult<Vec<Vec<TransactionSigned>>> {
        Ok(Vec::default())
    }

    fn senders_by_tx_range(&self, _range: impl RangeBounds<TxNumber>) -> RethResult<Vec<Address>> {
        Ok(Vec::default())
    }

    fn transactions_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> RethResult<Vec<reth_primitives::TransactionSignedNoHash>> {
        Ok(Vec::default())
    }

    fn transaction_sender(&self, _id: TxNumber) -> RethResult<Option<Address>> {
        Ok(None)
    }
}

impl ReceiptProvider for NoopProvider {
    fn receipt(&self, _id: TxNumber) -> RethResult<Option<Receipt>> {
        Ok(None)
    }

    fn receipt_by_hash(&self, _hash: TxHash) -> RethResult<Option<Receipt>> {
        Ok(None)
    }

    fn receipts_by_block(&self, _block: BlockHashOrNumber) -> RethResult<Option<Vec<Receipt>>> {
        Ok(None)
    }
}

impl ReceiptProviderIdExt for NoopProvider {}

impl HeaderProvider for NoopProvider {
    fn header(&self, _block_hash: &BlockHash) -> RethResult<Option<Header>> {
        Ok(None)
    }

    fn header_by_number(&self, _num: u64) -> RethResult<Option<Header>> {
        Ok(None)
    }

    fn header_td(&self, _hash: &BlockHash) -> RethResult<Option<U256>> {
        Ok(None)
    }

    fn header_td_by_number(&self, _number: BlockNumber) -> RethResult<Option<U256>> {
        Ok(None)
    }

    fn headers_range(&self, _range: impl RangeBounds<BlockNumber>) -> RethResult<Vec<Header>> {
        Ok(vec![])
    }

    fn sealed_headers_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> RethResult<Vec<SealedHeader>> {
        Ok(vec![])
    }

    fn sealed_header(&self, _number: BlockNumber) -> RethResult<Option<SealedHeader>> {
        Ok(None)
    }
}

impl AccountReader for NoopProvider {
    fn basic_account(&self, _address: Address) -> RethResult<Option<Account>> {
        Ok(None)
    }
}

impl ChangeSetReader for NoopProvider {
    fn account_block_changeset(
        &self,
        _block_number: BlockNumber,
    ) -> RethResult<Vec<AccountBeforeTx>> {
        Ok(Vec::default())
    }
}

impl StateRootProvider for NoopProvider {
    fn state_root(&self, _state: &BundleStateWithReceipts) -> RethResult<B256> {
        todo!()
    }
}

impl StateProvider for NoopProvider {
    fn storage(
        &self,
        _account: Address,
        _storage_key: StorageKey,
    ) -> RethResult<Option<StorageValue>> {
        Ok(None)
    }

    fn bytecode_by_hash(&self, _code_hash: B256) -> RethResult<Option<Bytecode>> {
        Ok(None)
    }

    fn proof(
        &self,
        _address: Address,
        _keys: &[B256],
    ) -> RethResult<(Vec<Bytes>, B256, Vec<Vec<Bytes>>)> {
        Ok((vec![], KECCAK_EMPTY, vec![]))
    }
}

impl EvmEnvProvider for NoopProvider {
    fn fill_env_at(
        &self,
        _cfg: &mut CfgEnv,
        _block_env: &mut BlockEnv,
        _at: BlockHashOrNumber,
    ) -> RethResult<()> {
        Ok(())
    }

    fn fill_env_with_header(
        &self,
        _cfg: &mut CfgEnv,
        _block_env: &mut BlockEnv,
        _header: &Header,
    ) -> RethResult<()> {
        Ok(())
    }

    fn fill_block_env_at(
        &self,
        _block_env: &mut BlockEnv,
        _at: BlockHashOrNumber,
    ) -> RethResult<()> {
        Ok(())
    }

    fn fill_block_env_with_header(
        &self,
        _block_env: &mut BlockEnv,
        _header: &Header,
    ) -> RethResult<()> {
        Ok(())
    }

    fn fill_cfg_env_at(&self, _cfg: &mut CfgEnv, _at: BlockHashOrNumber) -> RethResult<()> {
        Ok(())
    }

    fn fill_cfg_env_with_header(&self, _cfg: &mut CfgEnv, _header: &Header) -> RethResult<()> {
        Ok(())
    }
}

impl StateProviderFactory for NoopProvider {
    fn latest(&self) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(*self))
    }

    fn history_by_block_number(&self, _block: BlockNumber) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(*self))
    }

    fn history_by_block_hash(&self, _block: BlockHash) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(*self))
    }

    fn state_by_block_hash(&self, _block: BlockHash) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(*self))
    }

    fn pending(&self) -> RethResult<StateProviderBox<'_>> {
        Ok(Box::new(*self))
    }

    fn pending_state_by_hash(&self, _block_hash: B256) -> RethResult<Option<StateProviderBox<'_>>> {
        Ok(Some(Box::new(*self)))
    }

    fn pending_with_provider<'a>(
        &'a self,
        _post_state_data: Box<dyn crate::BundleStateDataProvider + 'a>,
    ) -> RethResult<StateProviderBox<'a>> {
        Ok(Box::new(*self))
    }
}

impl StageCheckpointReader for NoopProvider {
    fn get_stage_checkpoint(&self, _id: StageId) -> RethResult<Option<StageCheckpoint>> {
        Ok(None)
    }

    fn get_stage_checkpoint_progress(&self, _id: StageId) -> RethResult<Option<Vec<u8>>> {
        Ok(None)
    }
}

impl WithdrawalsProvider for NoopProvider {
    fn latest_withdrawal(&self) -> RethResult<Option<reth_primitives::Withdrawal>> {
        Ok(None)
    }
    fn withdrawals_by_block(
        &self,
        _id: BlockHashOrNumber,
        _timestamp: u64,
    ) -> RethResult<Option<Vec<reth_primitives::Withdrawal>>> {
        Ok(None)
    }
}

impl PruneCheckpointReader for NoopProvider {
    fn get_prune_checkpoint(&self, _segment: PruneSegment) -> RethResult<Option<PruneCheckpoint>> {
        Ok(None)
    }
}
