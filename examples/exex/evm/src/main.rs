use std::collections::{HashMap, HashSet};

use alloy_sol_types::{sol, SolEventInterface, SolInterface};
use eyre::OptionExt;
use reth::{
    providers::{
        providers::BundleStateProvider, DatabaseProviderFactory, HistoricalStateProviderRef,
    },
    revm::database::StateProviderDatabase,
};
use reth_evm::ConfigureEvm;
use reth_evm_ethereum::EthEvmConfig;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_primitives::{revm::env::fill_tx_env, Address, Transaction, TxKind, TxLegacy, U256};
use reth_tracing::tracing::info;

sol!(ERC20, "erc20.json");
use crate::ERC20::{ERC20Calls, ERC20Events};

async fn exex<Node: FullNodeComponents>(mut ctx: ExExContext<Node>) -> eyre::Result<()> {
    let evm_config = EthEvmConfig::default();

    while let Some(notification) = ctx.notifications.recv().await {
        if let Some(chain) = notification.committed_chain() {
            let database_provider = ctx.provider().database_provider_ro()?;
            let provider = BundleStateProvider::new(
                HistoricalStateProviderRef::new(
                    database_provider.tx_ref(),
                    chain.first().number.checked_sub(1).ok_or_eyre("block number underflow")?,
                    database_provider.static_file_provider().clone(),
                ),
                chain.execution_outcome(),
            );
            let db = StateProviderDatabase::new(&provider);

            let mut evm = evm_config.evm(db);

            // Collect all ERC20 contract addresses and addresses that had balance changes
            let erc20_contracts_and_addresses = chain
                .block_receipts_iter()
                .flatten()
                .flatten()
                .flat_map(|receipt| receipt.logs.iter())
                .fold(HashMap::<Address, HashSet<Address>>::new(), |mut acc, log| {
                    if let Ok(ERC20Events::Transfer(ERC20::Transfer { from, to, value: _ })) =
                        ERC20Events::decode_raw_log(log.topics(), &log.data.data, true)
                    {
                        acc.entry(log.address).or_default().extend([from, to]);
                    }

                    acc
                });

            // Construct transactions to check the decimals of ERC20 contracts and balances of
            //addresses that had balance changes
            let txs = erc20_contracts_and_addresses.into_iter().map(|(contract, addresses)| {
                (
                    contract,
                    Transaction::Legacy(TxLegacy {
                        gas_limit: 50_000_000,
                        to: TxKind::Call(contract),
                        input: ERC20Calls::decimals(ERC20::decimalsCall {}).abi_encode().into(),
                        ..Default::default()
                    }),
                    addresses.into_iter().map(move |address| {
                        (
                            address,
                            Transaction::Legacy(TxLegacy {
                                gas_limit: 50_000_000,
                                to: TxKind::Call(contract),
                                input: ERC20Calls::balanceOf(ERC20::balanceOfCall {
                                    _owner: address,
                                })
                                .abi_encode()
                                .into(),
                                ..Default::default()
                            }),
                        )
                    }),
                )
            });

            for (contract, decimals_tx, balance_txs) in txs {
                fill_tx_env(evm.tx_mut(), &decimals_tx, Address::ZERO);
                let result = evm.transact()?.result;
                let output = result.output().ok_or_eyre("no output for decimals tx")?;
                let decimals = U256::try_from_be_slice(output);

                for (address, balance_tx) in balance_txs {
                    fill_tx_env(evm.tx_mut(), &balance_tx, Address::ZERO);
                    let result = evm.transact()?.result;
                    let output = result.output().ok_or_eyre("no output for balance tx")?;
                    let balance = U256::try_from_be_slice(output);

                    if let Some((balance, decimals)) = balance.zip(decimals) {
                        let divisor = U256::from(10).pow(decimals);
                        let (balance, rem) = balance.div_rem(divisor);
                        let balance = f64::from(balance) + f64::from(rem) / f64::from(divisor);
                        info!(?contract, ?address, %balance, "Balance updated");
                    } else {
                        info!(
                            ?contract,
                            ?address,
                            "Balance updated but too large to fit into U256"
                        );
                    }
                }
            }

            ctx.events.send(ExExEvent::FinishedHeight(chain.tip().number))?;
        }
    }
    Ok(())
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("EVM", |ctx| async move { Ok(exex(ctx)) })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
