use crate::{
    canonical_chain::CanonicalChain,
    chain::{BlockChainId, BlockKind},
    metrics::TreeMetrics,
    AppendableChain, BlockBuffer, BlockIndices, BlockchainTreeConfig, BundleStateData,
    TreeExternals,
};
use reth_db::{cursor::DbCursorRO, database::Database, tables, transaction::DbTx};
use reth_interfaces::{
    blockchain_tree::{
        error::{BlockchainTreeError, CanonicalError, InsertBlockError, InsertBlockErrorKind},
        BlockStatus, CanonicalOutcome, InsertPayloadOk,
    },
    consensus::{Consensus, ConsensusError},
    executor::{BlockExecutionError, BlockValidationError},
    RethResult,
};
use reth_primitives::{
    BlockHash, BlockNumHash, BlockNumber, ForkBlock, Hardfork, PruneModes, Receipt, SealedBlock,
    SealedBlockWithSenders, SealedHeader, U256,
};
use reth_provider::{
    chain::{ChainSplit, SplitAt},
    BlockExecutionWriter, BlockNumReader, BlockWriter, BundleStateWithReceipts,
    CanonStateNotification, CanonStateNotificationSender, CanonStateNotifications, Chain,
    DatabaseProvider, DisplayBlocksChain, ExecutorFactory, HeaderProvider,
};
use reth_stages::{MetricEvent, MetricEventsSender};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use tracing::{debug, error, info, instrument, trace, warn};

#[derive(Debug)]
pub struct BlockchainTree<DB: Database, C: Consensus, EF: ExecutorFactory> {
    inner: Arc<BlockchainTreeInner<DB, C, EF>>
}


pub struct BlockchainTreeInner<DB: Database, C: Consensus, EF: ExecutorFactory> {

}