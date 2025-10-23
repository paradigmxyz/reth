use super::{OpEthApi, OpNodeCore};
use alloy_consensus::{BlockHeader, Transaction, TxReceipt};
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::U256;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_primitives_traits::{Block, BlockBody};
use reth_rpc_eth_api::{
    helpers::{EthFees, LoadBlock, LoadFee},
    FromEthApiError,
};
use reth_rpc_eth_types::{EthApiError, FeeHistoryCache, GasPriceOracle};
use reth_rpc_server_types::constants::gas_oracle::DEFAULT_MIN_SUGGESTED_PRIORITY_FEE;
use reth_storage_api::{BlockReader, BlockReaderIdExt, ReceiptProvider, StateProviderFactory};

impl<N> LoadFee for OpEthApi<N>
where
    Self: LoadBlock<Provider = N::Provider>,
    N: OpNodeCore<
        Provider: BlockReaderIdExt
                      + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                      + StateProviderFactory,
    >,
{
    #[inline]
    fn gas_oracle(&self) -> &GasPriceOracle<Self::Provider> {
        self.inner.eth_api.gas_oracle()
    }

    #[inline]
    fn fee_history_cache(&self) -> &FeeHistoryCache {
        self.inner.eth_api.fee_history_cache()
    }

    /// Optimism-specific priority fee suggestion
    async fn suggested_priority_fee(&self) -> Result<U256, Self::Error>
    where
        Self: 'static,
    {
        // Delegate to the Optimism-specific implementation that mirrors op-geth's SuggestOptimismPriorityFee
        self.suggest_optimism_priority_fee().await
    }
}

impl<N> EthFees for OpEthApi<N>
where
    Self: LoadFee,
    N: OpNodeCore,
{
}

impl<N> OpEthApi<N>
where
    N: OpNodeCore<
        Provider: BlockReaderIdExt
                      + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                      + StateProviderFactory,
    >,
{
    /// Optimism-specific gas price suggestion algorithm
    ///
    /// This implements the same algorithm as op-geth's `SuggestOptimismPriorityFee`:
    /// 1. Start with minimum suggested priority fee from config (default 0.0001 gwei)
    /// 2. Check if the last block is at capacity
    /// 3. If at capacity, return median + 10% of previous block's effective tips
    /// 4. Otherwise, return the minimum suggestion
    async fn suggest_optimism_priority_fee(
        &self,
    ) -> Result<U256, <Self as reth_rpc_eth_api::EthApiTypes>::Error> {
        let min_suggestion = self
            .inner
            .eth_api
            .gas_oracle()
            .config()
            .min_suggested_priority_fee
            .unwrap_or(DEFAULT_MIN_SUGGESTED_PRIORITY_FEE);

        // Fetch the latest block header
        let header = self
            .inner
            .eth_api
            .provider()
            .latest_header()
            .map_err(<Self as reth_rpc_eth_api::EthApiTypes>::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(BlockNumberOrTag::Latest.into()))?;

        // Check if block is at capacity
        let receipts = self
            .inner
            .eth_api
            .provider()
            .receipts_by_block(alloy_eips::HashOrNumber::Hash(header.hash()))
            .map_err(<Self as reth_rpc_eth_api::EthApiTypes>::Error::from_eth_err)?;

        // Calculate suggestion based on receipts, min suggestion if None
        let suggestion = receipts
            .and_then(|receipts| {
                // Calculate max gas usage per transaction
                let max_tx_gas = receipts
                    .windows(2)
                    .map(|w| w[1].cumulative_gas_used() - w[0].cumulative_gas_used())
                    .chain(receipts.first().map(|r| r.cumulative_gas_used()).into_iter())
                    .max()
                    .unwrap_or(0);

                // Sanity check the max gas used value
                if max_tx_gas > header.gas_limit() {
                    tracing::error!(
                        "found tx consuming more gas than the block limit: {}",
                        max_tx_gas
                    );
                    return None;
                }

                // Check if block is at capacity, if not, return None
                if header.gas_used() + max_tx_gas <= header.gas_limit() {
                    return None;
                }

                tracing::debug!("Block is at capacity, calculating median + 10%");

                // Get block for tip calculation
                let block = match self
                    .inner
                    .eth_api
                    .provider()
                    .block_by_hash(header.hash())
                    .map_err(<Self as reth_rpc_eth_api::EthApiTypes>::Error::from_eth_err)
                {
                    Ok(Some(block)) => block,
                    Ok(None) | Err(_) => return None,
                };

                let base_fee = block.header().base_fee_per_gas().unwrap_or_default();
                let mut tips: Vec<U256> = block
                    .body()
                    .transactions_iter()
                    .filter_map(|tx| tx.effective_tip_per_gas(base_fee).map(U256::from))
                    .collect();

                if tips.is_empty() {
                    tracing::error!("block was at capacity but doesn't have transactions");
                    None
                } else {
                    tips.sort_unstable();
                    let median = tips[tips.len() / 2];
                    let new_suggestion = median + median / U256::from(10);
                    Some(new_suggestion.max(min_suggestion))
                }
            })
            .unwrap_or(min_suggestion);

        // The suggestion should be capped by oracle.maxPrice
        let final_suggestion = self
            .inner
            .eth_api
            .gas_oracle()
            .config()
            .max_price
            .map(|max_price| suggestion.min(max_price))
            .unwrap_or(suggestion);

        Ok(final_suggestion)
    }
}
