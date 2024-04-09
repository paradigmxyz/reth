use std::{
    collections::HashMap,
    future::poll_fn,
    pin::Pin,
    str::FromStr,
    sync::Arc,
    task::{ready, Context, Poll},
};

use alloy_sol_types::{sol, SolEventInterface};
use futures::{Future, FutureExt};
use itertools::Itertools;
use reth::{
    builder::{FullNodeTypes, FullNodeTypesAdapter, NodeConfig},
    dirs::PlatformPath,
};
use reth_blockchain_tree::noop::NoopBlockchainTree;
use reth_config::Config;
use reth_db::open_db_read_only;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_ethereum::EthereumNode;
use reth_primitives::{address, constants::ETH_TO_WEI, Address, Receipts, MAINNET, U256};
use reth_provider::{
    providers::BlockchainProvider, BlockNumReader, BlockReader, BundleStateWithReceipts,
    CanonStateNotification, Chain, ProviderFactory, ReceiptProvider,
};
use reth_revm::db::states::BundleBuilder;
use reth_tasks::TaskManager;

sol!(L1StandardBridge, "exex/optimism_l1_standard_bridge_abi.json");
use crate::L1StandardBridge::{ETHBridgeFinalized, ETHBridgeInitiated, L1StandardBridgeEvents};

struct OptimismExEx<Node: FullNodeTypes> {
    ctx: ExExContext<Node>,
}

impl<Node: FullNodeTypes> Future for OptimismExEx<Node> {
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Wait for a new chain state notification to arrive
        let notification = ready!(this.ctx.notifications.poll_recv(cx)).expect("channel closed");

        // Grab only new chain from both commit and reorg notifications
        let chain = match notification {
            CanonStateNotification::Commit { new } => new,
            CanonStateNotification::Reorg { old: _, new } => new,
        };

        // Initialize mappings for contract deposits and withdrawals
        let mut contract_deposits = HashMap::<Address, U256>::new();
        let mut contract_withdrawals = HashMap::<Address, U256>::new();

        // Fill deposit and withdrawal mappings with data from new notification
        for (log, bridge_event) in chain
            // Get all blocks and receipts
            .blocks_and_receipts()
            // Get all receipts
            .flat_map(|(_, receipts)| receipts.iter().flatten())
            // Get all logs
            .flat_map(|receipt| receipt.logs.iter())
            // Decode and filter bridge events
            .filter_map(|log| {
                L1StandardBridgeEvents::decode_raw_log(&log.topics, &log.data, true)
                    .ok()
                    .map(|event| (log, event))
            })
        {
            match bridge_event {
                // L1 -> L2 deposit
                L1StandardBridgeEvents::ETHBridgeInitiated(ETHBridgeInitiated {
                    amount, ..
                }) => {
                    *contract_deposits.entry(log.address).or_default() += amount;
                }
                // L2 -> L1 withdrawal
                L1StandardBridgeEvents::ETHBridgeFinalized(ETHBridgeFinalized {
                    amount, ..
                }) => {
                    *contract_withdrawals.entry(log.address).or_default() += amount;
                }
                _ => continue,
            };
        }

        // Finished filling the mappings, print the results
        println!("Finished block range: {:?}", chain.first().number..=chain.tip().number);
        if !contract_deposits.is_empty() {
            print_amounts("Contract Deposits", contract_deposits);
        }
        if !contract_withdrawals.is_empty() {
            print_amounts("Contract Withdrawals", contract_withdrawals);
        }
        println!();

        // Send a finished height event, signaling the node that we don't need any blocks below this
        // height anymore
        this.ctx.events.send(ExExEvent::FinishedHeight(chain.tip().number))?;

        Poll::Pending
    }
}

fn print_amounts(title: impl AsRef<str>, amounts: HashMap<Address, U256>) {
    println!("{}:", title.as_ref());
    for (address, amount) in amounts.into_iter().sorted_by_key(|(_, amount)| *amount).rev() {
        let amount = f64::from(amount) / ETH_TO_WEI as f64;
        if let Some(name) = contract_address_to_name(address) {
            println!("  {}: {}", name, amount);
        } else {
            println!("  {}: {}", address, amount);
        }
    }
}

fn contract_address_to_name(address: Address) -> Option<&'static str> {
    const BASE: Address = address!("3154Cf16ccdb4C6d922629664174b904d80F2C35");
    const BLAST_1: Address = address!("3a05E5d33d7Ab3864D53aaEc93c8301C1Fa49115");
    const BLAST_2: Address = address!("697402166Fbf2F22E970df8a6486Ef171dbfc524");
    const OPTIMISM: Address = address!("99C9fc46f92E8a1c0deC1b1747d010903E884bE1");
    const MODE: Address = address!("735aDBbE72226BD52e818E7181953f42E3b0FF21");
    const MANTA: Address = address!("3B95bC951EE0f553ba487327278cAc44f29715E5");

    match address {
        BASE => Some("Base"),
        BLAST_1 | BLAST_2 => Some("Blast"),
        OPTIMISM => Some("Optimism"),
        MODE => Some("Mode"),
        MANTA => Some("Manta"),
        _ => None,
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // TODO(alexey): uncomment when https://github.com/paradigmxyz/reth/pull/7340 is merged
    // reth::cli::Cli::parse_args()
    //     .run(|builder, _| async move {
    //         let handle = builder
    //             .node(EthereumNode::default())
    //             .install_exex(move |ctx| futures::future::ok(OptimismExEx { ctx }))
    //             .launch()
    //             .await?;
    //
    //         handle.wait_for_node_exit().await
    //     })
    //     .unwrap();

    let n_blocks = std::env::args()
        .nth(1)
        .and_then(|n_blocks| n_blocks.parse::<u64>().ok())
        .expect("n_blocks");

    let data_dir = PlatformPath::from_str(&std::env::var("RETH_DATA_PATH")?)?
        .with_chain(reth_primitives::Chain::mainnet());
    let db = Arc::new(open_db_read_only(data_dir.db_path().as_path(), Default::default())?);
    let factory = ProviderFactory::new(db, MAINNET.clone(), data_dir.static_files_path())?;
    let provider = BlockchainProvider::new(factory.clone(), NoopBlockchainTree::default())?;

    let task_manager = TaskManager::current();

    let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
    let (notifications_tx, notifications_rx) = tokio::sync::mpsc::channel(1);

    let ctx = ExExContext::<FullNodeTypesAdapter<EthereumNode, _, _>> {
        head: Default::default(),
        provider,
        task_executor: task_manager.executor(),
        data_dir,
        config: NodeConfig::test(),
        reth_config: Config::default(),
        events: events_tx,
        notifications: notifications_rx,
    };
    let mut exex = OptimismExEx { ctx };

    let provider_ro = factory.provider()?;
    let last_block_number = provider_ro.last_block_number()?;
    let block_range = last_block_number - n_blocks..=last_block_number;
    let blocks = provider_ro.block_range(block_range.clone())?;
    let receipts = provider_ro.receipts_by_block_range(block_range.clone())?;
    drop(provider_ro);

    let blocks = blocks
        .into_iter()
        .map(|block| block.seal_slow().seal_with_senders().expect("recover senders"))
        .collect::<Vec<_>>();

    let bundle_state = BundleStateWithReceipts::new(
        BundleBuilder::new(block_range.clone()).build(),
        Receipts::from_vec(
            receipts
                .into_iter()
                .map(|block_receipts| block_receipts.into_iter().map(Some).collect())
                .collect(),
        ),
        *block_range.start(),
    );
    let chain = Chain::new(blocks, bundle_state, None);

    let notification = CanonStateNotification::Commit { new: Arc::new(chain) };
    notifications_tx.send(notification).await?;

    let result = poll_fn(|cx| Poll::Ready(exex.poll_unpin(cx))).await?;
    assert_eq!(result, Poll::Pending);

    assert_eq!(events_rx.try_recv()?, reth_exex::ExExEvent::FinishedHeight(*block_range.end()));

    Ok(())
}
