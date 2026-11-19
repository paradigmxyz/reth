//! `eth_` RPC API for pubsub subscription.

use alloy_json_rpc::RpcObject;
use alloy_primitives::TxHash;
use alloy_rpc_types_eth::pubsub::{Params, SubscriptionKind, TransactionReceiptsParams};
use jsonrpsee::{
    core::{__reexports::log_fail_parse, SubscriptionResult},
    PendingSubscriptionSink, RpcModule,
};
use jsonrpsee_types::{
    params::{invalid_params_with_data, ParamsSequence},
    ErrorObjectOwned,
};
use serde::Deserialize;
use serde_json::value::RawValue;

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
    let Some(raw) = args.optional_next::<&RawValue>()? else { return Ok(None) };
    // Alloy serializes Params::None as an empty array.
    let filter = if raw.get().starts_with('[') {
        <[(); 0]>::deserialize(raw).map(|[]| Params::None)
    } else {
        ReceiptFilter::deserialize(raw).map(Params::TransactionReceipts)
    };
    filter
        .map(Some)
        .map_err(|err| invalid_params_with_data("Invalid params for transactionReceipts", err))
}

/// Enforces the receipt filter's wire fields before any JSON object normalization.
#[derive(Deserialize)]
#[serde(
    remote = "TransactionReceiptsParams",
    rename_all = "camelCase",
    deny_unknown_fields,
    expecting = "a receipt filter object"
)]
struct ReceiptFilter {
    transaction_hashes: Option<Vec<TxHash>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_filter_rejects_invalid_fields() {
        for (filter, detail) in [
            (
                r#"{"transactionHash":[]}"#,
                "unknown field `transactionHash`, expected `transactionHashes` at line 1 column 18",
            ),
            (
                r#"{"address":[]}"#,
                "unknown field `address`, expected `transactionHashes` at line 1 column 10",
            ),
            (
                r#"{"topics":[],"transactionHashes":[]}"#,
                "unknown field `topics`, expected `transactionHashes` at line 1 column 9",
            ),
            (
                r#"{"transactionHashes":null,"transactionHashes":[]}"#,
                "duplicate field `transactionHashes` at line 1 column 45",
            ),
        ] {
            let raw = format!(r#"["transactionReceipts",{filter}]"#);
            let err = decode_subscription(jsonrpsee_types::Params::new(Some(&raw))).unwrap_err();
            assert_eq!(err.code(), jsonrpsee_types::error::INVALID_PARAMS_CODE);
            assert_eq!(err.data().unwrap().get(), serde_json::to_string(detail).unwrap());
        }
    }
}
