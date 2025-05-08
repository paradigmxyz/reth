use async_trait::async_trait;
use futures::Future;
use reth_cli::chainspec::ChainSpecParser;
use reth_db::DatabaseEnv;
use reth_node_builder::{NodeBuilder, WithLaunchContext};
use std::{fmt, marker::PhantomData, sync::Arc};

#[async_trait(?Send)]
pub trait Launcher<C, Ext>
where
    C: ChainSpecParser,
    Ext: clap::Args + fmt::Debug,
{
    async fn entrypoint<'a>(
        self,
        builder: WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, C::ChainSpec>>,
        builder_args: Ext,
    ) -> eyre::Result<()>
    where
        Ext: 'a;
}

pub struct FnLauncher<F, Fut> {
    func: F,
    _result: PhantomData<Fut>,
}

impl<F, Fut> FnLauncher<F, Fut> {
    pub fn new(func: F) -> Self {
        Self { func, _result: PhantomData }
    }
}

#[async_trait(?Send)]
impl<C, Ext, F, Fut> Launcher<C, Ext> for FnLauncher<F, Fut>
where
    C: ChainSpecParser,
    Ext: clap::Args + fmt::Debug,
    F: FnOnce(WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, C::ChainSpec>>, Ext) -> Fut,
    Fut: Future<Output = eyre::Result<()>>,
{
    async fn entrypoint<'a>(
        self,
        builder: WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, C::ChainSpec>>,
        builder_args: Ext,
    ) -> eyre::Result<()>
    where
        Ext: 'a,
    {
        (self.func)(builder, builder_args).await
    }
}
