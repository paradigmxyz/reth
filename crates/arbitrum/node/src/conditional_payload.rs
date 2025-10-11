use reth_payload_builder::PayloadBuilderHandle;
use reth_node_api::{FullNodeTypes, NodeTypes, PrimitivesTy};
use reth_evm::ConfigureEvm;
use reth_transaction_pool::TransactionPool;
use reth_node_builder::components::{PayloadServiceBuilder, NoopPayloadServiceBuilder};
use std::marker::PhantomData;

pub struct ConditionalPayloadServiceBuilder<PB> {
    inner: PB,
    enabled: bool,
    _marker: PhantomData<()>,
}

impl<PB> ConditionalPayloadServiceBuilder<PB> {
    pub fn new(inner: PB, enabled: bool) -> Self {
        Self { inner, enabled, _marker: PhantomData }
    }
}

impl<N, Pool, EVM, PB> PayloadServiceBuilder<N, Pool, EVM> for ConditionalPayloadServiceBuilder<PB>
where
    N: FullNodeTypes,
    Pool: TransactionPool,
    EVM: ConfigureEvm<Primitives = PrimitivesTy<N::Types>> + 'static,
    PB: PayloadServiceBuilder<N, Pool, EVM>,
{
    async fn spawn_payload_builder_service(
        self,
        ctx: &reth_node_builder::BuilderContext<N>,
        pool: Pool,
        evm_config: EVM,
    ) -> eyre::Result<PayloadBuilderHandle<<N::Types as NodeTypes>::Payload>> {
        if self.enabled {
            self.inner.spawn_payload_builder_service(ctx, pool, evm_config).await
        } else {
            NoopPayloadServiceBuilder::default().spawn_payload_builder_service(ctx, pool, evm_config).await
        }
    }
}
