use reth::builder::NodeHandle;
use reth_network::{protocol::IntoRlpxSubProtocol, NetworkProtocols};
use reth_node_ethereum::EthereumNode;
use subprotocol::protocol::handler::{CustomRlpxProtoHandler, ProtocolState};
use tokio::sync::mpsc;

mod subprotocol;

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _args| async move {
        let NodeHandle { node, node_exit_future } =
            builder.node(EthereumNode::default()).launch().await?;

        let (tx, _rx) = mpsc::unbounded_channel();
        let custom_rlpx_handler = CustomRlpxProtoHandler { state: ProtocolState { events: tx } };

        node.network.add_rlpx_sub_protocol(custom_rlpx_handler.into_rlpx_sub_protocol());

        // Spawn a task to handle incoming messages from the custom RLPx protocol

        node_exit_future.await
    })
}
