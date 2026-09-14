use alloy_consensus::{BlockHeader, Typed2718};
use alloy_eips::{eip1898::LenientBlockNumberOrTag, BlockId};
use alloy_evm::block::calc::{base_block_reward, block_reward, ommer_reward};
use alloy_network::{primitives::HeaderResponse, ReceiptResponse, TransactionResponse};
use alloy_primitives::{Address, Bytes, TxHash, B256, U256};
use alloy_rpc_types_eth::{BlockTransactions, TransactionReceipt};
use alloy_rpc_types_trace::{
    otterscan::{
        BlockDetails, ContractCreator, InternalIssuance, InternalOperation, OperationType,
        OtsBlockTransactions, OtsReceipt, OtsTransactionReceipt, TraceEntry,
        TransactionsWithReceipts,
    },
    parity::{Action, CreateAction, CreateOutput, LocalizedTransactionTrace, TraceOutput},
};
use async_trait::async_trait;
use jsonrpsee::{core::RpcResult, types::ErrorObjectOwned};
use reth_chainspec::ChainSpecProvider;
use reth_primitives_traits::{BlockBody, TxTy};
use reth_rpc_api::{EthApiServer, OtterscanServer};
use reth_rpc_convert::RpcTxReq;
use reth_rpc_eth_api::{
    helpers::{EthTransactions, TraceExt},
    FullEthApiTypes, RpcBlock, RpcHeader, RpcReceipt, RpcTransaction,
};
use reth_rpc_eth_types::{utils::binary_search, EthApiError};
use reth_rpc_server_types::result::internal_rpc_err;
use revm::context_interface::result::ExecutionResult;
use revm_inspectors::tracing::{
    types::{CallKind, CallTraceNode},
    TracingInspectorConfig,
};

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
    Eth: FullEthApiTypes + TraceExt,
{
    /// Constructs a `BlockDetails` from a block and its receipts.
    async fn block_details(
        &self,
        block: RpcBlock<Eth::NetworkTypes>,
        receipts: Vec<RpcReceipt<Eth::NetworkTypes>>,
    ) -> RpcResult<BlockDetails<RpcHeader<Eth::NetworkTypes>>> {
        // Execution fees include the base fee; blob fees are not part of this field.
        let total_fees = receipts
            .iter()
            .map(|receipt| {
                U256::from(receipt.gas_used()) * U256::from(receipt.effective_gas_price())
            })
            .sum::<U256>();

        let chain_spec = self.eth.provider().chain_spec();
        let reward = if block.header.number() == 0 {
            None
        } else {
            base_block_reward(&chain_spec, block.header.number())
        };
        let issuance = if let Some(reward) = reward {
            let ommers = if block.uncles.is_empty() {
                Vec::new()
            } else {
                self.eth
                    .recovered_block(block.header.hash().into())
                    .await
                    .map_err(Into::<ErrorObjectOwned>::into)?
                    .ok_or(EthApiError::HeaderNotFound(block.header.hash().into()))?
                    .body()
                    .ommers()
                    .unwrap_or_default()
                    .to_vec()
            };
            calculate_issuance(
                reward,
                block.header.number(),
                ommers.iter().map(BlockHeader::number),
            )
        } else {
            InternalIssuance::default()
        };
        Ok(BlockDetails::new(block, issuance, total_fees))
    }
}

#[async_trait]
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
        + 'static,
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
        self.eth
            .spawn_trace_transaction_in_block(
                tx_hash,
                TracingInspectorConfig::default_parity(),
                |_tx_info, inspector, _, _| {
                    Ok(internal_operations(otterscan_traces(inspector.into_traces().into_nodes())))
                },
            )
            .await
            .map_err(Into::into)
            .map(Option::unwrap_or_default)
    }

    /// Handler for `ots_getTransactionError`
    async fn get_transaction_error(&self, tx_hash: TxHash) -> RpcResult<Option<Bytes>> {
        self.eth
            .spawn_replay_transaction(tx_hash, |_tx_info, res, _| Ok(transaction_error(res.result)))
            .await
            .map_err(Into::into)
    }

    /// Handler for `ots_traceTransaction`
    async fn trace_transaction(&self, tx_hash: TxHash) -> RpcResult<Option<Vec<TraceEntry>>> {
        self.eth
            .spawn_trace_transaction_in_block(
                tx_hash,
                TracingInspectorConfig::default_parity(),
                |_tx_info, inspector, _, _| {
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
        let block_number = block_number.into_inner();
        let block = self.eth.block_by_number(block_number, true);
        let block_id = block_number.into();
        let receipts = self.eth.block_receipts(block_id);
        let (block, receipts) = futures::try_join!(block, receipts)?;
        self.block_details(
            block.ok_or(EthApiError::HeaderNotFound(block_id))?,
            receipts.ok_or(EthApiError::ReceiptsNotFound(block_id))?,
        )
        .await
    }

    /// Handler for `ots_getBlockDetailsByHash`
    async fn get_block_details_by_hash(
        &self,
        block_hash: B256,
    ) -> RpcResult<BlockDetails<RpcHeader<Eth::NetworkTypes>>> {
        let block = self.eth.block_by_hash(block_hash, true);
        let block_id = block_hash.into();
        let receipts = self.eth.block_receipts(block_id);
        let (block, receipts) = futures::try_join!(block, receipts)?;
        self.block_details(
            block.ok_or(EthApiError::HeaderNotFound(block_id))?,
            receipts.ok_or(EthApiError::ReceiptsNotFound(block_id))?,
        )
        .await
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
        let block_number = block_number.into_inner();
        // retrieve full block and its receipts
        let block = self.eth.block_by_number(block_number, true);
        let block_id = block_number.into();
        let receipts = self.eth.block_receipts(block_id);
        let (block, receipts) = futures::try_join!(block, receipts)?;

        let mut block = block.ok_or(EthApiError::HeaderNotFound(block_id))?;
        let mut receipts = receipts.ok_or(EthApiError::ReceiptsNotFound(block_id))?;

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
                |tx_info, mut ctx| {
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
fn block_transaction_page_range(
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
    ommer_numbers: impl Iterator<Item = u64>,
) -> InternalIssuance {
    let mut count = 0;
    let uncle_reward = ommer_numbers
        .map(|ommer| {
            count += 1;
            U256::from(ommer_reward(reward, number, ommer))
        })
        .sum::<U256>();
    let block_reward = U256::from(block_reward(reward, count));
    InternalIssuance { block_reward, uncle_reward, issuance: block_reward + uncle_reward }
}

fn transaction_error<H>(result: ExecutionResult<H>) -> Bytes {
    match result {
        ExecutionResult::Revert { output, .. } => output,
        _ => Bytes::new(),
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
        if let Some(to) = trace.selfdestruct_refund_target {
            selfdestructs.push(TraceEntry {
                r#type: "SELFDESTRUCT".to_string(),
                depth: trace.depth as u32 + 1,
                from: trace.selfdestruct_address.unwrap_or(trace.address),
                to,
                value: trace.selfdestruct_transferred_value,
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

fn internal_operations(traces: Vec<TraceEntry>) -> Vec<InternalOperation> {
    traces
        .into_iter()
        .filter_map(|trace| {
            if trace.depth == 0 {
                return None
            }
            let value = trace.value.unwrap_or_default();
            let r#type = match trace.r#type.as_str() {
                "CALL" if !value.is_zero() => OperationType::OpTransfer,
                "CREATE" => OperationType::OpCreate,
                "CREATE2" => OperationType::OpCreate2,
                "SELFDESTRUCT" => OperationType::OpSelfDestruct,
                _ => return None,
            };
            Some(InternalOperation { from: trace.from, to: trace.to, value, r#type })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{constants::ETH_TO_WEI, Header};
    use alloy_primitives::{hex, TxKind};
    use reth_chainspec::MAINNET;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::test_utils::MockEthProvider;
    use reth_transaction_pool::test_utils::testing_pool;
    use revm::{
        context::TxEnv,
        context_interface::result::{HaltReason, Output, SuccessReason},
        database::InMemoryDB,
        inspector::InspectorEvmTr,
        primitives::hardfork::SpecId,
        state::{AccountInfo, Bytecode},
        Context, InspectEvm, MainBuilder, MainContext,
    };
    use revm_inspectors::tracing::TracingInspector;

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
        let success: ExecutionResult = ExecutionResult::Success {
            reason: SuccessReason::Return,
            gas: Default::default(),
            logs: vec![],
            output: Output::Call(Bytes::from_static(b"successful return data")),
        };
        let halt = ExecutionResult::Halt {
            reason: HaltReason::OutOfGas(revm::context_interface::result::OutOfGasError::Basic),
            gas: Default::default(),
            logs: vec![],
        };
        for result in [
            success,
            halt,
            ExecutionResult::Revert { gas: Default::default(), logs: vec![], output: Bytes::new() },
        ] {
            assert_eq!(serde_json::to_value(Some(transaction_error(result))).unwrap(), "0x");
        }
        let output = Bytes::from_static(b"revert data");
        assert_eq!(
            transaction_error::<HaltReason>(ExecutionResult::Revert {
                gas: Default::default(),
                logs: vec![],
                output: output.clone()
            }),
            output
        );
    }

    async fn cache_empty_block(
        cache: reth_rpc_eth_types::EthStateCache<reth_ethereum_primitives::EthPrimitives>,
        block: reth_ethereum_primitives::Block,
    ) {
        // MockEthProvider does not implement recovered_block; seed the normal RPC cache.
        let outcome = reth_execution_types::ExecutionOutcome::new(
            Default::default(),
            vec![vec![]],
            block.header.number,
            vec![],
        );
        let chain = reth_execution_types::Chain::new(
            [reth_primitives_traits::RecoveredBlock::new_unhashed(block, vec![])],
            outcome,
            Default::default(),
        );
        reth_rpc_eth_types::cache::cache_new_blocks_task(
            cache,
            futures::stream::iter([reth_chain_state::CanonStateNotification::Commit {
                new: std::sync::Arc::new(chain),
            }]),
        )
        .await;
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
            provider.add_block(hash, block.clone());
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
            cache_empty_block(api.eth.cache().clone(), block).await;
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
        provider.add_block(hash, block.clone());
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
        cache_empty_block(api.eth.cache().clone(), block).await;
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
    async fn execution_fees_use_full_u256_width() {
        let api = OtterscanApi::new(
            crate::eth::EthApiBuilder::new(
                MockEthProvider::default(),
                testing_pool(),
                NoopNetwork::default(),
                EthEvmConfig::new(MAINNET.clone()),
            )
            .build(),
        );
        let receipt = TransactionReceipt {
            inner: alloy_consensus::ReceiptEnvelope::Legacy(
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
        let details =
            api.block_details(Default::default(), vec![receipt.clone(), receipt]).await.unwrap();
        assert_eq!(details.total_fees, U256::from(u128::MAX) * U256::from(4));
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

    fn execute(code: Bytes, spec: SpecId) -> (ExecutionResult, Vec<TraceEntry>) {
        let contract = Address::repeat_byte(0x11);
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            Address::ZERO,
            AccountInfo { balance: U256::from(ETH_TO_WEI), ..Default::default() },
        );
        db.insert_account_info(
            contract,
            AccountInfo {
                balance: U256::from(100),
                code: Some(Bytecode::new_legacy(code)),
                ..Default::default()
            },
        );
        let mut evm = Context::mainnet()
            .modify_cfg_chained(|cfg| cfg.spec = spec)
            .with_db(db)
            .build_mainnet_with_inspector(TracingInspector::new(
                TracingInspectorConfig::default_parity(),
            ));
        let result = evm
            .inspect_tx(TxEnv {
                kind: TxKind::Call(contract),
                value: U256::from(7),
                gas_limit: 1_000_000,
                ..Default::default()
            })
            .unwrap();
        let (_, inspector) = evm.ctx_inspector();
        (result.result, otterscan_traces(inspector.traces().nodes().to_vec()))
    }

    #[test]
    fn internal_operations_exclude_root_and_include_zero_value_creates() {
        // CREATE and CREATE2 with empty initcode and zero endowment.
        let (result, traces) =
            execute(hex!("600060006000f0506000600060006000f55000").into(), SpecId::CANCUN);
        assert!(result.is_success());
        let operations = internal_operations(traces);
        assert_eq!(operations.len(), 2);
        assert_eq!(operations[0].r#type, OperationType::OpCreate);
        assert_eq!(operations[1].r#type, OperationType::OpCreate2);
        for operation in operations {
            assert_eq!(operation.from, Address::repeat_byte(0x11));
            assert_eq!(operation.value, U256::ZERO);
            assert_ne!(operation.to, Address::ZERO);
        }
        let (_, traces) = execute(hex!("00").into(), SpecId::CANCUN);
        assert!(internal_operations(traces).is_empty());
    }

    #[test]
    fn selfdestruct_preserves_enclosing_call_and_beneficiary() {
        for spec in [SpecId::SHANGHAI, SpecId::CANCUN] {
            let (result, traces) = execute(hex!("6022ff").into(), spec);
            assert!(result.is_success());
            assert_eq!(traces.len(), 2);
            assert_eq!(traces[0].r#type, "CALL");
            assert_eq!(traces[0].depth, 0);
            assert_eq!(traces[0].value, Some(U256::from(7)));
            assert_eq!(traces[1].r#type, "SELFDESTRUCT");
            assert_eq!(traces[1].depth, 1);
            assert_eq!(traces[1].from, Address::repeat_byte(0x11));
            assert_eq!(traces[1].to, Address::with_last_byte(0x22));
            assert_eq!(traces[1].value, Some(U256::from(107)));
            let operations = internal_operations(traces);
            assert_eq!(operations.len(), 1);
            assert_eq!(operations[0].r#type, OperationType::OpSelfDestruct);
        }
    }

    #[test]
    fn selfdestruct_after_a_child_call_uses_enclosing_depth() {
        // Call another account, then destroy the root contract. The destruction is a sibling
        // of that call, not its child.
        let (result, traces) =
            execute(hex!("60006000600060006000603361fffff1506022ff").into(), SpecId::CANCUN);
        assert!(result.is_success());
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
        use revm_inspectors::tracing::types::CallTrace;
        let nodes = [0, 1, 2, 1]
            .into_iter()
            .enumerate()
            .map(|(idx, depth)| CallTraceNode {
                trace: CallTrace {
                    depth,
                    address: Address::with_last_byte(idx as u8),
                    selfdestruct_address: (idx < 3).then_some(Address::with_last_byte(idx as u8)),
                    selfdestruct_refund_target: (idx < 3).then_some(Address::with_last_byte(0xff)),
                    selfdestruct_transferred_value: (idx < 3).then_some(U256::ZERO),
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
        let operations = internal_operations(traces);
        assert_eq!(
            operations.iter().map(|op| op.from).collect::<Vec<_>>(),
            [Address::with_last_byte(2), Address::with_last_byte(1), Address::ZERO]
        );
    }

    #[test]
    fn precompile_calls_remain_in_transaction_traces() {
        let (result, traces) =
            execute(hex!("60006000600060006000600461fffff15000").into(), SpecId::CANCUN);
        assert!(result.is_success());
        assert_eq!(traces.len(), 2);
        assert_eq!(traces[1].to, Address::with_last_byte(4));
    }

    #[test]
    fn reverted_internal_transfers_are_retained() {
        // CALL value 1 to 0x22, then revert the enclosing transaction.
        let (result, traces) =
            execute(hex!("60006000600060006001602261fffff15060006000fd").into(), SpecId::CANCUN);
        assert!(!result.is_success());
        let operations = internal_operations(traces);
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].to, Address::with_last_byte(0x22));
        assert_eq!(operations[0].value, U256::from(1));
    }

    #[test]
    fn contract_creator_ignores_reverted_ancestors() {
        use alloy_rpc_types_trace::parity::TransactionTrace;
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
        // STATICCALL and DELEGATECALL to 0x22; DELEGATECALL inherits the root's value.
        let (result, traces) = execute(
            hex!("6000600060006000602261fffffa506000600060006000602261fffff45000").into(),
            SpecId::CANCUN,
        );
        assert!(result.is_success());
        assert_eq!(traces.len(), 3);
        assert_eq!(traces[1].r#type, "STATICCALL");
        assert_eq!(traces[1].value, None);
        assert_eq!(traces[2].r#type, "DELEGATECALL");
        assert_eq!(traces[2].value, None);
        assert!(internal_operations(traces).is_empty());
    }
}
