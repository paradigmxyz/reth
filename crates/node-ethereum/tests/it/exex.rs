use futures::future;
use reth_db::test_utils::create_test_rw_db;
use reth_node_builder::{exex::ExExContext, FullNodeTypes, NodeBuilder, NodeConfig};
use reth_node_ethereum::EthereumNode;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

struct DummyExEx<Node: FullNodeTypes> {
    _ctx: ExExContext<Node>,
}

impl<Node: FullNodeTypes> Future for DummyExEx<Node> {
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
        .with_types(EthereumNode::default())
        .with_components(EthereumNode::components())
        .install_exex(move |ctx| future::ok(DummyExEx { _ctx: ctx }))
        .check_launch();
}
