use std::{
    borrow::Cow,
    pin::Pin,
    task::{Context, Poll},
};

use futures::Stream;
use reth_db::test_utils::create_test_rw_db;
use reth_node_builder::{
    exex::{ExEx, ExExContext, ExExEvent},
    FullNodeTypes, NodeBuilder, NodeConfig,
};
use reth_node_ethereum::EthereumNode;

struct DummyExEx<Node: FullNodeTypes> {
    _ctx: ExExContext<Node>,
}

impl<Node: FullNodeTypes> ExEx for DummyExEx<Node> {
    fn name(&self) -> Cow<'static, str> {
        "dummy".into()
    }
}

impl<N: FullNodeTypes> Stream for DummyExEx<N> {
    type Item = ExExEvent;

    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
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
        .install_exex(move |ctx| async move { Ok(DummyExEx { _ctx: ctx }) })
        .check_launch();
}
