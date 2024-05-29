//! Contains logic to validate derivation pipeline outputs.

use alloy_provider::{Provider, ReqwestProvider};
use alloy_rpc_types::{BlockNumberOrTag, BlockTransactionsKind, Header};
use alloy_transport::TransportResult;
use anyhow::Result;
use kona_derive::types::{L2AttributesWithParent, L2PayloadAttributes, RawTransaction};
use std::vec::Vec;
use tracing::warn;

/// OnlineValidator
///
/// Validates the [`L2AttributesWithParent`] by fetching the associated L2 block from
/// a trusted L2 RPC and constructing the L2 Attributes from the block.
#[derive(Debug, Clone)]
pub struct OnlineValidator {
    /// The L2 provider.
    provider: ReqwestProvider,
    /// The canyon activation timestamp.
    canyon_activation: u64,
}

impl OnlineValidator {
    /// Creates a new `OnlineValidator`.
    pub fn new(provider: ReqwestProvider, canyon: u64) -> Self {
        Self { provider, canyon_activation: canyon }
    }

    /// Creates a new [OnlineValidator] from the provided [reqwest::Url].
    pub fn new_http(url: reqwest::Url, canyon: u64) -> Self {
        let inner = ReqwestProvider::new_http(url);
        Self::new(inner, canyon)
    }

    /// Fetches a block [Header] and a list of raw RLP encoded transactions from the L2 provider.
    ///
    /// This method needs to fetch the non-hydrated block and then
    /// fetch the raw transactions using the `debug_*` namespace.
    pub(crate) async fn get_block(
        &self,
        tag: BlockNumberOrTag,
    ) -> Result<(Header, Vec<RawTransaction>)> {
        // Don't hydrate the block so we only get a list of transaction hashes.
        let block = self
            .provider
            .get_block(tag.into(), BlockTransactionsKind::Hashes)
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .ok_or(anyhow::anyhow!("Block not found"))?;
        // For each transaction hash, fetch the raw transaction RLP.
        let mut txs = vec![];
        for tx in block.transactions.hashes() {
            let tx: TransportResult<RawTransaction> =
                self.provider.raw_request("debug_getRawTransaction".into(), [tx]).await;
            if let Ok(tx) = tx {
                txs.push(tx);
            } else {
                warn!("Failed to fetch transaction: {:?}", tx);
            }
        }
        Ok((block.header, txs))
    }

    /// Gets the payload for the specified [BlockNumberOrTag].
    pub(crate) async fn get_payload(&self, tag: BlockNumberOrTag) -> Result<L2PayloadAttributes> {
        let (header, transactions) = self.get_block(tag).await?;
        Ok(L2PayloadAttributes {
            timestamp: header.timestamp,
            prev_randao: header.mix_hash.unwrap_or_default(),
            fee_recipient: header.miner,
            // Withdrawals on optimism are always empty, *after* canyon (Shanghai) activation
            withdrawals: (header.timestamp >= self.canyon_activation).then_some(Vec::default()),
            parent_beacon_block_root: header.parent_beacon_block_root,
            transactions,
            no_tx_pool: true,
            gas_limit: Some(header.gas_limit as u64),
        })
    }

    /// Validates the given [`L2AttributesWithParent`].
    pub async fn validate(&self, attributes: &L2AttributesWithParent) -> bool {
        let expected = attributes.parent.block_info.number + 1;
        let tag = BlockNumberOrTag::from(expected);
        let payload = self.get_payload(tag).await.unwrap();
        tracing::debug!("Check payload against: {:?}", payload);
        attributes.attributes == payload
    }
}
