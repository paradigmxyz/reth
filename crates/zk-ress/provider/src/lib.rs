//! Reth implementation of [`reth_zk_ress_protocol::ZkRessProtocolProvider`].

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use alloy_primitives::{Bytes, B256};
use reth_errors::ProviderResult;
use reth_ethereum_primitives::{Block, BlockBody, EthPrimitives};
use reth_evm::ConfigureEvm;
use reth_primitives_traits::Header;
use reth_ress_provider::RethRessProtocolProvider;
use reth_storage_api::{BlockReader, StateProviderFactory};
use reth_zk_ress_protocol::ZkRessProtocolProvider;
use tracing::*;

/// Provers supported by zkress.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum ZkRessProver {
    /// Execution witness.
    ExecutionWitness,
    /// SP1
    Sp1,
    /// Risc0
    Risc0,
}

impl ZkRessProver {
    /// Parse prover from CLI arg.
    pub fn try_from_arg(arg: &str) -> eyre::Result<Self> {
        match arg {
            "execution-witness" => Ok(Self::ExecutionWitness),
            "sp1" => Ok(Self::Sp1),
            "risc0" => Ok(Self::Risc0),
            _ => eyre::bail!("unknown protocol {arg}"),
        }
    }

    /// Returns the name of RLPx subprotocol for the given prover.
    pub fn protocol_name(&self) -> &'static str {
        match self {
            Self::ExecutionWitness => "zk-ress-execution-witness",
            Self::Sp1 => "zk-ress-sp1",
            Self::Risc0 => "zk-ress-risc0",
        }
    }
}

/// Reth provider implementing [`ZkRessProtocolProvider`].
#[expect(missing_debug_implementations)]
#[derive(Clone)]
pub struct RethZkRessProtocolProvider<P, E> {
    ress_provider: RethRessProtocolProvider<P, E>,
    prover: ZkRessProver,
}

impl<P, E> RethZkRessProtocolProvider<P, E> {
    /// Create new provider.
    pub fn new(ress_provider: RethRessProtocolProvider<P, E>, prover: ZkRessProver) -> Self {
        Self { ress_provider, prover }
    }
}

impl<P, E> ZkRessProtocolProvider for RethZkRessProtocolProvider<P, E>
where
    P: BlockReader<Block = Block> + StateProviderFactory + Clone + 'static,
    E: ConfigureEvm<Primitives = EthPrimitives> + 'static,
{
    type Proof = Bytes;

    fn header(&self, block_hash: B256) -> ProviderResult<Option<Header>> {
        trace!(target: "reth::zk_ress_provider", %block_hash, "Serving header");
        Ok(self.ress_provider.block_by_hash(block_hash)?.map(|b| b.header().clone()))
    }

    fn block_body(&self, block_hash: B256) -> ProviderResult<Option<BlockBody>> {
        trace!(target: "reth::zk_ress_provider", %block_hash, "Serving block body");
        Ok(self.ress_provider.block_by_hash(block_hash)?.map(|b| b.body().clone()))
    }

    async fn proof(&self, block_hash: B256) -> ProviderResult<Self::Proof> {
        let _witness = self.ress_provider.execution_witness(block_hash);

        // TODO: we have the witness, now make the request to the prover
        // match self.prover { .. }
        unimplemented!()
    }
}
