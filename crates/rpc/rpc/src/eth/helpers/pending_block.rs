//! Support for building a pending block with transactions from local view of mempool.

use alloy_consensus::{
    constants::EMPTY_WITHDRAWALS, transaction::Recovered, Header, Transaction,
    EMPTY_OMMER_ROOT_HASH,
};
use alloy_eips::{eip7685::EMPTY_REQUESTS_HASH, merge::BEACON_NONCE};
use alloy_primitives::U256;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_evm::{ConfigureEvm, HaltReasonFor};
use reth_primitives::{logs_bloom, BlockBody, Receipt};
use reth_primitives_traits::proofs::calculate_transaction_root;
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, ProviderBlock, ProviderReceipt, ProviderTx,
    StateProviderFactory,
};
use reth_rpc_eth_api::{
    helpers::{LoadPendingBlock, SpawnBlocking},
    types::RpcTypes,
    FromEvmError, RpcNodeCore,
};
use reth_rpc_eth_types::PendingBlock;
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use revm::{
    context::BlockEnv,
    context_interface::{result::ExecutionResult, Block},
};
use revm_primitives::B256;

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> LoadPendingBlock
    for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: SpawnBlocking<
            NetworkTypes: RpcTypes<Header = alloy_rpc_types_eth::Header>,
            Error: FromEvmError<Self::Evm>,
        > + RpcNodeCore<
            Provider: BlockReaderIdExt<
                Transaction = reth_primitives::TransactionSigned,
                Block = reth_primitives::Block,
                Receipt = reth_primitives::Receipt,
                Header = reth_primitives::Header,
            > + ChainSpecProvider<ChainSpec: EthChainSpec + EthereumHardforks>
                          + StateProviderFactory,
            Pool: TransactionPool<
                Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>,
            >,
            Evm: ConfigureEvm<Header = Header, Transaction = ProviderTx<Self::Provider>>,
        >,
    Provider: BlockReader<Block = reth_primitives::Block, Receipt = reth_primitives::Receipt>,
{
    #[inline]
    fn pending_block(
        &self,
    ) -> &tokio::sync::Mutex<
        Option<PendingBlock<ProviderBlock<Self::Provider>, ProviderReceipt<Self::Provider>>>,
    > {
        self.inner.pending_block()
    }

    fn assemble_block(
        &self,
        block_env: &BlockEnv,
        parent_hash: revm_primitives::B256,
        state_root: revm_primitives::B256,
        transactions: Vec<Recovered<ProviderTx<Self::Provider>>>,
        receipts: &[ProviderReceipt<Self::Provider>],
    ) -> reth_provider::ProviderBlock<Self::Provider> {
        let chain_spec = self.provider().chain_spec();

        let transactions_root = calculate_transaction_root(&transactions);
        let receipts_root = Receipt::calculate_receipt_root_no_memo(receipts);

        let logs_bloom = logs_bloom(receipts.iter().flat_map(|r| &r.logs));

        let timestamp = block_env.timestamp;
        let is_shanghai = chain_spec.is_shanghai_active_at_timestamp(timestamp);
        let is_cancun = chain_spec.is_cancun_active_at_timestamp(timestamp);
        let is_prague = chain_spec.is_prague_active_at_timestamp(timestamp);

        let header = Header {
            parent_hash,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: block_env.beneficiary,
            state_root,
            transactions_root,
            receipts_root,
            withdrawals_root: is_shanghai.then_some(EMPTY_WITHDRAWALS),
            logs_bloom,
            timestamp: block_env.timestamp,
            mix_hash: block_env.prevrandao.unwrap_or_default(),
            nonce: BEACON_NONCE.into(),
            base_fee_per_gas: Some(block_env.basefee),
            number: block_env.number,
            gas_limit: block_env.gas_limit,
            difficulty: U256::ZERO,
            gas_used: receipts.last().map(|r| r.cumulative_gas_used).unwrap_or_default(),
            blob_gas_used: is_cancun.then(|| {
                transactions.iter().map(|tx| tx.blob_gas_used().unwrap_or_default()).sum::<u64>()
            }),
            excess_blob_gas: block_env.blob_excess_gas(),
            extra_data: Default::default(),
            parent_beacon_block_root: is_cancun.then_some(B256::ZERO),
            requests_hash: is_prague.then_some(EMPTY_REQUESTS_HASH),
        };

        // seal the block
        reth_primitives::Block {
            header,
            body: BlockBody {
                transactions: transactions.into_iter().map(|tx| tx.into_tx()).collect(),
                ommers: vec![],
                withdrawals: None,
            },
        }
    }

    fn assemble_receipt(
        &self,
        tx: &ProviderTx<Self::Provider>,
        result: ExecutionResult<HaltReasonFor<Self::Evm>>,
        cumulative_gas_used: u64,
    ) -> reth_provider::ProviderReceipt<Self::Provider> {
        Receipt {
            tx_type: tx.tx_type(),
            success: result.is_success(),
            cumulative_gas_used,
            logs: result.into_logs().into_iter().collect(),
        }
    }
}
