use alloy_consensus::Transaction;
use alloy_network::{ReceiptResponse, TransactionResponse};
use alloy_primitives::{Address, Bytes, TxHash, B256, U256};
use alloy_rpc_types::{BlockTransactions, Header, TransactionReceipt};
use alloy_rpc_types_trace::{
    otterscan::{
        BlockDetails, ContractCreator, InternalOperation, OperationType, OtsBlockTransactions,
        OtsReceipt, OtsTransactionReceipt, TraceEntry, TransactionsWithReceipts,
    },
    parity::{Action, CreateAction, CreateOutput, TraceOutput},
};
use async_trait::async_trait;
use jsonrpsee::{core::RpcResult, types::ErrorObjectOwned};
use reth_primitives::{BlockId, BlockNumberOrTag};
use reth_rpc_api::{EthApiServer, OtterscanServer};
use reth_rpc_eth_api::{
    helpers::{EthTransactions, TraceExt},
    FullEthApiTypes, RpcBlock, RpcReceipt, RpcTransaction, TransactionCompat,
};
use reth_rpc_eth_types::{utils::binary_search, EthApiError};
use reth_rpc_server_types::result::internal_rpc_err;
use revm_inspectors::{
    tracing::{types::CallTraceNode, TracingInspectorConfig},
    transfer::{TransferInspector, TransferKind},
};
use revm_primitives::{ExecutionResult, SignedAuthorization};

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
    Eth: FullEthApiTypes,
{
    /// Constructs a `BlockDetails` from a block and its receipts.
    fn block_details(
        &self,
        block: RpcBlock<Eth::NetworkTypes>,
        receipts: Vec<RpcReceipt<Eth::NetworkTypes>>,
    ) -> RpcResult<BlockDetails> {
        // blob fee is burnt, so we don't need to calculate it
        let total_fees = receipts
            .iter()
            .map(|receipt| receipt.gas_used().saturating_mul(receipt.effective_gas_price()))
            .sum::<u128>();

        Ok(BlockDetails::new(block, Default::default(), U256::from(total_fees)))
    }
}

#[async_trait]
impl<Eth> OtterscanServer<RpcTransaction<Eth::NetworkTypes>> for OtterscanApi<Eth>
where
    Eth: EthApiServer<
            RpcTransaction<Eth::NetworkTypes>,
            RpcBlock<Eth::NetworkTypes>,
            RpcReceipt<Eth::NetworkTypes>,
        > + EthTransactions
        + TraceExt
        + 'static,
{
    /// Handler for `{ots,erigon}_getHeaderByNumber`
    async fn get_header_by_number(&self, block_number: u64) -> RpcResult<Option<Header>> {
        self.eth.header_by_number(BlockNumberOrTag::Number(block_number)).await
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
                            TransferKind::EofCreate => OperationType::OpEofCreate,
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
        let block_id = block_number.into();
        let block = self.eth.block_by_number(block_id, true);
        let block_id = block_id.into();
        let receipts = self.eth.block_receipts(block_id);
        let (block, receipts) = futures::try_join!(block, receipts)?;
        self.block_details(
            block.ok_or(EthApiError::HeaderNotFound(block_id))?,
            receipts.ok_or(EthApiError::ReceiptsNotFound(block_id))?,
        )
    }

    /// Handler for `getBlockDetailsByHash`
    async fn get_block_details_by_hash(&self, block_hash: B256) -> RpcResult<BlockDetails> {
        let block = self.eth.block_by_hash(block_hash, true);
        let block_id = block_hash.into();
        let receipts = self.eth.block_receipts(block_id);
        let (block, receipts) = futures::try_join!(block, receipts)?;
        self.block_details(
            block.ok_or(EthApiError::HeaderNotFound(block_id))?,
            receipts.ok_or(EthApiError::ReceiptsNotFound(block_id))?,
        )
    }

    /// Handler for `getBlockTransactions`
    async fn get_block_transactions(
        &self,
        block_number: u64,
        page_number: usize,
        page_size: usize,
    ) -> RpcResult<OtsBlockTransactions<RpcTransaction<Eth::NetworkTypes>>> {
        let block_id = block_number.into();
        // retrieve full block and its receipts
        let block = self.eth.block_by_number(block_id, true);
        let block_id = block_id.into();
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

        // Crop page
        let page_end = tx_len.saturating_sub(page_number * page_size);
        let page_start = page_end.saturating_sub(page_size);

        // Crop transactions
        *transactions = transactions.drain(page_start..page_end).collect::<Vec<_>>();

        // The input field returns only the 4 bytes method selector instead of the entire
        // calldata byte blob.
        for tx in transactions.iter_mut() {
            if tx.input().len() > 4 {
                Eth::TransactionCompat::otterscan_api_truncate_input(tx);
            }
        }

        // Crop receipts and transform them into OtsTransactionReceipt
        let timestamp = Some(block.header.timestamp);
        let receipts = receipts
            .drain(page_start..page_end)
            .zip(transactions.iter().map(Eth::TransactionCompat::tx_type))
            .map(|(receipt, tx_ty)| {
                let inner = OtsReceipt {
                    status: receipt.status(),
                    cumulative_gas_used: receipt.cumulative_gas_used() as u64,
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
                    state_root: receipt.state_root(),
                    authorization_list: receipt
                        .authorization_list()
                        .map(<[SignedAuthorization]>::to_vec),
                };

                OtsTransactionReceipt { receipt, timestamp }
            })
            .collect();

        // use `transaction_count` to indicate the paginate information
        let mut block = OtsBlockTransactions { fullblock: block.into(), receipts };
        block.fullblock.transaction_count = tx_len;
        Ok(block)
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
        Ok(self
            .eth
            .get_transaction_by_sender_and_nonce(sender, nonce, false)
            .await
            .map_err(Into::into)?
            .map(|tx| tx.tx_hash()))
    }

    /// Handler for `getContractCreator`
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
                                    .ok_or(EthApiError::TransactionNotFound)?,
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
        let found = traces.and_then(|traces| traces.first().copied());
        Ok(found)
    }
}
