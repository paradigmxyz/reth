use reth::builder::NodeHandle;
use reth_network::{
    config::SecretKey, protocol::IntoRlpxSubProtocol, NetworkConfig, NetworkManager,
    NetworkProtocols,
};
use reth_node_ethereum::EthereumNode;
use reth_provider::test_utils::NoopProvider;
use subprotocol::protocol::handler::{CustomRlpxProtoHandler, ProtocolState};
use tokio::sync::mpsc;

mod subprotocol;

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _args| async move {
        let NodeHandle { node, node_exit_future } =
            builder.node(EthereumNode::default()).launch().await?;

        let (tx, _rx) = mpsc::unbounded_channel();
        let custom_rlpx_handler =
            CustomRlpxProtoHandler { state: ProtocolState { events: tx.clone() } };

        node.network.add_rlpx_sub_protocol(custom_rlpx_handler.into_rlpx_sub_protocol());

        let custom_rlpx_handler_2 = CustomRlpxProtoHandler { state: ProtocolState { events: tx } };
        let secret_key = SecretKey::new(&mut rand::thread_rng());
        let net_cfg = NetworkConfig::builder(secret_key)
            .add_rlpx_sub_protocol(custom_rlpx_handler_2.into_rlpx_sub_protocol())
            .build(NoopProvider::default());
        let network = NetworkManager::new(net_cfg).await.unwrap();

        tokio::spawn(network);

        node_exit_future.await
    })
}
