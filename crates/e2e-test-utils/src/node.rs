use crate::{
    network::NetworkTestContext,
    payload::PayloadTestContext,
    rpc::RpcTestContext,
    wait::{poll_until, POLL_INTERVAL, WAIT_TIMEOUT},
};
use alloy_consensus::{transaction::TxHashRef, BlockHeader};
use alloy_eips::BlockId;
use alloy_network::{Ethereum, IntoWallet};
use alloy_primitives::{BlockHash, BlockNumber, Bytes, Sealable, B256};
use alloy_provider::{
    fillers::{FillProvider, RecommendedFillers, TxFiller},
    Provider, ProviderBuilder, RootProvider,
};
use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV5, ForkchoiceState, ForkchoiceUpdated};
use alloy_rpc_types_eth::BlockNumberOrTag;
use eyre::{ensure, eyre, Ok};
use futures_util::{
    future::{select, Either},
    Future,
};
use jsonrpsee::{core::client::ClientT, http_client::HttpClient};
use reth_chainspec::EthereumHardforks;
use reth_network_api::test_utils::PeersHandleProvider;
use reth_node_api::{Block, BlockBody, BlockTy, FullNodeComponents, PayloadTypes, PrimitivesTy};
use reth_node_builder::{rpc::RethRpcAddOns, FullNode, NodeTypes};
use reth_payload_primitives::BuiltPayload;
use reth_provider::{
    BlockNumReader, BlockReader, BlockReaderIdExt, CanonStateNotificationStream,
    CanonStateSubscriptions, DatabaseProviderFactory, HeaderProvider, PruneCheckpointReader,
    StageCheckpointReader,
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
    sync::Arc,
    time::Duration,
};
use tokio_stream::StreamExt;
use url::Url;

/// Interval at which the alloy providers of [`NodeTestContext`] poll the node, e.g. for receipts
/// of pending transactions.
///
/// Much shorter than alloy's default for local nodes, since test nodes build blocks on demand or
/// with short dev block times.
pub const RPC_PROVIDER_POLL_INTERVAL: Duration = Duration::from_millis(10);

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
    /// Canonical state events.
    pub canonical_stream: CanonStateNotificationStream<PrimitivesTy<Node::Types>>,
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
            canonical_stream: node.provider.canonical_state_stream(),
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
        let latest_header =
            self.inner.provider.sealed_header_by_number_or_tag(BlockNumberOrTag::Latest)?.unwrap();

        if latest_header.number() == 0 {
            return Ok(ForkchoiceState::same_hash(latest_header.hash()));
        }

        Ok(ForkchoiceState {
            head_block_hash: latest_header.hash(),
            safe_block_hash: self
                .inner
                .provider
                .sealed_header_by_number_or_tag(BlockNumberOrTag::Safe)?
                .unwrap()
                .hash(),
            finalized_block_hash: self
                .inner
                .provider
                .sealed_header_by_number_or_tag(BlockNumberOrTag::Finalized)?
                .unwrap()
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

    /// Creates a new payload from given attributes generator
    /// expects a payload attribute event and waits until the payload is built.
    ///
    /// It triggers the resolve payload via engine api and expects the built payload event.
    pub async fn new_payload(&mut self) -> eyre::Result<Payload::BuiltPayload> {
        let eth_attr = self.payload.next_attributes();
        let payload_id = self
            .inner
            .add_ons_handle
            .beacon_engine_handle
            .fork_choice_updated(self.current_forkchoice_state()?, Some(eth_attr.clone()))
            .await?
            .payload_id
            .unwrap();
        // first event is the payload attributes
        self.payload.expect_attr_event(eth_attr).await?;
        // wait for the payload builder to have finished building
        self.payload.wait_for_built_payload(payload_id).await;
        // ensure we're also receiving the built payload as event
        Ok(self.payload.expect_built_payload().await?)
    }

    /// Triggers payload building job and submits it to the engine.
    pub async fn build_and_submit_payload(&mut self) -> eyre::Result<Payload::BuiltPayload> {
        let payload = self.new_payload().await?;

        self.submit_payload(payload.clone()).await?;

        Ok(payload)
    }

    /// Advances the node forward one block by building a payload and importing it with
    /// [`Self::import_payload`].
    pub async fn advance_block(&mut self) -> eyre::Result<Payload::BuiltPayload> {
        let payload = self.new_payload().await?;

        self.import_payload(payload.clone()).await?;

        Ok(payload)
    }

    /// Advances the node forward one block like [`Self::advance_block`] and waits until the
    /// transaction pool processed the new block, see [`Self::wait_for_pool_head`].
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
                // Cancelling the block would leave its payload events in the stream and break the
                // next block, so finish it even if `fut` completes first.
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
    /// them with [`Self::wait_for_pool`], and commits deeper than the maximum update depth of the
    /// maintenance task, e.g. after a long sync, which only update the last seen block.
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
        // The stream buffers every canonical notification since the context was created, e.g. of
        // blocks mined with `advance_block`, so skip notifications until the one that commits the
        // block, then verify the tx is included in it.
        let wait = async {
            loop {
                let notification = self
                    .canonical_stream
                    .next()
                    .await
                    .ok_or_else(|| eyre!("canonical state stream closed"))?;
                let committed = notification.committed();
                if let Some(block) = committed.blocks().get(&block_number) &&
                    block.hash() == block_hash
                {
                    return eyre::Ok(Arc::clone(block))
                }
            }
        };
        let block = tokio::time::timeout(WAIT_TIMEOUT, wait)
            .await
            .map_err(|_| eyre!("timed out waiting for block {block_number}"))??;
        ensure!(
            block.body().transactions().iter().any(|tx| *tx.tx_hash() == tip_tx_hash),
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
    /// Returns an error if the node does not sync within [`WAIT_TIMEOUT`].
    pub async fn sync_to(&self, block: BlockHash) -> eyre::Result<()> {
        let sync = async {
            while self
                .inner
                .provider
                .sealed_header_by_id(BlockId::latest())?
                .is_none_or(|h| h.hash() != block)
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
                self.update_forkchoice(block, block).await?;
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

    /// Submits a payload to the engine.
    pub async fn submit_payload(&self, payload: Payload::BuiltPayload) -> eyre::Result<B256> {
        let block_hash = payload.block().hash();
        self.inner.add_ons_handle.beacon_engine_handle.new_payload(payload.into()).await?;

        Ok(block_hash)
    }

    /// Submits a payload to the engine and makes its block the canonical head, returning the block
    /// hash.
    ///
    /// The forkchoice update marks the block as head, safe, and finalized block, like
    /// [`Self::advance_block`] does, so the node can't reorg to a chain without the block
    /// afterwards. The engine only reports the forkchoice update valid after making the head
    /// canonical, so the block is the latest block of the node once this returns. The transaction
    /// pool processes the new block in the background, see [`Self::wait_for_pool`].
    ///
    /// The parent of the payload must be known to the node, e.g. to import a payload built by
    /// another node into its peers. Returns an error if the engine does not report the forkchoice
    /// update valid, e.g. because the payload is invalid.
    pub async fn import_payload(&self, payload: Payload::BuiltPayload) -> eyre::Result<B256> {
        let block_hash = self.submit_payload(payload).await?;
        let updated = self.update_forkchoice(block_hash, block_hash).await?;
        ensure!(
            updated.is_valid(),
            "forkchoice update to block {block_hash} is not valid: {}",
            updated.payload_status.status
        );

        Ok(block_hash)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeHelperType;
    use reth_node_ethereum::{EthEngineTypes, EthereumNode};

    fn assert_send<T: Send>(_: T) {}

    /// Tests of downstream nodes await these helpers in spawned tasks, so their futures must be
    /// `Send`.
    #[expect(dead_code)]
    fn test_helper_futures_are_send(
        node: &mut NodeHelperType<EthereumNode>,
        payload: <EthEngineTypes as PayloadTypes>::BuiltPayload,
    ) {
        assert_send(node.advance_block());
        assert_send(node.advance_block_synced());
        assert_send(node.inject_and_advance(Bytes::new()));
        assert_send(node.advance_blocks(0));
        assert_send(node.advance_until_receipt(B256::ZERO));
        assert_send(node.advance_until_pool_drained());
        assert_send(node.advance_while(async {}));
        assert_send(node.wait_block(0, B256::ZERO, false));
        assert_send(node.wait_unwind(0));
        assert_send(node.wait_for_pool(|_| true));
        assert_send(node.wait_for_pool_head(B256::ZERO));
        assert_send(node.wait_for_persisted_block(0));
        assert_send(node.wait_for_prune_checkpoint(PruneSegment::SenderRecovery, 0));
        assert_send(node.assert_new_block(B256::ZERO, B256::ZERO, 0));
        assert_send(node.sync_to(B256::ZERO));
        assert_send(node.import_payload(payload));
    }
}
