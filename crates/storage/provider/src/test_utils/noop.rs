use crate::{
    traits::{BlockSource, ReceiptProvider},
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, BlockReaderIdExt,
    ChainSpecProvider, ChangeSetReader, EvmEnvProvider, HeaderProvider, PostState,
    PruneCheckpointReader, ReceiptProviderIdExt, StageCheckpointReader, StateProvider,
    StateProviderBox, StateProviderFactory, StateRootProvider, TransactionsProvider,
    WithdrawalsProvider,
};
use reth_db::models::{AccountBeforeTx, StoredBlockBodyIndices};
use reth_interfaces::Result;
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    Account, Address, Block, BlockHash, BlockHashOrNumber, BlockId, BlockNumber, Bytecode, Bytes,
    ChainInfo, ChainSpec, Header, PruneCheckpoint, PrunePart, Receipt, SealedBlock, SealedHeader,
    StorageKey, StorageValue, TransactionMeta, TransactionSigned, TransactionSignedNoHash, TxHash,
    TxNumber, H256, KECCAK_EMPTY, MAINNET, U256,
};
use reth_revm_primitives::primitives::{BlockEnv, CfgEnv};
use std::{ops::RangeBounds, sync::Arc};

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
    fn block_hash(&self, _number: u64) -> Result<Option<H256>> {
        Ok(None)
    }

    fn canonical_hashes_range(&self, _start: BlockNumber, _end: BlockNumber) -> Result<Vec<H256>> {
        Ok(vec![])
    }
}

impl BlockNumReader for NoopProvider {
    fn chain_info(&self) -> Result<ChainInfo> {
        Ok(ChainInfo::default())
    }

    fn best_block_number(&self) -> Result<BlockNumber> {
        Ok(0)
    }

    fn last_block_number(&self) -> Result<BlockNumber> {
        Ok(0)
    }

    fn block_number(&self, _hash: H256) -> Result<Option<BlockNumber>> {
        Ok(None)
    }
}

impl BlockReader for NoopProvider {
    fn find_block_by_hash(&self, hash: H256, _source: BlockSource) -> Result<Option<Block>> {
        self.block(hash.into())
    }

    fn block(&self, _id: BlockHashOrNumber) -> Result<Option<Block>> {
        Ok(None)
    }

    fn pending_block(&self) -> Result<Option<SealedBlock>> {
        Ok(None)
    }

    fn pending_block_and_receipts(&self) -> Result<Option<(SealedBlock, Vec<Receipt>)>> {
        Ok(None)
    }

    fn ommers(&self, _id: BlockHashOrNumber) -> Result<Option<Vec<Header>>> {
        Ok(None)
    }

    fn block_body_indices(&self, _num: u64) -> Result<Option<StoredBlockBodyIndices>> {
        Ok(None)
    }

    fn block_with_senders(
        &self,
        _number: BlockNumber,
    ) -> Result<Option<reth_primitives::BlockWithSenders>> {
        Ok(None)
    }
}

impl BlockReaderIdExt for NoopProvider {
    fn block_by_id(&self, _id: BlockId) -> Result<Option<Block>> {
        Ok(None)
    }

    fn sealed_header_by_id(&self, _id: BlockId) -> Result<Option<SealedHeader>> {
        Ok(None)
    }

    fn header_by_id(&self, _id: BlockId) -> Result<Option<Header>> {
        Ok(None)
    }

    fn ommers_by_id(&self, _id: BlockId) -> Result<Option<Vec<Header>>> {
        Ok(None)
    }
}

impl BlockIdReader for NoopProvider {
    fn pending_block_num_hash(&self) -> Result<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }

    fn safe_block_num_hash(&self) -> Result<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }

    fn finalized_block_num_hash(&self) -> Result<Option<reth_primitives::BlockNumHash>> {
        Ok(None)
    }
}

impl TransactionsProvider for NoopProvider {
    fn transaction_id(&self, _tx_hash: TxHash) -> Result<Option<TxNumber>> {
        Ok(None)
    }

    fn transaction_by_id(&self, _id: TxNumber) -> Result<Option<TransactionSigned>> {
        Ok(None)
    }

    fn transaction_by_id_no_hash(&self, _id: TxNumber) -> Result<Option<TransactionSignedNoHash>> {
        Ok(None)
    }

    fn transaction_by_hash(&self, _hash: TxHash) -> Result<Option<TransactionSigned>> {
        Ok(None)
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: TxHash,
    ) -> Result<Option<(TransactionSigned, TransactionMeta)>> {
        Ok(None)
    }

    fn transaction_block(&self, _id: TxNumber) -> Result<Option<BlockNumber>> {
        todo!()
    }

    fn transactions_by_block(
        &self,
        _block_id: BlockHashOrNumber,
    ) -> Result<Option<Vec<TransactionSigned>>> {
        Ok(None)
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<Vec<TransactionSigned>>> {
        Ok(Vec::default())
    }

    fn senders_by_tx_range(&self, _range: impl RangeBounds<TxNumber>) -> Result<Vec<Address>> {
        Ok(Vec::default())
    }

    fn transactions_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> Result<Vec<reth_primitives::TransactionSignedNoHash>> {
        Ok(Vec::default())
    }

    fn transaction_sender(&self, _id: TxNumber) -> Result<Option<Address>> {
        Ok(None)
    }
}

impl ReceiptProvider for NoopProvider {
    fn receipt(&self, _id: TxNumber) -> Result<Option<Receipt>> {
        Ok(None)
    }

    fn receipt_by_hash(&self, _hash: TxHash) -> Result<Option<Receipt>> {
        Ok(None)
    }

    fn receipts_by_block(&self, _block: BlockHashOrNumber) -> Result<Option<Vec<Receipt>>> {
        Ok(None)
    }
}

impl ReceiptProviderIdExt for NoopProvider {}

impl HeaderProvider for NoopProvider {
    fn header(&self, _block_hash: &BlockHash) -> Result<Option<Header>> {
        Ok(None)
    }

    fn header_by_number(&self, _num: u64) -> Result<Option<Header>> {
        Ok(None)
    }

    fn header_td(&self, _hash: &BlockHash) -> Result<Option<U256>> {
        Ok(None)
    }

    fn header_td_by_number(&self, _number: BlockNumber) -> Result<Option<U256>> {
        Ok(None)
    }

    fn headers_range(&self, _range: impl RangeBounds<BlockNumber>) -> Result<Vec<Header>> {
        Ok(vec![])
    }

    fn sealed_headers_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<SealedHeader>> {
        Ok(vec![])
    }

    fn sealed_header(&self, _number: BlockNumber) -> Result<Option<SealedHeader>> {
        Ok(None)
    }
}

impl AccountReader for NoopProvider {
    fn basic_account(&self, _address: Address) -> Result<Option<Account>> {
        Ok(None)
    }
}

impl ChangeSetReader for NoopProvider {
    fn account_block_changeset(&self, _block_number: BlockNumber) -> Result<Vec<AccountBeforeTx>> {
        Ok(Vec::default())
    }
}

impl StateRootProvider for NoopProvider {
    fn state_root(&self, _post_state: PostState) -> Result<H256> {
        todo!()
    }
}

impl StateProvider for NoopProvider {
    fn storage(&self, _account: Address, _storage_key: StorageKey) -> Result<Option<StorageValue>> {
        Ok(None)
    }

    fn bytecode_by_hash(&self, _code_hash: H256) -> Result<Option<Bytecode>> {
        Ok(None)
    }

    fn proof(
        &self,
        _address: Address,
        _keys: &[H256],
    ) -> Result<(Vec<Bytes>, H256, Vec<Vec<Bytes>>)> {
        Ok((vec![], KECCAK_EMPTY, vec![]))
    }
}

impl EvmEnvProvider for NoopProvider {
    fn fill_env_at(
        &self,
        _cfg: &mut CfgEnv,
        _block_env: &mut BlockEnv,
        _at: BlockHashOrNumber,
    ) -> Result<()> {
        Ok(())
    }

    fn fill_env_with_header(
        &self,
        _cfg: &mut CfgEnv,
        _block_env: &mut BlockEnv,
        _header: &Header,
    ) -> Result<()> {
        Ok(())
    }

    fn fill_block_env_at(&self, _block_env: &mut BlockEnv, _at: BlockHashOrNumber) -> Result<()> {
        Ok(())
    }

    fn fill_block_env_with_header(
        &self,
        _block_env: &mut BlockEnv,
        _header: &Header,
    ) -> Result<()> {
        Ok(())
    }

    fn fill_cfg_env_at(&self, _cfg: &mut CfgEnv, _at: BlockHashOrNumber) -> Result<()> {
        Ok(())
    }

    fn fill_cfg_env_with_header(&self, _cfg: &mut CfgEnv, _header: &Header) -> Result<()> {
        Ok(())
    }
}

impl StateProviderFactory for NoopProvider {
    fn latest(&self) -> Result<StateProviderBox<'_>> {
        Ok(Box::new(*self))
    }

    fn history_by_block_number(&self, _block: BlockNumber) -> Result<StateProviderBox<'_>> {
        Ok(Box::new(*self))
    }

    fn history_by_block_hash(&self, _block: BlockHash) -> Result<StateProviderBox<'_>> {
        Ok(Box::new(*self))
    }

    fn state_by_block_hash(&self, _block: BlockHash) -> Result<StateProviderBox<'_>> {
        Ok(Box::new(*self))
    }

    fn pending(&self) -> Result<StateProviderBox<'_>> {
        Ok(Box::new(*self))
    }

    fn pending_state_by_hash(&self, _block_hash: H256) -> Result<Option<StateProviderBox<'_>>> {
        Ok(Some(Box::new(*self)))
    }

    fn pending_with_provider<'a>(
        &'a self,
        _post_state_data: Box<dyn crate::PostStateDataProvider + 'a>,
    ) -> Result<StateProviderBox<'a>> {
        Ok(Box::new(*self))
    }
}

impl StageCheckpointReader for NoopProvider {
    fn get_stage_checkpoint(&self, _id: StageId) -> Result<Option<StageCheckpoint>> {
        Ok(None)
    }

    fn get_stage_checkpoint_progress(&self, _id: StageId) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

impl WithdrawalsProvider for NoopProvider {
    fn latest_withdrawal(&self) -> Result<Option<reth_primitives::Withdrawal>> {
        Ok(None)
    }
    fn withdrawals_by_block(
        &self,
        _id: BlockHashOrNumber,
        _timestamp: u64,
    ) -> Result<Option<Vec<reth_primitives::Withdrawal>>> {
        Ok(None)
    }
}

impl PruneCheckpointReader for NoopProvider {
    fn get_prune_checkpoint(&self, _part: PrunePart) -> Result<Option<PruneCheckpoint>> {
        Ok(None)
    }
}
