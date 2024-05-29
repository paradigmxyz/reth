//! A minimal consensus client that derives L2 blocks using [kona-derive][1].
//!
//! [1]: https://gituhb.com/ethereum-optimism/kona

use std::sync::Arc;
use std::sync::Mutex;

use reth_transaction_pool::TransactionPool;
use reth_provider::Chain;
use reth_node_ethereum::EthereumNode;
use reth_node_api::FullNodeComponents;
use reth_revm::InMemoryDB;
use reth_exex::{ExExContext, ExExEvent};
use reth_tracing::tracing::{error, info};
use reth_primitives::{Receipt, TxHash, B256, BlobTransactionSidecar, SealedBlockWithSenders};

use kona_derive::types::Blob;
use kona_derive::DerivationPipeline;
use kona_derive::sources::EthereumDataSource;
use kona_derive::stages::{L1Traversal, StatefulAttributesBuilder, L1Retrieval, FrameQueue, ChannelBank, ChannelReader, BatchQueue, AttributesQueue};
use kona_primitives::{BlockInfo, L2BlockInfo, RollupConfig, SystemConfig};

mod execution;

mod blobs;
use blobs::{ExExBlobProvider, InMemoryBlobProvider};

mod reset;
use reset::{TipState, ExExResetProvider};

mod providers;
use providers::{InMemoryChainProvider, InMemoryL2ChainProvider, ExExChainProvider, ExExL2ChainProvider};

/* Custom ExEx Kona Derivation Types */
type ExExDerivationPipeline = DerivationPipeline<ExExAttributesQueue, ExExResetProvider>;
type ExExAttributesBuilder = StatefulAttributesBuilder<ExExChainProvider, ExExL2ChainProvider>;
type ExExAttributesQueue = AttributesQueue<ExExBatchQueue, ExExAttributesBuilder>;
type ExExBatchQueue = BatchQueue<ExExChannelReader, ExExL2ChainProvider>;
type ExExChannelReader = ChannelReader<ExExChannelBank>;
type ExExChannelBank = ChannelBank<ExExFrameQueue>;
type ExExFrameQueue = FrameQueue<ExExL1Retrieval>;
type ExExL1Retrieval = L1Retrieval<EthereumDataSource<ExExChainProvider, ExExBlobProvider>, ExExL1Traversal>;
type ExExL1Traversal = L1Traversal<ExExChainProvider>;

const DEFAULT_ATTRIBUTE_CHANNEL_SIZE: usize = 100;

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("Kona", move |ctx| async {
                Ok(Deriver::new(ctx).start())
            })
            .launch()
            .await?;
        handle.wait_for_node_exit().await
    })
}

struct Deriver<Node: FullNodeComponents> {
    ctx: ExExContext<Node>,
    db: InMemoryDB,
    chain_provider: Arc<Mutex<InMemoryChainProvider>>,
    tip_state: Arc<Mutex<TipState>>,
    l2_chain_provider: Arc<Mutex<InMemoryL2ChainProvider>>,
    blob_provider: Arc<Mutex<InMemoryBlobProvider>>,
}

impl<Node: FullNodeComponents> Deriver<Node> {
    fn new(ctx: ExExContext<Node>) -> Self {
        Self {
            ctx,
            db: InMemoryDB::new(reth_revm::db::EmptyDB::default()),
            chain_provider: Arc::new(Mutex::new(InMemoryChainProvider::new())),
            tip_state: Arc::new(Mutex::new(TipState::new(BlockInfo::default(), SystemConfig::default()))),
            l2_chain_provider: Arc::new(Mutex::new(InMemoryL2ChainProvider::new())),
            blob_provider: Arc::new(Mutex::new(InMemoryBlobProvider::new())),
        }
    }

    async fn start(mut self) -> eyre::Result<()> {
        // Build the kona derivation pipeline
        info!("Kona ExEx Deriver starting");

        // Create a channel for the derivation pipeline to send back prepared
        // L2AttributesWithParent.
        let (sender, mut receiver) = tokio::sync::mpsc::channel(DEFAULT_ATTRIBUTE_CHANNEL_SIZE);

        // Create a channel for the deriver to reset the pipeline.
        let (reset_sender, mut reset_receiver) = tokio::sync::mpsc::channel(1);

        // Continuously attempt to step on the derivation pipeline.
        let mut pipeline = self.new_pipeline();
        tokio::spawn(async move {
            loop {
                // Reset the pipeline if needed
                if (reset_receiver.recv().await).is_some() {
                    pipeline.reset();
                }
                // Step the pipeline.
                if let Err(e) = pipeline.step().await {
                    error!("Error stepping derivation pipeline: {:?}", e);
                }
                // Try to pop an item off the prepared attributes.
                if let Some(attributes) = pipeline.prepared.pop_front() {
                    if let Err(e) = sender.send(attributes).await {
                        error!("Error sending prepared attributes: {:?}", e);
                    }
                }
            }
        });

        // TODO: get rollup config from superchain-registry + configured chain id
        //       get genesis from rollup config
        //       derive ChainSpec from genesis
        let chain_spec = Arc::new(reth_primitives::ChainSpec::default());

        // Process all new chain state notifications
        loop {
            tokio::select! {
                Some(attributes) = receiver.recv() => {
                    let block_hash = execution::exec_payload(
                        &mut self.db,
                        attributes,
                        self.ctx.pool().clone(),
                        self.ctx.evm_config().clone(),
                        Arc::clone(&chain_spec),
                    ).await?;
                    info!("Executed block with hash: {}", block_hash);
                    // TODO: fetch block from sequencer and verify block hash matches
                }
                Some(notification) = self.ctx.notifications.recv() => {
                    if let Some(reverted_chain) = notification.reverted_chain() {
                        reset_sender.send(()).await?;
                        self.revert(&reverted_chain)?;
                    }

                    if let Some(committed_chain) = notification.committed_chain() {
                        self.commit(&committed_chain).await?;
                        self.ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
                    }
                }
            }
            if receiver.is_closed() { break; }
        }

        Ok(())
    }

    /// Creates a new derivation pipeline.
    fn new_pipeline(&self) -> ExExDerivationPipeline {
        // Place the cursor at the origin.
        let cursor = L2BlockInfo::default();

        // Create the base rollup config.
        // TODO: get this from superchain-registry?
        let rollup_config = Arc::new(RollupConfig::default());

        // Create a stateful attributes provider.
        let builder = StatefulAttributesBuilder::new(
            Arc::clone(&rollup_config),
            ExExL2ChainProvider::new(Arc::clone(&self.l2_chain_provider)),
            ExExChainProvider::new(Arc::clone(&self.chain_provider)),
        );

        // Other Providers
        let reset = ExExResetProvider::new(Arc::clone(&self.tip_state));
        let l2_chain_provider = ExExL2ChainProvider::new(Arc::clone(&self.l2_chain_provider));

        // Build the ethereum data source
        let chain_provider = ExExChainProvider::new(Arc::clone(&self.chain_provider));
        let blob_provider = ExExBlobProvider::new(Arc::clone(&self.blob_provider));
        let dap_source = EthereumDataSource::new(chain_provider, blob_provider, &rollup_config);

        // Instantiates and link all the stages.
        let chain_provider = ExExChainProvider::new(Arc::clone(&self.chain_provider));
        let l1_traversal = L1Traversal::new(chain_provider, Arc::clone(&rollup_config));
        let l1_retrieval = L1Retrieval::new(l1_traversal, dap_source);
        let frame_queue = FrameQueue::new(l1_retrieval);
        let channel_bank = ChannelBank::new(Arc::clone(&rollup_config), frame_queue);
        let channel_reader = ChannelReader::new(channel_bank, Arc::clone(&rollup_config));
        let batch_queue = BatchQueue::new(rollup_config.clone(), channel_reader, l2_chain_provider);
        let queue = AttributesQueue::new(*rollup_config, batch_queue, builder);

        // Assemble and return the pipeline.
        DerivationPipeline::new(queue, reset, cursor)
    }

    /// Process a new chain commit.
    ///
    /// This function decodes all transactions to the rollup contract into events, executes the
    /// corresponding actions and inserts the results into the database.
    async fn commit(&mut self, chain: &Chain) -> eyre::Result<()> {
        // Insert blocks and receipts into the provider for the derivation pipeline.
        let mut blocks = chain.blocks_iter().cloned().collect::<Vec<SealedBlockWithSenders>>();
        blocks.sort_by(|a, b| b.number.cmp(&a.number));
        let highest_block = blocks.first().cloned();
        let block_with_receipts: Vec<(&SealedBlockWithSenders, &Vec<Option<Receipt>>)> = chain.blocks_and_receipts().collect();
        let receipts: Vec<(B256, Vec<Option<alloy_consensus::Receipt>>)> = block_with_receipts.into_iter().map(|(block, receipts)| (block.hash(), receipts.clone().into_iter().map(to_consensus).collect())).collect();
        {
            let mut locked_chain_provider = self.chain_provider.lock().map_err(|_| eyre::eyre!("Failed to lock chain provider"))?;
            locked_chain_provider.insert_blocks(blocks);
            locked_chain_provider.insert_receipts(receipts);
        }

        // Fetch all blobs for all transactions.
        let blocks = chain.blocks_iter().collect::<Vec<&SealedBlockWithSenders>>();
        for block in blocks {
            let tx_hashes = block.transactions().map(|tx| tx.hash).collect::<Vec<TxHash>>();
            let blobs = self.ctx.pool().get_all_blobs(tx_hashes)?;
            let blobs = blobs.into_iter().map(|b| b.1).collect::<Vec<BlobTransactionSidecar>>();
            let mut locked_blob_provider = self.blob_provider.lock().map_err(|_| eyre::eyre!("Failed to lock blob provider"))?;
            // Flatten blob sidecar blobs.
            let blobs = blobs.into_iter().flat_map(|b| b.blobs).collect::<Vec<Blob>>();
            locked_blob_provider.insert_blobs(block.hash(), blobs);
        }

        // Update the reset provider with the latest origin.
        self.update_tip_state(highest_block)?;

        Ok(())
    }

    /// Process a chain revert.
    fn revert(&mut self, chain: &Chain) -> eyre::Result<()> {
        // Get the highest block after the chain revert.
        let mut blocks = chain.blocks_iter().collect::<Vec<&SealedBlockWithSenders>>();
        blocks.sort_by(|a, b| b.number.cmp(&a.number));
        let highest_block = blocks.first().cloned().cloned();
        let highest_block_number = highest_block.as_ref().map(|b| b.number).unwrap_or(0);

        // Prune the Chain Provider.
        self.chain_provider.lock().map_err(|_| eyre::eyre!("Failed to lock chain provider"))?.prune(
            highest_block_number
        );

        // Prune the L2 Chain Provider.
        self.l2_chain_provider.lock().map_err(|_| eyre::eyre!("Failed to lock L2 chain provider"))?.prune(
            highest_block_number
        );

        // Update the tip state.
        self.update_tip_state(highest_block)?;

        Ok(())
    }

    fn update_tip_state(&mut self, highest_block: Option<SealedBlockWithSenders>) -> eyre::Result<()> {
        if let Some(origin) = highest_block.map(|b| BlockInfo {
            hash: b.hash(),
            number: b.number,
            parent_hash: b.parent_hash,
            timestamp: b.timestamp
        }) {
            let mut locked = self.tip_state.lock().map_err(|_| eyre::eyre!("Failed to lock tip state"))?;
            locked.set_origin(origin);
        }
        Ok(())
    }
}

fn to_consensus(receipt: Option<Receipt>) -> Option<alloy_consensus::Receipt> {
    let receipt = receipt?;
    Some(alloy_consensus::Receipt {
        status: receipt.success,
        cumulative_gas_used: receipt.cumulative_gas_used,
        logs: receipt.logs,
    })
}

