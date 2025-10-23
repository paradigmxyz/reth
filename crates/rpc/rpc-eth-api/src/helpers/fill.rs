//! Fills transaction fields and simulates execution.

use super::{estimate::EstimateCall, Call, EthFees, LoadPendingBlock, LoadState, SpawnBlocking};
use crate::{FromEthApiError, RpcNodeCore};
use alloy_consensus::BlockHeader;
use alloy_network::TransactionBuilder;
use alloy_primitives::U256;
use alloy_rpc_types_eth::{state::StateOverride, BlockId};
use futures::Future;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_rpc_convert::{RpcConvert, RpcTxReq};
use reth_rpc_eth_types::{EthApiError, RpcInvalidTransactionError};
use reth_storage_api::BlockIdReader;
use tracing::trace;

/// Fills transaction fields for the [`EthApiServer`](crate::EthApiServer) trait in
/// the `eth_` namespace.
///
/// This trait provides functionality to fill missing transaction fields (nonce, gas, fees, chain
/// id).
pub trait FillTransaction: Call + EstimateCall + EthFees + LoadPendingBlock + LoadState {
    /// Fills missing fields in a transaction request.
    fn fill_transaction(
        &self,
        mut request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        block_id: BlockId,
        state_override: Option<StateOverride>,
    ) -> impl Future<Output = Result<RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>, Self::Error>>
           + Send
    where
        Self: SpawnBlocking,
    {
        async move {
            let provider = RpcNodeCore::provider(self);
            let chain_spec = provider.chain_spec();

            let header_fut = async {
                let is_post_london = match block_id {
                    BlockId::Number(num) => self
                        .provider()
                        .convert_block_number(num)
                        .map_err(Self::Error::from_eth_err)?
                        .map(|block_number| chain_spec.is_london_active_at_block(block_number))
                        .unwrap_or(false),
                    _ => false, // Need to fetch header to determine
                };

                if !is_post_london {
                    return Ok(None);
                }

                // Post-London: fetch header for base fee
                let block_hash = provider
                    .block_hash_for_id(block_id)
                    .map_err(Self::Error::from_eth_err)?
                    .ok_or_else(|| EthApiError::HeaderNotFound(block_id))
                    .map_err(Self::Error::from_eth_err)?;

                self.cache()
                    .get_header(block_hash)
                    .await
                    .map_err(Self::Error::from_eth_err)
                    .map(Some)
            };

            let chain_id_fut = async {
                if request.as_ref().chain_id().is_none() {
                    let (evm_env, _) = self.evm_env_at(block_id).await?;
                    Ok(Some(evm_env.cfg_env.chain_id))
                } else {
                    Ok(None)
                }
            };

            let nonce_fut = async {
                if request.as_ref().nonce().is_none() &&
                    let Some(from) = request.as_ref().from()
                {
                    let state = self.state_at_block_id(block_id).await?;
                    let nonce = state
                        .account_nonce(&from)
                        .map_err(Self::Error::from_eth_err)?
                        .unwrap_or_default();
                    return Ok(Some(nonce));
                }
                Ok(None)
            };

            let (header, chain_id, nonce) =
                futures::try_join!(header_fut, chain_id_fut, nonce_fut)?;

            if let Some(chain_id) = chain_id {
                request.as_mut().set_chain_id(chain_id);
            }

            if let Some(nonce) = nonce {
                request.as_mut().set_nonce(nonce);
            }

            let base_fee = header.and_then(|h| h.base_fee_per_gas());
            if let Some(base_fee) = base_fee {
                // Derive EIP-1559 fee fields
                let suggested_priority_fee = EthFees::suggested_priority_fee(self).await?;

                if request.as_ref().max_priority_fee_per_gas().is_none() {
                    request.as_mut().set_max_priority_fee_per_gas(suggested_priority_fee.to());
                }

                if request.as_ref().max_fee_per_gas().is_none() {
                    let max_fee = suggested_priority_fee.saturating_add(U256::from(base_fee));
                    request.as_mut().set_max_fee_per_gas(max_fee.to());
                }
            } else {
                // Derive legacy gas price field
                if request.as_ref().max_fee_per_gas().is_some() ||
                    request.as_ref().max_priority_fee_per_gas().is_some()
                {
                    return Err(Self::Error::from_eth_err(EthApiError::InvalidTransaction(
                        RpcInvalidTransactionError::TxTypeNotSupported,
                    )));
                }

                if request.as_ref().gas_price().is_none() {
                    let gas_price = EthFees::gas_price(self).await?;
                    request.as_mut().set_gas_price(gas_price.to());
                }
            }

            // TODO: Fill blob fee for EIP-4844 transactions

            let gas_limit = EstimateCall::estimate_gas_at(
                self,
                request.clone(),
                block_id,
                state_override.clone(),
            )
            .await?;

            // Set the estimated gas if not already set by the user
            if request.as_ref().gas_limit().is_none() {
                request.as_mut().set_gas_limit(gas_limit.to());
            }

            trace!(
                target: "rpc::eth",
                ?request,
                "Filled transaction"
            );

            Ok(request)
        }
    }
}
