//! `eth_` RPC API for pubsub subscription.

use alloy_json_rpc::RpcObject;
use alloy_rpc_types_eth::pubsub::{Params, SubscriptionKind, TransactionReceiptsParams};
use jsonrpsee::{core::SubscriptionResult, PendingSubscriptionSink, RpcModule};
use jsonrpsee_types::ErrorObjectOwned;
use reth_rpc_server_types::result::invalid_params_rpc_err;
use serde_json::Value;

/// Ethereum pub-sub RPC interface.
pub trait EthPubSubApiServer<T: RpcObject>: Sized + Send + Sync + 'static {
    /// Handles a subscription after its kind-specific arguments have been decoded.
    fn subscribe(
        &self,
        subscription_sink: PendingSubscriptionSink,
        kind: SubscriptionKind,
        params: Option<Params>,
    ) -> impl Future<Output = SubscriptionResult> + Send;

    /// Registers `eth_subscribe` and `eth_unsubscribe` on a new RPC module.
    /// The first positional argument determines how the second one is decoded.
    fn into_rpc(self) -> RpcModule<Self> {
        let mut module = RpcModule::new(self);
        let registration = module.register_subscription(
            "eth_subscribe",
            "eth_subscription",
            "eth_unsubscribe",
            |params, pending, context| async move {
                match decode_subscription(params) {
                    Ok((kind, params)) => context.subscribe(pending, kind, params).await,
                    Err(err) => {
                        pending.reject(err).await;
                        Ok(())
                    }
                }
            },
        );
        assert!(registration.is_ok(), "subscription names must be unique in a new module");
        module
    }
}

/// Decodes receipt objects before Alloy normalizes log fields such as `"topics": []` to a wildcard.
fn decode_subscription(
    params: jsonrpsee_types::Params<'_>,
) -> Result<(SubscriptionKind, Option<Params>), ErrorObjectOwned> {
    let mut args = params.sequence();
    let kind = args.next()?;
    if kind != SubscriptionKind::TransactionReceipts {
        return Ok((kind, args.optional_next()?))
    }

    let invalid_filter = || invalid_params_rpc_err("Invalid params for transactionReceipts");
    let filter = match args.optional_next::<Value>()? {
        None => None,
        // Alloy serializes Params::None as an empty array.
        Some(Value::Array(items)) if items.is_empty() => Some(Params::None),
        Some(Value::Object(fields)) if fields.keys().all(|key| key == "transactionHashes") => {
            let filter = serde_json::from_value::<TransactionReceiptsParams>(Value::Object(fields))
                .map_err(|_| invalid_filter())?;
            Some(Params::TransactionReceipts(filter))
        }
        _ => return Err(invalid_filter()),
    };
    Ok((kind, filter))
}
