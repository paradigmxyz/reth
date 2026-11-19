//! `eth_` RPC API for pubsub subscription.

use alloy_json_rpc::RpcObject;
use alloy_rpc_types_eth::pubsub::{Params, SubscriptionKind, TransactionReceiptsParams};
use jsonrpsee::{
    core::{__reexports::log_fail_parse, SubscriptionResult},
    PendingSubscriptionSink, RpcModule,
};
use jsonrpsee_types::{
    params::{invalid_params_with_data, ParamsSequence},
    ErrorObjectOwned,
};
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
    let kind =
        args.next().inspect_err(|err| log_fail_parse("kind", "SubscriptionKind", err, false))?;
    let params = if kind == SubscriptionKind::TransactionReceipts {
        decode_receipt_filter(args)
    } else {
        args.optional_next()
    }
    .inspect_err(|err| log_fail_parse("params", "Option<Params>", err, true))?;
    Ok((kind, params))
}

fn decode_receipt_filter(mut args: ParamsSequence<'_>) -> Result<Option<Params>, ErrorObjectOwned> {
    const INVALID_FILTER: &str = "Invalid params for transactionReceipts";
    match args.optional_next::<Value>()? {
        None => Ok(None),
        // Alloy serializes Params::None as an empty array.
        Some(Value::Array(items)) if items.is_empty() => Ok(Some(Params::None)),
        Some(Value::Object(fields)) if fields.keys().all(|key| key == "transactionHashes") => {
            let filter = serde_json::from_value::<TransactionReceiptsParams>(Value::Object(fields))
                .map_err(|err| invalid_params_with_data(INVALID_FILTER, err))?;
            Ok(Some(Params::TransactionReceipts(filter)))
        }
        _ => Err(invalid_params_rpc_err(INVALID_FILTER)),
    }
}
