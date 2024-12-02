use std::{
    ops::{RangeBounds, RangeInclusive},
    path::PathBuf,
    sync::Arc,
};

use alloy_consensus::Header;
use alloy_eips::{
    eip4895::{Withdrawal, Withdrawals},
    BlockHashOrNumber, BlockId, BlockNumberOrTag,
};
use alloy_primitives::{
    map::{HashMap, HashSet},
    Address, BlockHash, BlockNumber, Bytes, StorageKey, StorageValue, TxHash, TxNumber, B256, U256,
};
use reth_chain_state::{
    CanonStateNotifications, CanonStateSubscriptions, ForkChoiceNotifications,
    ForkChoiceSubscriptions,
};
use reth_chainspec::{ChainInfo, ChainSpec, MAINNET};
use reth_db_api::models::{AccountBeforeTx, StoredBlockBodyIndices};
use reth_errors::ProviderError;
use reth_evm::ConfigureEvmEnv;
use reth_primitives::{
    Account, Block, BlockWithSenders, Bytecode, EthPrimitives, Receipt, SealedBlock,
    SealedBlockWithSenders, SealedHeader, TransactionMeta, TransactionSigned,
};
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{NodePrimitivesProvider, StateProofProvider, StorageRootProvider};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof, TrieInput,
};
use revm::primitives::{BlockEnv, CfgEnvWithHandlerCfg};
use tokio::sync::{broadcast, watch};

use crate::{
    providers::StaticFileProvider,
    traits::{BlockSource, ReceiptProvider},
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, BlockReaderIdExt,
    ChainSpecProvider, ChangeSetReader, EvmEnvProvider, HeaderProvider, PruneCheckpointReader,
    ReceiptProviderIdExt, StageCheckpointReader, StateProvider, StateProviderBox,
    StateProviderFactory, StateRootProvider, StaticFileProviderFactory, TransactionVariant,
    TransactionsProvider, WithdrawalsProvider,
};

/// Supports various api interfaces for testing purposes.
#[derive(Debug, Clone, Default, Copy)]
#[non_exhaustive]
pub struct NoopProvider;

impl ChainSpecProvider for NoopProvider {
    type ChainSpec = ChainSpec;

    fn chain_spec(&self) -> Arc<ChainSpec> {
        MAINNET.clone()
    }
}

/// Noop implementation for testing purposes
impl BlockHashReader for NoopProvider {
    fn block_hash(&self, _number: u64) -> ProviderResult<Option<B256>> {
        Ok(None)
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        Ok(vec![])
    }
}

impl BlockNumReader for NoopProvider {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        Ok(ChainInfo::default())
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(0)
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        Ok(0)
    }

    fn block_number(&self, _hash: B256) -> ProviderResult<Option<BlockNumber>> {
        Ok(None)
    }
}

impl BlockReader for NoopProvider {
    type Block = Block;

    fn find_block_by_hash(
        &self,
        hash: B256,
        _source: BlockSource,
    ) -> ProviderResult<Option<Block>> {
        self.block(hash.into())
    }

    fn block(&self, _id: BlockHashOrNumber) -> ProviderResult<Option<Block>> {
        Ok(None)
    }

    fn pending_block(&self) -> ProviderResult<Option<SealedBlock>> {
        Ok(None)
    }

    fn pending_block_with_senders(&self) -> ProviderResult<Option<SealedBlockWithSenders>> {
        Ok(None)
    }

    fn pending_block_and_receipts(&self) -> ProviderResult<Option<(SealedBlock, Vec<Receipt>)>> {
        Ok(None)
    }

    fn ommers(&self, _id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Header>>> {
        Ok(None)
    }

    fn block_body_indices(&self, _num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        Ok(None)
    }

    fn block_with_senders(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<reth_primitives::BlockWithSenders>> {
        Ok(None)
    }

    fn sealed_block_with_senders(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<SealedBlockWithSenders>> {
        Ok(None)
    }

    fn block_range(&self, _range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Block>> {
        Ok(vec![])
    }

    fn block_with_senders_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<BlockWithSenders>> {
        Ok(vec![])
    }

    fn sealed_block_with_senders_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<SealedBlockWithSenders>> {
        Ok(vec![])
    }
}

impl BlockReaderIdExt for NoopProvider {
    fn block_by_id(&self, _id: BlockId) -> ProviderResult<Option<Block>> {
        Ok(None)
    }

    fn sealed_header_by_id(&self, _id: BlockId) -> ProviderResult<Option<SealedHeader>> {
        Ok(None)
    }

    fn header_by_id(&self, _id: BlockId) -> ProviderResult<Option<Header>> {
        Ok(None)
    }

    fn ommers_by_id(&self, _id: BlockId) -> ProviderResult<Option<Vec<Header>>> {
        Ok(None)
    }
}

impl BlockIdReader for NoopProvider {
    fn pending_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        Ok(None)
    }

    fn safe_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        Ok(None)
    }

    fn finalized_block_num_hash(&self) -> ProviderResult<Option<alloy_eips::BlockNumHash>> {
        Ok(None)
    }
}

impl TransactionsProvider for NoopProvider {
    type Transaction = TransactionSigned;

    fn transaction_id(&self, _tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        Ok(None)
    }

    fn transaction_by_id(&self, _id: TxNumber) -> ProviderResult<Option<Self::Transaction>> {
        Ok(None)
    }

    fn transaction_by_id_unhashed(
        &self,
        _id: TxNumber,
    ) -> ProviderResult<Option<Self::Transaction>> {
        Ok(None)
    }

    fn transaction_by_hash(&self, _hash: TxHash) -> ProviderResult<Option<Self::Transaction>> {
        Ok(None)
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: TxHash,
    ) -> ProviderResult<Option<(Self::Transaction, TransactionMeta)>> {
        Ok(None)
    }

    fn transaction_block(&self, _id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        todo!()
    }

    fn transactions_by_block(
        &self,
        _block_id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Transaction>>> {
        Ok(None)
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Transaction>>> {
        Ok(Vec::default())
    }

    fn transactions_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Transaction>> {
        Ok(Vec::default())
    }

    fn senders_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        Ok(Vec::default())
    }

    fn transaction_sender(&self, _id: TxNumber) -> ProviderResult<Option<Address>> {
        Ok(None)
    }
}

impl ReceiptProvider for NoopProvider {
    type Receipt = Receipt;
    fn receipt(&self, _id: TxNumber) -> ProviderResult<Option<Receipt>> {
        Ok(None)
    }

    fn receipt_by_hash(&self, _hash: TxHash) -> ProviderResult<Option<Receipt>> {
        Ok(None)
    }

    fn receipts_by_block(&self, _block: BlockHashOrNumber) -> ProviderResult<Option<Vec<Receipt>>> {
        Ok(None)
    }

    fn receipts_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Receipt>> {
        Ok(vec![])
    }
}

impl ReceiptProviderIdExt for NoopProvider {}

impl HeaderProvider for NoopProvider {
    fn header(&self, _block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        Ok(None)
    }

    fn header_by_number(&self, _num: u64) -> ProviderResult<Option<Header>> {
        Ok(None)
    }

    fn header_td(&self, _hash: &BlockHash) -> ProviderResult<Option<U256>> {
        Ok(None)
    }

    fn header_td_by_number(&self, _number: BlockNumber) -> ProviderResult<Option<U256>> {
        Ok(None)
    }

    fn headers_range(&self, _range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        Ok(vec![])
    }

    fn sealed_header(&self, _number: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        Ok(None)
    }

    fn sealed_headers_while(
        &self,
        _range: impl RangeBounds<BlockNumber>,
        _predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        Ok(vec![])
    }
}

impl AccountReader for NoopProvider {
    fn basic_account(&self, _address: Address) -> ProviderResult<Option<Account>> {
        Ok(None)
    }
}

impl ChangeSetReader for NoopProvider {
    fn account_block_changeset(
        &self,
        _block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        Ok(Vec::default())
    }
}

impl StateRootProvider for NoopProvider {
    fn state_root(&self, _state: HashedPostState) -> ProviderResult<B256> {
        Ok(B256::default())
    }

    fn state_root_from_nodes(&self, _input: TrieInput) -> ProviderResult<B256> {
        Ok(B256::default())
    }

    fn state_root_with_updates(
        &self,
        _state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        Ok((B256::default(), TrieUpdates::default()))
    }

    fn state_root_from_nodes_with_updates(
        &self,
        _input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        Ok((B256::default(), TrieUpdates::default()))
    }
}

impl StorageRootProvider for NoopProvider {
    fn storage_root(
        &self,
        _address: Address,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        Ok(B256::default())
    }

    fn storage_proof(
        &self,
        _address: Address,
        slot: B256,
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        Ok(reth_trie::StorageProof::new(slot))
    }

    fn storage_multiproof(
        &self,
        _address: Address,
        _slots: &[B256],
        _hashed_storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageMultiProof> {
        Ok(reth_trie::StorageMultiProof::empty())
    }
}

impl StateProofProvider for NoopProvider {
    fn proof(
        &self,
        _input: TrieInput,
        address: Address,
        _slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        Ok(AccountProof::new(address))
    }

    fn multiproof(
        &self,
        _input: TrieInput,
        _targets: HashMap<B256, HashSet<B256>>,
    ) -> ProviderResult<MultiProof> {
        Ok(MultiProof::default())
    }

    fn witness(
        &self,
        _input: TrieInput,
        _target: HashedPostState,
    ) -> ProviderResult<HashMap<B256, Bytes>> {
        Ok(HashMap::default())
    }
}

impl StateProvider for NoopProvider {
    fn storage(
        &self,
        _account: Address,
        _storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        Ok(None)
    }

    fn bytecode_by_hash(&self, _code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        Ok(None)
    }
}

impl EvmEnvProvider for NoopProvider {
    fn fill_env_at<EvmConfig>(
        &self,
        _cfg: &mut CfgEnvWithHandlerCfg,
        _block_env: &mut BlockEnv,
        _at: BlockHashOrNumber,
        _evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>,
    {
        Ok(())
    }

    fn fill_env_with_header<EvmConfig>(
        &self,
        _cfg: &mut CfgEnvWithHandlerCfg,
        _block_env: &mut BlockEnv,
        _header: &Header,
        _evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>,
    {
        Ok(())
    }

    fn fill_cfg_env_at<EvmConfig>(
        &self,
        _cfg: &mut CfgEnvWithHandlerCfg,
        _at: BlockHashOrNumber,
        _evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>,
    {
        Ok(())
    }

    fn fill_cfg_env_with_header<EvmConfig>(
        &self,
        _cfg: &mut CfgEnvWithHandlerCfg,
        _header: &Header,
        _evm_config: EvmConfig,
    ) -> ProviderResult<()>
    where
        EvmConfig: ConfigureEvmEnv<Header = Header>,
    {
        Ok(())
    }
}

impl StateProviderFactory for NoopProvider {
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(*self))
    }

    fn state_by_block_number_or_tag(
        &self,
        number_or_tag: BlockNumberOrTag,
    ) -> ProviderResult<StateProviderBox> {
        match number_or_tag {
            BlockNumberOrTag::Latest => self.latest(),
            BlockNumberOrTag::Finalized => {
                // we can only get the finalized state by hash, not by num
                let hash =
                    self.finalized_block_hash()?.ok_or(ProviderError::FinalizedBlockNotFound)?;

                // only look at historical state
                self.history_by_block_hash(hash)
            }
            BlockNumberOrTag::Safe => {
                // we can only get the safe state by hash, not by num
                let hash = self.safe_block_hash()?.ok_or(ProviderError::SafeBlockNotFound)?;

                self.history_by_block_hash(hash)
            }
            BlockNumberOrTag::Earliest => self.history_by_block_number(0),
            BlockNumberOrTag::Pending => self.pending(),
            BlockNumberOrTag::Number(num) => self.history_by_block_number(num),
        }
    }

    fn history_by_block_number(&self, _block: BlockNumber) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(*self))
    }

    fn history_by_block_hash(&self, _block: BlockHash) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(*self))
    }

    fn state_by_block_hash(&self, _block: BlockHash) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(*self))
    }

    fn pending(&self) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(*self))
    }

    fn pending_state_by_hash(&self, _block_hash: B256) -> ProviderResult<Option<StateProviderBox>> {
        Ok(Some(Box::new(*self)))
    }
}

impl StageCheckpointReader for NoopProvider {
    fn get_stage_checkpoint(&self, _id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
        Ok(None)
    }

    fn get_stage_checkpoint_progress(&self, _id: StageId) -> ProviderResult<Option<Vec<u8>>> {
        Ok(None)
    }

    fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>> {
        Ok(Vec::new())
    }
}

impl WithdrawalsProvider for NoopProvider {
    fn withdrawals_by_block(
        &self,
        _id: BlockHashOrNumber,
        _timestamp: u64,
    ) -> ProviderResult<Option<Withdrawals>> {
        Ok(None)
    }
    fn latest_withdrawal(&self) -> ProviderResult<Option<Withdrawal>> {
        Ok(None)
    }
}

impl PruneCheckpointReader for NoopProvider {
    fn get_prune_checkpoint(
        &self,
        _segment: PruneSegment,
    ) -> ProviderResult<Option<PruneCheckpoint>> {
        Ok(None)
    }

    fn get_prune_checkpoints(&self) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>> {
        Ok(Vec::new())
    }
}

impl NodePrimitivesProvider for NoopProvider {
    type Primitives = EthPrimitives;
}

impl StaticFileProviderFactory for NoopProvider {
    fn static_file_provider(&self) -> StaticFileProvider<Self::Primitives> {
        StaticFileProvider::read_only(PathBuf::default(), false).unwrap()
    }
}

impl CanonStateSubscriptions for NoopProvider {
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications {
        broadcast::channel(1).1
    }
}

impl ForkChoiceSubscriptions for NoopProvider {
    fn subscribe_safe_block(&self) -> ForkChoiceNotifications {
        let (_, rx) = watch::channel(None);
        ForkChoiceNotifications(rx)
    }

    fn subscribe_finalized_block(&self) -> ForkChoiceNotifications {
        let (_, rx) = watch::channel(None);
        ForkChoiceNotifications(rx)
    }
}
