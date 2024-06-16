use clap::Parser;
use network::{cli_ext::Discv5ArgsExt, Discv5Network};
use reth_node_ethereum::EthereumNode;
use std::net::{SocketAddrV4, SocketAddrV6};

mod network;
fn main() -> eyre::Result<()> {
    reth::cli::Cli::<Discv5ArgsExt>::parse().run(|builder, args| async move {
        let addr_v4 = args.addr_v4;
        let addr_v6 = args.addr_v6;
        let port = args.port;

        let handle = builder
            .node(EthereumNode::default())
            .install_exex("exex-discv5", move |_ctx| async move {
                let ipv4 = SocketAddrV4::new(addr_v4, port);
                let ipv6 = SocketAddrV6::new(addr_v6, port, 0, 0);

                Ok(Discv5Network::new(ipv4, ipv6).await)
            })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
