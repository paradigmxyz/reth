//! Invalid block hook helpers for the node builder.

use crate::AddOnsContext;
use alloy_rpc_types::{Block, Header, Receipt, Transaction, TransactionRequest};
use eyre::OptionExt;
use reth_chainspec::EthChainSpec;
use reth_engine_primitives::InvalidBlockHook;
use reth_node_api::{FullNodeComponents, NodeTypes};
use reth_node_core::{
    args::InvalidBlockHookType,
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
};
use reth_primitives_traits::NodePrimitives;
use reth_provider::ChainSpecProvider;
use reth_rpc_api::EthApiClient;

/// Extension trait for [`AddOnsContext`] to create invalid block hooks.
pub trait InvalidBlockHookExt {
    /// Node primitives type.
    type Primitives: NodePrimitives;

    /// Creates an invalid block hook based on the node configuration.
    fn create_invalid_block_hook(
        &self,
        data_dir: &ChainPath<DataDirPath>,
    ) -> impl std::future::Future<Output = eyre::Result<Box<dyn InvalidBlockHook<Self::Primitives>>>>
           + Send;
}

impl<N> InvalidBlockHookExt for AddOnsContext<'_, N>
where
    N: FullNodeComponents,
{
    type Primitives = <N::Types as NodeTypes>::Primitives;

    async fn create_invalid_block_hook(
        &self,
        data_dir: &ChainPath<DataDirPath>,
    ) -> eyre::Result<Box<dyn InvalidBlockHook<Self::Primitives>>> {
        create_invalid_block_hook(
            self.config,
            data_dir,
            self.node.provider().clone(),
            self.node.evm_config().clone(),
            self.node.provider().chain_spec().chain().id(),
        )
        .await
    }
}

/// Creates an invalid block hook based on the node configuration.
///
/// This function constructs the appropriate [`InvalidBlockHook`] based on the debug
/// configuration in the node config. It supports:
/// - Witness hooks for capturing block witness data
/// - Healthy node verification via RPC
///
/// # Arguments
/// * `config` - The node configuration containing debug settings
/// * `data_dir` - The data directory for storing hook outputs
/// * `provider` - The blockchain database provider
/// * `evm_config` - The EVM configuration
/// * `chain_id` - The chain ID for verification
pub async fn create_invalid_block_hook<N, P, E>(
    config: &NodeConfig<P::ChainSpec>,
    data_dir: &ChainPath<DataDirPath>,
    provider: P,
    evm_config: E,
    chain_id: u64,
) -> eyre::Result<Box<dyn InvalidBlockHook<N>>>
where
    N: NodePrimitives,
    P: reth_provider::StateProviderFactory
        + reth_provider::ChainSpecProvider
        + Clone
        + Send
        + Sync
        + 'static,
    E: reth_evm::ConfigureEvm<Primitives = N> + Clone + 'static,
{
    use reth_engine_primitives::{InvalidBlockHooks, NoopInvalidBlockHook};
    use reth_invalid_block_hooks::InvalidBlockWitnessHook;

    let Some(ref hook) = config.debug.invalid_block_hook else {
        return Ok(Box::new(NoopInvalidBlockHook::default()))
    };

    let healthy_node_rpc_client = get_healthy_node_client(config, chain_id).await?;

    let output_directory = data_dir.invalid_block_hooks();
    let hooks = hook
        .iter()
        .copied()
        .map(|hook| {
            let output_directory = output_directory.join(hook.to_string());
            std::fs::create_dir_all(&output_directory)?;

            Ok(match hook {
                InvalidBlockHookType::Witness => Box::new(InvalidBlockWitnessHook::new(
                    provider.clone(),
                    evm_config.clone(),
                    output_directory,
                    healthy_node_rpc_client.clone(),
                )),
                InvalidBlockHookType::PreState | InvalidBlockHookType::Opcode => {
                    eyre::bail!("invalid block hook {hook:?} is not implemented yet")
                }
            } as Box<dyn InvalidBlockHook<_>>)
        })
        .collect::<Result<_, _>>()?;

    Ok(Box::new(InvalidBlockHooks(hooks)))
}

/// Returns an RPC client for the healthy node, if configured in the node config.
async fn get_healthy_node_client<C>(
    config: &NodeConfig<C>,
    chain_id: u64,
) -> eyre::Result<Option<jsonrpsee::http_client::HttpClient>>
where
    C: EthChainSpec,
{
    let Some(url) = config.debug.healthy_node_rpc_url.as_ref() else {
        return Ok(None);
    };

    let client = jsonrpsee::http_client::HttpClientBuilder::default().build(url)?;

    // Verify that the healthy node is running the same chain as the current node.
    let healthy_chain_id =
        EthApiClient::<TransactionRequest, Transaction, Block, Receipt, Header>::chain_id(&client)
            .await?
            .ok_or_eyre("healthy node rpc client didn't return a chain id")?;

    if healthy_chain_id.to::<u64>() != chain_id {
        eyre::bail!("Invalid chain ID. Expected {}, got {}", chain_id, healthy_chain_id);
    }

    Ok(Some(client))
}
