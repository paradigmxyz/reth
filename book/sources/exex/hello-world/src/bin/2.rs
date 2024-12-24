use reth::api::FullNodeComponents;
use reth_exex::ExExContext;
use reth_node_ethereum::EthereumNode;

async fn my_exex<Node: FullNodeComponents>(mut _ctx: ExExContext<Node>) -> eyre::Result<()> {
    #[allow(clippy::empty_loop)]
    loop {}
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("my-exex", |ctx| async move { Ok(my_exex(ctx)) })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
