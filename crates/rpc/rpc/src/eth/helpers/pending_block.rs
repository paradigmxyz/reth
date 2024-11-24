//! Support for building a pending block with transactions from local view of mempool.

use alloy_consensus::Header;
use alloy_primitives::Bytes;
use alloy_rpc_types_engine::PayloadAttributes;
use reth_basic_payload_builder::{
    BuildArguments, BuildOutcome, Cancelled, PayloadBuilder, PayloadConfig,
};
use reth_chainspec::{ChainSpec, EthChainSpec, EthereumHardforks};
use reth_ethereum_payload_builder::EthereumPayloadBuilder;
use reth_evm::ConfigureEvm;
use reth_node_api::{BuiltPayload, EngineApiMessageVersion, PayloadBuilderAttributes};
use reth_primitives::{Receipt, SealedBlockWithSenders};
use reth_provider::{
    BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ProviderError, StateProviderFactory,
};
use reth_revm::cached::CachedReads;
use reth_rpc_eth_api::{
    helpers::{LoadPendingBlock, SpawnBlocking},
    RpcNodeCore,
};
use reth_rpc_eth_types::{EthApiError, PendingBlock, PendingBlockEnv};
use reth_transaction_pool::TransactionPool;
use std::sync::Arc;

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> LoadPendingBlock
    for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: SpawnBlocking
        + RpcNodeCore<
            Provider: BlockReaderIdExt
                          + EvmEnvProvider
                          + ChainSpecProvider<ChainSpec = ChainSpec>
                          + StateProviderFactory,
            Pool: TransactionPool,
            Evm: ConfigureEvm<Header = Header>,
            PayloadBuilder = EthereumPayloadBuilder<EvmConfig>,
        >,
    ChainSpec: EthChainSpec + EthereumHardforks,
    EvmConfig: ConfigureEvm<Header = Header>,
{
    #[inline]
    fn pending_block(&self) -> &tokio::sync::Mutex<Option<PendingBlock>> {
        self.inner.pending_block()
    }

    /// Builds a pending block using the configured provider and pool.
    ///
    /// If the origin is the actual pending block, the block is built with withdrawals.
    ///
    /// After Cancun, if the origin is the actual pending block, the block includes the EIP-4788 pre
    /// block contract call using the parent beacon block root received from the CL.
    fn build_block(
        &self,
        env: PendingBlockEnv,
    ) -> Result<(SealedBlockWithSenders, Vec<Receipt>), Self::Error>
    where
        EthApiError: From<ProviderError>,
    {
        let PendingBlockEnv { cfg: _, block_env, origin } = env;

        // Prepare payload attributes
        let payload_attributes = PayloadAttributes {
            parent_beacon_block_root: origin.header().parent_beacon_block_root,
            prev_randao: block_env.prevrandao.unwrap_or_default(),
            timestamp: block_env.timestamp.to::<u64>(),
            suggested_fee_recipient: block_env.coinbase,
            withdrawals: self
                .provider()
                .chain_spec()
                .is_shanghai_active_at_timestamp(block_env.timestamp.to::<u64>())
                .then(Default::default),
        };

        // Create the payload config
        let payload_config = PayloadConfig::new(
            Arc::new(origin.header().clone()),
            Bytes::default(),
            reth_payload_builder::EthPayloadBuilderAttributes::try_new(
                origin.header().hash(),
                payload_attributes,
                EngineApiMessageVersion::default() as u8, // Engine API message version
            )
            .unwrap(), // Unwrap the Result to get EthPayloadBuilderAttributes
        );

        // Build arguments
        let args = BuildArguments::new(
            self.provider().clone(),
            self.pool().clone(),
            CachedReads::default(),
            payload_config,
            Cancelled::default(), // Or provide a cancellation token if necessary
            None,                 // Optional best payload
        );

        let payload_builder = self.payload_builder().clone();

        // Build the payload
        match payload_builder.try_build(args).unwrap() {
            BuildOutcome::Better { payload, .. } => {
                let sealed_block = payload.block().clone();
                let receipts = payload
                    .executed_block()
                    .expect("executed block missing")
                    .execution_outcome()
                    .receipts()
                    .iter()
                    .flatten()
                    .filter_map(|opt| opt.clone())
                    .collect();
                let senders = sealed_block.senders().expect("sender recovery failed");

                Ok((SealedBlockWithSenders { block: sealed_block, senders }, receipts))
            }
            _ => unreachable!("other outcomes are unreachable"),
        }
    }
}
