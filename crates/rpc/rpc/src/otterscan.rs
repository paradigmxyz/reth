use alloy_primitives::Bytes;
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_primitives::{Address, BlockNumberOrTag, TxHash, B256, U256};
use reth_rpc_api::{EthApiServer, OtterscanServer};
use reth_rpc_eth_api::helpers::TraceExt;
use reth_rpc_eth_types::EthApiError;
use reth_rpc_server_types::result::internal_rpc_err;
use reth_rpc_types::{
    trace::{
        otterscan::{
            BlockDetails, ContractCreator, InternalOperation, OperationType, OtsBlockTransactions,
            OtsReceipt, OtsTransactionReceipt, TraceEntry, TransactionsWithReceipts,
        },
        parity::{Action, CreateAction, CreateOutput, TraceOutput},
    },
    AnyTransactionReceipt, BlockTransactions, Header, RichBlock,
};
use revm_inspectors::{
    tracing::{types::CallTraceNode, TracingInspectorConfig},
    transfer::{TransferInspector, TransferKind},
};
use revm_primitives::ExecutionResult;
use std::future::Future;

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

    /// Constructs a `BlockDetails` from a block and its receipts.
    fn block_details(
        &self,
        block: Option<RichBlock>,
        receipts: Option<Vec<AnyTransactionReceipt>>,
    ) -> RpcResult<BlockDetails> {
        let block = block.ok_or_else(|| EthApiError::UnknownBlockNumber)?;
        let receipts = receipts.ok_or_else(|| EthApiError::UnknownBlockNumber)?;

        // blob fee is burnt, so we don't need to calculate it
        let total_fees = receipts
            .iter()
            .map(|receipt| receipt.gas_used.saturating_mul(receipt.effective_gas_price))
            .sum::<u128>();

        Ok(BlockDetails::new(block, Default::default(), U256::from(total_fees)))
    }
}

#[async_trait]
impl<Eth> OtterscanServer for OtterscanApi<Eth>
where
    Eth: EthApiServer + TraceExt + 'static,
{
    /// Handler for `{ots,erigon}_getHeaderByNumber`
    async fn get_header_by_number(&self, block_number: u64) -> RpcResult<Option<Header>> {
        self.eth.header_by_number(BlockNumberOrTag::Number(block_number)).await
    }

    /// Handler for `ots_hasCode`
    async fn has_code(&self, address: Address, block_number: Option<u64>) -> RpcResult<bool> {
        self.eth.get_code(address, block_number.map(Into::into)).await.map(|code| !code.is_empty())
    }

    /// Handler for `ots_getApiLevel`
    async fn get_api_level(&self) -> RpcResult<u64> {
        Ok(API_LEVEL)
    }

    /// Handler for `ots_getInternalOperations`
    async fn get_internal_operations(&self, tx_hash: TxHash) -> RpcResult<Vec<InternalOperation>> {
        let internal_operations = self
            .eth
            .spawn_trace_transaction_in_block_with_inspector(
                tx_hash,
                TransferInspector::new(false),
                |_tx_info, inspector, _, _| Ok(inspector.into_transfers()),
            )
            .await
            .map_err(Into::into)?
            .map(|transfer_operations| {
                transfer_operations
                    .iter()
                    .map(|op| InternalOperation {
                        from: op.from,
                        to: op.to,
                        value: op.value,
                        r#type: match op.kind {
                            TransferKind::Call => OperationType::OpTransfer,
                            TransferKind::Create => OperationType::OpCreate,
                            TransferKind::Create2 => OperationType::OpCreate2,
                            TransferKind::SelfDestruct => OperationType::OpSelfDestruct,
                        },
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(internal_operations)
    }

    /// Handler for `ots_getTransactionError`
    async fn get_transaction_error(&self, tx_hash: TxHash) -> RpcResult<Option<Bytes>> {
        let maybe_revert = self
            .eth
            .spawn_replay_transaction(tx_hash, |_tx_info, res, _| match res.result {
                ExecutionResult::Revert { output, .. } => Ok(Some(output)),
                _ => Ok(None),
            })
            .await
            .map(Option::flatten)
            .map_err(Into::into)?;
        Ok(maybe_revert)
    }

    /// Handler for `ots_traceTransaction`
    async fn trace_transaction(&self, tx_hash: TxHash) -> RpcResult<Option<Vec<TraceEntry>>> {
        let traces = self
            .eth
            .spawn_trace_transaction_in_block(
                tx_hash,
                TracingInspectorConfig::default_parity(),
                move |_tx_info, inspector, _, _| Ok(inspector.into_traces().into_nodes()),
            )
            .await
            .map_err(Into::into)?
            .map(|traces| {
                traces
                    .into_iter()
                    .map(|CallTraceNode { trace, .. }| TraceEntry {
                        r#type: if trace.is_selfdestruct() {
                            "SELFDESTRUCT".to_string()
                        } else {
                            trace.kind.to_string()
                        },
                        depth: trace.depth as u32,
                        from: trace.caller,
                        to: trace.address,
                        value: trace.value,
                        input: trace.data,
                        output: trace.output,
                    })
                    .collect::<Vec<_>>()
            });
        Ok(traces)
    }

    /// Handler for `ots_getBlockDetails`
    async fn get_block_details(&self, block_number: u64) -> RpcResult<BlockDetails> {
        let block = self.eth.block_by_number(block_number.into(), true);
        let receipts = self.eth.block_receipts(block_number.into());
        let (block, receipts) = futures::try_join!(block, receipts)?;
        self.block_details(block, receipts)
    }

    /// Handler for `getBlockDetailsByHash`
    async fn get_block_details_by_hash(&self, block_hash: B256) -> RpcResult<BlockDetails> {
        let block = self.eth.block_by_hash(block_hash, true);
        let receipts = self.eth.block_receipts(block_hash.into());
        let (block, receipts) = futures::try_join!(block, receipts)?;
        self.block_details(block, receipts)
    }

    /// Handler for `getBlockTransactions`
    async fn get_block_transactions(
        &self,
        block_number: u64,
        page_number: usize,
        page_size: usize,
    ) -> RpcResult<OtsBlockTransactions> {
        // retrieve full block and its receipts
        let block = self.eth.block_by_number(block_number.into(), true);
        let receipts = self.eth.block_receipts(block_number.into());
        let (block, receipts) = futures::try_join!(block, receipts)?;

        let mut block = block.ok_or_else(|| EthApiError::UnknownBlockNumber)?;
        let mut receipts = receipts.ok_or_else(|| internal_rpc_err("receipts not found"))?;

        // check if the number of transactions matches the number of receipts
        let tx_len = block.transactions.len();
        if tx_len != receipts.len() {
            return Err(internal_rpc_err(
                "the number of transactions does not match the number of receipts",
            ))
        }

        // make sure the block is full
        let BlockTransactions::Full(transactions) = &mut block.inner.transactions else {
            return Err(internal_rpc_err("block is not full"));
        };

        // Crop page
        let page_end = tx_len.saturating_sub(page_number * page_size);
        let page_start = page_end.saturating_sub(page_size);

        // Crop transactions
        *transactions = transactions.drain(page_start..page_end).collect::<Vec<_>>();

        // The input field returns only the 4 bytes method selector instead of the entire
        // calldata byte blob.
        for tx in transactions {
            if tx.input.len() > 4 {
                tx.input = tx.input.slice(..4);
            }
        }

        // Crop receipts and transform them into OtsTransactionReceipt
        let timestamp = Some(block.header.timestamp);
        let receipts = receipts
            .drain(page_start..page_end)
            .map(|receipt| {
                let receipt = receipt.inner.map_inner(|receipt| OtsReceipt {
                    status: receipt
                        .inner
                        .receipt
                        .status
                        .as_eip658()
                        .expect("ETH API returned pre-EIP-658 status"),
                    cumulative_gas_used: receipt.inner.receipt.cumulative_gas_used as u64,
                    logs: None,
                    logs_bloom: None,
                    r#type: receipt.r#type,
                });

                OtsTransactionReceipt { receipt, timestamp }
            })
            .collect();
        Ok(OtsBlockTransactions { fullblock: block.inner.into(), receipts })
    }

    /// Handler for `searchTransactionsBefore`
    async fn search_transactions_before(
        &self,
        _address: Address,
        _block_number: u64,
        _page_size: usize,
    ) -> RpcResult<TransactionsWithReceipts> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `searchTransactionsAfter`
    async fn search_transactions_after(
        &self,
        _address: Address,
        _block_number: u64,
        _page_size: usize,
    ) -> RpcResult<TransactionsWithReceipts> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `getTransactionBySenderAndNonce`
    async fn get_transaction_by_sender_and_nonce(
        &self,
        sender: Address,
        nonce: u64,
    ) -> RpcResult<Option<TxHash>> {
        // Check if the sender is a contract
        if self.has_code(sender, None).await? {
            return Ok(None)
        }

        let highest =
            EthApiServer::transaction_count(&self.eth, sender, None).await?.saturating_to::<u64>();

        // If the nonce is higher or equal to the highest nonce, the transaction is pending or not
        // exists.
        if nonce >= highest {
            return Ok(None)
        }

        // perform a binary search over the block range to find the block in which the sender's
        // nonce reached the requested nonce.
        let num = binary_search(1, self.eth.block_number()?.saturating_to(), |mid| {
            async move {
                let mid_nonce =
                    EthApiServer::transaction_count(&self.eth, sender, Some(mid.into()))
                        .await?
                        .saturating_to::<u64>();

                // The `transaction_count` returns the `nonce` after the transaction was
                // executed, which is the state of the account after the block, and we need to find
                // the transaction whose nonce is the pre-state, so need to compare with `nonce`(no
                // equal).
                Ok(mid_nonce > nonce)
            }
        })
        .await?;

        let Some(BlockTransactions::Full(transactions)) =
            self.eth.block_by_number(num.into(), true).await?.map(|block| block.inner.transactions)
        else {
            return Err(EthApiError::UnknownBlockNumber.into());
        };

        Ok(transactions
            .into_iter()
            .find(|tx| tx.from == sender && tx.nonce == nonce)
            .map(|tx| tx.hash))
    }

    /// Handler for `getContractCreator`
    async fn get_contract_creator(&self, address: Address) -> RpcResult<Option<ContractCreator>> {
        if !self.has_code(address, None).await? {
            return Ok(None);
        }

        let num = binary_search(1, self.eth.block_number()?.saturating_to(), |mid| {
            Box::pin(
                async move { Ok(!self.eth.get_code(address, Some(mid.into())).await?.is_empty()) },
            )
        })
        .await?;

        let traces = self
            .eth
            .trace_block_with(
                num.into(),
                TracingInspectorConfig::default_parity(),
                |tx_info, inspector, _, _, _| {
                    Ok(inspector.into_parity_builder().into_localized_transaction_traces(tx_info))
                },
            )
            .await
            .map_err(Into::into)?
            .map(|traces| {
                traces
                    .into_iter()
                    .flatten()
                    .map(|tx_trace| {
                        let trace = tx_trace.trace;
                        Ok(match (trace.action, trace.result, trace.error) {
                            (
                                Action::Create(CreateAction { from: creator, .. }),
                                Some(TraceOutput::Create(CreateOutput {
                                    address: contract, ..
                                })),
                                None,
                            ) if contract == address => Some(ContractCreator {
                                hash: tx_trace
                                    .transaction_hash
                                    .ok_or_else(|| EthApiError::TransactionNotFound)?,
                                creator,
                            }),
                            _ => None,
                        })
                    })
                    .filter_map(Result::transpose)
                    .collect::<Result<Vec<_>, EthApiError>>()
            })
            .transpose()?;

        // A contract maybe created and then destroyed in multiple transactions, here we
        // return the first found transaction, this behavior is consistent with etherscan's
        let found = traces.and_then(|traces| traces.first().cloned());
        Ok(found)
    }
}

/// Performs a binary search within a given block range to find the desired block number.
///
/// The binary search is performed by calling the provided asynchronous `check` closure on the
/// blocks of the range. The closure should return a future representing the result of performing
/// the desired logic at a given block. The future resolves to an `bool` where:
/// - `true` indicates that the condition has been matched, but we can try to find a lower block to
///   make the condition more matchable.
/// - `false` indicates that the condition not matched, so the target is not present in the current
///   block and should continue searching in a higher range.
///
/// Args:
/// - `low`: The lower bound of the block range (inclusive).
/// - `high`: The upper bound of the block range (inclusive).
/// - `check`: A closure that performs the desired logic at a given block.
async fn binary_search<F, Fut>(low: u64, high: u64, check: F) -> RpcResult<u64>
where
    F: Fn(u64) -> Fut,
    Fut: Future<Output = RpcResult<bool>>,
{
    let mut low = low;
    let mut high = high;
    let mut num = high;

    while low <= high {
        let mid = (low + high) / 2;
        if check(mid).await? {
            high = mid - 1;
            num = mid;
        } else {
            low = mid + 1
        }
    }

    Ok(num)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_binary_search() {
        // in the middle
        let num = binary_search(1, 10, |mid| Box::pin(async move { Ok(mid >= 5) })).await;
        assert_eq!(num, Ok(5));

        // in the upper
        let num = binary_search(1, 10, |mid| Box::pin(async move { Ok(mid >= 7) })).await;
        assert_eq!(num, Ok(7));

        // in the lower
        let num = binary_search(1, 10, |mid| Box::pin(async move { Ok(mid >= 1) })).await;
        assert_eq!(num, Ok(1));

        // high than the upper
        let num = binary_search(1, 10, |mid| Box::pin(async move { Ok(mid >= 11) })).await;
        assert_eq!(num, Ok(10));
    }
}
