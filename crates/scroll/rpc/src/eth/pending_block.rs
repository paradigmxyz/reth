//! Loads Scroll pending block for an RPC response.

use alloy_consensus::{
    constants::EMPTY_WITHDRAWALS, proofs::calculate_transaction_root, Header, Transaction,
    TxReceipt, EMPTY_OMMER_ROOT_HASH,
};
use alloy_eips::{eip7685::EMPTY_REQUESTS_HASH, merge::BEACON_NONCE};
use alloy_primitives::{logs_bloom, B256, U256};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_evm::ConfigureEvm;
use reth_primitives::BlockBody;
use reth_provider::{
    BlockReaderIdExt, ChainSpecProvider, ProviderBlock, ProviderHeader, ProviderReceipt,
    ProviderTx, StateProviderFactory,
};
use reth_rpc_eth_api::{
    helpers::{LoadPendingBlock, SpawnBlocking},
    EthApiTypes, RpcNodeCore,
};
use reth_rpc_eth_types::{error::FromEvmError, PendingBlock};
use reth_scroll_forks::ScrollHardforks;
use reth_scroll_primitives::{ScrollBlock, ScrollReceipt, ScrollTransactionSigned};
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use revm::primitives::{BlockEnv, ExecutionResult};
use scroll_alloy_consensus::{ScrollTransactionReceipt, ScrollTxType};
use scroll_alloy_network::Network;

use crate::ScrollEthApi;

impl<N> LoadPendingBlock for ScrollEthApi<N>
where
    Self: SpawnBlocking
        + EthApiTypes<
            NetworkTypes: Network<
                HeaderResponse = alloy_rpc_types_eth::Header<ProviderHeader<Self::Provider>>,
            >,
            Error: FromEvmError<Self::Evm>,
        >,
    N: RpcNodeCore<
        Provider: BlockReaderIdExt<
            Transaction = ScrollTransactionSigned,
            Block = ScrollBlock,
            Receipt = ScrollReceipt,
            Header = reth_primitives::Header,
        > + ChainSpecProvider<ChainSpec: EthChainSpec + ScrollHardforks>
                      + StateProviderFactory,
        Pool: TransactionPool<Transaction: PoolTransaction<Consensus = ProviderTx<N::Provider>>>,
        Evm: ConfigureEvm<
            Header = ProviderHeader<Self::Provider>,
            Transaction = ProviderTx<Self::Provider>,
        >,
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

    fn assemble_block(
        &self,
        block_env: &BlockEnv,
        parent_hash: B256,
        state_root: B256,
        transactions: Vec<ProviderTx<Self::Provider>>,
        receipts: &[ProviderReceipt<Self::Provider>],
    ) -> ProviderBlock<Self::Provider> {
        let chain_spec = self.provider().chain_spec();
        let timestamp = block_env.timestamp.to::<u64>();

        let transactions_root = calculate_transaction_root(&transactions);
        let receipts_root = ProviderReceipt::<Self::Provider>::calculate_receipt_root_no_memo(
            &receipts.iter().collect::<Vec<_>>(),
        );

        let logs_bloom = logs_bloom(receipts.iter().flat_map(|r| r.logs()));
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
            gas_used: receipts.last().map(|r| r.cumulative_gas_used()).unwrap_or_default(),
            blob_gas_used: is_cancun.then(|| {
                transactions.iter().map(|tx| tx.blob_gas_used().unwrap_or_default()).sum::<u64>()
            }),
            excess_blob_gas: block_env.get_blob_excess_gas(),
            extra_data: Default::default(),
            parent_beacon_block_root: is_cancun.then_some(B256::ZERO),
            requests_hash: is_prague.then_some(EMPTY_REQUESTS_HASH),
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
    ) -> ProviderReceipt<Self::Provider> {
        let receipt = alloy_consensus::Receipt {
            status: result.is_success().into(),
            cumulative_gas_used,
            logs: result.into_logs().into_iter().collect(),
        };

        let into_scroll_receipt =
            |inner: alloy_consensus::Receipt| ScrollTransactionReceipt::new(inner, U256::ZERO);
        match tx.tx_type() {
            ScrollTxType::Legacy => ScrollReceipt::Legacy(into_scroll_receipt(receipt)),
            ScrollTxType::Eip2930 => ScrollReceipt::Eip2930(into_scroll_receipt(receipt)),
            ScrollTxType::Eip1559 => ScrollReceipt::Eip1559(into_scroll_receipt(receipt)),
            ScrollTxType::L1Message => ScrollReceipt::L1Message(receipt),
        }
    }
}
