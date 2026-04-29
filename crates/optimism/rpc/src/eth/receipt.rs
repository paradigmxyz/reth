//! Loads and formats OP receipt RPC response.

use crate::{eth::RpcNodeCore, OpEthApi, OpEthApiError};
use alloy_consensus::{BlockHeader, Receipt, ReceiptWithBloom, TxReceipt};
use alloy_eips::{eip2718::Encodable2718, BlockHashOrNumber};
use alloy_primitives::{b256, B256, U256};
use alloy_rpc_types_eth::{Log, TransactionReceipt};
use op_alloy_consensus::OpTransaction;
use op_alloy_rpc_types::{L1BlockInfo, OpTransactionReceipt, OpTransactionReceiptFields};
use op_revm::{
    constants::{GAS_ORACLE_CONTRACT, TOKEN_RATIO_SLOT},
    estimate_tx_compressed_size,
};
use parking_lot::Mutex;
use reth_chainspec::ChainSpecProvider;
use reth_mantle_forks::MantleHardforks;
use reth_node_api::NodePrimitives;
use reth_optimism_evm::RethL1BlockInfo;
use reth_optimism_primitives::OpReceipt;
use reth_primitives_traits::{BlockBody, Receipt as ReceiptTrait, SealedBlock};
use reth_rpc_eth_api::{
    helpers::LoadReceipt,
    transaction::{ConvertReceiptInput, ReceiptConverter},
    RpcConvert,
};
use reth_rpc_eth_types::{receipt::build_receipt, EthApiError};
use reth_storage_api::{BlockReader, ReceiptProvider, StateProviderFactory};
use schnellru::{ByLength, LruMap};
use std::{fmt::Debug, sync::Arc};

const TOKEN_RATIO_UPDATED_TOPIC: U256 = U256::from_be_bytes(
    b256!("5d6ae9db2d6725497bed0302a8212c0db5fdb3bd7d14f188a83b5589089caafd").0,
);

const MAX_REASONABLE_TOKEN_RATIO: u128 = 1_000_000_000;
const TOKEN_RATIO_PREFIX_CACHE_MAX_BLOCKS: usize = 1024;

type TokenRatioPrefixCache = Arc<Mutex<LruMap<B256, Arc<Vec<U256>>, ByLength>>>;

/// Returns `token_ratio` after applying any `TokenRatioUpdated` event in the given logs.
fn token_ratio_after_logs(mut current: U256, logs: &[alloy_primitives::Log]) -> U256 {
    for log in logs {
        if log.address != GAS_ORACLE_CONTRACT {
            continue;
        }
        let topics = log.topics();
        let is_token_ratio_updated = topics
            .first()
            .is_some_and(|t| U256::from_be_bytes(t.0) == TOKEN_RATIO_UPDATED_TOPIC) ||
            (topics.len() == 2);
        if is_token_ratio_updated && let Some(new_ratio) = topics.last() {
            let new_ratio_val = U256::from_be_bytes(new_ratio.0);
            if new_ratio_val <= U256::from(MAX_REASONABLE_TOKEN_RATIO) {
                current = new_ratio_val;
            }
        }
    }
    current
}

fn has_full_block_indices(
    block_tx_count: usize,
    indices: impl ExactSizeIterator<Item = u64>,
) -> bool {
    indices.len() == block_tx_count &&
        indices.enumerate().all(|(idx, input_index)| input_index == idx as u64)
}

fn build_token_ratio_prefixes_from_logs<'a>(
    initial_ratio: U256,
    logs_by_receipt: impl IntoIterator<Item = &'a [alloy_primitives::Log]>,
) -> Vec<U256> {
    let mut ratio_before = vec![initial_ratio];
    let mut current = initial_ratio;

    for logs in logs_by_receipt {
        current = token_ratio_after_logs(current, logs);
        ratio_before.push(current);
    }

    ratio_before
}

fn get_or_insert_token_ratio_prefix(
    cache: &TokenRatioPrefixCache,
    block_hash: B256,
    build: impl FnOnce() -> Option<Arc<Vec<U256>>>,
) -> Option<Arc<Vec<U256>>> {
    if let Some(cached) = cache.lock().get(&block_hash).cloned() {
        return Some(cached);
    }

    let computed = build()?;
    cache.lock().insert(block_hash, computed.clone());
    Some(computed)
}

impl<N, Rpc> LoadReceipt for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError>,
{
}

/// Converter for OP receipts.
#[derive(Debug, Clone)]
pub struct OpReceiptConverter<Provider> {
    provider: Provider,
    token_ratio_prefix_cache: TokenRatioPrefixCache,
}

impl<Provider> OpReceiptConverter<Provider> {
    /// Creates a new [`OpReceiptConverter`].
    pub fn new(provider: Provider) -> Self {
        Self {
            provider,
            token_ratio_prefix_cache: Arc::new(Mutex::new(LruMap::new(ByLength::new(
                TOKEN_RATIO_PREFIX_CACHE_MAX_BLOCKS as u32,
            )))),
        }
    }
}

impl<Provider, N> ReceiptConverter<N> for OpReceiptConverter<Provider>
where
    N: NodePrimitives<SignedTx: OpTransaction, Receipt = OpReceipt>,
    Provider: BlockReader<Block = N::Block>
        + ChainSpecProvider<ChainSpec: MantleHardforks>
        + ReceiptProvider<Receipt: ReceiptTrait>
        + StateProviderFactory
        + Debug
        + 'static,
{
    type RpcReceipt = OpTransactionReceipt;
    type Error = OpEthApiError;

    fn convert_receipts(
        &self,
        inputs: Vec<ConvertReceiptInput<'_, N>>,
    ) -> Result<Vec<Self::RpcReceipt>, Self::Error> {
        let Some(block_number) = inputs.first().map(|r| r.meta.block_number) else {
            return Ok(Vec::new());
        };

        let block = self
            .provider
            .block_by_number(block_number)?
            .ok_or(EthApiError::HeaderNotFound(block_number.into()))?;

        self.convert_receipts_with_block(inputs, &SealedBlock::new_unhashed(block))
    }

    fn convert_receipts_with_block(
        &self,
        inputs: Vec<ConvertReceiptInput<'_, N>>,
        block: &SealedBlock<N::Block>,
    ) -> Result<Vec<Self::RpcReceipt>, Self::Error> {
        let mut l1_block_info = match reth_optimism_evm::extract_l1_info(block.body()) {
            Ok(l1_block_info) => l1_block_info,
            Err(err) => {
                // If it is the genesis block (i.e. block number is 0), there is no L1 info, so
                // we return an empty l1_block_info.
                if block.header().number() == 0 {
                    return Ok(vec![]);
                }
                return Err(err.into());
            }
        };

        // Get token ratio from parent block state (start of current block)
        // If we can't get parent state (unlikely for non-genesis), fallback to current block state
        let state = self
            .provider
            .state_by_block_hash(block.parent_hash())
            .or_else(|_| self.provider.state_by_block_hash(block.hash()))?;
        let mut token_ratio =
            state.storage(GAS_ORACLE_CONTRACT, TOKEN_RATIO_SLOT.into())?.unwrap_or_default();

        let block_tx_count = block.body().transactions().len();
        let block_hash = B256::from(*block.hash());
        let has_full_block_inputs =
            has_full_block_indices(block_tx_count, inputs.iter().map(|input| input.meta.index));

        // For single-receipt requests (e.g. eth_getTransactionReceipt), we only get one input, so
        // we never see earlier tx logs and token_ratio stays at parent state. Load full block
        // receipts and compute token_ratio before each tx so the requested receipt gets the
        // correct ratio (e.g. after TokenRatioUpdated in a previous tx).
        let token_ratio_before_tx: Option<Arc<Vec<U256>>> = if has_full_block_inputs {
            None
        } else {
            get_or_insert_token_ratio_prefix(&self.token_ratio_prefix_cache, block_hash, || {
                self.provider
                    .receipts_by_block(BlockHashOrNumber::Hash(block_hash))
                    .ok()
                    .flatten()
                    .filter(|all_receipts| all_receipts.len() == block_tx_count)
                    .map(|all_receipts| {
                        Arc::new(build_token_ratio_prefixes_from_logs(
                            token_ratio,
                            all_receipts.iter().map(|receipt| receipt.logs()),
                        ))
                    })
            })
        };

        let mut receipts = Vec::with_capacity(inputs.len());

        for input in inputs {
            let ratio_for_this_tx = token_ratio_before_tx
                .as_ref()
                .and_then(|rb| rb.get(input.meta.index as usize).copied())
                .unwrap_or(token_ratio);
            l1_block_info.token_ratio = ratio_for_this_tx;

            // Update running token_ratio when we did not precompute (before consuming input)
            if token_ratio_before_tx.is_none() {
                token_ratio = token_ratio_after_logs(token_ratio, input.receipt.logs());
            }

            l1_block_info.clear_tx_l1_cost();

            let receipt =
                OpReceiptBuilder::new(&self.provider.chain_spec(), input, &mut l1_block_info)?
                    .build();

            receipts.push(receipt);
        }

        Ok(receipts)
    }
}

/// L1 fee and data gas for a non-deposit transaction, or deposit nonce and receipt version for a
/// deposit transaction.
#[derive(Debug, Clone)]
pub struct OpReceiptFieldsBuilder {
    /// Block number.
    pub block_number: u64,
    /// Block timestamp.
    pub block_timestamp: u64,
    /// The L1 fee for transaction.
    pub l1_fee: Option<u128>,
    /// L1 gas used by transaction.
    pub l1_data_gas: Option<u128>,
    /// L1 fee scalar.
    pub l1_fee_scalar: Option<f64>,
    /* ---------------------------------------- Bedrock ---------------------------------------- */
    /// The base fee of the L1 origin block.
    pub l1_base_fee: Option<u128>,
    /* --------------------------------------- Regolith ---------------------------------------- */
    /// Deposit nonce, if this is a deposit transaction.
    pub deposit_nonce: Option<u64>,
    /* --------------------------------------- Mantle ---------------------------------------- */
    /// The token ratio.
    pub token_ratio: Option<u128>,
    /* ---------------------------------------- Canyon ----------------------------------------- */
    /// Deposit receipt version, if this is a deposit transaction.
    pub deposit_receipt_version: Option<u64>,
    /* ---------------------------------------- Ecotone ---------------------------------------- */
    /// The current L1 fee scalar.
    pub l1_base_fee_scalar: Option<u128>,
    /// The current L1 blob base fee.
    pub l1_blob_base_fee: Option<u128>,
    /// The current L1 blob base fee scalar.
    pub l1_blob_base_fee_scalar: Option<u128>,
    /* ---------------------------------------- Isthmus ---------------------------------------- */
    /// The current operator fee scalar.
    pub operator_fee_scalar: Option<u128>,
    /// The current L1 blob base fee scalar.
    pub operator_fee_constant: Option<u128>,
    /* ---------------------------------------- Jovian ----------------------------------------- */
    /// The current DA footprint gas scalar.
    pub da_footprint_gas_scalar: Option<u16>,
}

impl OpReceiptFieldsBuilder {
    /// Returns a new builder.
    pub const fn new(block_timestamp: u64, block_number: u64) -> Self {
        Self {
            block_number,
            block_timestamp,
            l1_fee: None,
            l1_data_gas: None,
            l1_fee_scalar: None,
            l1_base_fee: None,
            deposit_nonce: None,
            token_ratio: None,
            deposit_receipt_version: None,
            l1_base_fee_scalar: None,
            l1_blob_base_fee: None,
            l1_blob_base_fee_scalar: None,
            operator_fee_scalar: None,
            operator_fee_constant: None,
            da_footprint_gas_scalar: None,
        }
    }

    /// Applies [`L1BlockInfo`](op_revm::L1BlockInfo).
    pub fn l1_block_info<T: Encodable2718 + OpTransaction>(
        mut self,
        chain_spec: &impl MantleHardforks,
        tx: &T,
        l1_block_info: &mut op_revm::L1BlockInfo,
    ) -> Result<Self, OpEthApiError> {
        let raw_tx = tx.encoded_2718();
        let timestamp = self.block_timestamp;

        self.l1_fee = Some(
            l1_block_info
                .l1_tx_data_fee(chain_spec, timestamp, &raw_tx, tx.is_deposit())
                .map_err(|_| OpEthApiError::L1BlockFeeError)?
                .saturating_to(),
        );

        self.l1_data_gas = Some(
            l1_block_info
                .l1_data_gas(chain_spec, timestamp, &raw_tx)
                .map_err(|_| OpEthApiError::L1BlockGasError)?
                .saturating_add(l1_block_info.l1_fee_overhead.unwrap_or_default())
                .saturating_to(),
        );

        self.l1_fee_scalar = (!chain_spec.is_ecotone_active_at_timestamp(timestamp))
            .then_some(f64::from(l1_block_info.l1_base_fee_scalar) / 1_000_000.0);

        self.l1_base_fee = Some(l1_block_info.l1_base_fee.saturating_to());
        self.l1_base_fee_scalar = Some(l1_block_info.l1_base_fee_scalar.saturating_to());
        self.l1_blob_base_fee = l1_block_info.l1_blob_base_fee.map(|fee| fee.saturating_to());
        self.l1_blob_base_fee_scalar =
            l1_block_info.l1_blob_base_fee_scalar.map(|scalar| scalar.saturating_to());

        // Align with geth: explicit zero operator fee values should still be emitted as 0x0.
        // Only truly absent values (`None`) remain omitted in JSON serialization.
        self.operator_fee_scalar =
            l1_block_info.operator_fee_scalar.map(|scalar| scalar.saturating_to());
        self.operator_fee_constant =
            l1_block_info.operator_fee_constant.map(|constant| constant.saturating_to());

        self.token_ratio = Some(l1_block_info.token_ratio.saturating_to());

        self.da_footprint_gas_scalar = l1_block_info.da_footprint_gas_scalar;

        Ok(self)
    }

    /// Applies deposit transaction metadata: deposit nonce.
    pub const fn deposit_nonce(mut self, nonce: Option<u64>) -> Self {
        self.deposit_nonce = nonce;
        self
    }

    // /// Applies deposit transaction metadata: deposit receipt version.
    // pub const fn deposit_version(mut self, version: Option<u64>) -> Self {
    //     self.deposit_receipt_version = version;
    //     self
    // }

    /// Builds the [`OpTransactionReceiptFields`] object.
    pub const fn build(self) -> OpTransactionReceiptFields {
        let Self {
            block_number: _,    // used to compute other fields
            block_timestamp: _, // used to compute other fields
            l1_fee,
            l1_data_gas: l1_gas_used,
            l1_fee_scalar,
            l1_base_fee: l1_gas_price,
            deposit_nonce,
            token_ratio,
            deposit_receipt_version,
            l1_base_fee_scalar,
            l1_blob_base_fee,
            l1_blob_base_fee_scalar,
            operator_fee_scalar,
            operator_fee_constant,
            da_footprint_gas_scalar,
        } = self;

        OpTransactionReceiptFields {
            l1_block_info: L1BlockInfo {
                l1_gas_price,
                l1_gas_used,
                l1_fee,
                l1_fee_scalar,
                l1_base_fee_scalar,
                l1_blob_base_fee,
                l1_blob_base_fee_scalar,
                operator_fee_scalar,
                operator_fee_constant,
                token_ratio,
                da_footprint_gas_scalar,
            },
            deposit_nonce,
            deposit_receipt_version,
        }
    }
}

/// Builds an [`OpTransactionReceipt`].
#[derive(Debug)]
pub struct OpReceiptBuilder {
    /// Core receipt, has all the fields of an L1 receipt and is the basis for the OP receipt.
    pub core_receipt: TransactionReceipt<ReceiptWithBloom<op_alloy_consensus::OpReceipt<Log>>>,
    /// Additional OP receipt fields.
    pub op_receipt_fields: OpTransactionReceiptFields,
}

impl OpReceiptBuilder {
    /// Returns a new builder.
    pub fn new<N>(
        chain_spec: &impl MantleHardforks,
        input: ConvertReceiptInput<'_, N>,
        l1_block_info: &mut op_revm::L1BlockInfo,
    ) -> Result<Self, OpEthApiError>
    where
        N: NodePrimitives<SignedTx: OpTransaction, Receipt = OpReceipt>,
    {
        let timestamp = input.meta.timestamp;
        let block_number = input.meta.block_number;
        let tx_signed = *input.tx.inner();
        let mut core_receipt = build_receipt(input, None, |receipt, next_log_index, meta| {
            let map_logs = move |receipt: alloy_consensus::Receipt| {
                let Receipt { status, cumulative_gas_used, logs } = receipt;
                let logs = Log::collect_for_receipt(next_log_index, meta, logs);
                Receipt { status, cumulative_gas_used, logs }
            };
            let logs_bloom = receipt.bloom();
            match receipt {
                OpReceipt::Legacy(r) => ReceiptWithBloom {
                    receipt: op_alloy_consensus::OpReceipt::Legacy(map_logs(r)),
                    logs_bloom,
                },
                OpReceipt::Eip2930(r) => ReceiptWithBloom {
                    receipt: op_alloy_consensus::OpReceipt::Eip2930(map_logs(r)),
                    logs_bloom,
                },
                OpReceipt::Eip1559(r) => ReceiptWithBloom {
                    receipt: op_alloy_consensus::OpReceipt::Eip1559(map_logs(r)),
                    logs_bloom,
                },
                OpReceipt::Eip7702(r) => ReceiptWithBloom {
                    receipt: op_alloy_consensus::OpReceipt::Eip7702(map_logs(r)),
                    logs_bloom,
                },
                OpReceipt::Deposit(r) => ReceiptWithBloom {
                    receipt: op_alloy_consensus::OpReceipt::Deposit(
                        op_alloy_consensus::OpDepositReceipt {
                            deposit_nonce: r.deposit_nonce,
                            deposit_receipt_version: r.deposit_receipt_version,
                            inner: map_logs(r.inner),
                        },
                    ),
                    logs_bloom,
                },
            }
        });

        // Geth only adds L1 fee fields for non-deposit transactions.
        // For deposit transactions, only depositNonce is included (no L1 fee fields or
        // blobGasUsed).
        let is_deposit = tx_signed.is_deposit();

        // In jovian, we're using the blob gas used field to store the current da
        // footprint's value.
        // We're computing the jovian blob gas used before building the receipt since the inputs get
        // consumed by the `build_receipt` function.
        // Note: blobGasUsed is only set for non-deposit transactions to match geth behavior.
        if !is_deposit {
            chain_spec.is_jovian_active_at_timestamp(timestamp).then(|| {
                // Estimate the size of the transaction in bytes and multiply by the DA
                // footprint gas scalar.
                // Jovian specs: `https://github.com/ethereum-optimism/specs/blob/main/specs/protocol/jovian/exec-engine.md#da-footprint-block-limit`
                let da_size = estimate_tx_compressed_size(tx_signed.encoded_2718().as_slice())
                    .saturating_div(1_000_000)
                    .saturating_mul(
                        l1_block_info.da_footprint_gas_scalar.unwrap_or_default().into(),
                    );

                core_receipt.blob_gas_used = Some(da_size);
            });
        }

        // Build receipt fields: for deposit transactions, skip L1 fee fields to match geth
        // behavior.
        let op_receipt_fields = if is_deposit {
            let deposit_nonce = match &core_receipt.inner.receipt {
                op_alloy_consensus::OpReceipt::Deposit(d) => d.deposit_nonce,
                _ => None,
            };
            OpReceiptFieldsBuilder::new(timestamp, block_number)
                .deposit_nonce(deposit_nonce)
                .build()
        } else {
            OpReceiptFieldsBuilder::new(timestamp, block_number)
                .l1_block_info(chain_spec, tx_signed, l1_block_info)?
                .build()
        };

        Ok(Self { core_receipt, op_receipt_fields })
    }

    /// Builds [`OpTransactionReceipt`] by combining core (l1) receipt fields and additional OP
    /// receipt fields.
    pub fn build(self) -> OpTransactionReceipt {
        let Self { core_receipt: inner, op_receipt_fields } = self;

        let OpTransactionReceiptFields { l1_block_info, .. } = op_receipt_fields;

        OpTransactionReceipt { inner, l1_block_info }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use alloy_consensus::{transaction::TransactionMeta, Block, BlockBody, Eip658Value, TxEip7702};
    use alloy_primitives::{hex, Address, Bytes, LogData, Signature, B256, U256};
    use op_alloy_consensus::OpTypedTransaction;
    use op_alloy_network::eip2718::Decodable2718;
    use reth_mantle_forks::{MantleChainHardforks, MANTLE_MAINNET_ARSIA_TIMESTAMP};
    use reth_optimism_chainspec::{BASE_MAINNET, OP_MAINNET};
    use reth_optimism_forks::OpHardforks;
    use reth_optimism_primitives::{OpPrimitives, OpTransactionSigned};
    use reth_primitives_traits::Recovered;
    use std::{cell::Cell, hint::black_box, time::Instant};

    fn topic_from_u128(value: u128) -> B256 {
        U256::from(value).to_be_bytes::<32>().into()
    }

    fn token_ratio_log(address: Address, topics: Vec<B256>) -> alloy_primitives::Log {
        alloy_primitives::Log { address, data: LogData::new_unchecked(topics, Bytes::new()) }
    }

    /// OP Mainnet transaction at index 0 in block 124665056.
    ///
    /// <https://optimistic.etherscan.io/tx/0x312e290cf36df704a2217b015d6455396830b0ce678b860ebfcc30f41403d7b1>
    const TX_SET_L1_BLOCK_OP_MAINNET_BLOCK_124665056: [u8; 251] = hex!(
        "7ef8f8a0683079df94aa5b9cf86687d739a60a9b4f0835e520ec4d664e2e415dca17a6df94deaddeaddeaddeaddeaddeaddeaddeaddead00019442000000000000000000000000000000000000158080830f424080b8a4440a5e200000146b000f79c500000000000000040000000066d052e700000000013ad8a3000000000000000000000000000000000000000000000000000000003ef1278700000000000000000000000000000000000000000000000000000000000000012fdf87b89884a61e74b322bbcf60386f543bfae7827725efaaf0ab1de2294a590000000000000000000000006887246668a3b87f54deb3b94ba47a6f63f32985"
    );

    /// OP Mainnet transaction at index 1 in block 124665056.
    ///
    /// <https://optimistic.etherscan.io/tx/0x1059e8004daff32caa1f1b1ef97fe3a07a8cf40508f5b835b66d9420d87c4a4a>
    const TX_1_OP_MAINNET_BLOCK_124665056: [u8; 1176] = hex!(
        "02f904940a8303fba78401d6d2798401db2b6d830493e0943e6f4f7866654c18f536170780344aa8772950b680b904246a761202000000000000000000000000087000a300de7200382b55d40045000000e5d60e0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000014000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000003a0000000000000000000000000000000000000000000000000000000000000022482ad56cb0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000000000000000120000000000000000000000000dc6ff44d5d932cbd77b52e5612ba0529dc6226f1000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000600000000000000000000000000000000000000000000000000000000000000044095ea7b300000000000000000000000021c4928109acb0659a88ae5329b5374a3024694c0000000000000000000000000000000000000000000000049b9ca9a6943400000000000000000000000000000000000000000000000000000000000000000000000000000000000021c4928109acb0659a88ae5329b5374a3024694c000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000600000000000000000000000000000000000000000000000000000000000000024b6b55f250000000000000000000000000000000000000000000000049b9ca9a694340000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000415ec214a3950bea839a7e6fbb0ba1540ac2076acd50820e2d5ef83d0902cdffb24a47aff7de5190290769c4f0a9c6fabf63012986a0d590b1b571547a8c7050ea1b00000000000000000000000000000000000000000000000000000000000000c080a06db770e6e25a617fe9652f0958bd9bd6e49281a53036906386ed39ec48eadf63a07f47cf51a4a40b4494cf26efc686709a9b03939e20ee27e59682f5faa536667e"
    );

    /// Timestamp of OP mainnet block 124665056.
    ///
    /// <https://optimistic.etherscan.io/block/124665056>
    const BLOCK_124665056_TIMESTAMP: u64 = 1724928889;

    /// L1 block info for transaction at index 1 in block 124665056.
    ///
    /// <https://optimistic.etherscan.io/tx/0x1059e8004daff32caa1f1b1ef97fe3a07a8cf40508f5b835b66d9420d87c4a4a>
    const TX_META_TX_1_OP_MAINNET_BLOCK_124665056: OpTransactionReceiptFields =
        OpTransactionReceiptFields {
            l1_block_info: L1BlockInfo {
                l1_gas_price: Some(1055991687), // since bedrock l1 base fee
                l1_gas_used: Some(4471),
                l1_fee: Some(24681034813),
                l1_fee_scalar: None,
                l1_base_fee_scalar: Some(5227),
                l1_blob_base_fee: Some(1),
                l1_blob_base_fee_scalar: Some(1014213),
                operator_fee_scalar: None,
                operator_fee_constant: None,
                token_ratio: None,
                da_footprint_gas_scalar: None,
            },
            deposit_nonce: None,
            deposit_receipt_version: None,
        };

    /// Decoding the L1 block tx (0x7e) hex can yield RlpError(Overflow) with current
    /// `op_alloy_consensus/alloy_rlp`; ignore until upstream fix or test vectors updated.
    #[test]
    #[ignore = "OpTransactionSigned::decode_2718(L1 block tx) returns RlpError(Overflow)"]
    fn op_receipt_fields_from_block_and_tx() {
        // rig
        let tx_0 = OpTransactionSigned::decode_2718(
            &mut TX_SET_L1_BLOCK_OP_MAINNET_BLOCK_124665056.as_slice(),
        )
        .unwrap();

        let tx_1 =
            OpTransactionSigned::decode_2718(&mut TX_1_OP_MAINNET_BLOCK_124665056.as_slice())
                .unwrap();

        let block: Block<OpTransactionSigned> = Block {
            body: BlockBody { transactions: [tx_0, tx_1.clone()].to_vec(), ..Default::default() },
            ..Default::default()
        };

        let mut l1_block_info =
            reth_optimism_evm::extract_l1_info(&block.body).expect("should extract l1 info");

        // test
        assert!(OP_MAINNET.is_fjord_active_at_timestamp(BLOCK_124665056_TIMESTAMP));

        let receipt_meta = OpReceiptFieldsBuilder::new(BLOCK_124665056_TIMESTAMP, 124665056)
            .l1_block_info(&*OP_MAINNET, &tx_1, &mut l1_block_info)
            .expect("should parse revm l1 info")
            .build();

        let L1BlockInfo {
            l1_gas_price,
            l1_gas_used,
            l1_fee,
            l1_fee_scalar,
            l1_base_fee_scalar,
            l1_blob_base_fee,
            l1_blob_base_fee_scalar,
            operator_fee_scalar,
            operator_fee_constant,
            token_ratio,
            da_footprint_gas_scalar,
        } = receipt_meta.l1_block_info;

        assert_eq!(
            l1_gas_price, TX_META_TX_1_OP_MAINNET_BLOCK_124665056.l1_block_info.l1_gas_price,
            "incorrect l1 base fee (former gas price)"
        );
        assert_eq!(
            l1_gas_used, TX_META_TX_1_OP_MAINNET_BLOCK_124665056.l1_block_info.l1_gas_used,
            "incorrect l1 gas used"
        );
        assert_eq!(
            l1_fee, TX_META_TX_1_OP_MAINNET_BLOCK_124665056.l1_block_info.l1_fee,
            "incorrect l1 fee"
        );
        assert_eq!(
            l1_fee_scalar, TX_META_TX_1_OP_MAINNET_BLOCK_124665056.l1_block_info.l1_fee_scalar,
            "incorrect l1 fee scalar"
        );
        assert_eq!(
            l1_base_fee_scalar,
            TX_META_TX_1_OP_MAINNET_BLOCK_124665056.l1_block_info.l1_base_fee_scalar,
            "incorrect l1 base fee scalar"
        );
        assert_eq!(
            l1_blob_base_fee,
            TX_META_TX_1_OP_MAINNET_BLOCK_124665056.l1_block_info.l1_blob_base_fee,
            "incorrect l1 blob base fee"
        );
        assert_eq!(
            l1_blob_base_fee_scalar,
            TX_META_TX_1_OP_MAINNET_BLOCK_124665056.l1_block_info.l1_blob_base_fee_scalar,
            "incorrect l1 blob base fee scalar"
        );
        assert_eq!(
            operator_fee_scalar,
            TX_META_TX_1_OP_MAINNET_BLOCK_124665056.l1_block_info.operator_fee_scalar,
            "incorrect operator fee scalar"
        );
        assert_eq!(
            operator_fee_constant,
            TX_META_TX_1_OP_MAINNET_BLOCK_124665056.l1_block_info.operator_fee_constant,
            "incorrect operator fee constant"
        );
        assert_eq!(
            token_ratio, TX_META_TX_1_OP_MAINNET_BLOCK_124665056.l1_block_info.token_ratio,
            "incorrect token ratio"
        );
        assert_eq!(
            da_footprint_gas_scalar,
            TX_META_TX_1_OP_MAINNET_BLOCK_124665056.l1_block_info.da_footprint_gas_scalar,
            "incorrect da footprint gas scalar"
        );
    }

    #[test]
    fn op_non_zero_operator_fee_params_included_in_receipt() {
        let tx_1 =
            OpTransactionSigned::decode_2718(&mut TX_1_OP_MAINNET_BLOCK_124665056.as_slice())
                .unwrap();

        let mut l1_block_info = op_revm::L1BlockInfo {
            operator_fee_scalar: Some(U256::from(0)),
            operator_fee_constant: Some(U256::from(2)),
            ..Default::default()
        };

        let receipt_meta = OpReceiptFieldsBuilder::new(BLOCK_124665056_TIMESTAMP, 124665056)
            .l1_block_info(&*OP_MAINNET, &tx_1, &mut l1_block_info)
            .expect("should parse revm l1 info")
            .build();

        let L1BlockInfo { operator_fee_scalar, operator_fee_constant, .. } =
            receipt_meta.l1_block_info;

        assert_eq!(operator_fee_scalar, Some(0), "incorrect operator fee scalar");
        assert_eq!(operator_fee_constant, Some(2), "incorrect operator fee constant");
    }

    #[test]
    fn op_explicit_zero_operator_fee_params_included_in_receipt() {
        let tx_1 =
            OpTransactionSigned::decode_2718(&mut TX_1_OP_MAINNET_BLOCK_124665056.as_slice())
                .unwrap();

        let mut l1_block_info = op_revm::L1BlockInfo {
            operator_fee_scalar: Some(U256::ZERO),
            operator_fee_constant: Some(U256::ZERO),
            ..Default::default()
        };

        let receipt_meta = OpReceiptFieldsBuilder::new(BLOCK_124665056_TIMESTAMP, 124665056)
            .l1_block_info(&*OP_MAINNET, &tx_1, &mut l1_block_info)
            .expect("should parse revm l1 info")
            .build();

        let L1BlockInfo { operator_fee_scalar, operator_fee_constant, .. } =
            receipt_meta.l1_block_info;

        assert_eq!(operator_fee_scalar, Some(0), "incorrect operator fee scalar");
        assert_eq!(operator_fee_constant, Some(0), "incorrect operator fee constant");
    }

    #[test]
    fn op_zero_operator_fee_params_not_included_in_receipt() {
        let tx_1 =
            OpTransactionSigned::decode_2718(&mut TX_1_OP_MAINNET_BLOCK_124665056.as_slice())
                .unwrap();

        let mut l1_block_info = op_revm::L1BlockInfo::default();

        let receipt_meta = OpReceiptFieldsBuilder::new(BLOCK_124665056_TIMESTAMP, 124665056)
            .l1_block_info(&*OP_MAINNET, &tx_1, &mut l1_block_info)
            .expect("should parse revm l1 info")
            .build();

        let L1BlockInfo { operator_fee_scalar, operator_fee_constant, .. } =
            receipt_meta.l1_block_info;

        assert_eq!(operator_fee_scalar, None, "incorrect operator fee scalar");
        assert_eq!(operator_fee_constant, None, "incorrect operator fee constant");
    }

    #[test]
    fn token_ratio_update_from_signature_topic_uses_last_topic_as_new_ratio() {
        let current = U256::from(10);
        let logs = vec![token_ratio_log(
            GAS_ORACLE_CONTRACT,
            vec![
                TOKEN_RATIO_UPDATED_TOPIC.to_be_bytes::<32>().into(),
                topic_from_u128(10),
                topic_from_u128(25),
            ],
        )];

        assert_eq!(token_ratio_after_logs(current, &logs), U256::from(25));
    }

    #[test]
    fn token_ratio_update_accepts_two_topic_fallback_shape() {
        let current = U256::from(1);
        let logs = vec![token_ratio_log(
            GAS_ORACLE_CONTRACT,
            vec![topic_from_u128(1), topic_from_u128(7)],
        )];

        assert_eq!(token_ratio_after_logs(current, &logs), U256::from(7));
    }

    #[test]
    fn token_ratio_update_ignores_unreasonable_values() {
        let current = U256::from(42);
        let logs = vec![token_ratio_log(
            GAS_ORACLE_CONTRACT,
            vec![
                TOKEN_RATIO_UPDATED_TOPIC.to_be_bytes::<32>().into(),
                topic_from_u128(42),
                topic_from_u128(MAX_REASONABLE_TOKEN_RATIO + 1),
            ],
        )];

        assert_eq!(token_ratio_after_logs(current, &logs), U256::from(42));
    }

    #[test]
    fn token_ratio_updates_apply_in_order_across_multiple_logs() {
        let current = U256::from(5);
        let logs = vec![
            token_ratio_log(
                GAS_ORACLE_CONTRACT,
                vec![
                    TOKEN_RATIO_UPDATED_TOPIC.to_be_bytes::<32>().into(),
                    topic_from_u128(5),
                    topic_from_u128(8),
                ],
            ),
            token_ratio_log(Address::ZERO, vec![topic_from_u128(1), topic_from_u128(999)]),
            token_ratio_log(GAS_ORACLE_CONTRACT, vec![topic_from_u128(8), topic_from_u128(12)]),
        ];

        assert_eq!(token_ratio_after_logs(current, &logs), U256::from(12));
    }

    #[test]
    fn full_block_indices_detected_only_for_sequential_range() {
        assert!(has_full_block_indices(3, [0, 1, 2].into_iter()));
        assert!(!has_full_block_indices(3, [0, 2, 1].into_iter()));
        assert!(!has_full_block_indices(3, [0, 1].into_iter()));
        assert!(!has_full_block_indices(3, [1, 2, 3].into_iter()));
    }

    #[test]
    fn token_ratio_prefixes_store_pre_tx_ratios() {
        let logs_by_receipt = [
            vec![token_ratio_log(
                GAS_ORACLE_CONTRACT,
                vec![
                    TOKEN_RATIO_UPDATED_TOPIC.to_be_bytes::<32>().into(),
                    topic_from_u128(1),
                    topic_from_u128(10),
                ],
            )],
            vec![],
            vec![token_ratio_log(
                GAS_ORACLE_CONTRACT,
                vec![
                    TOKEN_RATIO_UPDATED_TOPIC.to_be_bytes::<32>().into(),
                    topic_from_u128(10),
                    topic_from_u128(25),
                ],
            )],
        ];

        let prefixes = build_token_ratio_prefixes_from_logs(
            U256::from(1),
            logs_by_receipt.iter().map(Vec::as_slice),
        );

        assert_eq!(prefixes, vec![U256::from(1), U256::from(10), U256::from(10), U256::from(25)]);
    }

    #[test]
    fn token_ratio_prefix_cache_reuses_existing_entry() {
        let cache: TokenRatioPrefixCache = Arc::new(Mutex::new(LruMap::new(ByLength::new(2))));
        let block_hash = B256::from([7_u8; 32]);
        let build_calls = Cell::new(0_u32);

        let first = get_or_insert_token_ratio_prefix(&cache, block_hash, || {
            build_calls.set(build_calls.get() + 1);
            Some(Arc::new(vec![U256::ZERO, U256::from(9)]))
        })
        .expect("expected first build");

        let second = get_or_insert_token_ratio_prefix(&cache, block_hash, || {
            build_calls.set(build_calls.get() + 1);
            None
        })
        .expect("expected cached value");

        assert_eq!(build_calls.get(), 1);
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    #[ignore = "manual perf benchmark"]
    fn benchmark_token_ratio_prefix_cache_vs_uncached_queries() {
        const RECEIPT_COUNT: usize = 1_200;
        const LOGS_PER_RECEIPT: usize = 40;
        const QUERY_COUNT: usize = 8_000;

        let logs_by_receipt = (0..RECEIPT_COUNT)
            .map(|tx_idx| {
                (0..LOGS_PER_RECEIPT)
                    .map(|log_idx| {
                        if log_idx % 9 == 0 {
                            token_ratio_log(
                                GAS_ORACLE_CONTRACT,
                                vec![
                                    TOKEN_RATIO_UPDATED_TOPIC.to_be_bytes::<32>().into(),
                                    topic_from_u128(((tx_idx + log_idx) % 1000) as u128),
                                    topic_from_u128(((tx_idx + log_idx + 1) % 1000) as u128),
                                ],
                            )
                        } else {
                            token_ratio_log(Address::ZERO, vec![topic_from_u128(log_idx as u128)])
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        let query_indices =
            (0..QUERY_COUNT).map(|i| (i.saturating_mul(7919)) % RECEIPT_COUNT).collect::<Vec<_>>();

        let uncached_start = Instant::now();
        let mut uncached_sink = U256::ZERO;
        for &idx in &query_indices {
            let prefix = build_token_ratio_prefixes_from_logs(
                U256::from(1),
                logs_by_receipt.iter().map(Vec::as_slice),
            );
            uncached_sink ^= black_box(prefix[idx]);
        }
        let uncached_elapsed = uncached_start.elapsed();

        let cached_start = Instant::now();
        let cached_prefix = build_token_ratio_prefixes_from_logs(
            U256::from(1),
            logs_by_receipt.iter().map(Vec::as_slice),
        );
        let mut cached_sink = U256::ZERO;
        for &idx in &query_indices {
            cached_sink ^= black_box(cached_prefix[idx]);
        }
        let cached_elapsed = cached_start.elapsed();

        eprintln!(
            "token-ratio benchmark -> uncached={:?}, cached={:?}, speedup={:.2}x, sinks=({:?},{:?})",
            uncached_elapsed,
            cached_elapsed,
            uncached_elapsed.as_secs_f64() / cached_elapsed.as_secs_f64(),
            uncached_sink,
            cached_sink
        );
    }

    #[test]
    #[ignore = "manual perf benchmark"]
    fn benchmark_full_block_prefetch_vs_streaming_token_ratio_scan() {
        const RECEIPT_COUNT: usize = 1_200;
        const LOGS_PER_RECEIPT: usize = 40;

        let logs_by_receipt = (0..RECEIPT_COUNT)
            .map(|tx_idx| {
                (0..LOGS_PER_RECEIPT)
                    .map(|log_idx| {
                        if log_idx % 9 == 0 {
                            token_ratio_log(
                                GAS_ORACLE_CONTRACT,
                                vec![
                                    TOKEN_RATIO_UPDATED_TOPIC.to_be_bytes::<32>().into(),
                                    topic_from_u128(((tx_idx + log_idx) % 1000) as u128),
                                    topic_from_u128(((tx_idx + log_idx + 1) % 1000) as u128),
                                ],
                            )
                        } else {
                            token_ratio_log(Address::ZERO, vec![topic_from_u128(log_idx as u128)])
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        // Legacy full-block flow: scan every tx log once by precomputing prefixes from an
        // independently loaded full-receipt list.
        let prefetch_start = Instant::now();
        let prefix = build_token_ratio_prefixes_from_logs(
            U256::from(1),
            logs_by_receipt.iter().map(Vec::as_slice),
        );
        let mut prefetch_sink = U256::ZERO;
        for (idx, logs) in logs_by_receipt.iter().enumerate() {
            prefetch_sink ^= black_box(prefix[idx]);
            black_box(logs.len());
        }
        let prefetch_elapsed = prefetch_start.elapsed();

        // New full-block flow: skip the extra prefetch and update token_ratio while consuming
        // the already available input receipts.
        let streaming_start = Instant::now();
        let mut current = U256::from(1);
        let mut streaming_sink = U256::ZERO;
        for logs in &logs_by_receipt {
            streaming_sink ^= black_box(current);
            current = token_ratio_after_logs(current, logs);
        }
        let streaming_elapsed = streaming_start.elapsed();

        eprintln!(
            "full-block benchmark -> prefetch={:?}, streaming={:?}, speedup={:.2}x, sinks=({:?},{:?})",
            prefetch_elapsed,
            streaming_elapsed,
            prefetch_elapsed.as_secs_f64() / streaming_elapsed.as_secs_f64(),
            prefetch_sink,
            streaming_sink
        );
    }

    // <https://github.com/paradigmxyz/reth/issues/12177>
    /// Decoding the L1 block (0x7e) system tx hex can yield RlpError(Overflow); ignore until fixed.
    #[test]
    #[ignore = "OpTransactionSigned::decode_2718(system L1 tx) returns RlpError(Overflow)"]
    fn base_receipt_gas_fields() {
        // https://basescan.org/tx/0x510fd4c47d78ba9f97c91b0f2ace954d5384c169c9545a77a373cf3ef8254e6e
        let system = hex!(
            "7ef8f8a0389e292420bcbf9330741f72074e39562a09ff5a00fd22e4e9eee7e34b81bca494deaddeaddeaddeaddeaddeaddeaddeaddead00019442000000000000000000000000000000000000158080830f424080b8a4440a5e20000008dd00101c120000000000000004000000006721035b00000000014189960000000000000000000000000000000000000000000000000000000349b4dcdc000000000000000000000000000000000000000000000000000000004ef9325cc5991ce750960f636ca2ffbb6e209bb3ba91412f21dd78c14ff154d1930f1f9a0000000000000000000000005050f69a9786f081509234f1a7f4684b5e5b76c9"
        );
        let tx_0 = OpTransactionSigned::decode_2718(&mut &system[..]).unwrap();

        let block: alloy_consensus::Block<OpTransactionSigned> = Block {
            body: BlockBody { transactions: vec![tx_0], ..Default::default() },
            ..Default::default()
        };
        let mut l1_block_info =
            reth_optimism_evm::extract_l1_info(&block.body).expect("should extract l1 info");

        // https://basescan.org/tx/0xf9420cbaf66a2dda75a015488d37262cbfd4abd0aad7bb2be8a63e14b1fa7a94
        let tx = hex!(
            "02f86c8221058034839a4ae283021528942f16386bb37709016023232523ff6d9daf444be380841249c58bc080a001b927eda2af9b00b52a57be0885e0303c39dd2831732e14051c2336470fd468a0681bf120baf562915841a48601c2b54a6742511e535cf8f71c95115af7ff63bd"
        );
        let tx_1 = OpTransactionSigned::decode_2718(&mut &tx[..]).unwrap();

        let receipt_meta = OpReceiptFieldsBuilder::new(1730216981, 21713817)
            .l1_block_info(&*BASE_MAINNET, &tx_1, &mut l1_block_info)
            .expect("should parse revm l1 info")
            .build();

        let L1BlockInfo {
            l1_gas_price,
            l1_gas_used,
            l1_fee,
            l1_fee_scalar,
            l1_base_fee_scalar,
            l1_blob_base_fee,
            l1_blob_base_fee_scalar,
            operator_fee_scalar,
            operator_fee_constant,
            token_ratio,
            da_footprint_gas_scalar,
        } = receipt_meta.l1_block_info;

        assert_eq!(l1_gas_price, Some(14121491676), "incorrect l1 base fee (former gas price)");
        assert_eq!(l1_gas_used, Some(1600), "incorrect l1 gas used");
        assert_eq!(l1_fee, Some(191150293412), "incorrect l1 fee");
        assert!(l1_fee_scalar.is_none(), "incorrect l1 fee scalar");
        assert_eq!(l1_base_fee_scalar, Some(2269), "incorrect l1 base fee scalar");
        assert_eq!(l1_blob_base_fee, Some(1324954204), "incorrect l1 blob base fee");
        assert_eq!(l1_blob_base_fee_scalar, Some(1055762), "incorrect l1 blob base fee scalar");
        assert_eq!(operator_fee_scalar, None, "incorrect operator fee scalar");
        assert_eq!(operator_fee_constant, None, "incorrect operator fee constant");
        assert_eq!(token_ratio, None, "incorrect token ratio");
        assert_eq!(da_footprint_gas_scalar, None, "incorrect da footprint gas scalar");
    }

    #[test]
    fn da_footprint_gas_scalar_included_in_receipt_post_jovian() {
        const DA_FOOTPRINT_GAS_SCALAR: u16 = 10;

        let tx = TxEip7702 {
            chain_id: 1u64,
            nonce: 0,
            max_fee_per_gas: 0x28f000fff,
            max_priority_fee_per_gas: 0x28f000fff,
            gas_limit: 10,
            to: Address::default(),
            value: U256::from(3_u64),
            input: Bytes::from(vec![1, 2]),
            access_list: Default::default(),
            authorization_list: Default::default(),
        };

        let signature = Signature::new(U256::default(), U256::default(), true);

        let tx = OpTransactionSigned::new_unhashed(OpTypedTransaction::Eip7702(tx), signature);

        let mut l1_block_info = op_revm::L1BlockInfo {
            da_footprint_gas_scalar: Some(DA_FOOTPRINT_GAS_SCALAR),
            ..Default::default()
        };

        let mantle_hardforks = MantleChainHardforks::mantle_mainnet();

        let receipt = OpReceiptFieldsBuilder::new(MANTLE_MAINNET_ARSIA_TIMESTAMP, u64::MAX)
            .l1_block_info(&mantle_hardforks, &tx, &mut l1_block_info)
            .expect("should parse revm l1 info")
            .build();

        assert_eq!(receipt.l1_block_info.da_footprint_gas_scalar, Some(DA_FOOTPRINT_GAS_SCALAR));
    }

    #[test]
    fn blob_gas_used_included_in_receipt_post_jovian() {
        const DA_FOOTPRINT_GAS_SCALAR: u16 = 100;
        let tx = TxEip7702 {
            chain_id: 1u64,
            nonce: 0,
            max_fee_per_gas: 0x28f000fff,
            max_priority_fee_per_gas: 0x28f000fff,
            gas_limit: 10,
            to: Address::default(),
            value: U256::from(3_u64),
            access_list: Default::default(),
            authorization_list: Default::default(),
            input: Bytes::from(vec![0; 1_000_000]),
        };

        let signature = Signature::new(U256::default(), U256::default(), true);

        let tx = OpTransactionSigned::new_unhashed(OpTypedTransaction::Eip7702(tx), signature);

        let mut l1_block_info = op_revm::L1BlockInfo {
            da_footprint_gas_scalar: Some(DA_FOOTPRINT_GAS_SCALAR),
            ..Default::default()
        };

        let mantle_hardforks = MantleChainHardforks::mantle_mainnet();

        let op_receipt = OpReceiptBuilder::new(
            &mantle_hardforks,
            ConvertReceiptInput::<OpPrimitives> {
                tx: Recovered::new_unchecked(&tx, Address::default()),
                receipt: OpReceipt::Eip7702(Receipt {
                    status: Eip658Value::Eip658(true),
                    cumulative_gas_used: 100,
                    logs: vec![],
                }),
                gas_used: 100,
                next_log_index: 0,
                meta: TransactionMeta {
                    timestamp: MANTLE_MAINNET_ARSIA_TIMESTAMP,
                    ..Default::default()
                },
            },
            &mut l1_block_info,
        )
        .unwrap();

        let expected_blob_gas_used = estimate_tx_compressed_size(tx.encoded_2718().as_slice())
            .saturating_div(1_000_000)
            .saturating_mul(DA_FOOTPRINT_GAS_SCALAR.into());

        assert_eq!(op_receipt.core_receipt.blob_gas_used, Some(expected_blob_gas_used));
    }

    #[test]
    fn blob_gas_used_not_included_in_receipt_pre_jovian() {
        const DA_FOOTPRINT_GAS_SCALAR: u16 = 100;
        let tx = TxEip7702 {
            chain_id: 1u64,
            nonce: 0,
            max_fee_per_gas: 0x28f000fff,
            max_priority_fee_per_gas: 0x28f000fff,
            gas_limit: 10,
            to: Address::default(),
            value: U256::from(3_u64),
            access_list: Default::default(),
            authorization_list: Default::default(),
            input: Bytes::from(vec![0; 1_000_000]),
        };

        let signature = Signature::new(U256::default(), U256::default(), true);

        let tx = OpTransactionSigned::new_unhashed(OpTypedTransaction::Eip7702(tx), signature);

        let mut l1_block_info = op_revm::L1BlockInfo {
            da_footprint_gas_scalar: Some(DA_FOOTPRINT_GAS_SCALAR),
            ..Default::default()
        };

        let mantle_hardforks = MantleChainHardforks::mantle_mainnet();

        let op_receipt = OpReceiptBuilder::new(
            &mantle_hardforks,
            ConvertReceiptInput::<OpPrimitives> {
                tx: Recovered::new_unchecked(&tx, Address::default()),
                receipt: OpReceipt::Eip7702(Receipt {
                    status: Eip658Value::Eip658(true),
                    cumulative_gas_used: 100,
                    logs: vec![],
                }),
                gas_used: 100,
                next_log_index: 0,
                meta: TransactionMeta {
                    timestamp: MANTLE_MAINNET_ARSIA_TIMESTAMP - 1,
                    ..Default::default()
                },
            },
            &mut l1_block_info,
        )
        .unwrap();

        assert_eq!(op_receipt.core_receipt.blob_gas_used, None);
    }
}
