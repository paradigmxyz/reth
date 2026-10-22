use crate::{
    engine::EngineTestContext,
    network::NetworkTestContext,
    payload::PayloadTestContext,
    receipt::ensure_successful,
    rpc::RpcTestContext,
    wait::{poll_until, POLL_INTERVAL, WAIT_TIMEOUT},
    TmpDB,
};
use alloy_consensus::BlockHeader;
use alloy_eips::{eip2718::Encodable2718, BlockId};
use alloy_network::{Ethereum, IntoWallet, ReceiptResponse};
use alloy_primitives::{BlockHash, BlockNumber, Bytes, Sealable, B256};
use alloy_provider::{
    fillers::{FillProvider, RecommendedFillers, TxFiller},
    Provider, ProviderBuilder, RootProvider,
};
use alloy_rpc_types_engine::{
    ExecutionPayloadEnvelopeV5, ForkchoiceState, ForkchoiceUpdateError, ForkchoiceUpdated,
    PayloadStatus, PayloadStatusEnum,
};
use alloy_rpc_types_eth::BlockNumberOrTag;
use eyre::{bail, ensure, eyre, Ok, WrapErr};
use futures_util::{
    future::{select, BoxFuture, Either},
    Future,
};
use jsonrpsee::{core::client::ClientT, http_client::HttpClient};
use reth_chainspec::EthereumHardforks;
use reth_db::{mdbx::DatabaseArguments, open_db_read_only};
use reth_engine_primitives::BeaconForkChoiceUpdateError;
use reth_network_api::test_utils::PeersHandleProvider;
use reth_node_api::{
    Block, BlockBody, BlockTy, FullNodeComponents, NodePrimitives, PayloadTypes, PrimitivesTy,
};
use reth_node_builder::{rpc::RethRpcAddOns, FullNode, NodeTypes};
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    exit::NodeExitFuture,
};
use reth_payload_primitives::BuiltPayload;
use reth_primitives_traits::SealedBlock;
use reth_provider::{
    providers::RocksDBProvider, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader,
    BlockReaderIdExt, CanonStateNotificationStream, CanonStateSubscriptions, Chain,
    DatabaseProviderFactory, HeaderProvider, PruneCheckpointReader, StageCheckpointReader,
};
use reth_prune_types::PruneSegment;
use reth_rpc_api::TestingBuildBlockRequestV1;
use reth_rpc_builder::auth::AuthServerHandle;
use reth_rpc_eth_api::{
    helpers::{EthApiSpec, EthTransactions, LoadReceipt, TraceExt},
    EthApiTypes, RpcReceipt,
};
use reth_stages_types::StageId;
use reth_transaction_pool::TransactionPool;
use std::{
    pin::{pin, Pin},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};
use tokio_stream::StreamExt;
use url::Url;

/// Interval at which the alloy providers of [`NodeTestContext`] poll the node, e.g. for receipts
/// of pending transactions.
///
/// Much shorter than alloy's default for local nodes, since test nodes build blocks on demand or
/// with short dev block times.
pub const RPC_PROVIDER_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Maximum time [`NodeTestContext::stop`] waits for the database of a node to be released after
/// the node shut down.
///
/// The node itself releases it within milliseconds, so running into this timeout means that
/// something else still holds a handle of the database.
pub const DATABASE_RELEASE_TIMEOUT: Duration = Duration::from_secs(10);

/// A helper struct to handle node actions
#[expect(missing_debug_implementations)]
pub struct NodeTestContext<Node, AddOns>
where
    Node: FullNodeComponents,
    AddOns: RethRpcAddOns<Node>,
{
    /// The core structure representing the full node.
    pub inner: FullNode<Node, AddOns>,
    /// Context for testing payload-related features.
    pub payload: PayloadTestContext<<Node::Types as NodeTypes>::Payload>,
    /// Context for testing network functionalities.
    pub network: NetworkTestContext<Node::Network>,
    /// Context for testing RPC features.
    pub rpc: RpcTestContext<Node, AddOns::EthApi>,
    /// Context for sending payloads and forkchoice updates to the engine independent of the
    /// Engine API version.
    pub engine: EngineTestContext<<Node::Types as NodeTypes>::Payload>,
    /// Canonical state events.
    pub canonical_stream: CanonStateNotificationStream<PrimitivesTy<Node::Types>>,
    /// Which blocks [`Self::import_payload`] marks as safe and finalized.
    finality: Finality,
    /// What [`Self::stop`] needs to stop the node and launch it again, only set for restartable
    /// nodes of [`E2ETestSetupBuilder`](crate::E2ETestSetupBuilder).
    pub(crate) restart: Option<NodeRestart<Self>>,
    /// Resolves when the consensus engine of the node exits, see [`Self::take_exit_future`].
    ///
    /// The future is not `Sync`, but the context must be, so that helpers borrowing it return
    /// `Send` futures.
    pub(crate) exit_future: Mutex<Option<NodeExitFuture>>,
}

impl<Node, Payload, AddOns> NodeTestContext<Node, AddOns>
where
    Payload: PayloadTypes,
    Node: FullNodeComponents,
    Node::Types: NodeTypes<ChainSpec: EthereumHardforks, Payload = Payload>,
    Node::Network: PeersHandleProvider,
    AddOns: RethRpcAddOns<Node>,
{
    /// Creates a new test node
    ///
    /// Payload timestamps start after the default Cancun timestamp of the payload context, or after
    /// the timestamp of the latest block if it is newer, e.g. for a custom genesis or an imported
    /// chain.
    pub async fn new(
        node: FullNode<Node, AddOns>,
        attributes_generator: impl Fn(u64) -> Payload::PayloadAttributes + Send + Sync + 'static,
    ) -> eyre::Result<Self> {
        let mut payload =
            PayloadTestContext::new(node.payload_builder_handle.clone(), attributes_generator)
                .await?;
        if let Some(latest) =
            node.provider.sealed_header_by_number_or_tag(BlockNumberOrTag::Latest)?
        {
            payload.timestamp = payload.timestamp.max(latest.timestamp());
        }
        Ok(Self {
            inner: node.clone(),
            payload,
            network: NetworkTestContext::new(node.network.clone()),
            rpc: RpcTestContext { inner: node.add_ons_handle.rpc_registry },
            engine: EngineTestContext::new(node.add_ons_handle.beacon_engine_handle),
            canonical_stream: node.provider.canonical_state_stream(),
            finality: Finality::default(),
            restart: None,
            exit_future: Mutex::new(None),
        })
    }

    /// Establish a connection to the node
    pub async fn connect(&mut self, node: &mut Self) {
        self.network.add_peer(node.network.record()).await;
        node.network.next_session_established().await;
        self.network.next_session_established().await;
    }

    /// Advances the chain `length` blocks.
    ///
    /// Returns the added chain as a Vec of block hashes.
    pub async fn advance(
        &mut self,
        length: u64,
        tx_generator: impl Fn(u64) -> Pin<Box<dyn Future<Output = Bytes>>>,
    ) -> eyre::Result<Vec<Payload::BuiltPayload>>
    where
        AddOns::EthApi: EthApiSpec<Provider: BlockReader<Block = BlockTy<Node::Types>>>
            + EthTransactions
            + TraceExt,
    {
        let mut chain = Vec::with_capacity(length as usize);
        for i in 0..length {
            let (_, payload) = self.inject_and_advance(tx_generator(i).await).await?;
            chain.push(payload);
        }
        Ok(chain)
    }

    /// Injects the raw transaction into the pool, advances the node one block and waits until the
    /// block is committed, returning the transaction hash and the built payload.
    ///
    /// Returns an error if the transaction is not included in the block, see
    /// [`Self::assert_new_block`].
    pub async fn inject_and_advance(
        &mut self,
        raw_tx: Bytes,
    ) -> eyre::Result<(B256, Payload::BuiltPayload)>
    where
        AddOns::EthApi: EthApiSpec<Provider: BlockReader<Block = BlockTy<Node::Types>>>
            + EthTransactions
            + TraceExt,
    {
        let tx_hash = self.rpc.inject_tx(raw_tx).await?;
        let payload = self.advance_block().await?;
        self.assert_new_block(tx_hash, payload.block().hash(), payload.block().number()).await?;
        Ok((tx_hash, payload))
    }

    /// Returns the current forkchoice state of the node.
    pub fn current_forkchoice_state(&self) -> eyre::Result<ForkchoiceState> {
        let latest_header = self
            .inner
            .provider
            .sealed_header_by_number_or_tag(BlockNumberOrTag::Latest)?
            .ok_or_else(|| eyre!("latest block not found"))?;

        if latest_header.number() == 0 {
            return Ok(ForkchoiceState::same_hash(latest_header.hash()));
        }

        Ok(ForkchoiceState {
            head_block_hash: latest_header.hash(),
            safe_block_hash: self
                .inner
                .provider
                .sealed_header_by_number_or_tag(BlockNumberOrTag::Safe)?
                .ok_or_else(|| eyre!("safe block not found"))?
                .hash(),
            finalized_block_hash: self
                .inner
                .provider
                .sealed_header_by_number_or_tag(BlockNumberOrTag::Finalized)?
                .ok_or_else(|| eyre!("finalized block not found"))?
                .hash(),
        })
    }

    /// Sets the timestamp of the next payload built by [`Self::new_payload`] and the methods
    /// built on it, such as [`Self::advance_block`].
    ///
    /// Later payloads continue from this timestamp, one second apart. The attributes generator is
    /// called with the new timestamp, so fork-dependent attributes follow it.
    ///
    /// Returns an error if the timestamp is not greater than the timestamp of the current latest
    /// block, which is the parent of the next payload.
    pub fn set_next_payload_timestamp(&mut self, timestamp: u64) -> eyre::Result<()> {
        let latest = self
            .inner
            .provider
            .sealed_header_by_number_or_tag(BlockNumberOrTag::Latest)?
            .ok_or_else(|| eyre!("latest block not found"))?;
        ensure!(
            timestamp > latest.timestamp(),
            "next payload timestamp {timestamp} must be greater than the latest block timestamp {}",
            latest.timestamp()
        );
        // The payload context increments its timestamp before generating the next attributes.
        self.payload.timestamp = timestamp - 1;
        Ok(())
    }

    /// Builds a payload on top of the latest block and returns it without submitting it to the
    /// engine.
    ///
    /// Sends a forkchoice update with the next payload attributes, see
    /// [`Self::set_next_payload_timestamp`], and resolves the payload job it starts with
    /// [`PayloadKind::WaitForPending`], which waits for the build in progress instead of returning
    /// an empty payload. The payload can still contain no transactions, e.g. if the pool had none
    /// when the build started. Payload jobs started by others, e.g. by forkchoice updates with
    /// payload attributes sent by the test, do not affect this.
    ///
    /// Returns an error if the forkchoice update fails or starts no payload job, e.g. because the
    /// engine is syncing, or if the payload job fails to build a payload.
    ///
    /// [`PayloadKind::WaitForPending`]: reth_payload_primitives::PayloadKind::WaitForPending
    pub async fn new_payload(&mut self) -> eyre::Result<Payload::BuiltPayload> {
        self.build_payload(self.current_forkchoice_state()?).await
    }

    /// Builds a payload on the head of `state` with the next payload attributes and returns it
    /// without submitting it.
    ///
    /// Returns an error if the engine does not report the forkchoice update valid, see
    /// [`Self::build_payload_on`].
    async fn build_payload(
        &mut self,
        state: ForkchoiceState,
    ) -> eyre::Result<Payload::BuiltPayload> {
        let attributes = self.payload.next_attributes();
        let updated = self.send_forkchoice_updated(state, Some(attributes)).await?;
        let payload_id = updated.payload_id.ok_or_else(|| {
            eyre!(
                "forkchoice update to block {} with payload attributes started no payload job: {}",
                state.head_block_hash,
                updated.payload_status.status
            )
        })?;
        self.payload.resolve_payload(payload_id).await
    }

    /// Triggers payload building job and submits it to the engine.
    ///
    /// Returns an error if the engine reports the payload invalid, see [`Self::submit_payload`].
    pub async fn build_and_submit_payload(&mut self) -> eyre::Result<Payload::BuiltPayload> {
        let payload = self.new_payload().await?;

        self.submit_payload(payload.clone()).await?;

        Ok(payload)
    }

    /// Advances the node forward one block by building a payload and importing it with
    /// [`Self::import_payload`].
    ///
    /// The block becomes the head, the safe and finalized blocks follow the [`Finality`] policy of
    /// the context.
    pub async fn advance_block(&mut self) -> eyre::Result<Payload::BuiltPayload> {
        let payload = self.new_payload().await?;

        self.import_payload(payload.clone()).await?;

        Ok(payload)
    }

    /// Advances the node forward one block like [`Self::advance_block`] and waits until the
    /// transaction pool processed the new block, see [`Self::wait_for_pool_head`].
    ///
    /// This only waits for the canonical state update of the pool itself, which removes the
    /// transactions mined in the block and updates sender nonces and pending fees. Nodes that run
    /// additional pool maintenance tasks on new blocks have to wait for the expected contents of
    /// the pool instead, e.g. with [`Self::wait_for_pool_removal`] for the mined transactions.
    pub async fn advance_block_synced(&mut self) -> eyre::Result<Payload::BuiltPayload> {
        let payload = self.advance_block().await?;
        self.wait_for_pool_head(payload.block().hash()).await?;
        Ok(payload)
    }

    /// Advances the chain `length` blocks, see [`Self::advance_block`].
    ///
    /// Unlike [`Self::advance`], this does not inject transactions, so the blocks include the
    /// pending transactions of the pool, if any.
    ///
    /// Returns the built payloads.
    pub async fn advance_blocks(
        &mut self,
        length: u64,
    ) -> eyre::Result<Vec<Payload::BuiltPayload>> {
        let mut chain = Vec::with_capacity(length as usize);
        for _ in 0..length {
            chain.push(self.advance_block().await?);
        }
        Ok(chain)
    }

    /// Builds a payload on the block with the given hash with the next payload attributes and
    /// returns it without submitting it, like [`Self::new_payload`] does for the head.
    ///
    /// The payload job is started with a forkchoice update to `parent` that keeps the safe and
    /// finalized blocks of the node. Its effect depends on `parent`:
    /// - The head, or a canonical ancestor of it that is not below the finalized block: the head
    ///   stays, the engine builds on `parent` without changing the canonical chain. The exception
    ///   is a tree config with both `always_process_payload_attributes_on_canonical_head` and
    ///   `unwind_canonical_header`, under which the engine moves the head back to an ancestor.
    /// - A block on a side chain: the engine first makes the side chain canonical, so `parent`
    ///   becomes the head, reorging the chain.
    /// - A canonical block below the finalized block, or a side chain block that does not descend
    ///   from the finalized block: the engine rejects the update and this returns an error, the
    ///   head stays. Blocks imported under [`Finality::Head`] are finalized, import them under
    ///   [`Finality::Keep`] or [`Finality::Lag`] to build on them later, see
    ///   [`Self::set_finality`].
    /// - A block the node does not know: the engine starts no payload job and requests the block
    ///   from its peers, and this returns an error.
    ///
    /// The payload includes transactions of the pool, which tracks the head: transactions mined in
    /// the blocks above `parent` are not in the pool anymore, so the payload does not include them,
    /// and pool transactions that are invalid on `parent` are skipped.
    pub async fn build_payload_on(&mut self, parent: B256) -> eyre::Result<Payload::BuiltPayload> {
        self.build_payload(ForkchoiceState {
            head_block_hash: parent,
            ..self.current_forkchoice_state()?
        })
        .await
    }

    /// Builds a block on the block with the given hash and imports it with
    /// [`Self::import_payload`], so it becomes the head.
    ///
    /// If `parent` is not the head, this reorgs the chain, so `parent` must be the finalized block
    /// or descend from it, see [`Self::build_payload_on`]. The safe and finalized blocks follow
    /// the [`Finality`] policy of the context. The transaction pool processes the reorg in the
    /// background, see [`Self::reorg_to`].
    pub async fn advance_block_on(&mut self, parent: B256) -> eyre::Result<Payload::BuiltPayload> {
        let payload = self.build_payload_on(parent).await?;

        self.import_payload(payload.clone()).await?;

        Ok(payload)
    }

    /// Builds a chain of `length` blocks on the block with the given hash and makes its tip the
    /// head, see [`Self::advance_block_on`].
    ///
    /// The first block is built on `parent` and each further block on the previous one, so this
    /// reorgs the chain if `parent` is not the head. Returns the built payloads, does nothing for
    /// a length of 0.
    pub async fn advance_fork(
        &mut self,
        parent: B256,
        length: u64,
    ) -> eyre::Result<Vec<Payload::BuiltPayload>> {
        let mut chain = Vec::with_capacity(length as usize);
        let mut parent = parent;
        for _ in 0..length {
            let payload = self.advance_block_on(parent).await?;
            parent = payload.block().hash();
            chain.push(payload);
        }
        Ok(chain)
    }

    /// Advances the chain one block at a time until the transaction with the given hash is
    /// included in a canonical block, returning its receipt.
    ///
    /// Returns the receipt without advancing if the transaction is already included. Returns an
    /// error if the transaction is not included within [`WAIT_TIMEOUT`].
    pub async fn advance_until_receipt(
        &mut self,
        hash: B256,
    ) -> eyre::Result<RpcReceipt<<AddOns::EthApi as EthApiTypes>::NetworkTypes>>
    where
        AddOns::EthApi: EthApiSpec<Provider: BlockReader<Block = BlockTy<Node::Types>>>
            + EthTransactions
            + TraceExt
            + LoadReceipt
            + 'static,
    {
        let wait = async {
            loop {
                if let Some(receipt) = self.rpc.transaction_receipt(hash).await? {
                    return Ok(receipt)
                }
                self.advance_block().await?;
            }
        };
        tokio::time::timeout(WAIT_TIMEOUT, wait)
            .await
            .map_err(|_| eyre!("timed out waiting for the receipt of transaction {hash}"))?
    }

    /// Advances the chain one block at a time until the transaction pool has no pending
    /// transactions left, returning the built payloads.
    ///
    /// Pending transactions are the ones the pool considers ready for the next block, which the
    /// payload builder picks from. This does not wait for the other transactions of the pool,
    /// which stay in it: queued transactions, e.g. behind a nonce gap, and transactions whose fee
    /// cap is below the base fee or blob fee of the next block. Transactions that become pending
    /// once a built block is processed, e.g. because it lowered the base fee, are included in the
    /// following blocks. The pool decides when to stop, not the content of the blocks, since
    /// payload builders can add transactions of their own that are not pool transactions.
    ///
    /// The pool processes new blocks in the background, so this waits until it processed the
    /// current head before it looks at the pool, see [`Self::wait_for_pool_head`], and advances
    /// with [`Self::advance_block_synced`]. No block is built once the pool has no pending
    /// transactions: the last payload is the block after which none were left, not an extra empty
    /// block, and no payload is returned if the pool has none to begin with.
    ///
    /// Returns an error if the pool does not process the current head within [`WAIT_TIMEOUT`],
    /// e.g. because it was synced by backfill, or, listing them, if pending transactions are left
    /// after [`WAIT_TIMEOUT`], e.g. because the payload builder skips them.
    pub async fn advance_until_pool_drained(&mut self) -> eyre::Result<Vec<Payload::BuiltPayload>> {
        let head = self
            .inner
            .provider
            .sealed_header_by_number_or_tag(BlockNumberOrTag::Latest)?
            .ok_or_else(|| eyre!("latest block not found"))?;
        self.wait_for_pool_head(head.hash()).await?;

        let wait = async {
            let mut chain = Vec::new();
            while self.inner.pool.pool_size().pending > 0 {
                chain.push(self.advance_block_synced().await?);
            }
            Ok(chain)
        };
        tokio::time::timeout(WAIT_TIMEOUT, wait).await.map_err(|_| {
            eyre!(
                "timed out advancing the chain until the transaction pool has no pending \
                 transactions, {}",
                describe_pending_transactions(&self.inner.pool)
            )
        })?
    }

    /// Drives `fut` to completion while advancing the chain one block every [`POLL_INTERVAL`],
    /// returning its output.
    ///
    /// This is useful for futures that only complete once their transaction is mined, e.g. an
    /// `eth_sendRawTransactionSync` request. `fut` is also polled while a block is built, but the
    /// block is always built to completion, so the chain may advance one more block after `fut`
    /// completed.
    ///
    /// Returns an error if `fut` does not complete within [`WAIT_TIMEOUT`].
    pub async fn advance_while<F: Future>(&mut self, fut: F) -> eyre::Result<F::Output> {
        let wait = async {
            let mut fut = pin!(fut);
            loop {
                if let Result::Ok(output) = tokio::time::timeout(POLL_INTERVAL, fut.as_mut()).await
                {
                    return Ok(output)
                }
                // The engine can still import a cancelled block and make it canonical after this
                // returns, so finish it even if `fut` completes first.
                let mut advance = pin!(self.advance_block());
                match select(advance.as_mut(), fut.as_mut()).await {
                    Either::Left((payload, _)) => {
                        payload?;
                    }
                    Either::Right((output, _)) => {
                        advance.await?;
                        return Ok(output)
                    }
                }
            }
        };
        tokio::time::timeout(WAIT_TIMEOUT, wait)
            .await
            .map_err(|_| eyre!("timed out advancing the chain until the future completed"))?
    }

    /// Injects the raw transactions into the pool in the given order, advances the node one block
    /// and returns the mined block with the receipts of the transactions.
    ///
    /// The block must include all given transactions, in any order, and no other transaction of the
    /// pool, so a transaction that is dropped or that rides along fails instead of going unnoticed.
    /// An empty `txs` mines a block without pool transactions. Transactions that the payload
    /// builder adds by itself, e.g. from the payload attributes, are not pool transactions and may
    /// be included. Use [`Self::mine_including`] if other pool transactions may be included too.
    ///
    /// The receipts are in the order of `txs` and can report reverted transactions, see
    /// [`MinedBlock::ensure_success`]. The committed chain segment is taken from
    /// [`Self::canonical_stream`], whose notifications up to the one of the mined block are
    /// consumed, so the stream does not need to be aligned with the mined blocks before.
    ///
    /// The block is imported with [`Self::import_payload`], so the transaction pool processes it in
    /// the background, see [`Self::advance_block_synced`].
    ///
    /// Returns an error if the pool rejects a transaction, or, without importing the block, if the
    /// block does not include the expected transactions, listing the missing and the unexpected
    /// ones.
    ///
    /// Use [`Self::mine_pooled`] for transactions that are already in the pool, e.g. sent through
    /// [`Self::rpc_provider_with_wallet`] or added with the pool API, and [`Self::mine_signed`]
    /// for signed transactions that should all succeed.
    pub async fn mine(
        &mut self,
        txs: impl IntoIterator<Item = Bytes>,
    ) -> eyre::Result<
        MinedBlock<
            Payload::BuiltPayload,
            RpcReceipt<<AddOns::EthApi as EthApiTypes>::NetworkTypes>,
        >,
    >
    where
        // Implied by `NodeTypes`, but not derived by the compiler through the `Payload` parameter.
        Payload::BuiltPayload: BuiltPayload<Primitives = PrimitivesTy<Node::Types>>,
        AddOns::EthApi: EthApiSpec<Provider: BlockReader<Block = BlockTy<Node::Types>>>
            + EthTransactions
            + TraceExt
            + LoadReceipt
            + 'static,
    {
        self.mine_txs(txs, true).await
    }

    /// Mines the raw transactions like [`Self::mine`], but the block may also include other
    /// transactions of the pool, e.g. transactions that peers gossiped to the node.
    ///
    /// Only the receipts of the given transactions are returned.
    pub async fn mine_including(
        &mut self,
        txs: impl IntoIterator<Item = Bytes>,
    ) -> eyre::Result<
        MinedBlock<
            Payload::BuiltPayload,
            RpcReceipt<<AddOns::EthApi as EthApiTypes>::NetworkTypes>,
        >,
    >
    where
        Payload::BuiltPayload: BuiltPayload<Primitives = PrimitivesTy<Node::Types>>,
        AddOns::EthApi: EthApiSpec<Provider: BlockReader<Block = BlockTy<Node::Types>>>
            + EthTransactions
            + TraceExt
            + LoadReceipt
            + 'static,
    {
        self.mine_txs(txs, false).await
    }

    /// Mines the signed transactions like [`Self::mine`] and returns an error if one of them
    /// reverted, see [`MinedBlock::ensure_success`].
    ///
    /// The transactions are EIP-2718 encoded before they are sent to the pool, so signed
    /// transactions or envelopes, e.g. of a custom transaction type, can be passed without
    /// encoding them first.
    pub async fn mine_signed<T: Encodable2718>(
        &mut self,
        txs: impl IntoIterator<Item = T>,
    ) -> eyre::Result<
        MinedBlock<
            Payload::BuiltPayload,
            RpcReceipt<<AddOns::EthApi as EthApiTypes>::NetworkTypes>,
        >,
    >
    where
        Payload::BuiltPayload: BuiltPayload<Primitives = PrimitivesTy<Node::Types>>,
        AddOns::EthApi: EthApiSpec<Provider: BlockReader<Block = BlockTy<Node::Types>>>
            + EthTransactions
            + TraceExt
            + LoadReceipt
            + 'static,
    {
        // Encoded up front, so the returned future does not hold `T` across await points.
        let txs: Vec<Bytes> = txs.into_iter().map(|tx| tx.encoded_2718().into()).collect();
        self.mine(txs).await?.ensure_success()
    }

    /// Mines the transactions with the given hashes like [`Self::mine`], for transactions that are
    /// already in the pool, e.g. sent through [`Self::rpc_provider_with_wallet`] or added with the
    /// pool API.
    ///
    /// The receipts are in the order of `hashes`.
    ///
    /// Before building a block, returns an error naming the first hash that is not in the pool,
    /// e.g. of a transaction that was already mined or replaced. Transactions in any sub-pool pass
    /// this check, not only pending ones, since the pool sorts transactions for its own view of the
    /// next block, which can lag behind the block that is built. A transaction the block can not
    /// include, e.g. one queued behind a nonce gap, is reported as missing like with
    /// [`Self::mine`].
    pub async fn mine_pooled(
        &mut self,
        hashes: impl IntoIterator<Item = B256>,
    ) -> eyre::Result<
        MinedBlock<
            Payload::BuiltPayload,
            RpcReceipt<<AddOns::EthApi as EthApiTypes>::NetworkTypes>,
        >,
    >
    where
        Payload::BuiltPayload: BuiltPayload<Primitives = PrimitivesTy<Node::Types>>,
        AddOns::EthApi: EthApiSpec<Provider: BlockReader<Block = BlockTy<Node::Types>>>
            + EthTransactions
            + TraceExt
            + LoadReceipt
            + 'static,
    {
        self.mine_pooled_txs(hashes.into_iter().collect(), true).await
    }

    /// Mines the raw transactions, see [`Self::mine`]. Unless `exclusive` is set, the block may
    /// include other pool transactions.
    async fn mine_txs(
        &mut self,
        txs: impl IntoIterator<Item = Bytes>,
        exclusive: bool,
    ) -> eyre::Result<
        MinedBlock<
            Payload::BuiltPayload,
            RpcReceipt<<AddOns::EthApi as EthApiTypes>::NetworkTypes>,
        >,
    >
    where
        Payload::BuiltPayload: BuiltPayload<Primitives = PrimitivesTy<Node::Types>>,
        AddOns::EthApi: EthApiSpec<Provider: BlockReader<Block = BlockTy<Node::Types>>>
            + EthTransactions
            + TraceExt
            + LoadReceipt
            + 'static,
    {
        let mut hashes = Vec::new();
        for (index, raw_tx) in txs.into_iter().enumerate() {
            let hash = self
                .rpc
                .inject_tx(raw_tx)
                .await
                .wrap_err_with(|| format!("failed to inject transaction {index}"))?;
            hashes.push(hash);
        }

        self.mine_pooled_txs(hashes, exclusive).await
    }

    /// Mines the pool transactions with the given hashes, see [`Self::mine_pooled`]. Unless
    /// `exclusive` is set, the block may include other pool transactions.
    async fn mine_pooled_txs(
        &mut self,
        hashes: Vec<B256>,
        exclusive: bool,
    ) -> eyre::Result<
        MinedBlock<
            Payload::BuiltPayload,
            RpcReceipt<<AddOns::EthApi as EthApiTypes>::NetworkTypes>,
        >,
    >
    where
        Payload::BuiltPayload: BuiltPayload<Primitives = PrimitivesTy<Node::Types>>,
        AddOns::EthApi: EthApiSpec<Provider: BlockReader<Block = BlockTy<Node::Types>>>
            + EthTransactions
            + TraceExt
            + LoadReceipt
            + 'static,
    {
        for hash in &hashes {
            ensure!(self.inner.pool.contains(hash), "transaction {hash} is not in the pool");
        }

        let payload = self.new_payload().await?;
        let (number, block_hash) = (payload.block().number(), payload.block().hash());
        let body = payload.block().body();
        let missing: Vec<_> =
            hashes.iter().filter(|hash| !body.contains_transaction(hash)).collect();
        // The pool only removes the transactions of a block once it is canonical, so all pool
        // transactions of the block are still in the pool before the import.
        let unexpected: Vec<_> = body
            .transaction_hashes_iter()
            .filter(|hash| exclusive && !hashes.contains(hash) && self.inner.pool.contains(hash))
            .collect();
        ensure!(
            missing.is_empty() && unexpected.is_empty(),
            "block {number} does not include the expected transactions: missing {missing:?}, \
             unexpected pool transactions {unexpected:?}"
        );

        self.import_payload(payload.clone()).await?;
        let chain =
            wait_for_committed_chain(&mut self.canonical_stream, block_hash, number).await?;

        let mut receipts = Vec::with_capacity(hashes.len());
        for hash in hashes {
            let receipt = self.rpc.transaction_receipt(hash).await?.ok_or_else(|| {
                eyre!("receipt of transaction {hash} in block {number} not found")
            })?;
            receipts.push(receipt);
        }

        Ok(MinedBlock { payload, receipts, chain })
    }

    /// Waits for block to be available on node.
    ///
    /// Returns an error if the block is not available within [`WAIT_TIMEOUT`].
    pub async fn wait_block(
        &self,
        number: BlockNumber,
        expected_block_hash: BlockHash,
        wait_finish_checkpoint: bool,
    ) -> eyre::Result<()> {
        let provider = &self.inner.provider;
        poll_until(format!("block {number}"), move || async move {
            if wait_finish_checkpoint &&
                provider
                    .get_stage_checkpoint(StageId::Finish)?
                    .is_none_or(|checkpoint| checkpoint.block_number < number)
            {
                return Ok(None)
            }
            let Some(header) = provider.header_by_number(number)? else {
                ensure!(
                    !wait_finish_checkpoint,
                    "Finish checkpoint matches, but could not fetch block {number}"
                );
                return Ok(None)
            };
            let hash = header.hash_slow();
            ensure!(
                hash == expected_block_hash,
                "block {number} is {hash}, expected {expected_block_hash}"
            );
            Ok(Some(()))
        })
        .await
    }

    /// Waits for the node to unwind to the given block number.
    ///
    /// Returns an error if the node does not unwind within [`WAIT_TIMEOUT`].
    pub async fn wait_unwind(&self, number: BlockNumber) -> eyre::Result<()> {
        let provider = &self.inner.provider;
        poll_until(format!("unwind to block {number}"), move || async move {
            let checkpoint = provider.get_stage_checkpoint(StageId::Headers)?;
            Ok(checkpoint.is_some_and(|checkpoint| checkpoint.block_number == number).then_some(()))
        })
        .await
    }

    /// Waits until `condition` holds for the transaction pool of the node.
    ///
    /// Returns an error if the condition does not hold within [`WAIT_TIMEOUT`].
    pub async fn wait_for_pool(
        &self,
        mut condition: impl FnMut(&Node::Pool) -> bool,
    ) -> eyre::Result<()> {
        let pool = &self.inner.pool;
        poll_until("transaction pool condition", move || {
            let ready = condition(pool);
            async move { Ok(ready.then_some(())) }
        })
        .await
    }

    /// Waits until the transaction pool of the node processed the canonical state update that made
    /// the block with the given hash its head.
    ///
    /// The pool maintenance task processes new heads in the background, so right after e.g. a
    /// forkchoice update the pool can still contain the transactions mined in the new block or use
    /// outdated sender nonces and pending fees. The pool updates its last seen block together with
    /// these, so they are up to date once this returns. Exceptions are reorgs, after which the
    /// maintenance task re-injects the transactions of the old chain only afterwards, so wait for
    /// them with [`Self::wait_for_pooled`], and commits deeper than the maximum update depth of the
    /// maintenance task, e.g. after a long sync, which only update the last seen block.
    ///
    /// This only covers the canonical state update of the pool itself. Nodes can run additional
    /// pool maintenance tasks on new blocks, e.g. to remove mined transactions from a separate
    /// sub-pool or to evict transactions by their own rules, and the pool can report the new head
    /// before these processed the block. Tests of such nodes have to wait for the expected
    /// contents of the pool instead, with [`Self::wait_for_pool_removal`],
    /// [`Self::wait_for_pooled`] or [`Self::wait_for_pool`].
    ///
    /// Returns an error if the pool does not process the block within [`WAIT_TIMEOUT`], e.g.
    /// because the block is not the canonical head, the pool already processed a newer head, or
    /// the block was synced by backfill, which does not notify the pool.
    pub async fn wait_for_pool_head(&self, hash: B256) -> eyre::Result<()> {
        let pool = &self.inner.pool;
        poll_until(format!("transaction pool to process block {hash}"), move || {
            let ready = pool.block_info().last_seen_block_hash == hash;
            async move { Ok(ready.then_some(())) }
        })
        .await
    }

    /// Waits until all transactions with the given hashes are in the transaction pool of the
    /// node.
    ///
    /// The transactions must be in the pool at the same time, so this keeps waiting if one of them
    /// leaves the pool again, e.g. because it is mined or replaced, before the others entered.
    ///
    /// Returns an error listing the transactions that are still missing if they are not all in
    /// the pool within [`WAIT_TIMEOUT`].
    pub async fn wait_for_pooled(
        &self,
        hashes: impl IntoIterator<Item = B256>,
    ) -> eyre::Result<()> {
        wait_for_pool_transactions(&self.inner.pool, hashes, true).await
    }

    /// Waits until none of the transactions with the given hashes is in the transaction pool of
    /// the node, regardless of why they left, e.g. because they were mined, replaced or evicted.
    ///
    /// Unlike [`Self::wait_for_pool_head`], this does not depend on which task removes the
    /// transactions, so it also covers nodes with additional pool maintenance tasks. It returns
    /// right away for transactions that never entered the pool, so wait for transactions that
    /// arrive in the background, e.g. from peers, with [`Self::wait_for_pooled`] first.
    ///
    /// Returns an error listing the transactions that are still pooled if they do not all leave
    /// the pool within [`WAIT_TIMEOUT`].
    pub async fn wait_for_pool_removal(
        &self,
        hashes: impl IntoIterator<Item = B256>,
    ) -> eyre::Result<()> {
        wait_for_pool_transactions(&self.inner.pool, hashes, false).await
    }

    /// Waits until the node has persisted at least the block with the given number to disk.
    ///
    /// The engine keeps the most recent blocks in memory, so tests that inspect the database
    /// directly, e.g. via [`assert_trie_consistency`], must first advance the chain far enough
    /// and wait for the persistence service to catch up. This checks the `Finish` stage checkpoint
    /// of the database, which is committed together with the saved blocks, so all blocks up to
    /// `number` are readable from disk once this returns. Their state and trie are committed in
    /// the same transaction, unless the engine keeps the state of the most recently persisted
    /// blocks masked by its in-memory suffix (`TreeConfig::num_state_masking_blocks`, disabled by a
    /// persistence threshold of 0), in which case the persisted state can lag behind `number`. The
    /// pruner runs after the save in a separate commit, so pruning of the saved blocks can still be
    /// pending, wait for it with [`Self::wait_for_prune_checkpoint`].
    ///
    /// Unlike [`Self::wait_block`], this does not check the block hash, so it also returns if the
    /// persisted block at `number` is not canonical anymore.
    ///
    /// Returns an error if the block is not persisted within [`WAIT_TIMEOUT`].
    ///
    /// [`assert_trie_consistency`]: crate::trie::assert_trie_consistency
    pub async fn wait_for_persisted_block(&self, number: BlockNumber) -> eyre::Result<()> {
        let provider = &self.inner.provider;
        poll_until(format!("block {number} to be persisted"), move || async move {
            let persisted = provider.database_provider_ro()?.best_block_number()?;
            Ok((persisted >= number).then_some(()))
        })
        .await
    }

    /// Waits until the pruner has pruned `segment` of the node up to at least the block with the
    /// given number.
    ///
    /// The persistence service acknowledges a save before it runs the pruner for the new database
    /// tip in a separate commit, so a block being persisted, e.g. awaited with
    /// [`Self::wait_for_persisted_block`], does not mean the pruner has run for it yet. The
    /// pruner saves the checkpoint of a segment in the same provider commit as the pruned data,
    /// which commits static files and `RocksDB` before the database transaction, so the data of
    /// `segment` up to `block` is pruned once this returns.
    ///
    /// Returns an error if the segment is not pruned within [`WAIT_TIMEOUT`], e.g. because the
    /// node is not configured to prune it or `block` is within its retention window.
    pub async fn wait_for_prune_checkpoint(
        &self,
        segment: PruneSegment,
        block: BlockNumber,
    ) -> eyre::Result<()> {
        let provider = &self.inner.provider;
        poll_until(format!("{segment} to be pruned up to block {block}"), move || async move {
            let checkpoint = provider.get_prune_checkpoint(segment)?;
            Ok(checkpoint
                .and_then(|checkpoint| checkpoint.block_number)
                .is_some_and(|pruned| pruned >= block)
                .then_some(()))
        })
        .await
    }

    /// Waits until the block with the given hash is the latest block of the node, i.e. its
    /// canonical head.
    ///
    /// This only observes the node and sends nothing to the engine, so the block has to become the
    /// head otherwise, e.g. through a forkchoice update the test sent itself, which may start a
    /// backfill sync to the block. [`Self::sync_to`] instead keeps sending forkchoice updates that
    /// make the block the head, safe and finalized block until it is the head, and then waits
    /// briefly for the transaction pool, which this does not, see [`Self::wait_for_pool_head`].
    /// [`Self::wait_block`] waits for the header at a block number, which a canonical block that is
    /// not the head also satisfies, and after a backfill sync it can return before the engine made
    /// the synced block its head, even when it waits for the `Finish` checkpoint of the pipeline.
    ///
    /// Returns an error naming the head of the node if the block is not the head within
    /// [`WAIT_TIMEOUT`], e.g. because the node made another block its head.
    pub async fn wait_for_head(&self, hash: BlockHash) -> eyre::Result<()> {
        let provider = &self.inner.provider;
        let latest = || provider.sealed_header_by_number_or_tag(BlockNumberOrTag::Latest);
        let wait = async {
            while latest()?.is_none_or(|head| head.hash() != hash) {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            Ok(())
        };
        tokio::time::timeout(WAIT_TIMEOUT, wait).await.map_err(|_| {
            let head = latest().ok().flatten().map_or_else(
                || "unknown".to_string(),
                |head| format!("block {} ({})", head.number(), head.hash()),
            );
            eyre!("timed out waiting for block {hash} to become the head, the head is {head}")
        })?
    }

    /// Asserts that a new block has been added to the blockchain and the tx has been included in
    /// the block, at any position.
    ///
    /// Does NOT work for pipeline since there's no stream notification! Returns an error if the
    /// block is not committed within [`WAIT_TIMEOUT`].
    pub async fn assert_new_block(
        &mut self,
        tip_tx_hash: B256,
        block_hash: B256,
        block_number: BlockNumber,
    ) -> eyre::Result<()> {
        let chain =
            wait_for_committed_chain(&mut self.canonical_stream, block_hash, block_number).await?;
        ensure!(
            chain
                .blocks()
                .get(&block_number)
                .is_some_and(|block| block.body().contains_transaction(&tip_tx_hash)),
            "transaction {tip_tx_hash} is not included in block {block_number}"
        );

        // Wait for the block to commit and make sure the block hash we submitted via FCU engine
        // api is the new latest block.
        let provider = &self.inner.provider;
        poll_until(format!("block {block_number} to be committed"), move || async move {
            let Some(latest) = provider.block_by_number_or_tag(BlockNumberOrTag::Latest)? else {
                return Ok(None)
            };
            if latest.header().number() != block_number {
                return Ok(None)
            }
            let hash = latest.header().hash_slow();
            ensure!(
                hash == block_hash,
                "latest block {block_number} is {hash}, expected {block_hash}"
            );
            Ok(Some(()))
        })
        .await
    }

    /// Gets block hash by number.
    pub fn block_hash(&self, number: u64) -> BlockHash {
        self.inner
            .provider
            .sealed_header_by_number_or_tag(BlockNumberOrTag::Number(number))
            .unwrap()
            .unwrap()
            .hash()
    }

    /// Sends FCU and waits for the node to sync to the given block.
    ///
    /// The forkchoice update makes the block the head, safe and finalized block, regardless of the
    /// [`Finality`] policy of the context. Returns an error if the node does not sync within
    /// [`WAIT_TIMEOUT`].
    pub async fn sync_to(&self, block: BlockHash) -> eyre::Result<()> {
        self.sync_to_forkchoice(ForkchoiceState::same_hash(block)).await
    }

    /// Sends the forkchoice update `state` and waits for the node to sync to its head, like
    /// [`Self::sync_to`], but with the safe and finalized blocks of `state`.
    ///
    /// A finalized block below the head, e.g. one the node does not know yet, lets a backfill
    /// anchor to it, as a snap sync anchors its pivot to the finalized block. Returns an error if
    /// the node does not sync within [`WAIT_TIMEOUT`].
    pub async fn sync_to_forkchoice(&self, state: ForkchoiceState) -> eyre::Result<()> {
        let block = state.head_block_hash;
        let sync = async {
            while self
                .inner
                .provider
                .sealed_header_by_id(BlockId::latest())?
                .is_none_or(|h| h.hash() != block)
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
                self.inner
                    .add_ons_handle
                    .beacon_engine_handle
                    .fork_choice_updated(state, None)
                    .await?;
            }
            Ok(())
        };
        tokio::time::timeout(WAIT_TIMEOUT, sync)
            .await
            .map_err(|_| eyre!("timed out syncing to block {block}"))??;

        // The transaction pool processes the canonical state update in the background, and
        // advancing the chain before it did can fail with e.g. "nonce too low" errors. Blocks
        // synced by backfill don't notify the pool, so wait for at most a second.
        let _ = tokio::time::timeout(Duration::from_secs(1), self.wait_for_pool_head(block)).await;

        Ok(())
    }

    /// Sends a forkchoice update message to the engine and returns its response.
    ///
    /// The update makes `new_head` the head and `current_head` the safe and finalized block,
    /// regardless of the [`Finality`] policy of the context.
    pub async fn update_forkchoice(
        &self,
        current_head: B256,
        new_head: B256,
    ) -> eyre::Result<ForkchoiceUpdated> {
        Ok(self
            .inner
            .add_ons_handle
            .beacon_engine_handle
            .fork_choice_updated(
                ForkchoiceState {
                    head_block_hash: new_head,
                    safe_block_hash: current_head,
                    finalized_block_hash: current_head,
                },
                None,
            )
            .await?)
    }

    /// Sends forkchoice update to the engine api with a zero finalized hash and returns its
    /// response.
    pub async fn update_optimistic_forkchoice(
        &self,
        hash: B256,
    ) -> eyre::Result<ForkchoiceUpdated> {
        self.update_forkchoice(B256::ZERO, hash).await
    }

    /// Submits a payload to the engine with `newPayload` and returns its block hash.
    ///
    /// This only inserts the block, it does not make it canonical, see [`Self::import_payload`].
    /// Returns the block hash if the engine reports the payload:
    /// - `VALID`: the engine executed the block on top of its parent, or knew it as valid already.
    /// - `SYNCING` or `ACCEPTED`: the engine did not execute the block, e.g. because its parent is
    ///   unknown, so the block is not known to be valid yet, see
    ///   [`Self::submit_payload_with_status`].
    ///
    /// Returns an error with the validation error and the latest valid hash if the engine reports
    /// the payload `INVALID`. Use [`Self::submit_payload_with_status`] to inspect the status
    /// instead, e.g. to test that a payload is rejected.
    pub async fn submit_payload(&self, payload: Payload::BuiltPayload) -> eyre::Result<B256> {
        let block_hash = payload.block().hash();
        let status = self.submit_payload_with_status(payload).await?;
        if let PayloadStatusEnum::Invalid { validation_error } = status.status {
            let latest_valid_hash = status
                .latest_valid_hash
                .map_or_else(|| "none".to_string(), |hash| hash.to_string());
            bail!(
                "payload {block_hash} is invalid (latest valid hash: {latest_valid_hash}): \
                 {validation_error}"
            )
        }

        Ok(block_hash)
    }

    /// Submits a payload to the engine with `newPayload` and returns the payload status the engine
    /// reports.
    ///
    /// Unlike [`Self::submit_payload`], this returns `INVALID` statuses instead of an error. This
    /// only inserts the block, it does not make it canonical. The engine reports:
    /// - `VALID` if it executed the block on top of its parent, or knew it as valid already. The
    ///   latest valid hash is the block hash.
    /// - `INVALID` if the block failed validation, or descends from a block that did. The latest
    ///   valid hash is the hash of the most recent valid ancestor, or `None` if the engine can't
    ///   determine it, e.g. because the block hash does not match the payload.
    /// - `SYNCING` if it did not execute the block because the parent of the block is unknown, in
    ///   which case it buffers the block and executes it once the parent is inserted, or because
    ///   the node is syncing.
    /// - `ACCEPTED` if it accepted the block without executing it. The reth engine tree does not
    ///   report this status.
    ///
    /// Returns an error if the engine fails to process the payload, e.g. because it shut down.
    pub async fn submit_payload_with_status(
        &self,
        payload: Payload::BuiltPayload,
    ) -> eyre::Result<PayloadStatus> {
        Ok(self.inner.add_ons_handle.beacon_engine_handle.new_payload(payload.into()).await?)
    }

    /// Sets which blocks [`Self::import_payload`] and the block producing helpers built on it, such
    /// as [`Self::advance_block`], mark as safe and finalized, see [`Finality`].
    ///
    /// The policy applies from the next imported block on, the current safe and finalized blocks
    /// are not changed. Defaults to [`Finality::Head`].
    pub const fn set_finality(&mut self, finality: Finality) {
        self.finality = finality;
    }

    /// Submits a payload to the engine and makes its block the canonical head, returning the block
    /// hash.
    ///
    /// The forkchoice update marks the safe and finalized blocks according to the [`Finality`]
    /// policy of the context, see [`Self::set_finality`]. Under the default [`Finality::Head`],
    /// the block also becomes the safe and finalized block, so only a forkchoice update that moves
    /// the finalized block to another chain can reorg it. The engine only reports the forkchoice
    /// update valid after making the head canonical, so the block is the latest block of the node
    /// once this returns. The transaction pool processes the new block in the background, see
    /// [`Self::wait_for_pool`].
    ///
    /// The parent of the payload must be known to the node, e.g. to import a payload built by
    /// another node into its peers. Returns an error if the engine reports the payload invalid, see
    /// [`Self::submit_payload`], or does not report the forkchoice update valid, e.g. because the
    /// parent is unknown, or because the block does not descend from the safe and finalized blocks
    /// that [`Finality::Keep`] and [`Finality::Lag`] keep.
    pub async fn import_payload(&self, payload: Payload::BuiltPayload) -> eyre::Result<B256> {
        let block_number = payload.block().number();
        let block_hash = self.submit_payload(payload).await?;
        let state = match self.finality {
            Finality::Head => ForkchoiceState::same_hash(block_hash),
            Finality::Keep | Finality::Lag(_) => {
                ForkchoiceState { head_block_hash: block_hash, ..self.current_forkchoice_state()? }
            }
        };
        self.send_forkchoice_updated(state, None).await?;

        // Look up the block at the lagging height only now that the block is canonical, so it is an
        // ancestor of the block even if the block reorged the chain.
        if let Finality::Lag(lag) = self.finality &&
            let Some(number) = block_number.checked_sub(lag) &&
            self.inner
                .provider
                .finalized_block_number()?
                .is_none_or(|finalized| number > finalized) &&
            let Some(finalized) = self.inner.provider.block_hash(number)?
        {
            self.send_forkchoice_updated(
                ForkchoiceState {
                    head_block_hash: block_hash,
                    safe_block_hash: finalized,
                    finalized_block_hash: finalized,
                },
                None,
            )
            .await?;
        }

        Ok(block_hash)
    }

    /// Makes the known block with the given hash the head of the node, keeping its safe and
    /// finalized blocks, and returns once the block is the latest block.
    ///
    /// The engine makes a block on a side chain canonical, which reorgs the chain, e.g. back to a
    /// chain the node left with [`Self::advance_fork`]. This requires the block to descend from
    /// the finalized block, so blocks imported under [`Finality::Head`] can't be reorged away
    /// from, import them under [`Finality::Keep`] or [`Finality::Lag`], see
    /// [`Self::set_finality`]. The engine does not move the head back to a canonical ancestor, so
    /// this returns an error for those, build a block on the ancestor with
    /// [`Self::advance_block_on`] instead.
    ///
    /// The transaction pool processes the reorg in the background: once
    /// [`Self::wait_for_pool_head`] returns for the block, the pool has removed the transactions
    /// mined in the new chain and updated sender nonces and fees to it, while the maintenance task
    /// re-injects the transactions of the reorged blocks only afterwards, wait for them with
    /// [`Self::wait_for_pool`].
    ///
    /// Returns an error if the node does not know the block or one of its ancestors, if the block
    /// is invalid, or if it does not descend from the safe and finalized blocks.
    pub async fn reorg_to(&self, hash: B256) -> eyre::Result<()> {
        let state = ForkchoiceState { head_block_hash: hash, ..self.current_forkchoice_state()? };
        self.send_forkchoice_updated(state, None).await?;

        let latest = self
            .inner
            .provider
            .sealed_header_by_number_or_tag(BlockNumberOrTag::Latest)?
            .ok_or_else(|| eyre!("latest block not found"))?;
        ensure!(
            latest.hash() == hash,
            "block {hash} is a canonical ancestor of the head {}, which the engine does not move \
             the head back to, build a block on it with `advance_block_on` instead",
            latest.hash()
        );

        Ok(())
    }

    /// Sends the forkchoice state to the engine and returns its response, or an error that
    /// explains why the engine did not report it valid.
    async fn send_forkchoice_updated(
        &self,
        state: ForkchoiceState,
        attributes: Option<Payload::PayloadAttributes>,
    ) -> eyre::Result<ForkchoiceUpdated> {
        let head = state.head_block_hash;
        let updated = self
            .inner
            .add_ons_handle
            .beacon_engine_handle
            .fork_choice_updated(state, attributes)
            .await
            .map_err(|err| self.explain_forkchoice_error(err, head))?;
        ensure!(
            !updated.is_syncing(),
            "the node does not know block {head} or one of its ancestors, or is syncing, submit \
             the blocks first, e.g. with `submit_payload`"
        );
        ensure!(
            updated.is_valid(),
            "forkchoice update to head {head}, safe {}, finalized {} is not valid: {}",
            state.safe_block_hash,
            state.finalized_block_hash,
            updated.payload_status.status
        );
        Ok(updated)
    }

    /// Converts an error of a forkchoice update to `head` into a report, explaining how to avoid it
    /// if the engine rejected the update because `head` does not descend from the finalized block.
    fn explain_forkchoice_error(
        &self,
        err: BeaconForkChoiceUpdateError,
        head: B256,
    ) -> eyre::Report {
        if !matches!(
            err,
            BeaconForkChoiceUpdateError::ForkchoiceUpdateError(
                ForkchoiceUpdateError::TooDeepReorg | ForkchoiceUpdateError::InvalidState
            )
        ) {
            return err.into()
        }
        let finalized = match self.inner.provider.finalized_block_num_hash() {
            Result::Ok(Some(finalized)) => format!("{} ({})", finalized.number, finalized.hash),
            _ => "unknown".to_string(),
        };
        eyre::Report::new(err).wrap_err(format!(
            "the node can't reorg to block {head}: it does not descend from the safe and finalized \
             blocks of the node (finalized: {finalized}). Only blocks above the finalized block \
             can be reorged, import them under `Finality::Keep` or `Finality::Lag`, see \
             `NodeTestContext::set_finality`"
        ))
    }

    /// Returns the RPC URL.
    pub fn rpc_url(&self) -> Url {
        let addr = self.inner.rpc_server_handle().http_local_addr().unwrap();
        format!("http://{addr}").parse().unwrap()
    }

    /// Returns an RPC client.
    pub fn rpc_client(&self) -> Option<HttpClient> {
        self.inner.rpc_server_handle().http_client()
    }

    /// Returns an alloy provider with the recommended fillers connected to the HTTP RPC server.
    ///
    /// # Panics
    ///
    /// If the HTTP RPC server is disabled.
    pub fn rpc_provider(
        &self,
    ) -> FillProvider<impl TxFiller<Ethereum> + use<Node, Payload, AddOns>, RootProvider> {
        self.rpc_provider_for::<Ethereum>()
    }

    /// Returns an alloy provider with the recommended fillers and the given wallet connected to
    /// the HTTP RPC server.
    ///
    /// Transactions sent via the provider are signed by the wallet, e.g. a
    /// [`PrivateKeySigner`](alloy_signer_local::PrivateKeySigner) of the test
    /// [`Wallet`](crate::wallet::Wallet).
    ///
    /// # Panics
    ///
    /// If the HTTP RPC server is disabled.
    pub fn rpc_provider_with_wallet<W>(
        &self,
        wallet: W,
    ) -> FillProvider<impl TxFiller<Ethereum> + use<W, Node, Payload, AddOns>, RootProvider>
    where
        W: IntoWallet<Ethereum, NetworkWallet: Clone>,
    {
        self.rpc_provider_with_wallet_for::<Ethereum, W>(wallet)
    }

    /// Returns an alloy provider for the network `Net` with its recommended fillers connected to
    /// the HTTP RPC server, polling every [`RPC_PROVIDER_POLL_INTERVAL`].
    ///
    /// This is [`Self::rpc_provider`] for nodes whose RPC types differ from Ethereum's.
    ///
    /// # Panics
    ///
    /// If the HTTP RPC server is disabled.
    pub fn rpc_provider_for<Net: RecommendedFillers>(
        &self,
    ) -> FillProvider<impl TxFiller<Net> + use<Net, Node, Payload, AddOns>, RootProvider<Net>, Net>
    {
        let provider = ProviderBuilder::new_with_network::<Net>().connect_http(self.rpc_url());
        provider.client().set_poll_interval(RPC_PROVIDER_POLL_INTERVAL);
        provider
    }

    /// Returns an alloy provider for the network `Net` with its recommended fillers and the given
    /// wallet connected to the HTTP RPC server, polling every [`RPC_PROVIDER_POLL_INTERVAL`].
    ///
    /// This is [`Self::rpc_provider_with_wallet`] for nodes whose RPC types differ from
    /// Ethereum's.
    ///
    /// # Panics
    ///
    /// If the HTTP RPC server is disabled.
    pub fn rpc_provider_with_wallet_for<Net, W>(
        &self,
        wallet: W,
    ) -> FillProvider<impl TxFiller<Net> + use<Net, W, Node, Payload, AddOns>, RootProvider<Net>, Net>
    where
        Net: RecommendedFillers,
        W: IntoWallet<Net, NetworkWallet: Clone>,
    {
        let provider =
            ProviderBuilder::new_with_network::<Net>().wallet(wallet).connect_http(self.rpc_url());
        provider.client().set_poll_interval(RPC_PROVIDER_POLL_INTERVAL);
        provider
    }

    /// Returns an Engine API client.
    pub fn auth_server_handle(&self) -> AuthServerHandle {
        self.inner.auth_server_handle().clone()
    }

    /// Creates a [`crate::testsuite::NodeClient`] from this test context.
    ///
    /// This helper method extracts the necessary handles and creates a client
    /// that can interact with both the regular RPC and Engine API endpoints.
    /// It automatically includes the beacon engine handle for direct consensus engine interaction
    /// and read-only access to the node's database.
    pub fn to_node_client(&self) -> eyre::Result<crate::testsuite::NodeClient<Payload>> {
        let rpc = self
            .rpc_client()
            .ok_or_else(|| eyre::eyre!("Failed to create HTTP RPC client for node"))?;
        let auth = self.auth_server_handle();
        let url = self.rpc_url();
        let beacon_handle = self.inner.add_ons_handle.beacon_engine_handle.clone();

        let mut client =
            crate::testsuite::NodeClient::new_with_beacon_engine(rpc, auth, url, beacon_handle);
        client.payload_builder = Some(self.inner.payload_builder_handle.clone());
        let provider = self.inner.provider.clone();
        client.database = Some(Arc::new(move || {
            provider.database_provider_ro().map(|db| Box::new(db) as Box<dyn BlockNumReader>)
        }));
        Ok(client)
    }

    /// Calls the `testing_buildBlockV1` RPC on this node.
    ///
    /// This endpoint builds a block using the provided parent, payload attributes, and
    /// transactions. Requires the `Testing` RPC module to be enabled.
    pub async fn testing_build_block_v1(
        &self,
        request: TestingBuildBlockRequestV1,
    ) -> eyre::Result<ExecutionPayloadEnvelopeV5> {
        let client =
            self.rpc_client().ok_or_else(|| eyre::eyre!("HTTP RPC client not available"))?;

        let res: ExecutionPayloadEnvelopeV5 =
            client.request("testing_buildBlockV1", request.into_params()).await?;
        eyre::Ok(res)
    }

    /// Stops the node and closes its database, keeping its datadir to launch the node again with
    /// [`StoppedNode::start`].
    ///
    /// The node shuts down like a node that receives `SIGTERM`: the engine persists every block of
    /// its canonical chain together with its state, and the tasks of the node exit. Blocks that
    /// are not canonical, e.g. payloads that were only submitted with [`Self::submit_payload`],
    /// are lost. Local transactions of the pool are saved to the datadir and reinserted when the
    /// node starts again.
    ///
    /// Returns once the node released its database, so the database, static files and `RocksDB`
    /// are closed and nothing writes to the datadir anymore, including the pruner that runs after
    /// the last save. Peers of the node notice the disconnect in the background.
    ///
    /// Only restartable nodes, see
    /// [`E2ETestSetupBuilder::with_restartable_nodes`](crate::E2ETestSetupBuilder::with_restartable_nodes),
    /// can be stopped. Returns an error for other nodes, if the node does not shut down within
    /// [`WAIT_TIMEOUT`], or if the test keeps the database open:
    /// - a handle of the node that holds its database, e.g. a clone of `inner.provider` or a
    ///   [`NodeClient`](crate::testsuite::NodeClient), makes this fail once the database is still
    ///   in use [`DATABASE_RELEASE_TIMEOUT`] after the node shut down,
    /// - an open database transaction, e.g. a provider of `database_provider_ro`, makes this fail
    ///   right away,
    /// - a handle of `RocksDB`, e.g. of `inner.provider.rocksdb_provider()`, makes this fail once
    ///   `RocksDB` is still open [`DATABASE_RELEASE_TIMEOUT`] after the node shut down.
    ///
    /// Drop such handles before stopping the node.
    pub async fn stop(mut self) -> eyre::Result<StoppedNode<Node, AddOns>> {
        let Some(NodeRestart { database, relaunch }) = self.restart.take() else {
            bail!(
                "the node is not restartable, launch it with \
                 `E2ETestSetupBuilder::with_restartable_nodes` to stop it"
            )
        };
        let data_dir = self.inner.data_dir.clone();
        let runtime = self.inner.task_executor.clone();
        // Stop the RPC servers even if the test kept handles to them, which would otherwise keep
        // the database open.
        let _ = self.inner.rpc_server_handle().clone().stop();
        let _ = self.inner.auth_server_handle().clone().stop();
        drop(self);

        // The consensus engine is a graceful task that persists the canonical chain before it
        // exits. Waiting for graceful tasks blocks the thread.
        let shut_down = tokio::task::spawn_blocking(move || {
            runtime.graceful_shutdown_with_timeout(WAIT_TIMEOUT)
        })
        .await?;
        ensure!(shut_down, "the node did not shut down within {WAIT_TIMEOUT:?}");

        close_database(database, &data_dir).await?;
        Ok(StoppedNode { data_dir, relaunch, remove_data_dir: true })
    }

    /// Stops the node with [`Self::stop`] and starts it again with [`StoppedNode::start`],
    /// returning its new test context.
    pub async fn restart(self) -> eyre::Result<Self> {
        self.stop().await?.start().await
    }

    /// Takes the future that resolves when the consensus engine of the node exits.
    ///
    /// It resolves with an error if the engine exits because of a fatal error, and with `Ok` once
    /// the node is stopped, so a test can use it to notice a node that died. Returns `None` if the
    /// future was already taken or the node was not launched by
    /// [`E2ETestSetupBuilder`](crate::E2ETestSetupBuilder).
    pub fn take_exit_future(&mut self) -> Option<NodeExitFuture> {
        self.exit_future.get_mut().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// Skips canonical state notifications until the one that commits the block with the given hash
/// and number, returning the chain segment it commits.
///
/// The stream buffers every canonical notification since it was created, e.g. of blocks mined with
/// [`NodeTestContext::advance_block`], so it does not need to be aligned with the block.
///
/// Returns an error if the block is not committed within [`WAIT_TIMEOUT`].
async fn wait_for_committed_chain<N: NodePrimitives>(
    stream: &mut CanonStateNotificationStream<N>,
    block_hash: B256,
    block_number: BlockNumber,
) -> eyre::Result<Arc<Chain<N>>> {
    let wait = async {
        loop {
            let notification =
                stream.next().await.ok_or_else(|| eyre!("canonical state stream closed"))?;
            let committed = notification.committed();
            if committed.blocks().get(&block_number).is_some_and(|block| block.hash() == block_hash)
            {
                return Ok(committed)
            }
        }
    };
    tokio::time::timeout(WAIT_TIMEOUT, wait)
        .await
        .map_err(|_| eyre!("timed out waiting for block {block_number}"))?
}

/// Waits until `database` is the last handle of the database of a stopped node, closes the
/// database without removing its datadir, and waits until `RocksDB` is closed.
///
/// Returns an error if the database or `RocksDB` is still open afterwards.
async fn close_database(database: TmpDB, data_dir: &ChainPath<DataDirPath>) -> eyre::Result<()> {
    let released = tokio::time::timeout(DATABASE_RELEASE_TIMEOUT, async {
        while Arc::strong_count(&database) > 1 {
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await;
    if released.is_err() {
        bail!(
            "the database of the stopped node is still in use {DATABASE_RELEASE_TIMEOUT:?} after \
             the node shut down: drop all handles that hold a provider of the node, e.g. a clone \
             of `inner.provider`, before stopping it"
        )
    }
    let database = Arc::into_inner(database)
        .ok_or_else(|| eyre!("the database of the stopped node is still in use"))?;
    // Unlike dropping the temporary database, this keeps the datadir.
    drop(database.into_inner_db());

    // A database transaction keeps the database open without a handle of it. MDBX does not open
    // a database twice in a process, so opening it fails until the transaction is dropped.
    open_db_read_only(data_dir.db(), DatabaseArguments::test()).wrap_err(
        "the database of the stopped node is still open: drop all database transactions of the \
         node, e.g. providers of `database_provider_ro`, before stopping it",
    )?;

    // Providers drop their database handle before their `RocksDB` handle, whose drop flushes and
    // closes `RocksDB`, so the last provider can still be closing it. `RocksDB` does not open a
    // database twice in a process either, so wait until opening it succeeds.
    let deadline = Instant::now() + DATABASE_RELEASE_TIMEOUT;
    loop {
        let path = data_dir.rocksdb();
        let opened = tokio::task::spawn_blocking(move || {
            RocksDBProvider::builder(path).with_default_tables().build().map(drop)
        })
        .await?;
        match opened {
            Result::Ok(()) => return Ok(()),
            Err(err) if Instant::now() >= deadline => {
                return Err(err).wrap_err(format!(
                    "the RocksDB of the stopped node is still open {DATABASE_RELEASE_TIMEOUT:?} \
                     after the node shut down: drop all handles of it, e.g. of \
                     `inner.provider.rocksdb_provider()`, before stopping the node"
                ))
            }
            Err(_) => tokio::time::sleep(POLL_INTERVAL).await,
        }
    }
}

/// Waits until all `hashes` are in `pool` if `pooled` is true, or none of them otherwise.
///
/// On timeout, the error lists the transactions that are still missing or still pooled.
async fn wait_for_pool_transactions<P: TransactionPool>(
    pool: &P,
    hashes: impl IntoIterator<Item = B256>,
    pooled: bool,
) -> eyre::Result<()> {
    let hashes = hashes.into_iter().collect::<Vec<_>>();
    let (what, pending_kind) = if pooled {
        ("transactions to enter the pool", "still missing")
    } else {
        ("transactions to leave the pool", "still pooled")
    };
    let mut pending = Vec::new();
    poll_until(what, || {
        pending = hashes.iter().copied().filter(|hash| pool.contains(hash) != pooled).collect();
        let done = pending.is_empty();
        async move { Ok(done.then_some(())) }
    })
    .await
    .map_err(|err| eyre!("{err}, {pending_kind}: {pending:?}"))
}

/// Describes the pending transactions of `pool` for an error message, listing at most ten.
fn describe_pending_transactions<P: TransactionPool>(pool: &P) -> String {
    const MAX_LISTED: usize = 10;

    let pending = pool.pending_transactions();
    let mut listed = pending
        .iter()
        .take(MAX_LISTED)
        .map(|tx| format!("{} (sender {}, nonce {})", tx.hash(), tx.sender(), tx.nonce()))
        .collect::<Vec<_>>();
    if pending.len() > MAX_LISTED {
        listed.push(format!("and {} more", pending.len() - MAX_LISTED));
    }
    format!(
        "the pool still has {} at block {}: {}",
        pending.len(),
        pool.block_info().last_seen_block_number,
        listed.join(", ")
    )
}

/// A block mined by [`NodeTestContext::mine`], [`NodeTestContext::mine_including`],
/// [`NodeTestContext::mine_signed`] or [`NodeTestContext::mine_pooled`].
#[derive(Debug)]
pub struct MinedBlock<Payload: BuiltPayload, Receipt> {
    /// The built payload of the block.
    pub payload: Payload,
    /// The RPC receipts of the mined transactions, in the order they were passed.
    pub receipts: Vec<Receipt>,
    /// The chain segment committed by the canonical state notification of the block.
    ///
    /// The block is built on the canonical head, so the segment consists of this block only and
    /// its execution outcome, e.g. the bundle state, is the outcome of the block.
    pub chain: Arc<Chain<Payload::Primitives>>,
}

impl<Payload: BuiltPayload, Receipt: ReceiptResponse> MinedBlock<Payload, Receipt> {
    /// Returns the mined block.
    pub fn block(&self) -> &SealedBlock<<Payload::Primitives as NodePrimitives>::Block> {
        self.payload.block()
    }

    /// Returns the mined block if none of the mined transactions reverted.
    ///
    /// Returns an error naming the first reverted transaction otherwise.
    pub fn ensure_success(self) -> eyre::Result<Self> {
        for receipt in &self.receipts {
            ensure_successful(receipt)?;
        }
        Ok(self)
    }
}

/// Which blocks [`NodeTestContext::import_payload`] marks as safe and finalized when it makes a
/// block the head.
///
/// The policy applies to [`NodeTestContext::import_payload`] and the block producing helpers built
/// on it, e.g. [`NodeTestContext::advance_block`], [`NodeTestContext::advance`] and
/// [`NodeTestContext::advance_while`]. [`NodeTestContext::update_forkchoice`] and
/// [`NodeTestContext::sync_to`] send the forkchoice state given by their arguments, and
/// [`NodeTestContext::new_payload`] keeps the safe and finalized blocks the node reports.
///
/// The engine accepts a forkchoice update only if its safe and finalized blocks are ancestors of
/// its head, and rejects moving the head back to a canonical block below the finalized block. So
/// while the finalized block stays, e.g. under [`Finality::Keep`], only blocks above it can be
/// reorged.
///
/// Finality does not hold back persistence: the engine persists canonical blocks and evicts them
/// from memory once more of them than the persistence threshold are in memory, and the pruner runs
/// on the persisted tip, so [`NodeTestContext::wait_for_persisted_block`] and
/// [`NodeTestContext::wait_for_prune_checkpoint`] behave the same under every policy. The engine
/// only evicts a side chain from memory once the finalized block passes its fork point, and the
/// transaction pool keeps the blob sidecars of mined transactions until their block is finalized,
/// so under [`Finality::Keep`] both remain available for reorgs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Finality {
    /// The imported block becomes the safe and finalized block.
    #[default]
    Head,
    /// The safe and finalized blocks stay what the node reports, e.g. genesis after the setup, so
    /// the imported blocks can be reorged.
    Keep,
    /// The safe and finalized blocks trail the imported block by the given number of blocks.
    ///
    /// After importing the block at height `h`, its ancestor at height `h - n` becomes the safe
    /// and finalized block if it is above the current finalized block. Otherwise they stay, so
    /// they never move backwards, e.g. after switching from [`Finality::Head`], and stay at
    /// genesis until the chain is `n` blocks past it. The latest `n` blocks can be reorged.
    Lag(u64),
}

/// A test node that was stopped with [`NodeTestContext::stop`].
///
/// The datadir of the node is kept until the node is launched again with [`Self::start`], and
/// removed when this is dropped.
pub struct StoppedNode<Node, AddOns>
where
    Node: FullNodeComponents,
    AddOns: RethRpcAddOns<Node>,
{
    data_dir: ChainPath<DataDirPath>,
    relaunch: Relaunch<NodeTestContext<Node, AddOns>>,
    /// Whether the datadir is removed on drop, which is unset once a launched node owns it.
    remove_data_dir: bool,
}

impl<Node, AddOns> StoppedNode<Node, AddOns>
where
    Node: FullNodeComponents,
    AddOns: RethRpcAddOns<Node>,
{
    /// Returns the datadir of the node.
    pub const fn data_dir(&self) -> &ChainPath<DataDirPath> {
        &self.data_dir
    }

    /// Launches the node again on its datadir and returns its new test context.
    ///
    /// The node is launched like by
    /// [`E2ETestSetupBuilder::build`](crate::E2ETestSetupBuilder::build), with the same node
    /// factory, node builder modifiers, node and tree configuration and attributes generator,
    /// on a new runtime. It opens the database, static files and `RocksDB` in its datadir
    /// again, so it recovers from disk like a node restarted from the command line.
    ///
    /// Its head is the canonical head it had when it was stopped, and its safe and finalized blocks
    /// are the ones it persisted, which are the head after [`NodeTestContext::advance_block`].
    /// Unless the node mines in dev mode, it receives a forkchoice update that restates these
    /// blocks instead of the forkchoice update to genesis of a new node, and
    /// [`NodeTestContext::advance_block`] builds on its head right away.
    ///
    /// The node keeps its peer identity, which is stored in its datadir, but listens on new ports
    /// and does not reconnect to its peers. Connect it with `node.connect(&mut peer)` once `peer`
    /// noticed that the stopped node disconnected, e.g. once its number of connected peers
    /// dropped. `peer.connect(&mut node)` panics instead, because the network events of `peer`
    /// still contain the disconnect of the stopped node.
    ///
    /// Returns an error if the node can not be launched, or if the forkchoice update is not valid.
    pub async fn start(mut self) -> eyre::Result<NodeTestContext<Node, AddOns>> {
        // The database of the launched node removes the datadir once it is dropped.
        self.remove_data_dir = false;
        (self.relaunch)().await
    }
}

impl<Node, AddOns> std::fmt::Debug for StoppedNode<Node, AddOns>
where
    Node: FullNodeComponents,
    AddOns: RethRpcAddOns<Node>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoppedNode").field("data_dir", &self.data_dir).finish_non_exhaustive()
    }
}

impl<Node, AddOns> Drop for StoppedNode<Node, AddOns>
where
    Node: FullNodeComponents,
    AddOns: RethRpcAddOns<Node>,
{
    fn drop(&mut self) {
        if self.remove_data_dir {
            let _ = reth_fs_util::remove_dir_all(self.data_dir.data_dir());
        }
    }
}

/// What a restartable node of [`E2ETestSetupBuilder`](crate::E2ETestSetupBuilder) needs to be
/// stopped and launched again.
pub(crate) struct NodeRestart<Ctx> {
    /// A handle of the database of the node, to wait until the stopped node released it.
    pub(crate) database: TmpDB,
    /// Launches the node again on its datadir.
    pub(crate) relaunch: Relaunch<Ctx>,
}

/// Closure that launches a stopped node again, returning its new test context.
pub(crate) type Relaunch<Ctx> =
    Arc<dyn Fn() -> BoxFuture<'static, eyre::Result<Ctx>> + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        wait::{assert_holds_for, poll_until_with, PollOpts},
        NodeHelperType,
    };
    use alloy_consensus::TxEnvelope;
    use reth_node_ethereum::{EthEngineTypes, EthereumNode};
    use reth_transaction_pool::test_utils::{testing_pool, MockTransaction};

    fn assert_send<T: Send>(_: T) {}

    /// Tests of downstream nodes await these helpers in spawned tasks, so their futures must be
    /// `Send`.
    #[expect(dead_code)]
    fn test_helper_futures_are_send(
        node: &mut NodeHelperType<EthereumNode>,
        payload: <EthEngineTypes as PayloadTypes>::BuiltPayload,
    ) {
        assert_send(node.new_payload());
        assert_send(node.advance_block());
        assert_send(node.advance_block_synced());
        assert_send(node.inject_and_advance(Bytes::new()));
        assert_send(node.advance_blocks(0));
        assert_send(node.build_payload_on(B256::ZERO));
        assert_send(node.advance_block_on(B256::ZERO));
        assert_send(node.advance_fork(B256::ZERO, 0));
        assert_send(node.reorg_to(B256::ZERO));
        assert_send(node.advance_until_receipt(B256::ZERO));
        assert_send(node.advance_until_pool_drained());
        assert_send(node.advance_while(async {}));
        assert_send(node.engine.new_payload(payload.clone()));
        assert_send(node.engine.new_payload_from_block(payload.block().clone(), None));
        assert_send(node.engine.forkchoice_updated(ForkchoiceState::default()));
        assert_send(node.engine.forkchoice_updated_with_attributes(
            ForkchoiceState::default(),
            <EthEngineTypes as PayloadTypes>::PayloadAttributes::default(),
        ));
        assert_send(node.mine([Bytes::new()]));
        assert_send(node.mine_including(Vec::new()));
        assert_send(node.mine_signed(Vec::<TxEnvelope>::new()));
        assert_send(node.mine_pooled([B256::ZERO]));
        assert_send(node.wait_block(0, B256::ZERO, false));
        assert_send(node.wait_unwind(0));
        assert_send(node.wait_for_pool(|_| true));
        assert_send(node.wait_for_pool_head(B256::ZERO));
        assert_send(node.wait_for_pooled([B256::ZERO]));
        assert_send(node.wait_for_pool_removal([B256::ZERO]));
        assert_send(node.wait_for_persisted_block(0));
        assert_send(node.wait_for_prune_checkpoint(PruneSegment::SenderRecovery, 0));
        assert_send(node.wait_for_head(B256::ZERO));
        assert_send(node.assert_new_block(B256::ZERO, B256::ZERO, 0));
        assert_send(node.sync_to(B256::ZERO));
        assert_send(node.sync_to_forkchoice(ForkchoiceState::default()));
        assert_send(node.submit_payload(payload.clone()));
        assert_send(node.submit_payload_with_status(payload.clone()));
        assert_send(node.import_payload(payload));
        assert_send(poll_until_with(PollOpts::default(), "", || async { Ok(Some(())) }));
        assert_send(assert_holds_for(Duration::ZERO, "", || async { Ok(true) }));
    }

    /// Like [`test_helper_futures_are_send`], for the helpers that stop and restart a node.
    #[expect(dead_code)]
    fn test_restart_futures_are_send(
        node: NodeHelperType<EthereumNode>,
        other: NodeHelperType<EthereumNode>,
    ) {
        assert_send(node.stop());
        assert_send(other.restart());
    }

    // The paused clock lets the waits run into their timeout without waiting for it.
    #[tokio::test(start_paused = true)]
    async fn pool_transaction_waits_list_pending_transactions_on_timeout() {
        let pool = testing_pool();
        let tx = MockTransaction::eip1559();
        let pooled = *tx.get_hash();
        pool.add_external_transaction(tx).await.unwrap();
        let absent = B256::repeat_byte(1);

        let err = wait_for_pool_transactions(&pool, [pooled, absent], true).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "timed out after 60s waiting for transactions to enter the pool, still missing: \
                 [{absent}]"
            )
        );

        let err = wait_for_pool_transactions(&pool, [pooled, absent], false).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "timed out after 60s waiting for transactions to leave the pool, still pooled: \
                 [{pooled}]"
            )
        );
    }
}
