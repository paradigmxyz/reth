use std::{
    collections::HashMap,
    pin::Pin,
    task::{Context, Poll},
};

use alloy_sol_types::{sol, SolEventInterface};
use futures::Future;
use itertools::Itertools;
use reth::builder::FullNodeTypes;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_ethereum::EthereumNode;
use reth_primitives::{address, constants::ETH_TO_WEI, Address, U256};
use reth_provider::CanonStateNotification;

sol!(L1StandardBridge, "l1_standard_bridge_abi.json");
use crate::L1StandardBridge::{ETHBridgeFinalized, ETHBridgeInitiated, L1StandardBridgeEvents};

struct OptimismExEx<Node: FullNodeTypes> {
    ctx: ExExContext<Node>,
}

impl<Node: FullNodeTypes> Future for OptimismExEx<Node> {
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Process all new chain state notifications until there are no more
        while let Poll::Ready(Some(notification)) = this.ctx.notifications.poll_recv(cx) {
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
                        amount,
                        ..
                    }) => {
                        *contract_deposits.entry(log.address).or_default() += amount;
                    }
                    // L2 -> L1 withdrawal
                    L1StandardBridgeEvents::ETHBridgeFinalized(ETHBridgeFinalized {
                        amount,
                        ..
                    }) => {
                        *contract_withdrawals.entry(log.address).or_default() += amount;
                    }
                    _ => continue,
                };
            }

            // Finished filling the mappings, print the results
            println!("Finished block range: {:?}", chain.first().number..=chain.tip().number);
            if !contract_deposits.is_empty() {
                print_amounts("OP Stack Deposits", contract_deposits);
            }
            if !contract_withdrawals.is_empty() {
                print_amounts("OP Stack Withdrawals", contract_withdrawals);
            }

            // Send a finished height event, signaling the node that we don't need any blocks below
            // this height anymore
            this.ctx.events.send(ExExEvent::FinishedHeight(chain.tip().number))?;
        }

        Poll::Pending
    }
}

fn print_amounts(title: impl AsRef<str>, amounts: HashMap<Address, U256>) {
    println!("{}:", title.as_ref());
    for (address, amount) in amounts.into_iter().sorted_by_key(|(_, amount)| *amount).rev() {
        let amount = f64::from(amount) / ETH_TO_WEI as f64;
        if let Some(name) = contract_address_to_name(address) {
            println!("  {}: {} ETH", name, amount);
        } else {
            println!("  {}: {} ETH", address, amount);
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

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("Optimism", move |ctx| futures::future::ok(OptimismExEx { ctx }))
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
