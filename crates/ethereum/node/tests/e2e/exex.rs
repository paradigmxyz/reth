use futures::TryStreamExt;
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{wait::WAIT_TIMEOUT, E2ETestSetupExt};
use reth_exex::ExExEvent;
use reth_node_ethereum::EthereumNode;
use tokio::sync::mpsc;

#[tokio::test]
async fn test_exex_installed_by_node_builder_modifier() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (committed_tx, mut committed_rx) = mpsc::unbounded_channel();
    let (mut node, _) = EthereumNode::test_setup_for(EthereumHardfork::Cancun)
        .with_node_builder_modifier(move |builder| {
            let committed_tx = committed_tx.clone();
            builder.install_exex("committed-chains", |mut ctx| async move {
                Ok(async move {
                    while let Some(notification) = ctx.notifications.try_next().await? {
                        if let Some(chain) = notification.committed_chain() {
                            ctx.events.send(ExExEvent::FinishedHeight(chain.tip().num_hash()))?;
                            let _ = committed_tx.send(chain);
                        }
                    }
                    Ok(())
                })
            })
        })
        .build_single()
        .await?;

    let payload = node.advance_block().await?;

    let chain = tokio::time::timeout(WAIT_TIMEOUT, committed_rx.recv())
        .await?
        .ok_or_else(|| eyre::eyre!("ExEx stopped before receiving a committed chain"))?;
    assert_eq!(chain.tip().hash(), payload.block().hash());

    Ok(())
}
