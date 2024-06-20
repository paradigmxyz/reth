use clap::Parser;
use network::{cli_ext::Discv5ArgsExt, DiscV5ExEx};
use reth_node_ethereum::EthereumNode;

mod network;

fn main() -> eyre::Result<()> {
    reth::cli::Cli::<Discv5ArgsExt>::parse().run(|builder, args| async move {
        let tcp_port = args.tcp_port;
        let udp_port = args.udp_port;

        let handle = builder
            .node(EthereumNode::default())
            .install_exex("exex-discv5", move |_ctx| async move {
                DiscV5ExEx::new(tcp_port, udp_port).await
            })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
