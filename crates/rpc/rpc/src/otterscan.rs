use alloy_primitives::Bytes;
use async_trait::async_trait;
use jsonrpsee::{core::RpcResult, types::ErrorObjectOwned};
use reth_primitives::{Address, BlockId, BlockNumberOrTag, TxHash, B256};
use reth_rpc_api::{EthApiServer, OtterscanServer};
use reth_rpc_eth_api::helpers::TraceExt;
use reth_rpc_server_types::result::internal_rpc_err;
use reth_rpc_types::{
    trace::{
        otterscan::{
            BlockDetails, ContractCreator, InternalOperation, OperationType, OtsBlockTransactions,
            OtsReceipt, OtsTransactionReceipt, TraceEntry, TransactionsWithReceipts,
        },
        parity::{Action, CreateAction, CreateOutput, TraceOutput},
    },
    BlockTransactions, Transaction,
};
use revm_inspectors::{
    tracing::TracingInspectorConfig,
    transfer::{TransferInspector, TransferKind},
};
use revm_primitives::ExecutionResult;
use tracing::debug;

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

#[async_trait]
impl<Eth> OtterscanServer for OtterscanApi<Eth>
where
    Eth: EthApiServer + TraceExt + 'static,
{
    /// Handler for `ots_hasCode`
    async fn has_code(&self, address: Address, block_number: Option<BlockId>) -> RpcResult<bool> {
        self.eth.get_code(address, block_number).await.map(|code| !code.is_empty())
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
            .await?
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
            .map(Option::flatten)?;
        Ok(maybe_revert)
    }

    /// Handler for `ots_traceTransaction`
    async fn trace_transaction(&self, _tx_hash: TxHash) -> RpcResult<TraceEntry> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `ots_getBlockDetails`
    async fn get_block_details(
        &self,
        block_number: BlockNumberOrTag,
    ) -> RpcResult<Option<BlockDetails>> {
        let block = self.eth.block_by_number(block_number, true).await?;
        Ok(block.map(Into::into))
    }

    /// Handler for `getBlockDetailsByHash`
    async fn get_block_details_by_hash(&self, block_hash: B256) -> RpcResult<Option<BlockDetails>> {
        let block = self.eth.block_by_hash(block_hash, true).await?;
        Ok(block.map(Into::into))
    }

    /// Handler for `getBlockTransactions`
    async fn get_block_transactions(
        &self,
        block_number: BlockNumberOrTag,
        page_number: usize,
        page_size: usize,
    ) -> RpcResult<OtsBlockTransactions> {
        // retrieve full block and its receipts
        let block = self.eth.block_by_number(block_number, true);
        let receipts = self.eth.block_receipts(BlockId::Number(block_number));
        let (block, receipts) = futures::try_join!(block, receipts)?;

        let mut block = block.ok_or_else(|| internal_rpc_err("block not found"))?;
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
        _block_number: BlockNumberOrTag,
        _page_size: usize,
    ) -> RpcResult<TransactionsWithReceipts> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `searchTransactionsAfter`
    async fn search_transactions_after(
        &self,
        _address: Address,
        _block_number: BlockNumberOrTag,
        _page_size: usize,
    ) -> RpcResult<TransactionsWithReceipts> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `getTransactionBySenderAndNonce`
    async fn get_transaction_by_sender_and_nonce(
        &self,
        _sender: Address,
        _nonce: u64,
    ) -> RpcResult<Option<Transaction>> {
        Err(internal_rpc_err("unimplemented"))
    }

    /// Handler for `getContractCreator`
    async fn get_contract_creator(&self, address: Address) -> RpcResult<Option<ContractCreator>> {
        if !self.has_code(address, None).await? {
            return Ok(None);
        }

        // use binary search from block [1, latest block number] to find the first block where the
        // contract was deployed
        let mut low = 1;
        let mut high = self.eth.block_number()?.saturating_to::<u64>();
        let mut num = None;

        while low < high {
            let mid = (low + high) / 2;
            let block_number = Some(BlockId::Number(BlockNumberOrTag::Number(mid)));
            if self.eth.get_code(address, block_number).await?.is_empty() {
                low = mid + 1; // not found in current block, need to search in the later blocks
            } else {
                high = mid - 1; // found in current block, try to find a lower block
                num = Some(mid);
            }
        }

        // this should not happen, only if the state of the chain is inconsistent
        let Some(num) = num else {
            debug!(target: "rpc::otterscan", address = ?address, "Contract not found in history state");
            return Err(internal_rpc_err("contract not found in history state"))
        };

        let traces = self
            .eth
            .trace_block_with(
                num.into(),
                TracingInspectorConfig::default_parity(),
                |tx_info, inspector, _, _, _| {
                    Ok(inspector.into_parity_builder().into_localized_transaction_traces(tx_info))
                },
            )
            .await?
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
                                hash: tx_trace.transaction_hash.ok_or_else(|| {
                                    internal_rpc_err("transaction hash is missing")
                                })?,
                                creator,
                            }),
                            _ => None,
                        })
                    })
                    .filter_map(Result::transpose)
                    .collect::<Result<Vec<_>, ErrorObjectOwned>>()
            })
            .transpose()?;

        // A contract maybe created and then destroyed in multiple transactions, here we
        // return the first found transaction, this behavior is consistent with etherscan's
        let found = traces.and_then(|traces| traces.first().cloned());
        Ok(found)
    }
}
