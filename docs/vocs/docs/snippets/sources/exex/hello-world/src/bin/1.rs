use reth_node_ethereum::EthereumNode;

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(async move |builder, _| {
        let handle = builder.node(EthereumNode::default()).launch().await?;

        handle.wait_for_node_exit().await
    })
}
