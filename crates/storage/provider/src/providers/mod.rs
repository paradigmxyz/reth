mod database;
mod state;
pub use state::{
    historical::{HistoricalStateProvider, HistoricalStateProviderRef},
    latest::{LatestStateProvider, LatestStateProviderRef},
};
mod post_state_provider;
use crate::{
    providers::PostStateProvider, BlockHashProvider, BlockIdProvider, BlockProvider,
    BlockchainTreePendingStateProvider, EvmEnvProvider, HeaderProvider, PostStateDataProvider,
    ProviderError, ReceiptProvider, StateProviderBox, StateProviderFactory, TransactionsProvider,
    WithdrawalsProvider,
};
pub use database::*;
use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_interfaces::Result;
use reth_primitives::{
    Block, BlockHash, BlockId, BlockNumber, ChainInfo, ChainSpec, Hardfork, Head, Header, Receipt,
    TransactionMeta, TransactionSigned, TxHash, TxNumber, Withdrawal, H256, U256,
};
use reth_revm_primitives::{
    config::revm_spec,
    env::{fill_block_env, fill_cfg_and_block_env, fill_cfg_env},
    primitives::{BlockEnv, CfgEnv, SpecId},
};
use std::{ops::RangeBounds, sync::Arc};

/// The main type for interacting with the blockchain.
///
/// This type serves as the main entry point for interacting with the blockchain and provides data
/// from database storage and from the blockchain tree (pending state etc.) It is a simple wrapper
/// type that holds an instance of the database and the blockchain tree.
#[derive(Clone)]
pub struct BlockchainProvider<DB, Tree> {
    /// Provider type used to access the database.
    database: ShareableDatabase<DB>,
    /// The blockchain tree instance.
    tree: Tree,
}

impl<DB: Database, Tree> HeaderProvider for BlockchainProvider<DB, Tree> {
    fn header(&self, block_hash: &BlockHash) -> Result<Option<Header>> {
        self.database.header(block_hash)
    }

    fn header_by_number(&self, num: BlockNumber) -> Result<Option<Header>> {
        self.database.header_by_number(num)
    }

    fn header_td(&self, hash: &BlockHash) -> Result<Option<U256>> {
        self.database.header_td(hash)
    }

    fn header_td_by_number(&self, number: BlockNumber) -> Result<Option<U256>> {
        self.database.header_td_by_number(number)
    }

    fn headers_range(&self, range: impl RangeBounds<BlockNumber>) -> Result<Vec<Header>> {
        self.database.headers_range(range)
    }
}

impl<DB: Database, Tree> BlockHashProvider for BlockchainProvider<DB, Tree> {
    fn block_hash(&self, number: u64) -> Result<Option<H256>> {
        self.database.block_hash(number)
    }

    fn canonical_hashes_range(&self, start: BlockNumber, end: BlockNumber) -> Result<Vec<H256>> {
        self.database.canonical_hashes_range(start, end)
    }
}

impl<DB: Database, Tree> BlockIdProvider for BlockchainProvider<DB, Tree> {
    fn chain_info(&self) -> Result<ChainInfo> {
        self.database.chain_info()
    }

    fn block_number(&self, hash: H256) -> Result<Option<BlockNumber>> {
        self.database.block_number(hash)
    }
}

impl<DB: Database, Tree> BlockProvider for BlockchainProvider<DB, Tree> {
    fn block(&self, id: BlockId) -> Result<Option<Block>> {
        self.database.block(id)
    }

    fn ommers(&self, id: BlockId) -> Result<Option<Vec<Header>>> {
        self.database.ommers(id)
    }
}

impl<DB: Database, Tree> TransactionsProvider for BlockchainProvider<DB, Tree> {
    fn transaction_id(&self, tx_hash: TxHash) -> Result<Option<TxNumber>> {
        self.database.transaction_id(tx_hash)
    }

    fn transaction_by_id(&self, id: TxNumber) -> Result<Option<TransactionSigned>> {
        self.database.transaction_by_id(id)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> Result<Option<TransactionSigned>> {
        self.database.transaction_by_hash(hash)
    }

    fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> Result<Option<(TransactionSigned, TransactionMeta)>> {
        self.database.transaction_by_hash_with_meta(tx_hash)
    }

    fn transaction_block(&self, id: TxNumber) -> Result<Option<BlockNumber>> {
        self.database.transaction_block(id)
    }

    fn transactions_by_block(&self, id: BlockId) -> Result<Option<Vec<TransactionSigned>>> {
        self.database.transactions_by_block(id)
    }

    fn transactions_by_block_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> Result<Vec<Vec<TransactionSigned>>> {
        self.database.transactions_by_block_range(range)
    }
}

impl<DB: Database, Tree> ReceiptProvider for BlockchainProvider<DB, Tree> {
    fn receipt(&self, id: TxNumber) -> Result<Option<Receipt>> {
        self.database.receipt(id)
    }

    fn receipt_by_hash(&self, hash: TxHash) -> Result<Option<Receipt>> {
        self.database.receipt_by_hash(hash)
    }

    fn receipts_by_block(&self, block: BlockId) -> Result<Option<Vec<Receipt>>> {
        self.database.receipts_by_block(block)
    }
}

impl<DB: Database, Tree> WithdrawalsProvider for BlockchainProvider<DB, Tree> {
    fn withdrawals_by_block(&self, id: BlockId, timestamp: u64) -> Result<Option<Vec<Withdrawal>>> {
        self.database.withdrawals_by_block(id, timestamp)
    }

    fn latest_withdrawal(&self) -> Result<Option<Withdrawal>> {
        self.database.latest_withdrawal()
    }
}

impl<DB: Database, Tree> EvmEnvProvider for BlockchainProvider<DB, Tree> {
    fn fill_env_at(&self, cfg: &mut CfgEnv, block_env: &mut BlockEnv, at: BlockId) -> Result<()> {
        self.database.fill_env_at(cfg, block_env, at)
    }

    fn fill_env_with_header(
        &self,
        cfg: &mut CfgEnv,
        block_env: &mut BlockEnv,
        header: &Header,
    ) -> Result<()> {
        self.database.fill_env_with_header(cfg, block_env, header)
    }

    fn fill_block_env_at(&self, block_env: &mut BlockEnv, at: BlockId) -> Result<()> {
        self.database.fill_block_env_at(block_env, at)
    }

    fn fill_block_env_with_header(&self, block_env: &mut BlockEnv, header: &Header) -> Result<()> {
        self.database.fill_block_env_with_header(block_env, header)
    }

    fn fill_cfg_env_at(&self, cfg: &mut CfgEnv, at: BlockId) -> Result<()> {
        self.database.fill_cfg_env_at(cfg, at)
    }

    fn fill_cfg_env_with_header(&self, cfg: &mut CfgEnv, header: &Header) -> Result<()> {
        self.database.fill_cfg_env_with_header(cfg, header)
    }
}

impl<DB, Tree> StateProviderFactory for BlockchainProvider<DB, Tree>
where
    DB: Database,
    Tree: BlockchainTreePendingStateProvider,
{
    /// Storage provider for latest block
    fn latest(&self) -> Result<StateProviderBox<'_>> {
        self.database.latest()
    }

    fn history_by_block_number(&self, block_number: BlockNumber) -> Result<StateProviderBox<'_>> {
        self.database.history_by_block_number(block_number)
    }

    fn history_by_block_hash(&self, block_hash: BlockHash) -> Result<StateProviderBox<'_>> {
        self.database.history_by_block_hash(block_hash)
    }

    /// Storage provider for pending state.
    fn pending2(&self) -> Result<StateProviderBox<'_>> {
        todo!()
    }

    fn pending(
        &self,
        post_state_data: Box<dyn PostStateDataProvider>,
    ) -> Result<StateProviderBox<'_>> {
        todo!()
    }
}
