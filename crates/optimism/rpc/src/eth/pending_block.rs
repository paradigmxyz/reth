//! Loads OP pending block for a RPC response.

use crate::OpEthApi;
use alloy_consensus::{
    constants::EMPTY_WITHDRAWALS, proofs::calculate_transaction_root, Header, EMPTY_OMMER_ROOT_HASH,
};
use alloy_eips::{eip7685::EMPTY_REQUESTS_HASH, merge::BEACON_NONCE, BlockNumberOrTag};
use alloy_primitives::{B256, U256};
use op_alloy_network::Network;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_evm::ConfigureEvm;
use reth_optimism_consensus::calculate_receipt_root_no_memo_optimism;
use reth_primitives::{logs_bloom, BlockBody, Receipt, SealedBlockWithSenders, TransactionSigned};
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, ProviderBlock, ProviderHeader,
    ProviderReceipt, ProviderTx, ReceiptProvider, StateProviderFactory,
};
use reth_rpc_eth_api::{
    helpers::{LoadPendingBlock, SpawnBlocking},
    EthApiTypes, FromEthApiError, RpcNodeCore,
};
use reth_rpc_eth_types::{EthApiError, PendingBlock};
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use revm::primitives::{BlockEnv, ExecutionResult};

impl<N> LoadPendingBlock for OpEthApi<N>
where
    Self: SpawnBlocking
        + EthApiTypes<
            NetworkTypes: Network<
                HeaderResponse = alloy_rpc_types_eth::Header<ProviderHeader<Self::Provider>>,
            >,
        >,
    N: RpcNodeCore<
        Provider: BlockReaderIdExt<
            Transaction = reth_primitives::TransactionSigned,
            Block = reth_primitives::Block,
            Receipt = reth_primitives::Receipt,
            Header = reth_primitives::Header,
        > + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                      + StateProviderFactory,
        Pool: TransactionPool<Transaction: PoolTransaction<Consensus = ProviderTx<N::Provider>>>,
        Evm: ConfigureEvm<Header = Header, Transaction = TransactionSigned>,
    >,
{
    #[inline]
    fn pending_block(
        &self,
    ) -> &tokio::sync::Mutex<
        Option<PendingBlock<ProviderBlock<Self::Provider>, ProviderReceipt<Self::Provider>>>,
    > {
        self.inner.eth_api.pending_block()
    }

    /// Returns the locally built pending block
    async fn local_pending_block(
        &self,
    ) -> Result<Option<(SealedBlockWithSenders, Vec<Receipt>)>, Self::Error> {
        // See: <https://github.com/ethereum-optimism/op-geth/blob/f2e69450c6eec9c35d56af91389a1c47737206ca/miner/worker.go#L367-L375>
        let latest = self
            .provider()
            .latest_header()
            .map_err(Self::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(BlockNumberOrTag::Latest.into()))?;
        let block_id = latest.hash().into();
        let block = self
            .provider()
            .block_with_senders(block_id, Default::default())
            .map_err(Self::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(block_id.into()))?
            .seal_unchecked(latest.hash());

        let receipts = self
            .provider()
            .receipts_by_block(block_id)
            .map_err(Self::Error::from_eth_err)?
            .ok_or(EthApiError::ReceiptsNotFound(block_id.into()))?;

        Ok(Some((block, receipts)))
    }

    fn assemble_block(
        &self,
        block_env: &BlockEnv,
        parent_hash: B256,
        state_root: B256,
        transactions: Vec<ProviderTx<Self::Provider>>,
        receipts: &[ProviderReceipt<Self::Provider>],
    ) -> reth_provider::ProviderBlock<Self::Provider> {
        let chain_spec = self.provider().chain_spec();
        let timestamp = block_env.timestamp.to::<u64>();

        let transactions_root = calculate_transaction_root(&transactions);
        let receipts_root = calculate_receipt_root_no_memo_optimism(
            &receipts.iter().collect::<Vec<_>>(),
            &chain_spec,
            timestamp,
        );

        let logs_bloom = logs_bloom(receipts.iter().flat_map(|r| &r.logs));
        let is_cancun = chain_spec.is_cancun_active_at_timestamp(timestamp);
        let is_prague = chain_spec.is_prague_active_at_timestamp(timestamp);
        let is_shanghai = chain_spec.is_shanghai_active_at_timestamp(timestamp);

        let header = Header {
            parent_hash,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: block_env.coinbase,
            state_root,
            transactions_root,
            receipts_root,
            withdrawals_root: (is_shanghai).then_some(EMPTY_WITHDRAWALS),
            logs_bloom,
            timestamp,
            mix_hash: block_env.prevrandao.unwrap_or_default(),
            nonce: BEACON_NONCE.into(),
            base_fee_per_gas: Some(block_env.basefee.to::<u64>()),
            number: block_env.number.to::<u64>(),
            gas_limit: block_env.gas_limit.to::<u64>(),
            difficulty: U256::ZERO,
            gas_used: receipts.last().map(|r| r.cumulative_gas_used).unwrap_or_default(),
            blob_gas_used: is_cancun.then(|| {
                transactions.iter().map(|tx| tx.blob_gas_used().unwrap_or_default()).sum::<u64>()
            }),
            excess_blob_gas: block_env.get_blob_excess_gas().map(Into::into),
            extra_data: Default::default(),
            parent_beacon_block_root: is_cancun.then_some(B256::ZERO),
            requests_hash: is_prague.then_some(EMPTY_REQUESTS_HASH),
            target_blobs_per_block: None,
        };

        // seal the block
        reth_primitives::Block {
            header,
            body: BlockBody { transactions, ommers: vec![], withdrawals: None },
        }
    }

    fn assemble_receipt(
        &self,
        tx: &ProviderTx<Self::Provider>,
        result: ExecutionResult,
        cumulative_gas_used: u64,
    ) -> reth_provider::ProviderReceipt<Self::Provider> {
        #[allow(clippy::needless_update)]
        Receipt {
            tx_type: tx.tx_type(),
            success: result.is_success(),
            cumulative_gas_used,
            logs: result.into_logs().into_iter().map(Into::into).collect(),
            ..Default::default()
        }
    }
}
