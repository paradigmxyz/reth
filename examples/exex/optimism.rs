use std::{
    collections::HashMap,
    env,
    future::poll_fn,
    pin::Pin,
    str::FromStr,
    sync::Arc,
    task::{ready, Context, Poll},
};

use alloy_sol_types::{sol, SolEventInterface};
use futures::{Future, FutureExt};
use reth::{
    builder::{FullNodeTypes, FullNodeTypesAdapter, NodeConfig},
    dirs::PlatformPath,
};
use reth_blockchain_tree::noop::NoopBlockchainTree;
use reth_config::Config;
use reth_db::open_db_read_only;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_ethereum::EthereumNode;
use reth_primitives::{Address, Receipts, MAINNET, U256};
use reth_provider::{
    providers::BlockchainProvider, BlockNumReader, BlockReader, BundleStateWithReceipts,
    CanonStateNotification, Chain, ProviderFactory, ReceiptProvider,
};
use reth_revm::db::states::BundleBuilder;
use reth_tasks::TaskManager;
use L1StandardBridge::L1StandardBridgeEvents;

use crate::L1StandardBridge::{ETHBridgeFinalized, ETHBridgeInitiated};

sol!(
    L1StandardBridge,
    r#"[{"inputs":[{"internalType":"address payable","name":"_messenger","type":"address"}],"stateMutability":"nonpayable","type":"constructor"},{"anonymous":false,"inputs":[{"indexed":true,"internalType":"address","name":"localToken","type":"address"},{"indexed":true,"internalType":"address","name":"remoteToken","type":"address"},{"indexed":true,"internalType":"address","name":"from","type":"address"},{"indexed":false,"internalType":"address","name":"to","type":"address"},{"indexed":false,"internalType":"uint256","name":"amount","type":"uint256"},{"indexed":false,"internalType":"bytes","name":"extraData","type":"bytes"}],"name":"ERC20BridgeFinalized","type":"event"},{"anonymous":false,"inputs":[{"indexed":true,"internalType":"address","name":"localToken","type":"address"},{"indexed":true,"internalType":"address","name":"remoteToken","type":"address"},{"indexed":true,"internalType":"address","name":"from","type":"address"},{"indexed":false,"internalType":"address","name":"to","type":"address"},{"indexed":false,"internalType":"uint256","name":"amount","type":"uint256"},{"indexed":false,"internalType":"bytes","name":"extraData","type":"bytes"}],"name":"ERC20BridgeInitiated","type":"event"},{"anonymous":false,"inputs":[{"indexed":true,"internalType":"address","name":"l1Token","type":"address"},{"indexed":true,"internalType":"address","name":"l2Token","type":"address"},{"indexed":true,"internalType":"address","name":"from","type":"address"},{"indexed":false,"internalType":"address","name":"to","type":"address"},{"indexed":false,"internalType":"uint256","name":"amount","type":"uint256"},{"indexed":false,"internalType":"bytes","name":"extraData","type":"bytes"}],"name":"ERC20DepositInitiated","type":"event"},{"anonymous":false,"inputs":[{"indexed":true,"internalType":"address","name":"l1Token","type":"address"},{"indexed":true,"internalType":"address","name":"l2Token","type":"address"},{"indexed":true,"internalType":"address","name":"from","type":"address"},{"indexed":false,"internalType":"address","name":"to","type":"address"},{"indexed":false,"internalType":"uint256","name":"amount","type":"uint256"},{"indexed":false,"internalType":"bytes","name":"extraData","type":"bytes"}],"name":"ERC20WithdrawalFinalized","type":"event"},{"anonymous":false,"inputs":[{"indexed":true,"internalType":"address","name":"from","type":"address"},{"indexed":true,"internalType":"address","name":"to","type":"address"},{"indexed":false,"internalType":"uint256","name":"amount","type":"uint256"},{"indexed":false,"internalType":"bytes","name":"extraData","type":"bytes"}],"name":"ETHBridgeFinalized","type":"event"},{"anonymous":false,"inputs":[{"indexed":true,"internalType":"address","name":"from","type":"address"},{"indexed":true,"internalType":"address","name":"to","type":"address"},{"indexed":false,"internalType":"uint256","name":"amount","type":"uint256"},{"indexed":false,"internalType":"bytes","name":"extraData","type":"bytes"}],"name":"ETHBridgeInitiated","type":"event"},{"anonymous":false,"inputs":[{"indexed":true,"internalType":"address","name":"from","type":"address"},{"indexed":true,"internalType":"address","name":"to","type":"address"},{"indexed":false,"internalType":"uint256","name":"amount","type":"uint256"},{"indexed":false,"internalType":"bytes","name":"extraData","type":"bytes"}],"name":"ETHDepositInitiated","type":"event"},{"anonymous":false,"inputs":[{"indexed":true,"internalType":"address","name":"from","type":"address"},{"indexed":true,"internalType":"address","name":"to","type":"address"},{"indexed":false,"internalType":"uint256","name":"amount","type":"uint256"},{"indexed":false,"internalType":"bytes","name":"extraData","type":"bytes"}],"name":"ETHWithdrawalFinalized","type":"event"},{"anonymous":false,"inputs":[{"indexed":false,"internalType":"uint8","name":"version","type":"uint8"}],"name":"Initialized","type":"event"},{"inputs":[],"name":"MESSENGER","outputs":[{"internalType":"contract CrossDomainMessenger","name":"","type":"address"}],"stateMutability":"view","type":"function"},{"inputs":[],"name":"OTHER_BRIDGE","outputs":[{"internalType":"contract StandardBridge","name":"","type":"address"}],"stateMutability":"view","type":"function"},{"inputs":[{"internalType":"address","name":"_localToken","type":"address"},{"internalType":"address","name":"_remoteToken","type":"address"},{"internalType":"uint256","name":"_amount","type":"uint256"},{"internalType":"uint32","name":"_minGasLimit","type":"uint32"},{"internalType":"bytes","name":"_extraData","type":"bytes"}],"name":"bridgeERC20","outputs":[],"stateMutability":"nonpayable","type":"function"},{"inputs":[{"internalType":"address","name":"_localToken","type":"address"},{"internalType":"address","name":"_remoteToken","type":"address"},{"internalType":"address","name":"_to","type":"address"},{"internalType":"uint256","name":"_amount","type":"uint256"},{"internalType":"uint32","name":"_minGasLimit","type":"uint32"},{"internalType":"bytes","name":"_extraData","type":"bytes"}],"name":"bridgeERC20To","outputs":[],"stateMutability":"nonpayable","type":"function"},{"inputs":[{"internalType":"uint32","name":"_minGasLimit","type":"uint32"},{"internalType":"bytes","name":"_extraData","type":"bytes"}],"name":"bridgeETH","outputs":[],"stateMutability":"payable","type":"function"},{"inputs":[{"internalType":"address","name":"_to","type":"address"},{"internalType":"uint32","name":"_minGasLimit","type":"uint32"},{"internalType":"bytes","name":"_extraData","type":"bytes"}],"name":"bridgeETHTo","outputs":[],"stateMutability":"payable","type":"function"},{"inputs":[{"internalType":"address","name":"_l1Token","type":"address"},{"internalType":"address","name":"_l2Token","type":"address"},{"internalType":"uint256","name":"_amount","type":"uint256"},{"internalType":"uint32","name":"_minGasLimit","type":"uint32"},{"internalType":"bytes","name":"_extraData","type":"bytes"}],"name":"depositERC20","outputs":[],"stateMutability":"nonpayable","type":"function"},{"inputs":[{"internalType":"address","name":"_l1Token","type":"address"},{"internalType":"address","name":"_l2Token","type":"address"},{"internalType":"address","name":"_to","type":"address"},{"internalType":"uint256","name":"_amount","type":"uint256"},{"internalType":"uint32","name":"_minGasLimit","type":"uint32"},{"internalType":"bytes","name":"_extraData","type":"bytes"}],"name":"depositERC20To","outputs":[],"stateMutability":"nonpayable","type":"function"},{"inputs":[{"internalType":"uint32","name":"_minGasLimit","type":"uint32"},{"internalType":"bytes","name":"_extraData","type":"bytes"}],"name":"depositETH","outputs":[],"stateMutability":"payable","type":"function"},{"inputs":[{"internalType":"address","name":"_to","type":"address"},{"internalType":"uint32","name":"_minGasLimit","type":"uint32"},{"internalType":"bytes","name":"_extraData","type":"bytes"}],"name":"depositETHTo","outputs":[],"stateMutability":"payable","type":"function"},{"inputs":[{"internalType":"address","name":"","type":"address"},{"internalType":"address","name":"","type":"address"}],"name":"deposits","outputs":[{"internalType":"uint256","name":"","type":"uint256"}],"stateMutability":"view","type":"function"},{"inputs":[{"internalType":"address","name":"_localToken","type":"address"},{"internalType":"address","name":"_remoteToken","type":"address"},{"internalType":"address","name":"_from","type":"address"},{"internalType":"address","name":"_to","type":"address"},{"internalType":"uint256","name":"_amount","type":"uint256"},{"internalType":"bytes","name":"_extraData","type":"bytes"}],"name":"finalizeBridgeERC20","outputs":[],"stateMutability":"nonpayable","type":"function"},{"inputs":[{"internalType":"address","name":"_from","type":"address"},{"internalType":"address","name":"_to","type":"address"},{"internalType":"uint256","name":"_amount","type":"uint256"},{"internalType":"bytes","name":"_extraData","type":"bytes"}],"name":"finalizeBridgeETH","outputs":[],"stateMutability":"payable","type":"function"},{"inputs":[{"internalType":"address","name":"_l1Token","type":"address"},{"internalType":"address","name":"_l2Token","type":"address"},{"internalType":"address","name":"_from","type":"address"},{"internalType":"address","name":"_to","type":"address"},{"internalType":"uint256","name":"_amount","type":"uint256"},{"internalType":"bytes","name":"_extraData","type":"bytes"}],"name":"finalizeERC20Withdrawal","outputs":[],"stateMutability":"nonpayable","type":"function"},{"inputs":[{"internalType":"address","name":"_from","type":"address"},{"internalType":"address","name":"_to","type":"address"},{"internalType":"uint256","name":"_amount","type":"uint256"},{"internalType":"bytes","name":"_extraData","type":"bytes"}],"name":"finalizeETHWithdrawal","outputs":[],"stateMutability":"payable","type":"function"},{"inputs":[{"internalType":"contract SuperchainConfig","name":"_superchainConfig","type":"address"}],"name":"initialize","outputs":[],"stateMutability":"nonpayable","type":"function"},{"inputs":[],"name":"l2TokenBridge","outputs":[{"internalType":"address","name":"","type":"address"}],"stateMutability":"view","type":"function"},{"inputs":[],"name":"messenger","outputs":[{"internalType":"contract CrossDomainMessenger","name":"","type":"address"}],"stateMutability":"view","type":"function"},{"inputs":[],"name":"otherBridge","outputs":[{"internalType":"contract StandardBridge","name":"","type":"address"}],"stateMutability":"view","type":"function"},{"inputs":[],"name":"paused","outputs":[{"internalType":"bool","name":"","type":"bool"}],"stateMutability":"view","type":"function"},{"inputs":[],"name":"superchainConfig","outputs":[{"internalType":"contract SuperchainConfig","name":"","type":"address"}],"stateMutability":"view","type":"function"},{"inputs":[],"name":"version","outputs":[{"internalType":"string","name":"","type":"string"}],"stateMutability":"view","type":"function"},{"stateMutability":"payable","type":"receive"}]"#
);

struct OptimismExEx<Node: FullNodeTypes> {
    ctx: ExExContext<Node>,
    start_height: Option<u64>,
    deposits: HashMap<Address, U256>,
    withdrawals: HashMap<Address, U256>,
}

impl<Node: FullNodeTypes> OptimismExEx<Node> {
    fn new(ctx: ExExContext<Node>) -> Self {
        Self {
            ctx,
            start_height: Default::default(),
            deposits: Default::default(),
            withdrawals: Default::default(),
        }
    }
}

impl<Node: FullNodeTypes> Future for OptimismExEx<Node> {
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        let notification = ready!(this.ctx.notifications.poll_recv(cx)).expect("channel closed");

        let chain = match notification {
            CanonStateNotification::Commit { new } => new,
            CanonStateNotification::Reorg { old: _, new } => new,
        };

        this.start_height.get_or_insert(chain.first().number);
        let last_block = chain.tip().number;

        for (_, receipts) in chain.blocks_and_receipts() {
            for receipt in receipts.iter().flatten() {
                for log in &receipt.logs {
                    if let Ok(bridge_event) =
                        L1StandardBridgeEvents::decode_raw_log(&log.topics, &log.data, true)
                    {
                        match bridge_event {
                            L1StandardBridgeEvents::ETHBridgeInitiated(ETHBridgeInitiated {
                                from,
                                amount,
                                ..
                            }) => *this.deposits.entry(from).or_default() += amount,
                            L1StandardBridgeEvents::ETHBridgeFinalized(ETHBridgeFinalized {
                                to,
                                amount,
                                ..
                            }) => *this.withdrawals.entry(to).or_default() += amount,
                            _ => continue,
                        };
                    }
                }
            }
        }

        this.ctx.events.send(ExExEvent::FinishedHeight(last_block))?;
        println!("Start height: {}", this.start_height.unwrap());
        println!("Finished height: {}", last_block);
        println!("Deposits:");
        for (address, amount) in &this.deposits {
            println!("  {}: {}", address, amount);
        }
        println!("Withdrawals:");
        for (address, amount) in &this.withdrawals {
            println!("  {}: {}", address, amount);
        }

        Poll::Pending
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
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
    let mut exex = OptimismExEx::new(ctx);

    let provider_ro = factory.provider()?;
    let last_block_number = provider_ro.last_block_number()?;
    let block_range = last_block_number - 1000..=last_block_number;
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
