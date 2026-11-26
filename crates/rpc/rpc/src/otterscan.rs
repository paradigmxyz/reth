use crate::trace::{base_block_reward, block_reward, ommer_reward};
use alloy_consensus::{BlockHeader, Typed2718};
use alloy_eips::{eip1898::LenientBlockNumberOrTag, BlockId};
use alloy_network::{ReceiptResponse, TransactionResponse};
use alloy_primitives::{Address, Bytes, Sealable, TxHash, B256, U256};
use alloy_rpc_types_eth::{BlockTransactions, TransactionReceipt};
use alloy_rpc_types_trace::{
    otterscan::{
        BlockDetails, ContractCreator, InternalIssuance, InternalOperation, OtsBlockTransactions,
        OtsReceipt, OtsSlimBlock, OtsTransactionReceipt, TraceEntry, TransactionsWithReceipts,
    },
    parity::{Action, CreateAction, CreateOutput, LocalizedTransactionTrace, TraceOutput},
};
use evm2::interpreter::InstrStop;
use evm2_inspectors::{
    otterscan::InternalOperationsInspector,
    tracing::{
        types::{CallKind, CallTraceNode},
        TracingInspectorConfig,
    },
};
use jsonrpsee::{core::RpcResult, types::ErrorObjectOwned};
use reth_chainspec::ChainSpecProvider;
use reth_primitives_traits::{BlockBody, TxTy};
use reth_rpc_api::{EthApiServer, OtterscanServer};
use reth_rpc_convert::{RpcConvert, RpcTxReq};
use reth_rpc_eth_api::{
    helpers::{EthBlocks, EthTransactions, TraceExt},
    RpcBlock, RpcHeader, RpcReceipt, RpcTransaction,
};
use reth_rpc_eth_types::{block::BlockAndReceipts, utils::binary_search, EthApiError};
use reth_rpc_server_types::result::internal_rpc_err;
use tokio::sync::OwnedSemaphorePermit;

const API_LEVEL: u64 = 8;

/// Otterscan API.
#[derive(Debug)]
pub struct OtterscanApi<Eth> {
    eth: Eth,
}

impl<Eth> OtterscanApi<Eth> {
    /// Creates a new instance of `Otterscan`.
    pub const fn new(eth: Eth) -> Self {
        Self { eth }
    }
}

impl<Eth> OtterscanApi<Eth>
where
    Eth: TraceExt + EthBlocks,
{
    /// Loads a matching pair, including locally built pending blocks.
    async fn block_and_receipts(
        &self,
        block_id: BlockId,
    ) -> RpcResult<BlockAndReceipts<Eth::Primitives>> {
        // Missing blocks and missing receipts share the same RPC error for the requested ID.
        let (block, receipts) = self
            .eth
            .load_block_and_receipts(block_id)
            .await
            .map_err(Into::<ErrorObjectOwned>::into)?
            .ok_or(EthApiError::HeaderNotFound(block_id))?;
        Ok(BlockAndReceipts { block, receipts })
    }

    /// Constructs the block details from a single resolution of the requested block.
    async fn block_details(
        &self,
        block_id: BlockId,
    ) -> RpcResult<BlockDetails<RpcHeader<Eth::NetworkTypes>>> {
        let BlockAndReceipts { block, receipts } = self.block_and_receipts(block_id).await?;
        let receipts = self
            .eth
            .convert_block_receipts(&block, receipts)
            .map_err(Into::<ErrorObjectOwned>::into)?;
        let chain_spec = self.eth.provider().chain_spec();
        let reward =
            if block.number() == 0 { None } else { base_block_reward(&chain_spec, block.number()) };
        let ommers = block.body().ommers().unwrap_or_default();
        let issuance = reward.map_or_else(InternalIssuance::default, |reward| {
            calculate_issuance(reward, block.number(), ommers.iter().map(BlockHeader::number))
        });
        let header = self
            .eth
            .converter()
            .convert_header(block.clone_sealed_header(), Some(block.rlp_length()))
            .map_err(|err| Eth::Error::from(err).into())?;
        let block = OtsSlimBlock {
            header,
            uncles: ommers.iter().map(Sealable::hash_slow).collect(),
            withdrawals: block.body().withdrawals().cloned(),
            transaction_count: block.transaction_count(),
        };
        Ok(BlockDetails { block, issuance, total_fees: total_fees(&receipts) })
    }

    /// Acquires a permit to execute a tracing call.
    ///
    /// The permit should be moved into the spawned trace so it is held until the trace completes.
    async fn acquire_trace_permit(&self) -> RpcResult<OwnedSemaphorePermit> {
        Ok(self.eth.acquire_owned_tracing().await.map_err(|_| EthApiError::InternalEthError)?)
    }
}

impl<Eth> OtterscanServer<RpcTransaction<Eth::NetworkTypes>, RpcHeader<Eth::NetworkTypes>>
    for OtterscanApi<Eth>
where
    Eth: EthApiServer<
            RpcTxReq<Eth::NetworkTypes>,
            RpcTransaction<Eth::NetworkTypes>,
            RpcBlock<Eth::NetworkTypes>,
            RpcReceipt<Eth::NetworkTypes>,
            RpcHeader<Eth::NetworkTypes>,
            TxTy<Eth::Primitives>,
        > + EthTransactions
        + TraceExt
        + EthBlocks,
{
    /// Handler for `ots_getHeaderByNumber` and `erigon_getHeaderByNumber`
    async fn get_header_by_number(
        &self,
        block_number: LenientBlockNumberOrTag,
    ) -> RpcResult<Option<RpcHeader<Eth::NetworkTypes>>> {
        self.eth.header_by_number(block_number.into()).await
    }

    /// Handler for `ots_hasCode`
    async fn has_code(&self, address: Address, block_id: Option<BlockId>) -> RpcResult<bool> {
        EthApiServer::get_code(&self.eth, address, block_id).await.map(|code| !code.is_empty())
    }

    /// Handler for `ots_getApiLevel`
    async fn get_api_level(&self) -> RpcResult<u64> {
        Ok(API_LEVEL)
    }

    /// Handler for `ots_getInternalOperations`
    async fn get_internal_operations(&self, tx_hash: TxHash) -> RpcResult<Vec<InternalOperation>> {
        let permit = self.acquire_trace_permit().await?;
        self.eth
            .spawn_trace_transaction_in_block_with_inspector(
                tx_hash,
                InternalOperationsInspector::default(),
                move |_tx_info, inspector, _, _| {
                    let _permit = permit;
                    Ok(inspector.into_operations())
                },
            )
            .await
            .map_err(Into::into)
            .map(Option::unwrap_or_default)
    }

    /// Handler for `ots_getTransactionError`
    async fn get_transaction_error(&self, tx_hash: TxHash) -> RpcResult<Option<Bytes>> {
        let permit = self.acquire_trace_permit().await?;
        self.eth
            .spawn_replay_transaction(tx_hash, move |_tx_info, res, _| {
                let _permit = permit;
                Ok(transaction_error(res.result))
            })
            .await
            .map_err(Into::into)
    }

    /// Handler for `ots_traceTransaction`
    async fn trace_transaction(&self, tx_hash: TxHash) -> RpcResult<Option<Vec<TraceEntry>>> {
        let permit = self.acquire_trace_permit().await?;
        self.eth
            .spawn_trace_transaction_in_block(
                tx_hash,
                TracingInspectorConfig::default_parity(),
                move |_tx_info, inspector, _, _| {
                    let _permit = permit;
                    Ok(otterscan_traces(inspector.into_traces().into_nodes()))
                },
            )
            .await
            .map_err(Into::into)
    }

    /// Handler for `ots_getBlockDetails`
    async fn get_block_details(
        &self,
        block_number: LenientBlockNumberOrTag,
    ) -> RpcResult<BlockDetails<RpcHeader<Eth::NetworkTypes>>> {
        self.block_details(block_number.into_inner().into()).await
    }

    /// Handler for `ots_getBlockDetailsByHash`
    async fn get_block_details_by_hash(
        &self,
        block_hash: B256,
    ) -> RpcResult<BlockDetails<RpcHeader<Eth::NetworkTypes>>> {
        self.block_details(block_hash.into()).await
    }

    /// Handler for `ots_getBlockTransactions`
    async fn get_block_transactions(
        &self,
        block_number: LenientBlockNumberOrTag,
        page_number: usize,
        page_size: usize,
    ) -> RpcResult<
        OtsBlockTransactions<RpcTransaction<Eth::NetworkTypes>, RpcHeader<Eth::NetworkTypes>>,
    > {
        let BlockAndReceipts { block, receipts } =
            self.block_and_receipts(block_number.into_inner().into()).await?;
        let mut receipts = self
            .eth
            .convert_block_receipts(&block, receipts)
            .map_err(Into::<ErrorObjectOwned>::into)?;
        let mut block =
            self.eth.convert_block(&block, true).map_err(Into::<ErrorObjectOwned>::into)?;

        // check if the number of transactions matches the number of receipts
        let tx_len = block.transactions.len();
        if tx_len != receipts.len() {
            return Err(internal_rpc_err(
                "the number of transactions does not match the number of receipts",
            ))
        }

        // make sure the block is full
        let BlockTransactions::Full(transactions) = &mut block.transactions else {
            return Err(internal_rpc_err("block is not full"));
        };

        let page = block_transaction_page_range(tx_len, page_number, page_size);

        // Crop transactions
        *transactions = transactions.drain(page.clone()).collect::<Vec<_>>();

        // Crop receipts and transform them into OtsTransactionReceipt
        let timestamp = Some(block.header.timestamp());
        let receipts = receipts
            .drain(page)
            .zip(transactions.iter().map(Typed2718::ty))
            .map(|(receipt, tx_ty)| {
                let inner = OtsReceipt {
                    status: receipt.status(),
                    cumulative_gas_used: receipt.cumulative_gas_used(),
                    logs: None,
                    logs_bloom: None,
                    r#type: tx_ty,
                };

                let receipt = TransactionReceipt {
                    inner,
                    transaction_hash: receipt.transaction_hash(),
                    transaction_index: receipt.transaction_index(),
                    block_hash: receipt.block_hash(),
                    block_number: receipt.block_number(),
                    gas_used: receipt.gas_used(),
                    effective_gas_price: receipt.effective_gas_price(),
                    blob_gas_used: receipt.blob_gas_used(),
                    blob_gas_price: receipt.blob_gas_price(),
                    from: receipt.from(),
                    to: receipt.to(),
                    contract_address: receipt.contract_address(),
                };

                OtsTransactionReceipt { receipt, timestamp }
            })
            .collect();

        // use `transaction_count` to indicate the paginate information
        let mut block = OtsBlockTransactions { fullblock: block.into(), receipts };
        block.fullblock.transaction_count = tx_len;
        Ok(block)
    }

    /// Handler for `ots_searchTransactionsBefore`
    async fn search_transactions_before(
        &self,
        _address: Address,
        _block_number: LenientBlockNumberOrTag,
        _page_size: usize,
    ) -> RpcResult<TransactionsWithReceipts> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `ots_searchTransactionsAfter`
    async fn search_transactions_after(
        &self,
        _address: Address,
        _block_number: LenientBlockNumberOrTag,
        _page_size: usize,
    ) -> RpcResult<TransactionsWithReceipts> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `ots_getTransactionBySenderAndNonce`
    async fn get_transaction_by_sender_and_nonce(
        &self,
        sender: Address,
        nonce: u64,
    ) -> RpcResult<Option<TxHash>> {
        Ok(self
            .eth
            .get_transaction_by_sender_and_nonce(sender, nonce, false)
            .await
            .map_err(Into::into)?
            .map(|tx| tx.tx_hash()))
    }

    /// Handler for `ots_getContractCreator`
    async fn get_contract_creator(&self, address: Address) -> RpcResult<Option<ContractCreator>> {
        if !self.has_code(address, None).await? {
            return Ok(None);
        }

        let permit = self.acquire_trace_permit().await?;
        let num = binary_search::<_, _, ErrorObjectOwned>(
            1,
            self.eth.block_number()?.saturating_to(),
            |mid| {
                Box::pin(async move {
                    Ok(!EthApiServer::get_code(&self.eth, address, Some(mid.into()))
                        .await?
                        .is_empty())
                })
            },
        )
        .await?;

        let traces = self
            .eth
            .trace_block_with(
                num.into(),
                None,
                TracingInspectorConfig::default_parity(),
                move |tx_info, ctx| {
                    let _permit = &permit;
                    Ok(ctx
                        .take_inspector()
                        .into_parity_builder()
                        .into_localized_transaction_traces(tx_info))
                },
            )
            .await
            .map_err(Into::into)?
            .map(|traces| find_contract_creator(address, traces))
            .transpose()?;

        // Code-presence search assumes a single deployment. It cannot reliably identify
        // the first deployment of contracts that were destroyed and recreated.
        Ok(traces.flatten())
    }
}

/// Finds a creation that was not rolled back by its own frame or an enclosing frame.
fn find_contract_creator(
    address: Address,
    transactions: Vec<Vec<LocalizedTransactionTrace>>,
) -> Result<Option<ContractCreator>, EthApiError> {
    for traces in transactions {
        let mut reverted_path: Option<Vec<usize>> = None;
        for tx_trace in traces {
            let trace = tx_trace.trace;
            // Parity traces are in preorder, so a failed frame precedes its entire subtree.
            if reverted_path.as_ref().is_some_and(|path| trace.trace_address.starts_with(path)) {
                continue
            }
            reverted_path = None;
            if trace.error.is_some() {
                reverted_path = Some(trace.trace_address);
                continue
            }
            if let (
                Action::Create(CreateAction { from: creator, .. }),
                Some(TraceOutput::Create(CreateOutput { address: contract, .. })),
            ) = (trace.action, trace.result) &&
                contract == address
            {
                return Ok(Some(ContractCreator {
                    hash: tx_trace.transaction_hash.ok_or(EthApiError::TransactionNotFound)?,
                    creator,
                }))
            }
        }
    }
    Ok(None)
}

/// Returns the transaction slice for an Otterscan block page.
///
/// Pages are selected from the end of the block, retaining block order within each page.
/// The frontend reverses each page and uses this ordering for transaction-index links.
const fn block_transaction_page_range(
    tx_len: usize,
    page_number: usize,
    page_size: usize,
) -> std::ops::Range<usize> {
    let page_end = tx_len.saturating_sub(page_number.saturating_mul(page_size));
    let page_start = page_end.saturating_sub(page_size);
    page_start..page_end
}

/// Rewards issued to the miner (including ommer inclusion) and to the ommers themselves.
fn calculate_issuance(
    reward: u128,
    number: u64,
    ommer_numbers: impl ExactSizeIterator<Item = u64>,
) -> InternalIssuance {
    let block_reward = U256::from(block_reward(reward, ommer_numbers.len()));
    let uncle_reward =
        ommer_numbers.map(|ommer| U256::from(ommer_reward(reward, number, ommer))).sum::<U256>();
    InternalIssuance { block_reward, uncle_reward, issuance: block_reward + uncle_reward }
}

/// Execution fees include the base fee and exclude blob fees.
fn total_fees(receipts: &[impl ReceiptResponse]) -> U256 {
    receipts
        .iter()
        .map(|receipt| U256::from(receipt.gas_used()) * U256::from(receipt.effective_gas_price()))
        .sum()
}

fn transaction_error<E>(result: evm2::evm::TxResultExt<E>) -> Bytes {
    if !result.status && result.stop == InstrStop::Revert {
        result.output
    } else {
        Bytes::new()
    }
}

/// A self-destruct is a separate operation at the end of its enclosing call, after any children.
fn otterscan_traces(nodes: Vec<CallTraceNode>) -> Vec<TraceEntry> {
    let mut entries = Vec::with_capacity(nodes.len());
    let mut selfdestructs: Vec<TraceEntry> = Vec::new();
    for CallTraceNode { trace, .. } in nodes {
        // The arena is in call-entry order. Emit completed calls' self-destructs before
        // entering a sibling or returning to an ancestor.
        while selfdestructs.last().is_some_and(|entry| entry.depth > trace.depth as u32) {
            entries.push(selfdestructs.pop().expect("checked above"));
        }
        if let Some(to) = trace.selfdestruct_refund_target() {
            selfdestructs.push(TraceEntry {
                r#type: "SELFDESTRUCT".to_string(),
                depth: trace.depth as u32 + 1,
                from: trace.selfdestruct_address().unwrap_or(trace.address),
                to,
                value: trace.selfdestruct_transferred_value(),
                input: Bytes::new(),
                output: Bytes::new(),
            });
        }
        entries.push(TraceEntry {
            r#type: trace.kind.to_string(),
            depth: trace.depth as u32,
            from: trace.caller,
            to: trace.address,
            value: (!matches!(trace.kind, CallKind::StaticCall | CallKind::DelegateCall))
                .then_some(trace.value),
            input: trace.data,
            output: trace.output,
        });
    }
    entries.extend(selfdestructs.into_iter().rev());
    entries
}

#[cfg(test)]
mod tests {

    use super::*;
    use alloy_consensus::{
        constants::ETH_TO_WEI, proofs::calculate_transaction_root, Header, TxLegacy,
    };
    use alloy_eips::{eip4895::Withdrawal, BlockNumHash, BlockNumberOrTag};
    use alloy_primitives::{bytes, Signature, TxKind};
    use alloy_rpc_types_trace::parity::TransactionTrace;
    use evm2::{
        bytecode::Bytecode,
        evm::{AccountInfo, InMemoryDB},
        SpecId,
    };
    use evm2_inspectors::tracing::{
        types::{CallTrace, SelfdestructTrace},
        TracingInspector,
    };
    use reth_chain_state::ExecutedBlock;
    use reth_chainspec::{ChainInfo, MAINNET, MIN_TRANSACTION_GAS};
    use reth_ethereum_primitives::{Block, Receipt, TransactionSigned};
    use reth_evm::{ConfigureEvm, Evm};
    use reth_evm_ethereum::{EthEvmConfig, EthEvmEnv};
    use reth_network_api::noop::NoopNetwork;
    use reth_primitives_traits::{Block as _, Recovered};
    use reth_provider::test_utils::MockEthProvider;
    use reth_rpc_eth_api::helpers::{LoadPendingBlock, SpawnBlocking};
    use reth_rpc_eth_types::{builder::config::PendingBlockKind, PendingBlock};
    use reth_transaction_pool::test_utils::testing_pool;
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };

    #[test]
    fn block_transaction_pages_match_frontend_index_navigation() {
        // The frontend requests floor((count - 1 - index) / size), then reverses the page.
        for count in [0, 1, 10, 25, 51] {
            for size in [1, 10, 25] {
                for index in 0..count {
                    let reverse_index = count - 1 - index;
                    let page = block_transaction_page_range(count, reverse_index / size, size);
                    let displayed = page.rev().collect::<Vec<_>>();
                    assert_eq!(displayed[reverse_index % size], index);
                }
            }
        }
        assert_eq!(block_transaction_page_range(25, 0, 10), 15..25);
        assert_eq!(block_transaction_page_range(25, 1, 10), 5..15);
        assert_eq!(block_transaction_page_range(25, 2, 10), 0..5);
        assert_eq!(block_transaction_page_range(25, 3, 10), 0..0);
        assert_eq!(block_transaction_page_range(25, usize::MAX, 10), 0..0);
        assert_eq!(block_transaction_page_range(25, 0, usize::MAX), 0..25);
        assert_eq!(block_transaction_page_range(25, 1, usize::MAX), 0..0);
        assert_eq!(block_transaction_page_range(25, 0, 0), 25..25);
    }

    #[test]
    fn transaction_error_returns_only_revert_data() {
        for (status, stop, output) in [
            (
                true,
                evm2::interpreter::InstrStop::Return,
                Bytes::from_static(b"successful return data"),
            ),
            (false, evm2::interpreter::InstrStop::OutOfGas, Bytes::new()),
            (false, evm2::interpreter::InstrStop::Revert, Bytes::new()),
        ] {
            let result =
                evm2::evm::TxResultExt::<()> { status, stop, output, ..Default::default() };
            assert_eq!(serde_json::to_value(Some(transaction_error(result))).unwrap(), "0x");
        }
        let output = Bytes::from_static(b"revert data");
        assert_eq!(
            transaction_error(evm2::evm::TxResultExt::<()> {
                status: false,
                stop: evm2::interpreter::InstrStop::Revert,
                output: output.clone(),
                ..Default::default()
            }),
            output
        );
    }

    #[tokio::test]
    async fn block_details_issuance_follows_chain_forks() {
        for (spec, number, reward) in [
            (0, 0),
            (1, 5),
            (4_369_999, 5),
            (4_370_000, 3),
            (7_279_999, 3),
            (7_280_000, 2),
            (15_537_393, 2),
            (15_537_394, 0),
        ]
        .into_iter()
        .map(|(number, reward)| (MAINNET.clone(), number, reward))
        .chain(std::iter::once((
            std::sync::Arc::new(
                reth_chainspec::ChainSpecBuilder::mainnet().paris_activated().build(),
            ),
            1,
            0,
        ))) {
            let provider = MockEthProvider::default().with_chain_spec((*spec).clone());
            let header = Header { number, ..Default::default() };
            let hash = header.hash_slow();
            let block = reth_ethereum_primitives::Block { header, body: Default::default() };
            provider.add_block(hash, block);
            provider.add_receipts(number, vec![]);
            let api = OtterscanApi::new(
                crate::eth::EthApiBuilder::new(
                    provider,
                    testing_pool(),
                    NoopNetwork::default(),
                    EthEvmConfig::new(spec),
                )
                .build(),
            );
            let by_number = api
                .get_block_details(alloy_eips::BlockNumberOrTag::Number(number).into())
                .await
                .unwrap();
            let by_hash = api.get_block_details_by_hash(hash).await.unwrap();
            assert_eq!(
                by_number.issuance.block_reward,
                U256::from(reward * ETH_TO_WEI),
                "block {number}"
            );
            assert_eq!(by_number.issuance.uncle_reward, U256::ZERO);
            assert_eq!(by_number.issuance.issuance, by_number.issuance.block_reward);
            assert_eq!(by_number.issuance, by_hash.issuance);
        }
    }

    #[tokio::test]
    async fn block_details_loads_ommer_headers() {
        let provider = MockEthProvider::default();
        let block = reth_ethereum_primitives::Block {
            header: Header { number: 126, ..Default::default() },
            body: alloy_consensus::BlockBody {
                ommers: vec![Header { number: 123, ..Default::default() }],
                ..Default::default()
            },
        };
        let hash = block.header.hash_slow();
        provider.add_block(hash, block);
        provider.add_receipts(126, vec![]);
        let api = OtterscanApi::new(
            crate::eth::EthApiBuilder::new(
                provider,
                testing_pool(),
                NoopNetwork::default(),
                EthEvmConfig::new(MAINNET.clone()),
            )
            .build(),
        );
        let expected = calculate_issuance(5 * ETH_TO_WEI, 126, [123].into_iter());
        assert_eq!(
            api.get_block_details(alloy_eips::BlockNumberOrTag::Number(126).into())
                .await
                .unwrap()
                .issuance,
            expected
        );
        assert_eq!(api.get_block_details_by_hash(hash).await.unwrap().issuance, expected);
    }

    #[tokio::test]
    async fn unknown_transactions_and_unimplemented_history_remain_distinct() {
        let api = OtterscanApi::new(
            crate::eth::EthApiBuilder::new(
                MockEthProvider::default(),
                testing_pool(),
                NoopNetwork::default(),
                EthEvmConfig::new(MAINNET.clone()),
            )
            .build(),
        );
        assert_eq!(api.get_api_level().await.unwrap(), 8);
        assert_eq!(api.get_transaction_error(B256::ZERO).await.unwrap(), None);
        assert_eq!(api.trace_transaction(B256::ZERO).await.unwrap(), None);
        assert!(api.get_internal_operations(B256::ZERO).await.unwrap().is_empty());
        assert!(api
            .get_block_details(alloy_eips::BlockNumberOrTag::Number(1).into())
            .await
            .is_err());
        assert!(api
            .get_block_transactions(alloy_eips::BlockNumberOrTag::Number(1).into(), 0, 10)
            .await
            .is_err());
        for error in [
            api.search_transactions_before(
                Address::ZERO,
                alloy_eips::BlockNumberOrTag::Number(0).into(),
                10,
            )
            .await
            .unwrap_err(),
            api.search_transactions_after(
                Address::ZERO,
                alloy_eips::BlockNumberOrTag::Number(0).into(),
                10,
            )
            .await
            .unwrap_err(),
        ] {
            assert_eq!(error.code(), -32603);
            assert_eq!(error.message(), "unimplemented");
        }
    }

    #[tokio::test]
    async fn trace_methods_wait_for_tracing_permit() {
        let api = OtterscanApi::new(
            crate::eth::EthApiBuilder::new(
                MockEthProvider::default(),
                testing_pool(),
                NoopNetwork::default(),
                EthEvmConfig::new(MAINNET.clone()),
            )
            .proof_permits(1)
            .build(),
        );
        let permit = api.eth.acquire_owned_tracing().await.unwrap();

        let timeout = Duration::from_millis(100);
        assert!(tokio::time::timeout(timeout, api.trace_transaction(B256::ZERO)).await.is_err());
        assert!(tokio::time::timeout(timeout, api.get_transaction_error(B256::ZERO))
            .await
            .is_err());
        assert!(tokio::time::timeout(timeout, api.get_internal_operations(B256::ZERO))
            .await
            .is_err());

        drop(permit);
        assert_eq!(api.trace_transaction(B256::ZERO).await.unwrap(), None);
    }

    fn block_with_transaction(
        number: u64,
        parent_hash: B256,
        gas_price: u128,
        zero_calldata_bytes: u8,
    ) -> (Block, Receipt) {
        // A transfer to an empty account costs four extra gas per zero calldata byte.
        let gas_used = MIN_TRANSACTION_GAS + 4 * u64::from(zero_calldata_bytes);
        let tx = TransactionSigned::new_unhashed(
            TxLegacy {
                gas_price,
                gas_limit: gas_used,
                to: Address::ZERO.into(),
                input: vec![0; usize::from(zero_calldata_bytes)].into(),
                ..Default::default()
            }
            .into(),
            Signature::test_signature(),
        );
        let transactions = vec![tx];
        let receipt =
            Receipt { success: true, cumulative_gas_used: gas_used, ..Default::default() };
        let header = Header {
            number,
            parent_hash,
            transactions_root: calculate_transaction_root(&transactions),
            receipts_root: Receipt::calculate_receipt_root_no_memo(std::slice::from_ref(&receipt)),
            gas_used,
            gas_limit: 30_000_000,
            timestamp: number * 12,
            ..Default::default()
        };
        (
            Block {
                header,
                body: alloy_consensus::BlockBody { transactions, ..Default::default() },
            },
            receipt,
        )
    }

    #[tokio::test]
    async fn block_responses_pin_latest() {
        let provider = MockEthProvider::default();
        let (first, first_receipt) = block_with_transaction(1, B256::ZERO, 2, 0);
        let first_hash = first.header.hash_slow();
        let tx_hash = *first.body.transactions[0].hash();
        let first_head = ChainInfo { best_number: 1, best_hash: first_hash };
        let (second, second_receipt) = block_with_transaction(2, first_hash, 3, 1);
        provider.add_block(first_hash, first);
        provider.add_receipts(1, vec![first_receipt]);
        provider.add_block(second.header.hash_slow(), second);
        provider.add_receipts(2, vec![second_receipt]);
        let api = OtterscanApi::new(
            crate::eth::EthApiBuilder::new(
                provider,
                testing_pool(),
                NoopNetwork::default(),
                EthEvmConfig::new(MAINNET.clone()),
            )
            .build(),
        );

        // Each request sees the first block once, then the provider advances to the second.
        api.eth.provider().set_next_chain_info(first_head);
        let details = api.get_block_details(BlockNumberOrTag::Latest.into()).await.unwrap();
        assert_eq!(details.block.header.hash, first_hash);
        assert_eq!(details.total_fees, U256::from(42_000));

        api.eth.provider().set_next_chain_info(first_head);
        let page = api.get_block_transactions(BlockNumberOrTag::Latest.into(), 0, 1).await.unwrap();
        assert_eq!(page.fullblock.block.header.hash, first_hash);
        assert_eq!(page.fullblock.block.transactions.hashes().collect::<Vec<_>>(), [tx_hash]);
        let [receipt] = page.receipts.as_slice() else { panic!("expected one receipt") };
        assert_eq!(receipt.receipt.block_hash, Some(first_hash));
        assert_eq!(receipt.receipt.transaction_hash, tx_hash);
        assert_eq!(receipt.receipt.effective_gas_price, 2);
        assert_eq!(receipt.receipt.gas_used, MIN_TRANSACTION_GAS);
    }

    #[tokio::test]
    async fn block_responses_use_local_pending_receipts() {
        let provider = MockEthProvider::default();
        let parent =
            Block { header: Header { number: 1, ..Default::default() }, ..Default::default() };
        let parent_hash = parent.header.hash_slow();
        provider.add_block(parent_hash, parent);
        let api = OtterscanApi::new(
            crate::eth::EthApiBuilder::new(
                provider,
                testing_pool(),
                NoopNetwork::default(),
                EthEvmConfig::new(MAINNET.clone()),
            )
            .build(),
        );
        let (mut pending, receipt) = block_with_transaction(2, parent_hash, 3, 0);
        // The local pending block and its ommer are unavailable through canonical hash lookups.
        pending.body.ommers.push(Header { number: 1, timestamp: 6, ..Default::default() });
        pending.header.ommers_hash = pending.body.calculate_ommers_root();
        let hash = pending.header.hash_slow();
        let tx_hash = *pending.body.transactions[0].hash();
        let mut executed = ExecutedBlock {
            recovered_block: pending.try_into_recovered().unwrap().into(),
            ..Default::default()
        };
        Arc::make_mut(&mut executed.execution_output).receipts.push(receipt);
        *api.eth.pending_block().lock().await = Some(PendingBlock::with_executed_block(
            Instant::now() + Duration::from_secs(60),
            executed,
        ));
        let details = api.get_block_details(BlockNumberOrTag::Pending.into()).await.unwrap();
        assert_eq!(details.block.header.hash, hash);
        assert_eq!(details.total_fees, U256::from(63_000));
        assert_eq!(details.issuance, calculate_issuance(5 * ETH_TO_WEI, 2, [1].into_iter()));
        let page =
            api.get_block_transactions(BlockNumberOrTag::Pending.into(), 0, 1).await.unwrap();
        assert_eq!(details.block.uncles, page.fullblock.block.uncles);
        assert_eq!(page.fullblock.block.header.hash, hash);
        assert_eq!(page.fullblock.block.transactions.hashes().collect::<Vec<_>>(), [tx_hash]);
        let [receipt] = page.receipts.as_slice() else { panic!("expected one pending receipt") };
        assert_eq!(receipt.receipt.block_hash, Some(hash));
        assert_eq!(receipt.receipt.transaction_hash, tx_hash);
    }

    #[tokio::test]
    async fn block_responses_report_missing_data_with_requested_id() {
        let provider = MockEthProvider::default();
        let (block, receipt) = block_with_transaction(1, B256::ZERO, 2, 0);
        let hash = block.header.hash_slow();
        provider.add_block(hash, block);
        let api = OtterscanApi::new(
            crate::eth::EthApiBuilder::new(
                provider,
                testing_pool(),
                NoopNetwork::default(),
                EthEvmConfig::new(MAINNET.clone()),
            )
            .pending_block_kind(PendingBlockKind::None)
            .build(),
        );

        let not_found = |id| ErrorObjectOwned::from(EthApiError::HeaderNotFound(id));
        let latest = BlockNumberOrTag::Latest;
        assert_eq!(
            api.get_block_details(latest.into()).await.unwrap_err(),
            not_found(latest.into())
        );
        for id in [latest, BlockNumberOrTag::Number(2)] {
            assert_eq!(
                api.get_block_transactions(id.into(), 0, 1).await.unwrap_err(),
                not_found(id.into())
            );
        }
        for id in [hash, B256::ZERO] {
            assert_eq!(api.get_block_details_by_hash(id).await.unwrap_err(), not_found(id.into()));
        }

        // Disabled pending must fail even when the provider can resolve the block and its receipts.
        let provider = api.eth.provider();
        provider.add_receipts(1, vec![receipt]);
        provider.set_pending_block_num_hash(Some(BlockNumHash::new(1, hash)));
        let pending = BlockNumberOrTag::Pending;
        assert_eq!(
            api.get_block_details(pending.into()).await.unwrap_err(),
            not_found(pending.into())
        );
    }

    #[tokio::test]
    async fn block_details_match_rpc_block_with_withdrawals() {
        let provider = MockEthProvider::default();
        let (mut block, receipt) = block_with_transaction(17_034_870, B256::ZERO, 2, 0);
        block.header.timestamp = 1_681_338_455;
        block.header.base_fee_per_gas = Some(1);
        block.body.withdrawals = Some(vec![Withdrawal { amount: 42, ..Default::default() }].into());
        block.header.withdrawals_root = block.body.calculate_withdrawals_root();
        let hash = block.header.hash_slow();
        provider.add_receipts(block.header.number, vec![receipt]);
        provider.add_block(hash, block);
        let api = OtterscanApi::new(
            crate::eth::EthApiBuilder::new(
                provider,
                testing_pool(),
                NoopNetwork::default(),
                EthEvmConfig::new(MAINNET.clone()),
            )
            .build(),
        );

        let expected = api.eth.rpc_block(hash.into(), false).await.unwrap().unwrap();
        assert_eq!(api.get_block_details_by_hash(hash).await.unwrap().block, expected.into());
    }

    #[test]
    fn execution_fees_use_full_u256_width() {
        let receipt = TransactionReceipt {
            inner: alloy_consensus::ReceiptEnvelope::<alloy_rpc_types_eth::Log>::Legacy(
                alloy_consensus::Receipt {
                    status: true.into(),
                    cumulative_gas_used: 2,
                    logs: vec![],
                }
                .with_bloom(),
            ),
            transaction_hash: B256::ZERO,
            transaction_index: Some(0),
            block_hash: Some(B256::ZERO),
            block_number: Some(0),
            gas_used: 2,
            effective_gas_price: u128::MAX,
            blob_gas_used: None,
            blob_gas_price: None,
            from: Address::ZERO,
            to: None,
            contract_address: None,
        };
        assert_eq!(total_fees(&[receipt.clone(), receipt]), U256::from(u128::MAX) * U256::from(4));
    }

    #[test]
    fn issuance_includes_ommer_and_inclusion_rewards() {
        // A Frontier reward with an ommer three blocks behind.
        let issuance = calculate_issuance(5 * ETH_TO_WEI, 126, [123].into_iter());
        assert_eq!(issuance.block_reward, U256::from(5_156_250_000_000_000_000u128));
        assert_eq!(issuance.uncle_reward, U256::from(3_125_000_000_000_000_000u128));
        assert_eq!(issuance.issuance, U256::from(8_281_250_000_000_000_000u128));
        let issuance = calculate_issuance(2 * ETH_TO_WEI, 100, [99, 94].into_iter());
        assert_eq!(issuance.block_reward, U256::from(2_125_000_000_000_000_000u128));
        assert_eq!(issuance.uncle_reward, U256::from(2_250_000_000_000_000_000u128));
    }

    fn execute(code: Bytes, spec: SpecId) -> (evm2::TxResult, Vec<TraceEntry>) {
        let contract = Address::repeat_byte(0x11);
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            &Address::ZERO,
            AccountInfo::default().with_balance(U256::from(ETH_TO_WEI)),
        );
        db.insert_account_info(
            &contract,
            AccountInfo::default()
                .with_balance(U256::from(100))
                .with_code(Bytecode::new_legacy(code)),
        );
        let config = EthEvmConfig::mainnet();
        let env = EthEvmEnv::new(spec, Default::default(), 1);
        let mut evm = config.evm_with_env(db, env);
        let tx = Recovered::new_unchecked(
            evm2::ethereum::TxEnvelope::Legacy(alloy_consensus::TxLegacy {
                to: TxKind::Call(contract),
                value: U256::from(7),
                gas_limit: 1_000_000,
                ..Default::default()
            }),
            Address::ZERO,
        );
        let (inspector, result) = Evm::transact_with_inspector(
            &mut evm,
            &tx,
            TracingInspector::new(TracingInspectorConfig::default_parity()),
        )
        .unwrap();
        (result.result, otterscan_traces(inspector.traces().nodes().to_vec()))
    }

    #[test]
    fn selfdestruct_preserves_enclosing_call_and_beneficiary() {
        for spec in [SpecId::SHANGHAI, SpecId::CANCUN] {
            let (result, traces) = execute(bytes!("6022ff"), spec);
            assert!(result.status);
            assert_eq!(traces.len(), 2);
            assert_eq!(traces[0].r#type, "CALL");
            assert_eq!(traces[0].depth, 0);
            assert_eq!(traces[0].value, Some(U256::from(7)));
            assert_eq!(traces[1].r#type, "SELFDESTRUCT");
            assert_eq!(traces[1].depth, 1);
            assert_eq!(traces[1].from, Address::repeat_byte(0x11));
            assert_eq!(traces[1].to, Address::with_last_byte(0x22));
            assert_eq!(traces[1].value, Some(U256::from(107)));
        }
    }

    #[test]
    fn selfdestruct_after_a_child_call_uses_enclosing_depth() {
        // Call another account, then destroy the root contract. The destruction is a sibling
        // of that call, not its child.
        let (result, traces) =
            execute(bytes!("60006000600060006000603361fffff1506022ff"), SpecId::CANCUN);
        assert!(result.status);
        assert_eq!(traces.len(), 3);
        assert_eq!(traces[1].r#type, "CALL");
        assert_eq!(traces[1].depth, 1);
        assert_eq!(traces[2].r#type, "SELFDESTRUCT");
        assert_eq!(traces[2].depth, 1);
        assert_eq!(traces[2].from, Address::repeat_byte(0x11));
        assert_eq!(traces[2].to, Address::with_last_byte(0x22));
    }

    #[test]
    fn selfdestructs_follow_children_and_precede_siblings() {
        let nodes = [0, 1, 2, 1]
            .into_iter()
            .enumerate()
            .map(|(idx, depth)| CallTraceNode {
                trace: CallTrace {
                    depth,
                    address: Address::with_last_byte(idx as u8),
                    selfdestruct: (idx < 3).then(|| {
                        Box::new(SelfdestructTrace {
                            address: Some(Address::with_last_byte(idx as u8)),
                            refund_target: Some(Address::with_last_byte(0xff)),
                            transferred_value: Some(U256::ZERO),
                        })
                    }),
                    ..Default::default()
                },
                ..Default::default()
            })
            .collect();
        let traces = otterscan_traces(nodes);
        let order =
            traces.iter().map(|trace| (trace.r#type.as_str(), trace.depth)).collect::<Vec<_>>();
        assert_eq!(
            order,
            [
                ("CALL", 0),
                ("CALL", 1),
                ("CALL", 2),
                ("SELFDESTRUCT", 3),
                ("SELFDESTRUCT", 2),
                ("CALL", 1),
                ("SELFDESTRUCT", 1)
            ]
        );
        assert_eq!(
            traces
                .iter()
                .filter(|trace| trace.r#type == "SELFDESTRUCT")
                .map(|trace| trace.from)
                .collect::<Vec<_>>(),
            [Address::with_last_byte(2), Address::with_last_byte(1), Address::ZERO]
        );
    }

    #[test]
    fn precompile_calls_remain_in_transaction_traces() {
        let (result, traces) =
            execute(bytes!("60006000600060006000600461fffff15000"), SpecId::CANCUN);
        assert!(result.status);
        assert_eq!(traces.len(), 2);
        assert_eq!(traces[1].to, Address::with_last_byte(4));
    }

    #[test]
    fn contract_creator_ignores_reverted_ancestors() {
        let contract = Address::repeat_byte(0x11);
        let creator = Address::repeat_byte(0x22);
        let localize = |trace, hash| LocalizedTransactionTrace {
            trace,
            transaction_hash: Some(hash),
            block_hash: None,
            block_number: Some(1),
            transaction_position: None,
        };
        let creation = TransactionTrace {
            action: Action::Create(CreateAction { from: creator, ..Default::default() }),
            result: Some(TraceOutput::Create(CreateOutput {
                address: contract,
                code: Bytes::new(),
                gas_used: 0,
            })),
            trace_address: vec![0, 0],
            ..Default::default()
        };
        for path in [vec![], vec![0]] {
            let failed = TransactionTrace {
                trace_address: path,
                error: Some("Reverted".into()),
                ..Default::default()
            };
            let reverted =
                vec![localize(failed, B256::ZERO), localize(creation.clone(), B256::ZERO)];
            assert_eq!(find_contract_creator(contract, vec![reverted.clone()]).unwrap(), None);
            let successful = vec![localize(creation.clone(), B256::repeat_byte(1))];
            assert_eq!(
                find_contract_creator(contract, vec![reverted, successful]).unwrap(),
                Some(ContractCreator { creator, hash: B256::repeat_byte(1) })
            );
        }
        // A reverted sibling does not invalidate a subsequent successful creation.
        let failed = TransactionTrace {
            trace_address: vec![0],
            error: Some("Reverted".into()),
            ..Default::default()
        };
        let successful = TransactionTrace { trace_address: vec![1], ..creation };
        assert!(find_contract_creator(
            contract,
            vec![vec![localize(failed, B256::ZERO), localize(successful, B256::ZERO)]]
        )
        .unwrap()
        .is_some());
    }

    #[test]
    fn static_and_delegate_calls_have_no_value() {
        // STATICCALL, DELEGATECALL and CALLCODE to 0x22 do not transfer ETH.
        let (result, traces) = execute(
            bytes!("6000600060006000602261fffffa506000600060006000602261fffff45060006000600060006001602261fffff25000"),
            SpecId::CANCUN,
        );
        assert!(result.status);
        assert_eq!(traces.len(), 4);
        assert_eq!(traces[1].r#type, "STATICCALL");
        assert_eq!(traces[1].value, None);
        assert_eq!(traces[2].r#type, "DELEGATECALL");
        assert_eq!(traces[2].value, None);
        assert_eq!(traces[3].r#type, "CALLCODE");
        assert_eq!(traces[3].value, Some(U256::ONE));
    }
}
