use reth_rpc::RpcTypes;

use std::{collections::HashMap, sync::Arc};

use reth_rpc_eth_api::{helpers::EthCall, EthApiTypes};

use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{
    hex::{FromHex, FromHexError},
    FixedBytes, B256,
};

use jsonrpsee::{
    core::{async_trait, RpcResult},
    proc_macros::rpc,
    types::{error::INVALID_PARAMS_CODE, ErrorObjectOwned},
};

use xlayer_db::{
    internal_transaction_inspector::InternalTransaction,
    utils::{read_table_block, read_table_tx},
};

use reth_ethereum::provider::TransactionsProvider;
use reth_storage_api::BlockIdReader;
use tokio::{
    select,
    task::spawn_blocking,
    time::{interval, sleep_until, Duration, Instant, MissedTickBehavior},
};

const TX_HASH_LENGTH: usize = 66;
const TIMEOUT_DURATION_S: u64 = 3;
const INTERVAL_DELAY_MS: u64 = 10;

fn string_to_b256(hex_str: String) -> Result<B256, FromHexError> {
    let hex = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
    let fb: FixedBytes<32> = FixedBytes::from_hex(hex)?;
    Ok(B256::from(fb))
}

#[rpc(server, namespace = "eth", server_bounds(
    Net: 'static + RpcTypes,                     // Net itself needs no Serde
    <Net as RpcTypes>::TransactionRequest:
        serde::de::DeserializeOwned + serde::Serialize
))]
pub trait XlayerExtApi<Net: RpcTypes> {
    #[method(name = "getInternalTransactions")]
    async fn get_internal_transactions(
        &self,
        tx_hash: String,
    ) -> RpcResult<Vec<InternalTransaction>>;

    #[method(name = "getBlockInternalTransactions")]
    async fn get_block_internal_transactions(
        &self,
        block_number: BlockNumberOrTag,
    ) -> RpcResult<HashMap<String, Vec<InternalTransaction>>>;
}

#[derive(Debug)]
pub struct XlayerExt<T> {
    pub backend: Arc<T>,
}

#[async_trait]
impl<T, Net> XlayerExtApiServer<Net> for XlayerExt<T>
where
    T: EthCall + EthApiTypes<NetworkTypes = Net> + Send + Sync + 'static,
    Net: RpcTypes + Send + Sync + 'static,
{
    async fn get_internal_transactions(
        &self,
        tx_hash: String,
    ) -> RpcResult<Vec<InternalTransaction>> {
        if tx_hash.len() != TX_HASH_LENGTH || !tx_hash.starts_with("0x") {
            return Err(ErrorObjectOwned::owned(
                INVALID_PARAMS_CODE,
                "Invalid transaction hash format",
                None::<()>,
            ));
        }

        let hash = string_to_b256(tx_hash).map_err(|_| {
            ErrorObjectOwned::owned(INVALID_PARAMS_CODE, "Invalid transaction hash", None::<()>)
        })?;

        match self.backend.provider().transaction_by_hash(hash) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(ErrorObjectOwned::owned(-32000, "Transaction not found", None::<()>))
            }
            Err(_) => return Err(ErrorObjectOwned::owned(-32603, "Internal error", None::<()>)),
        }

        let deadline = Instant::now() + Duration::from_secs(TIMEOUT_DURATION_S);
        let mut tick = interval(Duration::from_millis(INTERVAL_DELAY_MS));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            let read = spawn_blocking(move || read_table_tx(hash))
                .await
                .map_err(|_| ())
                .and_then(|r| r.map_err(|_| ()));

            match read {
                Ok(result) if !result.is_empty() => return Ok(result),
                Ok(_) => {}
                Err(_) => {
                    return Err(ErrorObjectOwned::owned(
                        -32603,
                        "Internal error reading transaction data",
                        None::<()>,
                    ))
                }
            }

            select! {
                _ = tick.tick() => {},
                _ = sleep_until(deadline) => {
                    return Err(ErrorObjectOwned::owned(
                        -32000,
                        "Timeout waiting for transaction data",
                        None::<()>,
                    ))
                }
            }
        }
    }

    async fn get_block_internal_transactions(
        &self,
        block_number: BlockNumberOrTag,
    ) -> RpcResult<HashMap<String, Vec<InternalTransaction>>> {
        let hash = match self.backend.provider().block_hash_for_id(block_number.into()) {
            Ok(Some(hash)) => hash,
            Ok(None) => return Err(ErrorObjectOwned::owned(-32000, "Block not found", None::<()>)),
            Err(_) => return Err(ErrorObjectOwned::owned(-32603, "Internal error", None::<()>)),
        };

        let deadline = Instant::now() + Duration::from_secs(TIMEOUT_DURATION_S);
        let mut tick = interval(Duration::from_millis(INTERVAL_DELAY_MS));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let block_txs = loop {
            let read = spawn_blocking(move || read_table_block(hash))
                .await
                .map_err(|_| ())
                .and_then(|r| r.map_err(|_| ()));

            match read {
                Ok(result) if !result.is_empty() => break result,
                Ok(_) => {}
                Err(_) => {
                    return Err(ErrorObjectOwned::owned(
                        -32603,
                        "Internal error reading block data",
                        None::<()>,
                    ))
                }
            }

            select! {
                _ = tick.tick() => {},
                _ = sleep_until(deadline) => {
                    return Err(ErrorObjectOwned::owned(
                        -32000,
                        "Timeout waiting for block data",
                        None::<()>,
                    ))
                }
            }
        };

        let mut result = HashMap::<String, Vec<InternalTransaction>>::default();

        for tx_hash in block_txs {
            let internal_txs_result = read_table_tx(tx_hash);
            if let Err(_) = internal_txs_result {
                return Err(ErrorObjectOwned::owned(
                    -32603,
                    "Internal error reading transaction data",
                    None::<()>,
                ));
            }

            result.insert(tx_hash.to_string(), internal_txs_result.unwrap());
        }

        Ok(result)
    }
}
