//! Various noop implementations for traits.

use crate::{
    AccountReader, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader, BlockReaderIdExt,
    BlockSource, ChangeSetReader, HashedPostStateProvider, HeaderProvider, NodePrimitivesProvider,
    PruneCheckpointReader, ReceiptProvider, ReceiptProviderIdExt, StageCheckpointReader,
    StateProofProvider, StateProvider, StateProviderBox, StateProviderFactory, StateRootProvider,
    StorageRootProvider, TransactionVariant, TransactionsProvider, WithdrawalsProvider,
};
use alloy_eips::{
    eip4895::{Withdrawal, Withdrawals},
    BlockHashOrNumber, BlockId, BlockNumberOrTag,
};
use alloy_primitives::{
    map::{HashMap, HashSet},
    Address, BlockHash, BlockNumber, Bytes, StorageKey, StorageValue, TxHash, TxNumber, B256, U256,
};
use reth_chainspec::{ChainInfo, ChainSpecProvider, EthChainSpec, MAINNET};
use reth_db_models::{AccountBeforeTx, StoredBlockBodyIndices};
use reth_primitives::{
    BlockWithSenders, EthPrimitives, SealedBlockFor, SealedBlockWithSenders, TransactionMeta,
};
use reth_primitives_traits::{Account, Bytecode, NodePrimitives, SealedHeader};
use reth_prune_types::{PruneCheckpoint, PruneSegment};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_errors::provider::{ProviderError, ProviderResult};
use reth_trie::{
    updates::TrieUpdates, AccountProof, HashedPostState, HashedStorage, MultiProof, TrieInput,
};
use std::{
    marker::PhantomData,
    ops::{RangeBounds, RangeInclusive},
    sync::Arc,
};

/// Supports various api interfaces for testing purposes.
#[derive(Debug)]
#[non_exhaustive]
pub struct NoopProvider<ChainSpec = reth_chainspec::ChainSpec, N = EthPrimitives> {
    chain_spec: Arc<ChainSpec>,
    _phantom: PhantomData<N>,
}

impl<ChainSpec, N> NoopProvider<ChainSpec, N> {
    /// Create a new instance for specific primitive types.
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec, _phantom: Default::default() }
    }
}

impl<ChainSpec> NoopProvider<ChainSpec> {
    /// Create a new instance of the `NoopBlockReader`.
    pub fn eth(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec, _phantom: Default::default() }
    }
}

impl NoopProvider {
    /// Create a new instance of the [`NoopProvider`] with the mainnet chain spec.
    pub fn mainnet() -> Self {
        Self::eth(MAINNET.clone())
    }
}

impl Default for NoopProvider {
    fn default() -> Self {
        Self::mainnet()
    }
}

impl<ChainSpec, N> Clone for NoopProvider<ChainSpec, N> {
    fn clone(&self) -> Self {
        Self { chain_spec: Arc::clone(&self.chain_spec), _phantom: Default::default() }
    }
}

/// Noop implementation for testing purposes
impl<ChainSpec: Send + Sync, N: Send + Sync> BlockHashReader for NoopProvider<ChainSpec, N> {
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

impl<ChainSpec: Send + Sync, N: Send + Sync> BlockNumReader for NoopProvider<ChainSpec, N> {
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

impl<ChainSpec: EthChainSpec + 'static, N: Send + Sync + 'static> ChainSpecProvider
    for NoopProvider<ChainSpec, N>
{
    type ChainSpec = ChainSpec;

    fn chain_spec(&self) -> Arc<Self::ChainSpec> {
        self.chain_spec.clone()
    }
}

impl<C: Send + Sync, N: NodePrimitives> BlockIdReader for NoopProvider<C, N> {
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

impl<C: Send + Sync, N: NodePrimitives> BlockReaderIdExt for NoopProvider<C, N> {
    fn block_by_id(&self, _id: BlockId) -> ProviderResult<Option<N::Block>> {
        Ok(None)
    }

    fn sealed_header_by_id(
        &self,
        _id: BlockId,
    ) -> ProviderResult<Option<SealedHeader<N::BlockHeader>>> {
        Ok(None)
    }

    fn header_by_id(&self, _id: BlockId) -> ProviderResult<Option<N::BlockHeader>> {
        Ok(None)
    }

    fn ommers_by_id(&self, _id: BlockId) -> ProviderResult<Option<Vec<N::BlockHeader>>> {
        Ok(None)
    }
}

impl<C: Send + Sync, N: NodePrimitives> BlockReader for NoopProvider<C, N> {
    type Block = N::Block;

    fn find_block_by_hash(
        &self,
        _hash: B256,
        _source: BlockSource,
    ) -> ProviderResult<Option<Self::Block>> {
        Ok(None)
    }

    fn block(&self, _id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
        Ok(None)
    }

    fn pending_block(&self) -> ProviderResult<Option<SealedBlockFor<Self::Block>>> {
        Ok(None)
    }

    fn pending_block_with_senders(
        &self,
    ) -> ProviderResult<Option<SealedBlockWithSenders<Self::Block>>> {
        Ok(None)
    }

    fn pending_block_and_receipts(
        &self,
    ) -> ProviderResult<Option<(SealedBlockFor<Self::Block>, Vec<Self::Receipt>)>> {
        Ok(None)
    }

    fn ommers(&self, _id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Self::Header>>> {
        Ok(None)
    }

    fn block_body_indices(&self, _num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        Ok(None)
    }

    fn block_with_senders(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<BlockWithSenders<Self::Block>>> {
        Ok(None)
    }

    fn sealed_block_with_senders(
        &self,
        _id: BlockHashOrNumber,
        _transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<SealedBlockWithSenders<Self::Block>>> {
        Ok(None)
    }

    fn block_range(&self, _range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Self::Block>> {
        Ok(vec![])
    }

    fn block_with_senders_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<BlockWithSenders<Self::Block>>> {
        Ok(vec![])
    }

    fn sealed_block_with_senders_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<SealedBlockWithSenders<Self::Block>>> {
        Ok(vec![])
    }
}

impl<C: Send + Sync, N: NodePrimitives> TransactionsProvider for NoopProvider<C, N> {
    type Transaction = N::SignedTx;

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

impl<C: Send + Sync, N: NodePrimitives> ReceiptProvider for NoopProvider<C, N> {
    type Receipt = N::Receipt;

    fn receipt(&self, _id: TxNumber) -> ProviderResult<Option<Self::Receipt>> {
        Ok(None)
    }

    fn receipt_by_hash(&self, _hash: TxHash) -> ProviderResult<Option<Self::Receipt>> {
        Ok(None)
    }

    fn receipts_by_block(
        &self,
        _block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        Ok(None)
    }

    fn receipts_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Receipt>> {
        Ok(vec![])
    }
}

impl<C: Send + Sync, N: NodePrimitives> ReceiptProviderIdExt for NoopProvider<C, N> {}

impl<C: Send + Sync, N: NodePrimitives> HeaderProvider for NoopProvider<C, N> {
    type Header = N::BlockHeader;

    fn header(&self, _block_hash: &BlockHash) -> ProviderResult<Option<Self::Header>> {
        Ok(None)
    }

    fn header_by_number(&self, _num: u64) -> ProviderResult<Option<Self::Header>> {
        Ok(None)
    }

    fn header_td(&self, _hash: &BlockHash) -> ProviderResult<Option<U256>> {
        Ok(None)
    }

    fn header_td_by_number(&self, _number: BlockNumber) -> ProviderResult<Option<U256>> {
        Ok(None)
    }

    fn headers_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Self::Header>> {
        Ok(vec![])
    }

    fn sealed_header(
        &self,
        _number: BlockNumber,
    ) -> ProviderResult<Option<SealedHeader<Self::Header>>> {
        Ok(None)
    }

    fn sealed_headers_while(
        &self,
        _range: impl RangeBounds<BlockNumber>,
        _predicate: impl FnMut(&SealedHeader<Self::Header>) -> bool,
    ) -> ProviderResult<Vec<SealedHeader<Self::Header>>> {
        Ok(vec![])
    }
}

impl<C: Send + Sync, N: NodePrimitives> AccountReader for NoopProvider<C, N> {
    fn basic_account(&self, _address: Address) -> ProviderResult<Option<Account>> {
        Ok(None)
    }
}

impl<C: Send + Sync, N: NodePrimitives> ChangeSetReader for NoopProvider<C, N> {
    fn account_block_changeset(
        &self,
        _block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        Ok(Vec::default())
    }
}

impl<C: Send + Sync, N: NodePrimitives> StateRootProvider for NoopProvider<C, N> {
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

impl<C: Send + Sync, N: NodePrimitives> StorageRootProvider for NoopProvider<C, N> {
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

impl<C: Send + Sync, N: NodePrimitives> StateProofProvider for NoopProvider<C, N> {
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

impl<C: Send + Sync, N: NodePrimitives> HashedPostStateProvider for NoopProvider<C, N> {
    fn hashed_post_state(&self, _bundle_state: &revm::db::BundleState) -> HashedPostState {
        HashedPostState::default()
    }
}

impl<C: Send + Sync, N: NodePrimitives> StateProvider for NoopProvider<C, N> {
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

impl<C: Send + Sync + 'static, N: NodePrimitives> StateProviderFactory for NoopProvider<C, N> {
    fn latest(&self) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
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
        Ok(Box::new(self.clone()))
    }

    fn history_by_block_hash(&self, _block: BlockHash) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn state_by_block_hash(&self, _block: BlockHash) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn pending(&self) -> ProviderResult<StateProviderBox> {
        Ok(Box::new(self.clone()))
    }

    fn pending_state_by_hash(&self, _block_hash: B256) -> ProviderResult<Option<StateProviderBox>> {
        Ok(Some(Box::new(self.clone())))
    }
}

// impl EvmEnvProvider for NoopProvider {
//     fn fill_env_at<EvmConfig>(
//         &self,
//         _cfg: &mut CfgEnvWithHandlerCfg,
//         _block_env: &mut BlockEnv,
//         _at: BlockHashOrNumber,
//         _evm_config: EvmConfig,
//     ) -> ProviderResult<()>
//     where
//         EvmConfig: ConfigureEvmEnv<Header = Header>,
//     {
//         Ok(())
//     }
//
//     fn fill_env_with_header<EvmConfig>(
//         &self,
//         _cfg: &mut CfgEnvWithHandlerCfg,
//         _block_env: &mut BlockEnv,
//         _header: &Header,
//         _evm_config: EvmConfig,
//     ) -> ProviderResult<()>
//     where
//         EvmConfig: ConfigureEvmEnv<Header = Header>,
//     {
//         Ok(())
//     }
//
//     fn fill_cfg_env_at<EvmConfig>(
//         &self,
//         _cfg: &mut CfgEnvWithHandlerCfg,
//         _at: BlockHashOrNumber,
//         _evm_config: EvmConfig,
//     ) -> ProviderResult<()>
//     where
//         EvmConfig: ConfigureEvmEnv<Header = Header>,
//     {
//         Ok(())
//     }
//
//     fn fill_cfg_env_with_header<EvmConfig>(
//         &self,
//         _cfg: &mut CfgEnvWithHandlerCfg,
//         _header: &Header,
//         _evm_config: EvmConfig,
//     ) -> ProviderResult<()>
//     where
//         EvmConfig: ConfigureEvmEnv<Header = Header>,
//     {
//         Ok(())
//     }
// }

impl<C: Send + Sync, N: NodePrimitives> StageCheckpointReader for NoopProvider<C, N> {
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

impl<C: Send + Sync, N: NodePrimitives> WithdrawalsProvider for NoopProvider<C, N> {
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

impl<C: Send + Sync, N: NodePrimitives> PruneCheckpointReader for NoopProvider<C, N> {
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

impl<C: Send + Sync, N: NodePrimitives> NodePrimitivesProvider for NoopProvider<C, N> {
    type Primitives = N;
}
