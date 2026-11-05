// Copyright 2019-2021 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! # jsonrpsee-proc-macros
//!
//! Vendored from [jsonrpsee](https://github.com/paritytech/jsonrpsee), Copyright (c) 2019-2021
//! Parity Technologies (UK) Ltd., licensed under the MIT License (see
//! `crates/rpc/jsonrpsee/LICENSE`).

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

use proc_macro::TokenStream;
use rpc_macro::RpcDescription;

mod attributes;
mod helpers;
mod render_server;
mod rpc_macro;
pub(crate) mod visitor;

/// Main RPC macro.
///
/// ## Description
///
/// This macro generates the server implementation of an RPC trait, a trait mostly equivalent to
/// the input, with the following differences:
///
/// - The trait will have one additional (already implemented) method, `into_rpc`, which turns any
///   object that implements the server trait into an `RpcModule`.
/// - For subscription methods:
///   - There will be one additional argument inserted right after `&self`: `subscription_sink:
///     SubscriptionSink`. It should be used to accept or reject a subscription and send data back
///     to the subscribers.
///   - The return type of the subscription method must implement `IntoSubscriptionCloseResponse`.
///
/// For the `Foo` trait, the server trait will be named `FooServer`, which must be implemented on
/// your type.
///
/// Note: you need to import the `jsonrpsee` façade crate in your code for the macro to work
/// properly.
///
/// Parameters are positional: they must be sent as a JSON array, like geth expects. Requests with
/// object (named) parameters are answered with an invalid params error.
///
/// ## Prerequisites
///
/// - Implementors of the server trait must be `Sync`, `Send`, `Sized` and `'static`. If you want to
///   implement this trait on some type that is not thread-safe, consider using `Arc<RwLock<..>>`.
///
/// ## Examples
///
/// Below you can find examples of the macro usage along with the code
/// that generated for it by the macro.
///
/// ```ignore
/// #[rpc(server, namespace = "foo", namespace_separator = ".")]
/// pub trait Rpc {
///     #[method(name = "foo")]
///     async fn async_method(&self, param_a: u8, param_b: String) -> u16;
///     #[method(name = "bar")]
///     fn sync_method(&self) -> String;
///
///     #[subscription(name = "subscribe", item = String)]
///     async fn sub(&self) -> SubscriptionResult;
/// }
/// ```
///
/// Server code that will be generated:
///
/// ```ignore
/// pub trait RpcServer {
///     // RPC methods are normal methods and can be either sync or async. Async methods return a
///     // `Send` future, implementations can use `async fn`.
///     fn async_method(&self, param_a: u8, param_b: String) -> impl Future<Output = u16> + Send;
///     fn sync_method(&self) -> String;
///
///     // Note that `pending_subscription_sink` was added automatically.
///     fn sub(&self, pending: PendingSubscriptionSink) -> impl Future<Output = SubscriptionResult> + Send;
///
///     fn into_rpc(self) -> Result<Self, jsonrpsee::core::Error> {
///         // Actual implementation stripped, but inside we will create
///         // a module with one method and one subscription
///     }
/// }
/// ```
///
/// ## Attributes
///
/// ### `rpc` attribute
///
/// `rpc` attribute is applied to a trait in order to turn it into an RPC implementation.
///
/// **Arguments:**
///
/// - `server` (mandatory): generate `<Trait>Server` trait for the server implementation.
/// - `namespace`: add a prefix to all the methods and subscriptions in this RPC. For example, with
///   namespace `foo` and method `spam`, the resulting method name will be `foo_spam`.
/// - `namespace_separator`: customize the separator used between namespace and method name.
///   Defaults to `_`. For example, `namespace = "foo", namespace_separator = "."` results in method
///   names like `foo.bar` instead of `foo_bar`.
/// - `server_bounds`: replace *all* auto-generated trait bounds with the user-defined ones for the
///   server implementation.
///
/// **Trait requirements:**
///
/// A trait wrapped with the `rpc` attribute **must not**:
///
/// - have associated types or constants;
/// - have Rust methods not marked with either the `method` or `subscription` attribute;
/// - be empty.
///
/// ### `method` attribute
///
/// `method` attribute is used to define an RPC method.
///
/// **Arguments:**
///
/// - `name` (mandatory): name of the RPC method. Does not have to be the same as the Rust method
///   name.
/// - `aliases`: list of name aliases for the RPC method as a comma separated string. Aliases are
///   processed ignoring the namespace, so add the complete name, including the namespace.
/// - `blocking`: when set method execution will always spawn on a dedicated thread. Only usable
///   with non-`async` methods.
///
/// **Method requirements:**
///
/// A Rust method marked with the `method` attribute, **may**:
///
/// - be either `async` or not;
/// - have input parameters or not;
/// - have a return value or not (in the latter case, it will be considered a notification method).
///
/// ### `subscription` attribute
///
/// `subscription` attribute is used to define a publish/subscribe interface according to the [ethereum pubsub specification](https://geth.ethereum.org/docs/rpc/pubsub)
///
/// **Arguments:**
///
/// - `name` (mandatory): name of the RPC method. Does not have to be the same as the Rust method
///   name.
/// - `unsubscribe` (optional): name of the RPC method to unsubscribe from the subscription. Must
///   not be the same as `name`. This is generated for you if the subscription name starts with
///   `subscribe`.
/// - `aliases` (optional): aliases for `name`. Aliases are processed ignoring the namespace, so add
///   the complete name, including the namespace.
/// - `unsubscribe_aliases` (optional): Similar to `aliases` but for `unsubscribe`.
/// - `item` (mandatory): type of items yielded by the subscription. Note that it must be the type,
///   not string.
///
/// **Method requirements:**
///
/// Rust method marked with the `subscription` attribute **must**:
///
/// - be asynchronous;
/// - return a type that implements `jsonrpsee::server::IntoSubscriptionCloseResponse`.
///
/// Rust method marked with `subscription` attribute **may**:
///
/// - have input parameters or not.
///
/// ## Full workflow example
///
/// ```rust
/// //! Example of using proc macro to generate a working server.
///
/// use std::net::SocketAddr;
///
/// use jsonrpsee::server::ServerBuilder;
///
/// // RPC is put into a separate module to clearly show names of generated entities.
/// mod rpc_impl {
///     use jsonrpsee::proc_macros::rpc;
///     use jsonrpsee::server::{PendingSubscriptionSink, SubscriptionMessage, IntoSubscriptionCloseResponse, SubscriptionCloseResponse};
///     use jsonrpsee::core::{RpcResult, SubscriptionResult, to_json_raw_value};
///
///     pub enum CloseResponse {
///         None,
///         Failed,
///     }
///
///     impl IntoSubscriptionCloseResponse for CloseResponse {
///         fn into_response(self) -> SubscriptionCloseResponse {
///            match self {
///                // Do not send a close response when the subscription is terminated.
///                CloseResponse::None => SubscriptionCloseResponse::None,
///                // Send a close response as an ordinary subscription notification
///                // when the subscription is terminated.
///                CloseResponse::Failed => {
///                     let err = to_json_raw_value(&"Failed").unwrap();
///                     SubscriptionCloseResponse::Notif(err.into())
///                }
///            }
///         }
///     }
///
///     // Generate the server implementation, prepend all the methods with `foo_` prefix.
///     #[rpc(server, namespace = "foo")]
///     pub trait MyRpc {
///         #[method(name = "foo")]
///         async fn async_method(
///             &self,
///             param_a: u8,
///             param_b: String
///         ) -> RpcResult<u16>;
///
///         #[method(name = "bar")]
///         fn sync_method(&self) -> RpcResult<u16>;
///
///         /// Override the `foo_sub` and use `foo_subNotif` for the notifications.
///         ///
///         /// The item field indicates which type goes into result field below.
///         ///
///         /// The notification format:
///         ///
///         /// ```
///         /// {
///         ///     "jsonrpc":"2.0",
///         ///     "method":"foo_subNotif",
///         ///     "params":["subscription":"someID", "result":"some string"]
///         /// }
///         /// ```
///         #[subscription(name = "sub" => "subNotif", unsubscribe = "unsub", item = String)]
///         async fn sub_override_notif_method(&self) -> SubscriptionResult;
///
///         /// Use the same method name for both the `subscribe call` and `notifications`
///         ///
///         /// The unsubscribe method name is generated here `foo_unsubscribe`
///         /// Thus the `unsubscribe attribute` is not needed unless a custom unsubscribe method name is wanted.
///         ///
///         /// The notification format:
///         ///
///         /// ```
///         /// {
///         ///     "jsonrpc":"2.0",
///         ///     "method":"foo_subscribe",
///         ///     "params":["subscription":"someID", "result":"some string"]
///         /// }
///         /// ```
///         #[subscription(name = "subscribe", item = String)]
///         async fn sub(&self) -> SubscriptionResult;
///
///         #[subscription(name = "sub_custom_close_msg", unsubscribe = "unsub_custom_close_msg", item = String)]
///         async fn sub_custom_close_msg(&self) -> CloseResponse;
///     }
///
///     // Structure that will implement the `MyRpcServer` trait.
///     // It can have fields, if required, as long as it's still `Send + Sync + 'static`.
///     pub struct RpcServerImpl;
///
///     // Note that the trait name we use is `MyRpcServer`, not `MyRpc`!
///     impl MyRpcServer for RpcServerImpl {
///         async fn async_method(&self, _param_a: u8, _param_b: String) -> RpcResult<u16> {
///             Ok(42)
///         }
///
///         fn sync_method(&self) -> RpcResult<u16> {
///             Ok(10)
///         }
///
///         // The stream API can be used to pipe items from the underlying stream
///         // as subscription responses.
///         async fn sub_override_notif_method(&self, pending: PendingSubscriptionSink) -> SubscriptionResult {
///             let mut sink = pending.accept().await?;
///             let msg = to_json_raw_value(&"Response_A").unwrap();
///             sink.send(msg).await?;
///             Ok(())
///         }
///
///         // Send out two values on the subscription.
///         async fn sub(&self, pending: PendingSubscriptionSink) -> SubscriptionResult {
///             let sink = pending.accept().await?;
///
///             let msg1 = to_json_raw_value(&"Response_A").unwrap();
///             let msg2 = to_json_raw_value(&"Response_B").unwrap();
///
///             sink.send(msg1).await?;
///             sink.send(msg2).await?;
///
///             Ok(())
///         }
///
///         // If one doesn't want sent out a close message when a subscription terminates or treat
///         // errors as subscription error notifications then it's possible to implement
///         // `IntoSubscriptionCloseResponse` for customized behavior.
///         async fn sub_custom_close_msg(&self, pending: PendingSubscriptionSink) -> CloseResponse {
///             let Ok(sink) = pending.accept().await else {
///                 return CloseResponse::None;
///             };
///
///             let msg = to_json_raw_value(&"Response_A").unwrap();
///
///             if sink.send(msg).await.is_ok() {
///                 CloseResponse::Failed
///             } else {
///                 CloseResponse::None
///             }
///         }
///     }
/// }
///
/// // Use the generated server implementation.
/// use rpc_impl::{MyRpcServer, RpcServerImpl};
///
/// pub async fn server() -> SocketAddr {
///     let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
///     let addr = server.local_addr().unwrap();
///     let server_handle = server.start(RpcServerImpl.into_rpc());
///
///     // `into_rpc()` method was generated inside of the `RpcServer` trait under the hood.
///     tokio::spawn(server_handle.stopped());
///
///     addr
/// }
///
/// #[tokio::main]
/// async fn main() {
///     // The generated `RpcModule` can also be called directly without a transport.
///     let module = RpcServerImpl.into_rpc();
///     let (res, _) =
///         module.raw_json_request(r#"{"jsonrpc":"2.0","method":"foo_bar","id":1}"#, 1).await.unwrap();
///     assert_eq!(res, r#"{"jsonrpc":"2.0","id":1,"result":10}"#);
///
///     server().await;
/// }
/// ```
#[proc_macro_attribute]
pub fn rpc(attr: TokenStream, item: TokenStream) -> TokenStream {
    let rebuilt_rpc_attribute = syn::Attribute {
        pound_token: syn::token::Pound::default(),
        style: syn::AttrStyle::Outer,
        bracket_token: syn::token::Bracket::default(),
        meta: syn::Meta::List(syn::MetaList {
            path: syn::Ident::new("rpc", proc_macro2::Span::call_site()).into(),
            delimiter: syn::MacroDelimiter::Paren(
                syn::token::Paren(proc_macro2::Span::call_site()),
            ),
            tokens: attr.into(),
        }),
    };
    match rpc_impl(rebuilt_rpc_attribute, item) {
        Ok(tokens) => tokens,
        Err(err) => err.to_compile_error(),
    }
    .into()
}

/// Convenience form of `rpc` that may use `?` for error handling to avoid boilerplate.
fn rpc_impl(
    attr: syn::Attribute,
    item: TokenStream,
) -> Result<proc_macro2::TokenStream, syn::Error> {
    let trait_data: syn::ItemTrait = syn::parse(item)?;
    let rpc = RpcDescription::from_item(attr, trait_data)?;
    rpc.render()
}
