use futures::future;
use reth_db::test_utils::create_test_rw_db;
use reth_exex::ExExContext;
use reth_node_api::FullNodeComponents;
use reth_node_builder::{NodeBuilder, NodeConfig};
use reth_node_ethereum::{node::EthereumAddOns, EthereumNode};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

struct DummyExEx<Node: FullNodeComponents> {
    _ctx: ExExContext<Node>,
}

impl<Node> Future for DummyExEx<Node>
where
    Node: FullNodeComponents,
{
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

#[test]
fn basic_exex() {
    let config = NodeConfig::test();
    let db = create_test_rw_db();
    let _builder = NodeBuilder::new(config)
        .with_database(db)
        .with_types::<EthereumNode>()
        .with_components(EthereumNode::components())
        .with_add_ons(EthereumAddOns::default())
        .install_exex("dummy", move |ctx| future::ok(DummyExEx { _ctx: ctx }))
        .check_launch();
}
